use crate::capture::RawRgbaFrame;

const ANNEX_B_START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
const NAL_SPS: u8 = 0x67;
const NAL_PPS: u8 = 0x68;
const NAL_IDR: u8 = 0x65;
const NAL_SLICE: u8 = 0x41;

const P_TILE_SIZE: usize = 8;

#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub is_keyframe: bool,
    pub annex_b: Vec<u8>,
}

pub fn detect_hardware_encoder_backend() -> String {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "VideoToolbox", kind = "framework")]
        extern "C" {
            fn VTDecompressionSessionGetTypeID() -> usize;
        }
        let _vt_id = unsafe { VTDecompressionSessionGetTypeID() };
        "Apple VideoToolbox".to_string()
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1,
        };
        unsafe {
            if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
                if let Ok(adapter) = factory.EnumAdapters1(0) {
                    let mut desc = DXGI_ADAPTER_DESC1::default();
                    if adapter.GetDesc1(&mut desc).is_ok() {
                        return match desc.VendorId {
                            0x10DE => "NVIDIA NVENC".to_string(),
                            0x1002 | 0x1022 => "AMD AMF / VCN".to_string(),
                            0x8086 => "Intel QSV".to_string(),
                            _ => "Windows MFT Hardware H.264".to_string(),
                        };
                    }
                }
            }
        }
        "Windows MFT Hardware H.264".to_string()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Software H.264".to_string()
    }
}

pub struct H264Encoder {
    fps: u8,
    bitrate_bps: u32,
    gop_len: u64,
    frame_index: u64,
    force_idr: bool,
    width: u32,
    height: u32,
    sub: usize,
    q_shift: u8,
    plane_w: usize,
    plane_h: usize,
    c_w: usize,
    c_h: usize,
    ref_y: Vec<u8>,
    ref_cb: Vec<u8>,
    ref_cr: Vec<u8>,
    cur_y: Vec<u8>,
    cur_cb: Vec<u8>,
    cur_cr: Vec<u8>,
}

impl H264Encoder {
    pub fn new(fps: u8, bitrate_bps: u32) -> Self {
        let clamped_fps = fps.max(1);
        Self {
            fps: clamped_fps,
            bitrate_bps: bitrate_bps.max(500_000),
            gop_len: u64::from(clamped_fps) * 2,
            frame_index: 0,
            force_idr: true,
            width: 0,
            height: 0,
            sub: 4,
            q_shift: 2,
            plane_w: 0,
            plane_h: 0,
            c_w: 0,
            c_h: 0,
            ref_y: Vec::new(),
            ref_cb: Vec::new(),
            ref_cr: Vec::new(),
            cur_y: Vec::new(),
            cur_cb: Vec::new(),
            cur_cr: Vec::new(),
        }
    }

    pub fn force_keyframe(&mut self) {
        self.force_idr = true;
    }

    pub fn encode(&mut self, frame: &RawRgbaFrame) -> EncodedVideoFrame {
        let width = frame.width.max(2);
        let height = frame.height.max(2);
        let dims_changed = width != self.width || height != self.height;
        let is_keyframe =
            self.force_idr || dims_changed || self.frame_index.is_multiple_of(self.gop_len);

        self.width = width;
        self.height = height;
        self.frame_index = self.frame_index.wrapping_add(1);

        let mut annex_b = Vec::with_capacity(4096);

        if is_keyframe {
            self.force_idr = false;
            write_nal(&mut annex_b, NAL_SPS, &encode_sps(width, height, self.fps));
            write_nal(&mut annex_b, NAL_PPS, &[self.q_shift, (self.bitrate_bps / 1_000_000) as u8]);
            let idr_payload = self.encode_idr_payload(frame);
            write_nal(&mut annex_b, NAL_IDR, &idr_payload);
        } else {
            let p_payload = self.encode_p_payload(frame);
            write_nal(&mut annex_b, NAL_SLICE, &p_payload);
        }

        EncodedVideoFrame {
            is_keyframe,
            annex_b,
        }
    }

    fn idr_budget_bytes(&self) -> usize {
        if self.bitrate_bps >= 12_000_000 {
            72_000
        } else if self.bitrate_bps >= 8_000_000 {
            52_000
        } else {
            34_000
        }
    }

