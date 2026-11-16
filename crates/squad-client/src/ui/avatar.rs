use iced::widget::image as iced_image;
use image::imageops::FilterType;
use squad_proto::ClientUuid;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

pub const AVATAR_SIZE: u32 = 128;
pub const MAX_CUSTOM_AVATAR_BYTES: usize = 64 * 1024;
const AVATAR_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const IDENTICON_BG: [u8; 3] = [0x23, 0x27, 0x2E];

pub fn apply_circular_alpha_mask(rgba: &mut [u8], size: u32) {
    let radius = 63.0f32 * (size as f32 / 128.0);
    let feather = 1.5f32;
    let center = (size as f32 - 1.0) * 0.5;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = ((y * size + x) * 4) as usize;
            if idx + 3 >= rgba.len() {
                continue;
            }
            if dist >= radius {
                rgba[idx + 3] = 0;
            } else if dist > radius - feather {
                let coverage = ((radius - dist) / feather).clamp(0.0, 1.0);
                rgba[idx + 3] = ((rgba[idx + 3] as f32) * coverage).round() as u8;
            }
        }
    }
}

pub fn generate_identicon_rgba(client_uuid: &ClientUuid) -> Vec<u8> {
    let b = *blake3::hash(client_uuid).as_bytes();
    let hue = (u16::from_be_bytes([b[0], b[1]]) % 360) as f32;
    let saturation = 0.62 + (b[2] as f32 / 255.0) * 0.18;
    let lightness = 0.68 + (b[3] as f32 / 255.0) * 0.10;
    let fg_rgb = hsl_to_rgb(hue, saturation, lightness);

    let mut grid = [[false; 5]; 5];
    let mut active_count = 0usize;
    for (row, grid_row) in grid.iter_mut().enumerate() {
        for col in 0..3 {
            let active = (b[4 + row * 3 + col] & 1) == 0;
            grid_row[col] = active;
            if col < 2 {
                grid_row[4 - col] = active;
            }
        }
        active_count += grid_row.iter().filter(|&&c| c).count();
    }

    if active_count < 5 {
        grid[1][2] = true;
        grid[2][2] = true;
        grid[3][2] = true;
    }

    let size = AVATAR_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[0] = IDENTICON_BG[0];
        pixel[1] = IDENTICON_BG[1];
        pixel[2] = IDENTICON_BG[2];
        pixel[3] = 255;
    }

    let margin = 24usize;
    let cell_px = 16usize;
    for (row, grid_row) in grid.iter().enumerate() {
        for (col, &active) in grid_row.iter().enumerate() {
            if !active {
                continue;
            }
            let y0 = margin + row * cell_px;
            let x0 = margin + col * cell_px;
            for py in y0..y0 + cell_px {
                for px in x0..x0 + cell_px {
                    let idx = (py * size + px) * 4;
                    rgba[idx] = fg_rgb[0];
                    rgba[idx + 1] = fg_rgb[1];
                    rgba[idx + 2] = fg_rgb[2];
                    rgba[idx + 3] = 255;
                }
            }
        }
    }

    apply_circular_alpha_mask(&mut rgba, AVATAR_SIZE);
    rgba
}

pub type ProcessedAvatar = (Vec<u8>, [u8; 32], Vec<u8>);

pub fn process_custom_avatar_image(raw_bytes: &[u8]) -> Result<ProcessedAvatar, String> {
    let img = image::load_from_memory(raw_bytes)
        .map_err(|e| format!("Failed to decode avatar image: {e}"))?;
    let w = img.width();
    let h = img.height();
    let side = w.min(h).max(1);
    let x = (w.saturating_sub(side)) / 2;
    let y = (h.saturating_sub(side)) / 2;

    let cropped = img.crop_imm(x, y, side, side);
    let resized = cropped.resize_exact(AVATAR_SIZE, AVATAR_SIZE, FilterType::Lanczos3);

    let rgb = resized.to_rgb8();
    let mut jpeg_bytes = Vec::new();
    for quality in [85u8, 70u8, 55u8] {
        jpeg_bytes.clear();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
        encoder
            .encode_image(&rgb)
            .map_err(|e| format!("Failed to encode avatar JPEG: {e}"))?;
        if jpeg_bytes.len() < MAX_CUSTOM_AVATAR_BYTES {
            break;
        }
    }
    if jpeg_bytes.len() >= MAX_CUSTOM_AVATAR_BYTES {
        return Err("Compressed avatar JPEG still exceeds 64 KB limit".to_string());
    }

    let hash = *blake3::hash(&jpeg_bytes).as_bytes();
    let mut rgba = resized.to_rgba8().into_raw();
    apply_circular_alpha_mask(&mut rgba, AVATAR_SIZE);

    Ok((jpeg_bytes, hash, rgba))
}