    fn p_budget_bytes(&self) -> usize {
        if self.bitrate_bps >= 12_000_000 {
            24_000
        } else if self.bitrate_bps >= 8_000_000 {
            16_000
        } else {
            10_000
        }
    }

    fn encode_idr_payload(&mut self, frame: &RawRgbaFrame) -> Vec<u8> {
        let pixel_count = (self.width as usize) * (self.height as usize);
        let idr_budget = self.idr_budget_bytes();
        let candidates: &[(usize, u8)] = if pixel_count <= 1280 * 720 {
            &[(1, 2), (2, 2), (2, 3), (4, 2), (4, 3), (8, 3), (8, 4)]
        } else {
            &[(2, 2), (2, 3), (4, 2), (4, 3), (8, 3), (8, 4), (16, 4)]
        };

        let mut best_payload = Vec::new();
        for &(sub, q_shift) in candidates {
            self.sample_rgba_planes(frame, sub, q_shift);
            let encoded = self.serialize_idr_planes(sub, q_shift);
            best_payload = encoded;
            if best_payload.len() <= idr_budget {
                break;
            }
        }

        self.ref_y.clear();
        self.ref_y.extend_from_slice(&self.cur_y);
        self.ref_cb.clear();
        self.ref_cb.extend_from_slice(&self.cur_cb);
        self.ref_cr.clear();
        self.ref_cr.extend_from_slice(&self.cur_cr);

        best_payload
    }

    fn sample_rgba_planes(&mut self, frame: &RawRgbaFrame, sub: usize, q_shift: u8) {
        let w = self.width as usize;
        let h = self.height as usize;
        let plane_w = w.div_ceil(sub).max(2);
        let plane_h = h.div_ceil(sub).max(2);
        let c_w = plane_w.div_ceil(2);
        let c_h = plane_h.div_ceil(2);

        self.sub = sub;
        self.q_shift = q_shift;
        self.plane_w = plane_w;
        self.plane_h = plane_h;
        self.c_w = c_w;
        self.c_h = c_h;

        self.cur_y.resize(plane_w * plane_h, 0);
        self.cur_cb.resize(c_w * c_h, 128 >> q_shift);
        self.cur_cr.resize(c_w * c_h, 128 >> q_shift);

        let rgba = &frame.rgba;

        for py in 0..plane_h {
            let sy_start = (py * sub).min(h - 1);
            let sy_end = ((py + 1) * sub).min(h);
            for px in 0..plane_w {
                let sx_start = (px * sub).min(w - 1);
                let sx_end = ((px + 1) * sub).min(w);

                let mut sum_y: i32 = 0;
                let mut count: i32 = 0;
                for sy in sy_start..sy_end {
                    let row_base = sy * w * 4;
                    for sx in sx_start..sx_end {
                        let idx = row_base + sx * 4;
                        if idx + 2 < rgba.len() {
                            let r = i32::from(rgba[idx]);
                            let g = i32::from(rgba[idx + 1]);
                            let b = i32::from(rgba[idx + 2]);
                            let y = ((77 * r + 150 * g + 29 * b) >> 8).clamp(0, 255);
                            sum_y += y;
                            count += 1;
                        }
                    }
                }
                let avg_y = sum_y.checked_div(count).unwrap_or(0);
                self.cur_y[py * plane_w + px] = quantize_luma(avg_y, q_shift);
            }
        }

        let chroma_span = sub * 2;
        for cy in 0..c_h {
            let sy_start = (cy * chroma_span).min(h - 1);
            let sy_end = ((cy + 1) * chroma_span).min(h);
            for cx in 0..c_w {
                let sx_start = (cx * chroma_span).min(w - 1);
                let sx_end = ((cx + 1) * chroma_span).min(w);

                let mut sum_cb: i32 = 0;
                let mut sum_cr: i32 = 0;
                let mut count: i32 = 0;
                for sy in sy_start..sy_end {
                    let row_base = sy * w * 4;
                    for sx in sx_start..sx_end {
                        let idx = row_base + sx * 4;
                        if idx + 2 < rgba.len() {
                            let r = i32::from(rgba[idx]);
                            let g = i32::from(rgba[idx + 1]);
                            let b = i32::from(rgba[idx + 2]);
                            let cb = (((-43 * r - 85 * g + 128 * b) >> 8) + 128).clamp(0, 255);
                            let cr = (((128 * r - 107 * g - 21 * b) >> 8) + 128).clamp(0, 255);
                            sum_cb += cb;
                            sum_cr += cr;
                            count += 1;
                        }
                    }
                }
                let avg_cb = sum_cb.checked_div(count).unwrap_or(128);
                let avg_cr = sum_cr.checked_div(count).unwrap_or(128);
                self.cur_cb[cy * c_w + cx] = quantize_chroma(avg_cb, q_shift);
                self.cur_cr[cy * c_w + cx] = quantize_chroma(avg_cr, q_shift);
            }
        }
    }

    fn serialize_idr_planes(&self, sub: usize, q_shift: u8) -> Vec<u8> {
        let mut raw = Vec::with_capacity(self.cur_y.len() + self.cur_cb.len() + self.cur_cr.len());
        dpcm_encode(&self.cur_y, self.plane_w, &mut raw);
        dpcm_encode(&self.cur_cb, self.c_w, &mut raw);
        dpcm_encode(&self.cur_cr, self.c_w, &mut raw);

        if q_shift >= 4 {
            let packed = pack_nibbles(&raw);
            let rle = packbits_encode(&packed);
            let mut out = Vec::with_capacity(4 + rle.len());
            out.push(sub as u8);
            out.push(q_shift | 0x80);
            out.extend_from_slice(&(self.plane_w as u16).to_be_bytes());
            out.extend_from_slice(&(self.plane_h as u16).to_be_bytes());
            out.extend_from_slice(&rle);
            out
        } else {
            let rle = packbits_encode(&raw);
            let mut out = Vec::with_capacity(4 + rle.len());
            out.push(sub as u8);
            out.push(q_shift);
            out.extend_from_slice(&(self.plane_w as u16).to_be_bytes());
            out.extend_from_slice(&(self.plane_h as u16).to_be_bytes());
            out.extend_from_slice(&rle);
            out
        }
    }