pub fn decode_avatar_jpeg_to_circular_rgba(jpeg_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(jpeg_bytes)
        .map_err(|e| format!("Invalid cached avatar JPEG: {e}"))?;
    let resized = if img.width() == AVATAR_SIZE && img.height() == AVATAR_SIZE {
        img
    } else {
        img.resize_exact(AVATAR_SIZE, AVATAR_SIZE, FilterType::Lanczos3)
    };
    let mut rgba = resized.to_rgba8().into_raw();
    apply_circular_alpha_mask(&mut rgba, AVATAR_SIZE);
    Ok(rgba)
}

pub fn avatars_cache_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("squad-air")
        .join("cache")
        .join("avatars")
}

pub fn hash_to_hex(hash: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn hex_to_hash(hex: &str) -> Option<[u8; 32]> {
    let trimmed = hex.trim();
    if trimmed.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let pair = trimmed.get(i * 2..i * 2 + 2)?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

pub fn avatar_disk_path(hash: &[u8; 32]) -> PathBuf {
    avatars_cache_dir().join(format!("{}.jpg", hash_to_hex(hash)))
}

pub fn load_cached_avatar_jpeg(hash: &[u8; 32]) -> Option<Vec<u8>> {
    if *hash == [0u8; 32] {
        return None;
    }
    let path = avatar_disk_path(hash);
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() >= MAX_CUSTOM_AVATAR_BYTES {
        return None;
    }
    if *blake3::hash(&bytes).as_bytes() == *hash {
        Some(bytes)
    } else {
        None
    }
}

pub fn save_cached_avatar_jpeg(hash: &[u8; 32], jpeg_bytes: &[u8]) -> Result<PathBuf, String> {
    if *hash == [0u8; 32] || jpeg_bytes.is_empty() || jpeg_bytes.len() >= MAX_CUSTOM_AVATAR_BYTES {
        return Err("Invalid avatar payload".to_string());
    }
    if *blake3::hash(jpeg_bytes).as_bytes() != *hash {
        return Err("Avatar BLAKE3 digest mismatch".to_string());
    }
    let dir = avatars_cache_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create avatar cache dir: {e}"))?;
    let path = avatar_disk_path(hash);
    fs::write(&path, jpeg_bytes).map_err(|e| format!("Cannot write cached avatar: {e}"))?;
    Ok(path)
}

pub fn prune_expired_avatars() {
    let dir = avatars_cache_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = now.duration_since(modified) {
                if age > AVATAR_CACHE_TTL {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

pub struct AvatarCache {
    handles: Mutex<HashMap<ClientUuid, iced_image::Handle>>,
}

impl AvatarCache {
    pub fn new() -> Self {
        prune_expired_avatars();
        Self {
            handles: Mutex::new(HashMap::new()),
        }
    }

    pub fn handle_for(&self, client_uuid: &ClientUuid) -> iced_image::Handle {
        if let Ok(map) = self.handles.lock() {
            if let Some(existing) = map.get(client_uuid) {
                return existing.clone();
            }
        }
        let rgba = generate_identicon_rgba(client_uuid);
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.handles.lock() {
            map.insert(*client_uuid, handle.clone());
        }
        handle
    }

    pub fn set_custom_rgba(&self, client_uuid: ClientUuid, rgba: Vec<u8>) {
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.handles.lock() {
            map.insert(client_uuid, handle);
        }
    }

    pub fn set_from_jpeg_option(&self, client_uuid: ClientUuid, jpeg_bytes: Option<&[u8]>) {
        if let Some(jpeg) = jpeg_bytes {
            if let Ok(rgba) = decode_avatar_jpeg_to_circular_rgba(jpeg) {
                self.set_custom_rgba(client_uuid, rgba);
                return;
            }
        }
        self.reset_to_identicon(client_uuid);
    }

    pub fn reset_to_identicon(&self, client_uuid: ClientUuid) {
        let rgba = generate_identicon_rgba(&client_uuid);
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.handles.lock() {
            map.insert(client_uuid, handle);
        }
    }

    pub fn clear_memory_cache(&self) {
        if let Ok(mut map) = self.handles.lock() {
            map.clear();
            map.shrink_to_fit();
        }
    }
}

impl Default for AvatarCache {
    fn default() -> Self {
        Self::new()
    }
}

fn hsl_to_rgb(hue_deg: f32, sat: f32, light: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let h_prime = hue_deg / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = if (0.0..1.0).contains(&h_prime) {
        (c, x, 0.0)
    } else if (1.0..2.0).contains(&h_prime) {
        (x, c, 0.0)
    } else if (2.0..3.0).contains(&h_prime) {
        (0.0, c, x)
    } else if (3.0..4.0).contains(&h_prime) {
        (0.0, x, c)
    } else if (4.0..5.0).contains(&h_prime) {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = light - c * 0.5;
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use std::io::Cursor;

    #[test]
    fn test_generate_identicon_rgba_symmetry_and_circular_mask() {
        let uuid_a: ClientUuid = [0x1A; 16];
        let uuid_b: ClientUuid = [0x9F; 16];

        let rgba_a1 = generate_identicon_rgba(&uuid_a);
        let rgba_a2 = generate_identicon_rgba(&uuid_a);
        let rgba_b = generate_identicon_rgba(&uuid_b);

        assert_eq!(rgba_a1.len(), (AVATAR_SIZE * AVATAR_SIZE * 4) as usize);
        assert_eq!(rgba_a1, rgba_a2, "Identicon must be deterministic");
        assert_ne!(rgba_a1, rgba_b, "Distinct UUIDs must produce distinct identicons");

        // Corner pixels must be completely transparent outside circular mask.
        assert_eq!(rgba_a1[3], 0);
        // Center pixel must be fully opaque.
        let center_idx = (64 * AVATAR_SIZE as usize + 64) * 4;
        assert_eq!(rgba_a1[center_idx + 3], 255);

        // Horizontal mirror symmetry inside the 5x5 cell centers (cols 0..5).
        for row in 0..5 {
            let py = 24 + row * 16 + 8;
            let lx = 24 + 8;
            let rx = 24 + 4 * 16 + 8;
            let l_idx = (py * 128 + lx) * 4;
            let r_idx = (py * 128 + rx) * 4;
            assert_eq!(rgba_a1[l_idx..l_idx + 3], rgba_a1[r_idx..r_idx + 3]);
        }
    }

    #[test]
    fn test_process_custom_avatar_image_crop_resize_and_blake3() {
        let mut src = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(240, 120);
        for (x, y, px) in src.enumerate_pixels_mut() {
            *px = Rgba([
                ((x * 2) % 256) as u8,
                ((y * 2) % 256) as u8,
                0xC8,
                255,
            ]);
        }
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(src)
            .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();

        let (jpeg, hash, rgba) = process_custom_avatar_image(&png_bytes).unwrap();
        assert!(!jpeg.is_empty() && jpeg.len() < MAX_CUSTOM_AVATAR_BYTES);
        assert_eq!(*blake3::hash(&jpeg).as_bytes(), hash);
        assert_eq!(rgba.len(), (AVATAR_SIZE * AVATAR_SIZE * 4) as usize);
        assert_eq!(rgba[3], 0);
        let center_idx = (64 * 128 + 64) * 4;
        assert_eq!(rgba[center_idx + 3], 255);

        let decoded_rgba = decode_avatar_jpeg_to_circular_rgba(&jpeg).unwrap();
        assert_eq!(decoded_rgba.len(), (AVATAR_SIZE * AVATAR_SIZE * 4) as usize);
        assert_eq!(decoded_rgba[3], 0);
    }
}