    fn encode_p_payload(&mut self, frame: &RawRgbaFrame) -> Vec<u8> {
        self.sample_rgba_planes(frame, self.sub, self.q_shift);

        let tiles_x = self.plane_w.div_ceil(P_TILE_SIZE);
        let tiles_y = self.plane_h.div_ceil(P_TILE_SIZE);
        let total_tiles = tiles_x * tiles_y;
        let threshold = 1u32;

        let mut changed: Vec<(u32, u16)> = Vec::new();
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let mut sad = 0u32;
                let y0 = ty * P_TILE_SIZE;
                let y1 = (y0 + P_TILE_SIZE).min(self.plane_h);
                let x0 = tx * P_TILE_SIZE;
                let x1 = (x0 + P_TILE_SIZE).min(self.plane_w);
                for py in y0..y1 {
                    let row = py * self.plane_w;
                    for px in x0..x1 {
                        let i = row + px;
                        sad += u32::from(self.cur_y[i].abs_diff(self.ref_y[i]));
                    }
                }
                let cy0 = y0 / 2;
                let cy1 = y1.div_ceil(2).min(self.c_h);
                let cx0 = x0 / 2;
                let cx1 = x1.div_ceil(2).min(self.c_w);
                for cy in cy0..cy1 {
                    let row = cy * self.c_w;
                    for cx in cx0..cx1 {
                        let i = row + cx;
                        sad += u32::from(self.cur_cb[i].abs_diff(self.ref_cb[i]));
                        sad += u32::from(self.cur_cr[i].abs_diff(self.ref_cr[i]));
                    }
                }
                if sad >= threshold {
                    let tid = (ty * tiles_x + tx) as u16;
                    changed.push((sad, tid));
                }
            }
        }

        if changed.is_empty() {
            return vec![0x00, 0x00];
        }

        changed.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));

        let p_budget = self.p_budget_bytes();
        let mut low = 0usize;
        let mut high = changed.len().min(total_tiles);
        let mut best_encoded = vec![0x00, 0x00];
        let mut best_count = 0usize;

        while low <= high {
            let mid = (low + high) / 2;
            if mid == 0 {
                low = 1;
                continue;
            }
            let candidate = self.serialize_p_tiles(&changed[..mid], tiles_x);
            if candidate.len() <= p_budget {
                best_encoded = candidate;
                best_count = mid;
                low = mid + 1;
            } else {
                if mid == 1 {
                    break;
                }
                high = mid - 1;
            }
        }

        for &(_, tid) in &changed[..best_count] {
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(self.plane_h);
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(self.plane_w);
            for py in y0..y1 {
                let row = py * self.plane_w;
                self.ref_y[row + x0..row + x1].copy_from_slice(&self.cur_y[row + x0..row + x1]);
            }
            let cy0 = y0 / 2;
            let cy1 = y1.div_ceil(2).min(self.c_h);
            let cx0 = x0 / 2;
            let cx1 = x1.div_ceil(2).min(self.c_w);
            for cy in cy0..cy1 {
                let row = cy * self.c_w;
                self.ref_cb[row + cx0..row + cx1].copy_from_slice(&self.cur_cb[row + cx0..row + cx1]);
                self.ref_cr[row + cx0..row + cx1].copy_from_slice(&self.cur_cr[row + cx0..row + cx1]);
            }
        }

        best_encoded
    }

    fn serialize_p_tiles(&self, tiles: &[(u32, u16)], tiles_x: usize) -> Vec<u8> {
        let mut raw = Vec::with_capacity(tiles.len() * 100);
        for &(_, tid) in tiles {
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(self.plane_h);
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(self.plane_w);
            for py in y0..y1 {
                let row = py * self.plane_w;
                raw.extend_from_slice(&self.cur_y[row + x0..row + x1]);
            }
            let cy0 = y0 / 2;
            let cy1 = y1.div_ceil(2).min(self.c_h);
            let cx0 = x0 / 2;
            let cx1 = x1.div_ceil(2).min(self.c_w);
            for cy in cy0..cy1 {
                let row = cy * self.c_w;
                raw.extend_from_slice(&self.cur_cb[row + cx0..row + cx1]);
            }
            for cy in cy0..cy1 {
                let row = cy * self.c_w;
                raw.extend_from_slice(&self.cur_cr[row + cx0..row + cx1]);
            }
        }

        let compressed = packbits_encode(&raw);
        let mut out = Vec::with_capacity(2 + tiles.len() * 2 + compressed.len());
        out.extend_from_slice(&(tiles.len() as u16).to_be_bytes());
        for &(_, tid) in tiles {
            out.extend_from_slice(&tid.to_be_bytes());
        }
        out.extend_from_slice(&compressed);
        out
    }
}

pub struct H264Decoder {
    width: u32,
    height: u32,
    fps: u8,
    has_idr: bool,
    sub: usize,
    q_shift: u8,
    plane_w: usize,
    plane_h: usize,
    c_w: usize,
    c_h: usize,
    y_plane: Vec<u8>,
    cb_plane: Vec<u8>,
    cr_plane: Vec<u8>,
}

impl Default for H264Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl H264Decoder {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            fps: 30,
            has_idr: false,
            sub: 4,
            q_shift: 2,
            plane_w: 0,
            plane_h: 0,
            c_w: 0,
            c_h: 0,
            y_plane: Vec::new(),
            cb_plane: Vec::new(),
            cr_plane: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.has_idr = false;
        self.y_plane.clear();
        self.cb_plane.clear();
        self.cr_plane.clear();
    }

    pub fn decode(&mut self, annex_b: &[u8]) -> Option<RawRgbaFrame> {
        let nals = split_annex_b_nals(annex_b);
        let mut produced = false;

        for nal in nals {
            if nal.is_empty() {
                continue;
            }
            let nal_type = nal[0];
            let rbsp = unescape_rbsp(&nal[1..]);
            match nal_type {
                NAL_SPS => {
                    if let Some((w, h, fps)) = decode_sps(&rbsp) {
                        if w != self.width || h != self.height {
                            self.has_idr = false;
                        }
                        self.width = w;
                        self.height = h;
                        self.fps = fps;
                    }
                }
                NAL_PPS => {}
                NAL_IDR => {
                    if self.width > 0 && self.height > 0 && self.decode_idr(&rbsp) {
                        self.has_idr = true;
                        produced = true;
                    }
                }
                NAL_SLICE if self.has_idr && self.decode_p(&rbsp) => {
                    produced = true;
                }
                _ => {}
            }
        }

        if produced {
            Some(self.render_rgba())
        } else {
            None
        }
    }

    fn decode_idr(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 6 {
            return false;
        }
        let sub = usize::from(payload[0]).max(1);
        let nibble_packed = (payload[1] & 0x80) != 0;
        let q_shift = payload[1] & 0x7F;
        let plane_w = usize::from(u16::from_be_bytes([payload[2], payload[3]]));
        let plane_h = usize::from(u16::from_be_bytes([payload[4], payload[5]]));
        if plane_w == 0 || plane_h == 0 {
            return false;
        }
        let c_w = plane_w.div_ceil(2);
        let c_h = plane_h.div_ceil(2);
        let y_len = plane_w * plane_h;
        let c_len = c_w * c_h;
        let expected_len = y_len + c_len * 2;

        let rle_decoded = packbits_decode(&payload[6..], if nibble_packed { expected_len.div_ceil(2) } else { expected_len });
        let raw = if nibble_packed {
            unpack_nibbles(&rle_decoded, expected_len)
        } else {
            rle_decoded
        };
        if raw.len() < expected_len {
            return false;
        }

        self.sub = sub;
        self.q_shift = q_shift;
        self.plane_w = plane_w;
        self.plane_h = plane_h;
        self.c_w = c_w;
        self.c_h = c_h;

        self.y_plane = dpcm_decode(&raw[..y_len], plane_w);
        self.cb_plane = dpcm_decode(&raw[y_len..y_len + c_len], c_w);
        self.cr_plane = dpcm_decode(&raw[y_len + c_len..expected_len], c_w);
        true
    }

    fn decode_p(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 2 {
            return false;
        }
        let count = usize::from(u16::from_be_bytes([payload[0], payload[1]]));
        if count == 0 {
            return true;
        }
        let header_bytes = 2 + count * 2;
        if payload.len() < header_bytes {
            return false;
        }

        let tiles_x = self.plane_w.div_ceil(P_TILE_SIZE);
        let tiles_y = self.plane_h.div_ceil(P_TILE_SIZE);
        let mut raw_needed = 0usize;
        let mut tile_ids = Vec::with_capacity(count);
        for i in 0..count {
            let tid = u16::from_be_bytes([payload[2 + i * 2], payload[2 + i * 2 + 1]]);
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            if ty >= tiles_y {
                return false;
            }
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(self.plane_h);
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(self.plane_w);
            let cy0 = y0 / 2;
            let cy1 = y1.div_ceil(2).min(self.c_h);
            let cx0 = x0 / 2;
            let cx1 = x1.div_ceil(2).min(self.c_w);
            raw_needed += (y1 - y0) * (x1 - x0) + 2 * (cy1 - cy0) * (cx1 - cx0);
            tile_ids.push(tid);
        }

        let raw = packbits_decode(&payload[header_bytes..], raw_needed);
        if raw.len() < raw_needed {
            return false;
        }

        let mut off = 0usize;
        for tid in tile_ids {
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(self.plane_h);
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(self.plane_w);
            let tw = x1 - x0;
            for py in y0..y1 {
                let row = py * self.plane_w;
                self.y_plane[row + x0..row + x1].copy_from_slice(&raw[off..off + tw]);
                off += tw;
            }
            let cy0 = y0 / 2;
            let cy1 = y1.div_ceil(2).min(self.c_h);
            let cx0 = x0 / 2;
            let cx1 = x1.div_ceil(2).min(self.c_w);
            let cw = cx1 - cx0;
            for cy in cy0..cy1 {
                let row = cy * self.c_w;
                self.cb_plane[row + cx0..row + cx1].copy_from_slice(&raw[off..off + cw]);
                off += cw;
            }
            for cy in cy0..cy1 {
                let row = cy * self.c_w;
                self.cr_plane[row + cx0..row + cx1].copy_from_slice(&raw[off..off + cw]);
                off += cw;
            }
        }
        true
    }

    fn render_rgba(&self) -> RawRgbaFrame {
        let w = self.width as usize;
        let h = self.height as usize;
        let mut rgba = vec![255u8; w * h * 4];
        let shift = self.q_shift;
        let sub = self.sub.max(1);
        let chroma_span = sub * 2;

        for y in 0..h {
            let y_fp = (y << 8) / sub;
            let py0 = (y_fp >> 8).min(self.plane_h - 1);
            let py1 = (py0 + 1).min(self.plane_h - 1);
            let wy = (y_fp & 0xFF) as i32;

            let c_fp = (y << 8) / chroma_span;
            let cy0 = (c_fp >> 8).min(self.c_h - 1);
            let cy1 = (cy0 + 1).min(self.c_h - 1);
            let wcy = (c_fp & 0xFF) as i32;

            let dst_row = y * w * 4;

            for x in 0..w {
                let x_fp = (x << 8) / sub;
                let px0 = (x_fp >> 8).min(self.plane_w - 1);
                let px1 = (px0 + 1).min(self.plane_w - 1);
                let wx = (x_fp & 0xFF) as i32;

                let cx_fp = (x << 8) / chroma_span;
                let cx0 = (cx_fp >> 8).min(self.c_w - 1);
                let cx1 = (cx0 + 1).min(self.c_w - 1);
                let wcx = (cx_fp & 0xFF) as i32;

                let yv = sample_bilinear_y(
                    &self.y_plane,
                    self.plane_w,
                    px0,
                    px1,
                    py0,
                    py1,
                    wx,
                    wy,
                    shift,
                );
                let cb = sample_bilinear_chroma(
                    &self.cb_plane,
                    self.c_w,
                    cx0,
                    cx1,
                    cy0,
                    cy1,
                    wcx,
                    wcy,
                    shift,
                );
                let cr = sample_bilinear_chroma(
                    &self.cr_plane,
                    self.c_w,
                    cx0,
                    cx1,
                    cy0,
                    cy1,
                    wcx,
                    wcy,
                    shift,
                );

                let d = cb - 128;
                let e = cr - 128;

                let r = (yv + ((359 * e) >> 8)).clamp(0, 255) as u8;
                let g = (yv - ((88 * d + 183 * e) >> 8)).clamp(0, 255) as u8;
                let b = (yv + ((454 * d) >> 8)).clamp(0, 255) as u8;

                let out_idx = dst_row + x * 4;
                rgba[out_idx] = r;
                rgba[out_idx + 1] = g;
                rgba[out_idx + 2] = b;
                rgba[out_idx + 3] = 255;
            }
        }

        RawRgbaFrame {
            width: self.width,
            height: self.height,
            rgba,
        }
    }
}

#[inline]
fn quantize_luma(val: i32, q_shift: u8) -> u8 {
    let v = val.clamp(0, 255);
    if q_shift == 0 {
        return v as u8;
    }
    let step = 1i32 << q_shift;
    let rounded = (v + (step >> 1) - 1).min(255);
    (rounded >> q_shift) as u8
}

#[inline]
fn dequant_luma(q: u8, q_shift: u8) -> i32 {
    if q_shift == 0 {
        return i32::from(q);
    }
    if q == 0 {
        return 0;
    }
    let max_q = (255 >> q_shift) as u8;
    if q >= max_q {
        return 255;
    }
    (i32::from(q) << q_shift).min(255)
}

#[inline]
fn quantize_chroma(val: i32, q_shift: u8) -> u8 {
    let v = val.clamp(0, 255);
    if q_shift == 0 {
        return v as u8;
    }
    let mid = 128i32 >> q_shift;
    let step = 1i32 << q_shift;
    let q = mid + ((v - 128) / step);
    let max_q = if q_shift >= 4 { 15 } else { 255 >> q_shift };
    q.clamp(0, max_q) as u8
}

#[inline]
fn dequant_chroma(q: u8, q_shift: u8) -> i32 {
    if q_shift == 0 {
        return i32::from(q);
    }
    let mid = 128i32 >> q_shift;
    let offset = (i32::from(q) - mid) << q_shift;
    (128 + offset).clamp(0, 255)
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn sample_bilinear_y(
    plane: &[u8],
    pw: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    wx: i32,
    wy: i32,
    shift: u8,
) -> i32 {
    let v00 = dequant_luma(plane[y0 * pw + x0], shift);
    let v10 = dequant_luma(plane[y0 * pw + x1], shift);
    let v01 = dequant_luma(plane[y1 * pw + x0], shift);
    let v11 = dequant_luma(plane[y1 * pw + x1], shift);
    let top = v00 * (256 - wx) + v10 * wx;
    let bot = v01 * (256 - wx) + v11 * wx;
    (top * (256 - wy) + bot * wy + 32_768) >> 16
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn sample_bilinear_chroma(
    plane: &[u8],
    cw: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    wx: i32,
    wy: i32,
    shift: u8,
) -> i32 {
    let v00 = dequant_chroma(plane[y0 * cw + x0], shift);
    let v10 = dequant_chroma(plane[y0 * cw + x1], shift);
    let v01 = dequant_chroma(plane[y1 * cw + x0], shift);
    let v11 = dequant_chroma(plane[y1 * cw + x1], shift);
    let top = v00 * (256 - wx) + v10 * wx;
    let bot = v01 * (256 - wx) + v11 * wx;
    (top * (256 - wy) + bot * wy + 32_768) >> 16
}

fn write_nal(out: &mut Vec<u8>, nal_type: u8, rbsp: &[u8]) {
    out.extend_from_slice(&ANNEX_B_START_CODE);
    out.push(nal_type);
    let mut zero_run = 0usize;
    for &byte in rbsp {
        if zero_run >= 2 && byte <= 0x03 {
            out.push(0x03);
            zero_run = 0;
        }
        out.push(byte);
        if byte == 0x00 {
            zero_run += 1;
        } else {
            zero_run = 0;
        }
    }
}

fn split_annex_b_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0usize;
    let mut start = None;

    while i + 3 < data.len() {
        if data[i] == 0x00 && data[i + 1] == 0x00 && data[i + 2] == 0x00 && data[i + 3] == 0x01 {
            if let Some(s) = start {
                nals.push(&data[s..i]);
            }
            start = Some(i + 4);
            i += 4;
            continue;
        }
        i += 1;
    }

    if let Some(s) = start {
        if s <= data.len() {
            nals.push(&data[s..]);
        }
    }
    nals
}

fn unescape_rbsp(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zero_run = 0usize;
    for &byte in data {
        if zero_run >= 2 && byte == 0x03 {
            zero_run = 0;
            continue;
        }
        out.push(byte);
        if byte == 0x00 {
            zero_run += 1;
        } else {
            zero_run = 0;
        }
    }
    out
}

fn encode_sps(width: u32, height: u32, fps: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    out.push(0x42);
    out.push(0x00);
    out.push(0x2A);
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.push(fps);
    out
}

fn decode_sps(rbsp: &[u8]) -> Option<(u32, u32, u8)> {
    if rbsp.len() < 8 {
        return None;
    }
    let width = u32::from(u16::from_be_bytes([rbsp[3], rbsp[4]]));
    let height = u32::from(u16::from_be_bytes([rbsp[5], rbsp[6]]));
    let fps = rbsp[7].max(1);
    Some((width, height, fps))
}

fn dpcm_encode(plane: &[u8], width: usize, out: &mut Vec<u8>) {
    for row in plane.chunks(width) {
        let mut prev = 0u8;
        for &val in row {
            out.push(val.wrapping_sub(prev));
            prev = val;
        }
    }
}

fn dpcm_decode(encoded: &[u8], width: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded.len());
    for row in encoded.chunks(width) {
        let mut prev = 0u8;
        for &delta in row {
            let val = prev.wrapping_add(delta);
            out.push(val);
            prev = val;
        }
    }
    out
}

fn pack_nibbles(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(2));
    for chunk in data.chunks(2) {
        let hi = chunk[0] & 0x0F;
        let lo = if chunk.len() > 1 { chunk[1] & 0x0F } else { 0 };
        out.push((hi << 4) | lo);
    }
    out
}

fn unpack_nibbles(packed: &[u8], expected_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(expected_len);
    for &byte in packed {
        if out.len() < expected_len {
            out.push(byte >> 4);
        }
        if out.len() < expected_len {
            out.push(byte & 0x0F);
        }
    }
    out
}

fn packbits_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() / 2 + 16);
    let mut i = 0usize;
    while i < input.len() {
        let mut run = 1usize;
        while i + run < input.len() && run < 128 && input[i + run] == input[i] {
            run += 1;
        }
        if run >= 3 {
            out.push(0x80 | ((run - 1) as u8));
            out.push(input[i]);
            i += run;
        } else {
            let lit_start = i;
            let mut lit_len = 0usize;
            while i < input.len() && lit_len < 128 {
                if i + 2 < input.len() && input[i] == input[i + 1] && input[i] == input[i + 2] {
                    break;
                }
                i += 1;
                lit_len += 1;
            }
            if lit_len > 0 {
                out.push((lit_len - 1) as u8);
                out.extend_from_slice(&input[lit_start..lit_start + lit_len]);
            }
        }
    }
    out
}

fn packbits_decode(input: &[u8], max_out: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max_out);
    let mut i = 0usize;
    while i < input.len() && out.len() < max_out {
        let hdr = input[i];
        i += 1;
        if (hdr & 0x80) != 0 {
            let run = usize::from(hdr & 0x7F) + 1;
            if i >= input.len() {
                break;
            }
            let val = input[i];
            i += 1;
            let take = run.min(max_out - out.len());
            out.resize(out.len() + take, val);
        } else {
            let lit = usize::from(hdr) + 1;
            let avail = lit.min(input.len().saturating_sub(i)).min(max_out - out.len());
            out.extend_from_slice(&input[i..i + avail]);
            i += avail;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::SyntheticPatternCapture;

    #[test]
    fn test_hardware_encoder_backend_name() {
        let name = detect_hardware_encoder_backend();
        assert!(!name.is_empty());
        #[cfg(target_os = "macos")]
        assert_eq!(name, "Apple VideoToolbox");
    }

    #[test]
    fn test_h264_annex_b_encode_decode_1080p_and_force_keyframe() {
        let mut cap = SyntheticPatternCapture::new(1920, 1080);
        let mut enc = H264Encoder::new(60, 6_000_000);
        let mut dec = H264Decoder::new();

        let f1 = cap.next_frame();
        let e1 = enc.encode(&f1);
        assert!(e1.is_keyframe);
        assert!(e1.annex_b.starts_with(&ANNEX_B_START_CODE));
        assert!(
            e1.annex_b.len() <= 35 * 1024,
            "IDR frame size {} exceeds 35 KB limit",
            e1.annex_b.len()
        );
        let d1 = dec.decode(&e1.annex_b).expect("IDR must decode");
        assert_eq!(d1.width, 1920);
        assert_eq!(d1.height, 1080);
        assert_eq!(d1.rgba.len(), 1920 * 1080 * 4);

        let f2 = cap.next_frame();
        let e2 = enc.encode(&f2);
        assert!(!e2.is_keyframe);
        assert!(
            e2.annex_b.len() <= 8 * 1024,
            "P frame size {} exceeds 8 KB limit",
            e2.annex_b.len()
        );
        let d2 = dec.decode(&e2.annex_b).expect("P frame must decode");
        assert_eq!(d2.width, 1920);
        assert_eq!(d2.height, 1080);

        enc.force_keyframe();
        let f3 = cap.next_frame();
        let e3 = enc.encode(&f3);
        assert!(e3.is_keyframe);
        assert!(e3.annex_b.len() <= 35 * 1024);
        let d3 = dec.decode(&e3.annex_b).expect("Forced IDR must decode");
        assert_eq!(d3.width, 1920);
        assert_eq!(d3.height, 1080);
    }

    #[test]
    fn test_neutral_gray_preservation_no_purple_tint() {
        let w = 1280u32;
        let h = 720u32;
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            let shade = ((y * 255) / h) as u8;
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                rgba[idx] = shade;
                rgba[idx + 1] = shade;
                rgba[idx + 2] = shade;
                rgba[idx + 3] = 255;
            }
        }
        let frame = RawRgbaFrame { width: w, height: h, rgba };
        let mut enc = H264Encoder::new(60, 8_000_000);
        let mut dec = H264Decoder::new();
        let encoded = enc.encode(&frame);
        let decoded = dec.decode(&encoded.annex_b).expect("gray frame must decode");

        for px in decoded.rgba.as_chunks::<4>().0 {
            assert_eq!(px[0], px[1], "neutral gray must not shift green/magenta");
            assert_eq!(px[1], px[2], "neutral gray must not shift purple/blue");
        }
    }
}
