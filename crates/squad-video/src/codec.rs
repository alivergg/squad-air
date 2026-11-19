use crate::capture::{RawRgbaFrame, VideoCodec};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType, ImageFormat};
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

const ANNEX_B_START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

const NAL_SPS: u8 = 0x67;
const NAL_PPS: u8 = 0x68;
const NAL_IDR: u8 = 0x65;
const NAL_SLICE: u8 = 0x41;

const NAL_HEVC_VPS: u8 = 0x40;
const NAL_HEVC_SPS: u8 = 0x42;
const NAL_HEVC_PPS: u8 = 0x44;
const NAL_HEVC_IDR: u8 = 0x26;
const NAL_HEVC_SLICE: u8 = 0x02;

const IDR_MODE_DEFLATE_RGB: u8 = 0x01;
const IDR_MODE_JPEG_FULL: u8 = 0x02;
const IDR_MODE_JPEG_STRIPED: u8 = 0x03;

const P_MODE_DEFLATE_TILES: u8 = 0x01;
const P_MODE_JPEG_TILES: u8 = 0x02;

const P_TILE_SIZE: usize = 64;
const TILE_SAD_NOISE_FLOOR: u64 = 64;
const MAX_IDR_PAYLOAD_BYTES: usize = 140 * 1024;
const FLAT_IDR_DEFLATE_LIMIT: usize = 48 * 1024;
const MAX_VIDEO_DIM: u32 = 4096;
const MAX_VIDEO_PIXELS: usize = 3840 * 2160;
const STRIPE_LANES: usize = 4;

#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub is_keyframe: bool,
    pub annex_b: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecoderError {
    UnsupportedCodec(String),
    CorruptBitstream,
}

impl fmt::Display for DecoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecoderError::UnsupportedCodec(msg) => write!(f, "Unsupported video codec: {msg}"),
            DecoderError::CorruptBitstream => write!(f, "Corrupt video bitstream"),
        }
    }
}

impl std::error::Error for DecoderError {}

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

pub fn is_hevc_hardware_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "VideoToolbox", kind = "framework")]
        extern "C" {
            fn VTIsHardwareDecodeSupported(codec_type: u32) -> u8;
        }
        const K_CM_VIDEO_CODEC_TYPE_HEVC: u32 = 0x6876_6331; // 'hvc1'
        unsafe { VTIsHardwareDecodeSupported(K_CM_VIDEO_CODEC_TYPE_HEVC) != 0 }
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE,
        };
        unsafe {
            if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
                if let Ok(adapter) = factory.EnumAdapters1(0) {
                    let mut desc = DXGI_ADAPTER_DESC1::default();
                    if adapter.GetDesc1(&mut desc).is_ok() {
                        return (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) == 0;
                    }
                }
            }
        }
        false
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        true
    }
}

#[derive(Default)]
struct StripeWorkerSlot {
    job_id: u64,
    done_id: u64,
    shutdown: bool,
    is_encode: bool,
    width: u32,
    height: u32,
    quality: u8,
    rgba: Vec<u8>,
    rgb_scratch: Vec<u8>,
    jpeg: Vec<u8>,
    ok: bool,
}

struct StripeWorkerShared {
    state: Mutex<StripeWorkerSlot>,
    wake_worker: Condvar,
    wake_caller: Condvar,
}

struct StripePool {
    workers: Vec<(Arc<StripeWorkerShared>, Option<JoinHandle<()>>)>,
    caller_rgb_scratch: Vec<u8>,
    next_job_id: u64,
}

impl StripePool {
    fn new() -> Self {
        let mut workers = Vec::with_capacity(STRIPE_LANES - 1);
        for idx in 0..(STRIPE_LANES - 1) {
            let shared = Arc::new(StripeWorkerShared {
                state: Mutex::new(StripeWorkerSlot::default()),
                wake_worker: Condvar::new(),
                wake_caller: Condvar::new(),
            });
            let worker_shared = Arc::clone(&shared);
            let handle = thread::Builder::new()
                .name(format!("squad-video-stripe-{idx}"))
                .spawn(move || loop {
                    let mut guard = worker_shared
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    while !guard.shutdown && guard.job_id == guard.done_id {
                        guard = worker_shared
                            .wake_worker
                            .wait(guard)
                            .unwrap_or_else(|e| e.into_inner());
                    }
                    if guard.shutdown {
                        break;
                    }
                    let cur_job = guard.job_id;
                    if guard.is_encode {
                        let w = guard.width;
                        let h = guard.height;
                        let q = guard.quality;
                        let slot = &mut *guard;
                        rgba_to_rgb_slice(&slot.rgba, &mut slot.rgb_scratch);
                        slot.jpeg.clear();
                        slot.ok = JpegEncoder::new_with_quality(&mut slot.jpeg, q)
                            .encode(&slot.rgb_scratch, w, h, ExtendedColorType::Rgb8)
                            .is_ok()
                            && !slot.jpeg.is_empty();
                    } else {
                        let w = guard.width;
                        let h = guard.height;
                        let slot = &mut *guard;
                        slot.ok = decode_jpeg_stripe_into_rgba(&slot.jpeg, w, h, &mut slot.rgba);
                    }
                    guard.done_id = cur_job;
                    drop(guard);
                    worker_shared.wake_caller.notify_one();
                })
                .ok();
            workers.push((shared, handle));
        }
        Self {
            workers,
            caller_rgb_scratch: Vec::new(),
            next_job_id: 0,
        }
    }

    fn encode_stripes(
        &mut self,
        rgba: &[u8],
        width: u32,
        ranges: &[(usize, usize)],
        quality: u8,
        out: &mut Vec<u8>,
    ) -> bool {
        if ranges.is_empty() {
            return false;
        }
        let w = width as usize;
        let active_workers = (ranges.len() - 1).min(self.workers.len());
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        let job_id = self.next_job_id;

        for (w_idx, &(y0, y1)) in ranges.iter().skip(1).take(active_workers).enumerate() {
            let stripe_h = (y1 - y0) as u32;
            let src = &rgba[(y0 * w * 4)..(y1 * w * 4)];
            let (ref shared, ref handle) = self.workers[w_idx];
            if handle.is_some() {
                let mut slot = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                slot.is_encode = true;
                slot.width = width;
                slot.height = stripe_h;
                slot.quality = quality;
                slot.rgba.clear();
                slot.rgba.extend_from_slice(src);
                slot.job_id = job_id;
                drop(slot);
                shared.wake_worker.notify_one();
            }
        }

        let (y0_0, y1_0) = ranges[0];
        let h0 = (y1_0 - y0_0) as u32;
        let src0 = &rgba[(y0_0 * w * 4)..(y1_0 * w * 4)];
        rgba_to_rgb_slice(src0, &mut self.caller_rgb_scratch);
        let mut jpeg0 = Vec::with_capacity((src0.len() / 16).max(1024));
        let ok0 = JpegEncoder::new_with_quality(&mut jpeg0, quality)
            .encode(&self.caller_rgb_scratch, width, h0, ExtendedColorType::Rgb8)
            .is_ok()
            && !jpeg0.is_empty();

        out.clear();
        out.push(IDR_MODE_JPEG_STRIPED);
        out.push(ranges.len() as u8);
        out.extend_from_slice(&(jpeg0.len() as u32).to_be_bytes());
        out.extend_from_slice(&jpeg0);

        let mut all_ok = ok0;
        for (w_idx, &(y0, y1)) in ranges.iter().skip(1).enumerate() {
            if w_idx < active_workers && self.workers[w_idx].1.is_some() {
                let shared = &self.workers[w_idx].0;
                let mut slot = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                while slot.done_id != job_id && !slot.shutdown {
                    slot = shared
                        .wake_caller
                        .wait(slot)
                        .unwrap_or_else(|e| e.into_inner());
                }
                if slot.ok && !slot.jpeg.is_empty() {
                    out.extend_from_slice(&(slot.jpeg.len() as u32).to_be_bytes());
                    out.extend_from_slice(&slot.jpeg);
                } else {
                    all_ok = false;
                }
            } else {
                let sh = (y1 - y0) as u32;
                let src = &rgba[(y0 * w * 4)..(y1 * w * 4)];
                rgba_to_rgb_slice(src, &mut self.caller_rgb_scratch);
                let mut jbuf = Vec::new();
                if JpegEncoder::new_with_quality(&mut jbuf, quality)
                    .encode(&self.caller_rgb_scratch, width, sh, ExtendedColorType::Rgb8)
                    .is_ok()
                    && !jbuf.is_empty()
                {
                    out.extend_from_slice(&(jbuf.len() as u32).to_be_bytes());
                    out.extend_from_slice(&jbuf);
                } else {
                    all_ok = false;
                }
            }
        }

        if !all_ok {
            out.clear();
        }
        all_ok
    }

    fn decode_stripes(
        &mut self,
        width: u32,
        ranges: &[(usize, usize)],
        jpegs: &[&[u8]],
        dst_rgba: &mut [u8],
    ) -> bool {
        if ranges.is_empty() || ranges.len() != jpegs.len() {
            return false;
        }
        let w = width as usize;
        let active_workers = (ranges.len() - 1).min(self.workers.len());
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        let job_id = self.next_job_id;

        for w_idx in 0..active_workers {
            let (y0, y1) = ranges[w_idx + 1];
            let stripe_h = (y1 - y0) as u32;
            let (ref shared, ref handle) = self.workers[w_idx];
            if handle.is_some() {
                let mut slot = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                slot.is_encode = false;
                slot.width = width;
                slot.height = stripe_h;
                slot.jpeg.clear();
                slot.jpeg.extend_from_slice(jpegs[w_idx + 1]);
                slot.job_id = job_id;
                drop(slot);
                shared.wake_worker.notify_one();
            }
        }

        let (y0_0, y1_0) = ranges[0];
        let h0 = (y1_0 - y0_0) as u32;
        let mut all_ok = decode_jpeg_stripe_into_rgba_slice(
            jpegs[0],
            width,
            h0,
            &mut dst_rgba[(y0_0 * w * 4)..(y1_0 * w * 4)],
        );

        for (w_idx, &(y0, y1)) in ranges.iter().skip(1).enumerate() {
            let dst_slice = &mut dst_rgba[(y0 * w * 4)..(y1 * w * 4)];
            if w_idx < active_workers && self.workers[w_idx].1.is_some() {
                let shared = &self.workers[w_idx].0;
                let mut slot = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                while slot.done_id != job_id && !slot.shutdown {
                    slot = shared
                        .wake_caller
                        .wait(slot)
                        .unwrap_or_else(|e| e.into_inner());
                }
                if slot.ok && slot.rgba.len() == dst_slice.len() {
                    dst_slice.copy_from_slice(&slot.rgba);
                } else {
                    all_ok = false;
                }
            } else {
                let sh = (y1 - y0) as u32;
                if !decode_jpeg_stripe_into_rgba_slice(jpegs[w_idx + 1], width, sh, dst_slice) {
                    all_ok = false;
                }
            }
        }

        all_ok
    }
}

impl Drop for StripePool {
    fn drop(&mut self) {
        for (shared, _) in &self.workers {
            let mut slot = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            slot.shutdown = true;
            drop(slot);
            shared.wake_worker.notify_all();
        }
        for (_, handle) in &mut self.workers {
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

fn compute_stripe_ranges(height: usize, requested_stripes: usize) -> Vec<(usize, usize)> {
    let stripes = requested_stripes.clamp(1, 8);
    if stripes <= 1 || height < stripes * 16 {
        return vec![(0, height)];
    }
    let base = ((height / stripes) & !15).max(16);
    let mut ranges = Vec::with_capacity(stripes);
    let mut y = 0usize;
    for idx in 0..stripes {
        let next_y = if idx + 1 == stripes {
            height
        } else {
            (y + base).min(height)
        };
        if next_y > y {
            ranges.push((y, next_y));
        }
        y = next_y;
    }
    ranges
}

fn rgba_to_rgb_slice(rgba: &[u8], out_rgb: &mut Vec<u8>) {
    let pixels = rgba.len() / 4;
    out_rgb.resize(pixels * 3, 0);
    for (src, dst) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out_rgb.as_chunks_mut::<3>().0.iter_mut())
    {
        dst[0] = src[0];
        dst[1] = src[1];
        dst[2] = src[2];
    }
}

fn decode_jpeg_stripe_into_rgba(
    jpeg_bytes: &[u8],
    width: u32,
    height: u32,
    out_rgba: &mut Vec<u8>,
) -> bool {
    let Some(expected) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
    else {
        return false;
    };
    out_rgba.resize(expected, 255);
    decode_jpeg_stripe_into_rgba_slice(jpeg_bytes, width, height, out_rgba)
}

fn decode_jpeg_stripe_into_rgba_slice(
    jpeg_bytes: &[u8],
    width: u32,
    height: u32,
    dst_rgba: &mut [u8],
) -> bool {
    let Some(img) = decode_jpeg_bounded(jpeg_bytes, MAX_VIDEO_DIM, MAX_VIDEO_DIM) else {
        return false;
    };
    let rgb = if img.width() != width || img.height() != height {
        img.resize_exact(width, height, image::imageops::FilterType::Triangle)
            .into_rgb8()
    } else {
        img.into_rgb8()
    };
    let raw = rgb.as_raw();
    if raw.len() * 4 != dst_rgba.len() * 3 {
        return false;
    }
    for (src, dst) in raw
        .as_chunks::<3>()
        .0
        .iter()
        .zip(dst_rgba.as_chunks_mut::<4>().0.iter_mut())
    {
        dst[0] = src[0];
        dst[1] = src[1];
        dst[2] = src[2];
        dst[3] = 255;
    }
    true
}

fn is_all_achromatic_rgba(rgba: &[u8]) -> bool {
    for px in rgba.as_chunks::<4>().0 {
        if px[0] != px[1] || px[1] != px[2] {
            return false;
        }
    }
    true
}

fn is_flat_rgba_strided(rgba: &[u8], w: usize, h: usize) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    let mut sampled = 0usize;
    let mut transitions = 0usize;
    for y in (0..h).step_by(8) {
        let row_base = y * w * 4;
        let mut prev = [rgba[row_base], rgba[row_base + 1], rgba[row_base + 2]];
        for x in (0..w).step_by(4) {
            let idx = row_base + x * 4;
            let cur = [rgba[idx], rgba[idx + 1], rgba[idx + 2]];
            if x > 0 && cur != prev {
                transitions += 1;
            }
            prev = cur;
            sampled += 1;
        }
    }
    sampled > 0 && transitions * 10 <= sampled
}

pub struct H264Encoder {
    fps: u8,
    bitrate_bps: u32,
    codec: VideoCodec,
    gop_len: u64,
    frame_index: u64,
    force_idr: bool,
    last_idr_quality: u8,
    last_p_quality: u8,
    width: u32,
    height: u32,
    ref_rgba: Vec<u8>,
    scratch_rgb: Vec<u8>,
    scratch_dpcm: Vec<u8>,
    scratch_strip: Vec<u8>,
    stripe_pool: StripePool,
}

impl H264Encoder {
    pub fn new(fps: u8, bitrate_bps: u32) -> Self {
        Self::new_with_codec(fps, bitrate_bps, VideoCodec::H264)
    }

    pub fn new_with_codec(fps: u8, bitrate_bps: u32, codec: VideoCodec) -> Self {
        let clamped_fps = fps.max(1);
        let clamped_bitrate = bitrate_bps.max(500_000);
        let init_p_q = match (codec, clamped_bitrate) {
            (VideoCodec::H265, b) if b >= 12_000_000 => 82,
            (VideoCodec::H265, b) if b >= 8_000_000 => 76,
            (VideoCodec::H265, _) => 70,
            (VideoCodec::H264, b) if b >= 12_000_000 => 78,
            (VideoCodec::H264, b) if b >= 8_000_000 => 72,
            (VideoCodec::H264, _) => 66,
        };
        Self {
            fps: clamped_fps,
            bitrate_bps: clamped_bitrate,
            codec,
            gop_len: u64::from(clamped_fps) * 2,
            frame_index: 0,
            force_idr: true,
            last_idr_quality: 68,
            last_p_quality: init_p_q,
            width: 0,
            height: 0,
            ref_rgba: Vec::new(),
            scratch_rgb: Vec::new(),
            scratch_dpcm: Vec::new(),
            scratch_strip: Vec::new(),
            stripe_pool: StripePool::new(),
        }
    }

    pub fn set_codec(&mut self, codec: VideoCodec) {
        if self.codec != codec {
            self.codec = codec;
            self.force_idr = true;
        }
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    pub fn force_keyframe(&mut self) {
        self.force_idr = true;
    }

    pub fn encode(&mut self, frame: &RawRgbaFrame) -> EncodedVideoFrame {
        let width = frame.width.max(2);
        let height = frame.height.max(2);
        let expected_rgba_len = (width as usize) * (height as usize) * 4;
        let dims_changed = width != self.width
            || height != self.height
            || self.ref_rgba.len() != expected_rgba_len;

        let mut is_keyframe =
            self.force_idr || dims_changed || self.frame_index.is_multiple_of(self.gop_len);

        self.width = width;
        self.height = height;
        self.frame_index = self.frame_index.wrapping_add(1);

        let p_payload = if is_keyframe {
            None
        } else {
            self.try_encode_p_payload(frame)
        };

        if p_payload.is_none() {
            is_keyframe = true;
        }

        let mut annex_b = Vec::with_capacity(8192);
        if is_keyframe {
            self.force_idr = false;
            let idr_payload = self.encode_idr_payload(frame);
            self.emit_keyframe_nals(&mut annex_b, width, height, &idr_payload);
        } else if let Some(p_bytes) = p_payload {
            let nal_type = match self.codec {
                VideoCodec::H264 => NAL_SLICE,
                VideoCodec::H265 => NAL_HEVC_SLICE,
            };
            write_nal(&mut annex_b, nal_type, &p_bytes);
        }

        EncodedVideoFrame {
            is_keyframe,
            annex_b,
        }
    }

    fn emit_keyframe_nals(&self, out: &mut Vec<u8>, width: u32, height: u32, idr_payload: &[u8]) {
        let sps = encode_sps(width, height, self.fps);
        let pps = [self.codec.as_u8(), (self.bitrate_bps / 1_000_000) as u8];
        match self.codec {
            VideoCodec::H264 => {
                write_nal(out, NAL_SPS, &sps);
                write_nal(out, NAL_PPS, &pps);
                write_nal(out, NAL_IDR, idr_payload);
            }
            VideoCodec::H265 => {
                write_nal(out, NAL_HEVC_VPS, &[0x01, self.fps]);
                write_nal(out, NAL_HEVC_SPS, &sps);
                write_nal(out, NAL_HEVC_PPS, &pps);
                write_nal(out, NAL_HEVC_IDR, idr_payload);
            }
        }
    }

    fn p_budget_bytes(&self) -> usize {
        let base = if self.bitrate_bps >= 12_000_000 {
            48_000
        } else if self.bitrate_bps >= 8_000_000 {
            34_000
        } else {
            24_000
        };
        if self.codec == VideoCodec::H265 {
            (base * 115) / 100
        } else {
            base
        }
    }

    fn max_idr_quality(&self) -> u8 {
        if self.bitrate_bps >= 12_000_000 {
            82
        } else if self.bitrate_bps >= 8_000_000 {
            78
        } else {
            72
        }
    }

    fn p_tile_quality(&self) -> u8 {
        match (self.codec, self.bitrate_bps) {
            (VideoCodec::H265, b) if b >= 12_000_000 => 84,
            (VideoCodec::H265, b) if b >= 8_000_000 => 78,
            (VideoCodec::H265, _) => 72,
            (VideoCodec::H264, b) if b >= 12_000_000 => 80,
            (VideoCodec::H264, b) if b >= 8_000_000 => 74,
            (VideoCodec::H264, _) => 68,
        }
    }

    fn update_ref_rgba(&mut self, frame: &RawRgbaFrame) -> (bool, bool) {
        let w = self.width as usize;
        let h = self.height as usize;
        let pixel_count = w * h;
        let rgba_len = pixel_count * 4;

        self.ref_rgba.resize(rgba_len, 255);
        let copy_len = frame.rgba.len().min(rgba_len);
        self.ref_rgba[..copy_len].copy_from_slice(&frame.rgba[..copy_len]);
        if copy_len < rgba_len {
            self.ref_rgba[copy_len..].fill(255);
        }

        let all_achromatic = is_all_achromatic_rgba(&self.ref_rgba);
        let is_flat = all_achromatic || is_flat_rgba_strided(&self.ref_rgba, w, h);
        (all_achromatic, is_flat)
    }

    fn encode_jpeg_idr_with_quality(&mut self, w: u32, h: u32, quality: u8, out: &mut Vec<u8>) {
        out.clear();
        if w >= 256 && h >= 256 {
            let ranges = compute_stripe_ranges(h as usize, STRIPE_LANES);
            if ranges.len() >= 2
                && self
                    .stripe_pool
                    .encode_stripes(&self.ref_rgba, w, &ranges, quality, out)
            {
                return;
            }
        }

        rgba_to_rgb_slice(&self.ref_rgba, &mut self.scratch_rgb);
        let mut jpeg_buf = Vec::new();
        if JpegEncoder::new_with_quality(&mut jpeg_buf, quality)
            .encode(&self.scratch_rgb, w, h, ExtendedColorType::Rgb8)
            .is_ok()
            && !jpeg_buf.is_empty()
        {
            out.reserve(1 + jpeg_buf.len());
            out.push(IDR_MODE_JPEG_FULL);
            out.extend_from_slice(&jpeg_buf);
        }
    }

    fn encode_idr_payload(&mut self, frame: &RawRgbaFrame) -> Vec<u8> {
        let w = self.width;
        let h = self.height;
        let (all_achromatic, is_flat) = self.update_ref_rgba(frame);

        if is_flat || all_achromatic {
            rgba_to_rgb_slice(&self.ref_rgba, &mut self.scratch_rgb);
            dpcm_row_sub_rgb(&self.scratch_rgb, w as usize, &mut self.scratch_dpcm);
            if let Some(compressed) = deflate_fast(&self.scratch_dpcm) {
                let limit = if all_achromatic {
                    MAX_IDR_PAYLOAD_BYTES
                } else {
                    FLAT_IDR_DEFLATE_LIMIT
                };
                if compressed.len() < limit {
                    let mut out = Vec::with_capacity(1 + compressed.len());
                    out.push(IDR_MODE_DEFLATE_RGB);
                    out.extend_from_slice(&compressed);
                    return out;
                }
            }
        }

        let max_q = self.max_idr_quality();
        let q1 = self.last_idr_quality.clamp(24, max_q);
        let per_frame = ((u64::from(self.bitrate_bps) / 8) / u64::from(self.fps.max(1))) as usize;
        let target = (per_frame * 2).clamp(28_000, 96_000);

        let mut out = Vec::with_capacity(target.min(MAX_IDR_PAYLOAD_BYTES));
        self.encode_jpeg_idr_with_quality(w, h, q1, &mut out);

        if out.is_empty() || out.len() >= MAX_IDR_PAYLOAD_BYTES {
            let q2 = q1.saturating_sub(18).max(22);
            self.last_idr_quality = q2.max(24);
            self.encode_jpeg_idr_with_quality(w, h, q2, &mut out);

            if out.is_empty() || out.len() >= MAX_IDR_PAYLOAD_BYTES {
                let mut fallback = Vec::new();
                self.encode_jpeg_idr_with_quality(w, h, 14, &mut fallback);
                if !fallback.is_empty() {
                    out = fallback;
                    self.last_idr_quality = 24;
                }
            }
        } else if out.len() > (target * 12) / 10 {
            self.last_idr_quality = q1.saturating_sub(5).max(24);
        } else if out.len() > target {
            self.last_idr_quality = q1.saturating_sub(2).max(24);
        } else if out.len() < (target * 75) / 100 {
            self.last_idr_quality = (q1 + 3).min(max_q);
        }

        out
    }

    fn try_encode_p_payload(&mut self, frame: &RawRgbaFrame) -> Option<Vec<u8>> {
        let w = self.width as usize;
        let h = self.height as usize;
        let expected = w * h * 4;
        if frame.rgba.len() < expected || self.ref_rgba.len() < expected {
            return None;
        }

        let tiles_x = w.div_ceil(P_TILE_SIZE);
        let tiles_y = h.div_ceil(P_TILE_SIZE);
        let total_tiles = tiles_x * tiles_y;
        let max_p_tiles = (total_tiles / 4).clamp(16, 128);
        let mut changed: Vec<u16> = Vec::with_capacity(max_p_tiles + 1);

        for ty in 0..tiles_y {
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(h);
            for tx in 0..tiles_x {
                let x0 = tx * P_TILE_SIZE;
                let x1 = (x0 + P_TILE_SIZE).min(w);
                let mut sad = 0u64;
                let mut tile_dirty = false;

                'tile_rows: for y in y0..y1 {
                    let row_start = (y * w + x0) * 4;
                    let row_end = (y * w + x1) * 4;
                    let cur = &frame.rgba[row_start..row_end];
                    let prev = &self.ref_rgba[row_start..row_end];
                    for (c, p) in cur
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(prev.as_chunks::<4>().0.iter())
                    {
                        sad += u64::from(c[0].abs_diff(p[0]))
                            + u64::from(c[1].abs_diff(p[1]))
                            + u64::from(c[2].abs_diff(p[2]));
                        if sad >= TILE_SAD_NOISE_FLOOR {
                            tile_dirty = true;
                            break 'tile_rows;
                        }
                    }
                }

                if tile_dirty {
                    changed.push((ty * tiles_x + tx) as u16);
                    if changed.len() > max_p_tiles {
                        return None;
                    }
                }
            }
        }

        if changed.is_empty() {
            return Some(vec![0x00, 0x00, P_MODE_JPEG_TILES]);
        }

        let budget = self.p_budget_bytes();
        let max_pq = self.p_tile_quality();
        let quality = self.last_p_quality.clamp(36, max_pq);
        let candidate = self.serialize_p_tiles(&frame.rgba, w, h, tiles_x, &changed, quality);

        if candidate.is_empty() || candidate.len() > budget {
            self.last_p_quality = quality.saturating_sub(6).max(36);
            return None;
        }

        if candidate.len() < (budget * 65) / 100 {
            self.last_p_quality = (quality + 2).min(max_pq);
        } else if candidate.len() > (budget * 90) / 100 {
            self.last_p_quality = quality.saturating_sub(2).max(36);
        }

        for &tid in &changed {
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(h);
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(w);
            for y in y0..y1 {
                let row_start = (y * w + x0) * 4;
                let row_end = (y * w + x1) * 4;
                self.ref_rgba[row_start..row_end].copy_from_slice(&frame.rgba[row_start..row_end]);
            }
        }

        Some(candidate)
    }

    fn serialize_p_tiles(
        &mut self,
        rgba: &[u8],
        w: usize,
        h: usize,
        tiles_x: usize,
        tiles: &[u16],
        jpeg_quality: u8,
    ) -> Vec<u8> {
        let count = tiles.len();
        let strip_h = count * P_TILE_SIZE;
        self.scratch_strip.resize(P_TILE_SIZE * strip_h * 3, 0);

        for (slot, &tid) in tiles.iter().enumerate() {
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(w);
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(h);
            let slot_base_row = slot * P_TILE_SIZE;

            for dy in 0..P_TILE_SIZE {
                let sy = (y0 + dy).min(y1.saturating_sub(1));
                let dst_row = (slot_base_row + dy) * P_TILE_SIZE * 3;
                for dx in 0..P_TILE_SIZE {
                    let sx = (x0 + dx).min(x1.saturating_sub(1));
                    let sidx = (sy * w + sx) * 4;
                    let didx = dst_row + dx * 3;
                    self.scratch_strip[didx] = rgba[sidx];
                    self.scratch_strip[didx + 1] = rgba[sidx + 1];
                    self.scratch_strip[didx + 2] = rgba[sidx + 2];
                }
            }
        }

        let mut jpeg_buf = Vec::with_capacity(count * 512);
        let mut jpeg_ok = false;
        {
            let mut enc = JpegEncoder::new_with_quality(&mut jpeg_buf, jpeg_quality);
            if enc
                .encode(
                    &self.scratch_strip,
                    P_TILE_SIZE as u32,
                    strip_h as u32,
                    ExtendedColorType::Rgb8,
                )
                .is_ok()
            {
                jpeg_ok = true;
            }
        }

        let mut use_deflate = false;
        let mut chosen = jpeg_buf;
        if !jpeg_ok {
            dpcm_row_sub_rgb(&self.scratch_strip, P_TILE_SIZE, &mut self.scratch_dpcm);
            if let Some(def_buf) = deflate_fast(&self.scratch_dpcm) {
                chosen = def_buf;
                use_deflate = true;
            }
        }

        let mut out = Vec::with_capacity(3 + count * 2 + chosen.len());
        out.extend_from_slice(&(count as u16).to_be_bytes());
        out.push(if use_deflate {
            P_MODE_DEFLATE_TILES
        } else {
            P_MODE_JPEG_TILES
        });
        for &tid in tiles {
            out.extend_from_slice(&tid.to_be_bytes());
        }
        out.extend_from_slice(&chosen);
        out
    }
}

pub type SharedRgbaFrame = (u32, u32, Arc<Vec<u8>>);

pub struct H264Decoder {
    width: u32,
    height: u32,
    fps: u8,
    has_idr: bool,
    hevc_decode_supported: bool,
    frame_rgba: Arc<Vec<u8>>,
    stripe_pool: StripePool,
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
            hevc_decode_supported: is_hevc_hardware_supported(),
            frame_rgba: Arc::new(Vec::new()),
            stripe_pool: StripePool::new(),
        }
    }

    pub fn set_hevc_decode_supported(&mut self, supported: bool) {
        self.hevc_decode_supported = supported;
    }

    pub fn is_hevc_decode_supported(&self) -> bool {
        self.hevc_decode_supported
    }

    pub fn reset(&mut self) {
        self.has_idr = false;
        Arc::make_mut(&mut self.frame_rgba).clear();
    }

    pub fn reset_and_shrink(&mut self) {
        self.has_idr = false;
        self.width = 0;
        self.height = 0;
        self.frame_rgba = Arc::new(Vec::new());
    }

    pub fn decode(&mut self, annex_b: &[u8]) -> Option<RawRgbaFrame> {
        self.decode_checked(annex_b).ok().flatten()
    }

    pub fn decode_checked(&mut self, annex_b: &[u8]) -> Result<Option<RawRgbaFrame>, DecoderError> {
        self.decode_checked_shared(annex_b).map(|opt| {
            opt.map(|(width, height, rgba)| RawRgbaFrame {
                width,
                height,
                rgba: (*rgba).clone(),
            })
        })
    }

    pub fn decode_checked_shared(
        &mut self,
        annex_b: &[u8],
    ) -> Result<Option<SharedRgbaFrame>, DecoderError> {
        let nals = split_annex_b_nals(annex_b);
        if nals.is_empty() {
            return Err(DecoderError::CorruptBitstream);
        }

        let mut prev_state = (self.width, self.height, self.fps, self.has_idr);
        match self.decode_nals(&nals, &mut prev_state) {
            Ok(true) => Ok(Some((
                self.width,
                self.height,
                Arc::clone(&self.frame_rgba),
            ))),
            Ok(false) => Ok(None),
            Err(err) => {
                (self.width, self.height, self.fps, self.has_idr) = prev_state;
                Err(err)
            }
        }
    }

    fn decode_nals(
        &mut self,
        nals: &[&[u8]],
        prev_state: &mut (u32, u32, u8, bool),
    ) -> Result<bool, DecoderError> {
        let mut produced = false;
        for nal in nals {
            if nal.is_empty() {
                continue;
            }
            let nal_type = nal[0];
            let is_hevc_nal = matches!(
                nal_type,
                NAL_HEVC_VPS | NAL_HEVC_SPS | NAL_HEVC_PPS | NAL_HEVC_IDR | NAL_HEVC_SLICE
            );
            if is_hevc_nal && !self.hevc_decode_supported {
                return Err(DecoderError::UnsupportedCodec(
                    "HEVC decoder initialization failed on viewer".to_string(),
                ));
            }

            let rbsp = unescape_rbsp(&nal[1..]);
            match nal_type {
                NAL_HEVC_VPS => {}
                NAL_SPS | NAL_HEVC_SPS => {
                    let Some((w, h, fps)) = decode_sps(&rbsp) else {
                        return Err(DecoderError::CorruptBitstream);
                    };
                    if w != self.width || h != self.height {
                        self.has_idr = false;
                    }
                    self.width = w;
                    self.height = h;
                    self.fps = fps;
                }
                NAL_PPS | NAL_HEVC_PPS => {}
                NAL_IDR | NAL_HEVC_IDR => {
                    if self.width > 0 && self.height > 0 {
                        if self.decode_idr(&rbsp) {
                            self.has_idr = true;
                            produced = true;
                            *prev_state = (self.width, self.height, self.fps, true);
                        } else {
                            return Err(DecoderError::CorruptBitstream);
                        }
                    }
                }
                NAL_SLICE | NAL_HEVC_SLICE if self.has_idr => {
                    if self.decode_p(&rbsp) {
                        produced = true;
                    } else {
                        return Err(DecoderError::CorruptBitstream);
                    }
                }
                _ => {}
            }
        }
        Ok(produced)
    }

    fn prepare_idr_rgba_buf(&mut self, expected_rgba: usize) -> &mut Vec<u8> {
        if Arc::strong_count(&self.frame_rgba) != 1 {
            self.frame_rgba = Arc::new(vec![255u8; expected_rgba]);
        }
        let buf = Arc::get_mut(&mut self.frame_rgba).expect("unique Arc");
        buf.resize(expected_rgba, 255);
        buf
    }

    fn decode_idr(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 2 {
            return false;
        }
        let mode = payload[0];
        let body = &payload[1..];
        let w = self.width as usize;
        let h = self.height as usize;
        let Some(pixels) = w.checked_mul(h) else {
            return false;
        };
        let Some(expected_rgb) = pixels.checked_mul(3) else {
            return false;
        };
        let Some(expected_rgba) = pixels.checked_mul(4) else {
            return false;
        };

        match mode {
            IDR_MODE_DEFLATE_RGB => {
                let Ok(dpcm) = inflate_bounded(body, expected_rgb) else {
                    return false;
                };
                if dpcm.len() < expected_rgb {
                    return false;
                }
                let dst = self.prepare_idr_rgba_buf(expected_rgba);
                dpcm_row_unsub_rgb_to_rgba(&dpcm[..expected_rgb], w, dst);
                true
            }
            IDR_MODE_JPEG_FULL => {
                let Some(img) = decode_jpeg_bounded(body, MAX_VIDEO_DIM, MAX_VIDEO_DIM) else {
                    return false;
                };
                let rgba = if img.width() != self.width || img.height() != self.height {
                    let resized = img.resize_exact(
                        self.width,
                        self.height,
                        image::imageops::FilterType::Triangle,
                    );
                    resized.to_rgba8().into_raw()
                } else {
                    img.to_rgba8().into_raw()
                };
                if rgba.len() != expected_rgba {
                    return false;
                }
                if let Some(buf) = Arc::get_mut(&mut self.frame_rgba) {
                    *buf = rgba;
                } else {
                    self.frame_rgba = Arc::new(rgba);
                }
                true
            }
            IDR_MODE_JPEG_STRIPED => {
                if body.is_empty() {
                    return false;
                }
                let stripe_count = body[0] as usize;
                if !(2..=8).contains(&stripe_count) || h < stripe_count * 16 {
                    return false;
                }
                let ranges = compute_stripe_ranges(h, stripe_count);
                if ranges.len() != stripe_count {
                    return false;
                }
                let mut cursor = 1usize;
                let mut jpegs: Vec<&[u8]> = Vec::with_capacity(stripe_count);
                for _ in 0..stripe_count {
                    if cursor + 4 > body.len() {
                        return false;
                    }
                    let slen = u32::from_be_bytes([
                        body[cursor],
                        body[cursor + 1],
                        body[cursor + 2],
                        body[cursor + 3],
                    ]) as usize;
                    cursor += 4;
                    if slen == 0 || cursor + slen > body.len() {
                        return false;
                    }
                    jpegs.push(&body[cursor..cursor + slen]);
                    cursor += slen;
                }
                if cursor != body.len() {
                    return false;
                }

                let width = self.width;
                let mut decoded_rgba = if Arc::strong_count(&self.frame_rgba) == 1
                    && self.frame_rgba.len() == expected_rgba
                {
                    let mut reused = Vec::new();
                    std::mem::swap(&mut reused, Arc::get_mut(&mut self.frame_rgba).unwrap());
                    reused
                } else {
                    vec![255u8; expected_rgba]
                };
                decoded_rgba.resize(expected_rgba, 255);

                if !self
                    .stripe_pool
                    .decode_stripes(width, &ranges, &jpegs, &mut decoded_rgba)
                {
                    return false;
                }

                if let Some(buf) = Arc::get_mut(&mut self.frame_rgba) {
                    *buf = decoded_rgba;
                } else {
                    self.frame_rgba = Arc::new(decoded_rgba);
                }
                true
            }
            _ => false,
        }
    }

    fn decode_p(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 3 {
            return false;
        }
        let count = usize::from(u16::from_be_bytes([payload[0], payload[1]]));
        if count == 0 {
            return true;
        }
        let mode = payload[2];

        let w = self.width as usize;
        let h = self.height as usize;
        let tiles_x = w.div_ceil(P_TILE_SIZE);
        let tiles_y = h.div_ceil(P_TILE_SIZE);
        if tiles_x == 0 || tiles_y == 0 {
            return false;
        }
        let max_tiles = tiles_x.saturating_mul(tiles_y);
        if count > max_tiles || count > 256 {
            return false;
        }

        let header_bytes = 3 + count * 2;
        if payload.len() < header_bytes {
            return false;
        }
        let Some(expected_rgba) = w.checked_mul(h).and_then(|px| px.checked_mul(4)) else {
            return false;
        };
        if self.frame_rgba.len() < expected_rgba {
            return false;
        }

        let mut tile_ids = Vec::with_capacity(count);
        for i in 0..count {
            let tid = u16::from_be_bytes([payload[3 + i * 2], payload[3 + i * 2 + 1]]);
            let ty = usize::from(tid) / tiles_x;
            if ty >= tiles_y {
                return false;
            }
            tile_ids.push(tid);
        }

        let strip_w = P_TILE_SIZE;
        let strip_h = count * P_TILE_SIZE;
        let compressed = &payload[header_bytes..];

        let strip_rgb: Vec<u8> = match mode {
            P_MODE_DEFLATE_TILES => {
                let Some(expected) = strip_w
                    .checked_mul(strip_h)
                    .and_then(|px| px.checked_mul(3))
                else {
                    return false;
                };
                let Ok(dpcm) = inflate_bounded(compressed, expected) else {
                    return false;
                };
                if dpcm.len() < expected {
                    return false;
                }
                dpcm_row_unsub_rgb(&dpcm[..expected], strip_w)
            }
            P_MODE_JPEG_TILES => {
                let Some(img) = decode_jpeg_bounded(compressed, strip_w as u32, strip_h as u32)
                else {
                    return false;
                };
                let rgb = img.to_rgb8();
                if rgb.width() as usize != strip_w || rgb.height() as usize != strip_h {
                    return false;
                }
                rgb.into_raw()
            }
            _ => return false,
        };

        let dst_rgba = Arc::make_mut(&mut self.frame_rgba);
        for (slot, tid) in tile_ids.into_iter().enumerate() {
            let tx = usize::from(tid) % tiles_x;
            let ty = usize::from(tid) / tiles_x;
            let x0 = tx * P_TILE_SIZE;
            let x1 = (x0 + P_TILE_SIZE).min(w);
            let y0 = ty * P_TILE_SIZE;
            let y1 = (y0 + P_TILE_SIZE).min(h);
            let slot_base_row = slot * P_TILE_SIZE;

            for dy in 0..(y1 - y0) {
                let src_row = (slot_base_row + dy) * P_TILE_SIZE * 3;
                let dst_row = ((y0 + dy) * w + x0) * 4;
                for dx in 0..(x1 - x0) {
                    let sidx = src_row + dx * 3;
                    let didx = dst_row + dx * 4;
                    dst_rgba[didx] = strip_rgb[sidx];
                    dst_rgba[didx + 1] = strip_rgb[sidx + 1];
                    dst_rgba[didx + 2] = strip_rgb[sidx + 2];
                    dst_rgba[didx + 3] = 255;
                }
            }
        }

        true
    }
}

fn deflate_fast(input: &[u8]) -> Option<Vec<u8>> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(input).ok()?;
    enc.finish().ok()
}

fn decode_jpeg_bounded(
    bytes: &[u8],
    max_width: u32,
    max_height: u32,
) -> Option<image::DynamicImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_width);
    limits.max_image_height = Some(max_height);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg);
    reader.limits(limits);
    reader.decode().ok()
}

fn inflate_bounded(input: &[u8], expected_len: usize) -> Result<Vec<u8>, ()> {
    if expected_len > MAX_VIDEO_PIXELS * 3 {
        return Err(());
    }
    let mut dec = DeflateDecoder::new(Cursor::new(input)).take((expected_len as u64) + 1);
    let mut out = Vec::with_capacity(expected_len.min(64 * 1024));
    dec.read_to_end(&mut out).map_err(|_| ())?;
    if out.len() != expected_len {
        return Err(());
    }
    Ok(out)
}

fn dpcm_row_sub_rgb(rgb: &[u8], width: usize, out: &mut Vec<u8>) {
    out.resize(rgb.len(), 0);
    let row_bytes = width * 3;
    for (src_row, dst_row) in rgb
        .chunks_exact(row_bytes)
        .zip(out.chunks_exact_mut(row_bytes))
    {
        let mut prev = [0u8; 3];
        for (s, d) in src_row
            .as_chunks::<3>()
            .0
            .iter()
            .zip(dst_row.as_chunks_mut::<3>().0.iter_mut())
        {
            d[0] = s[0].wrapping_sub(prev[0]);
            d[1] = s[1].wrapping_sub(prev[1]);
            d[2] = s[2].wrapping_sub(prev[2]);
            prev = *s;
        }
    }
}

fn dpcm_row_unsub_rgb(dpcm: &[u8], width: usize) -> Vec<u8> {
    let mut out = vec![0u8; dpcm.len()];
    let row_bytes = width * 3;
    for (src_row, dst_row) in dpcm
        .chunks_exact(row_bytes)
        .zip(out.chunks_exact_mut(row_bytes))
    {
        let mut prev = [0u8; 3];
        for (s, d) in src_row
            .as_chunks::<3>()
            .0
            .iter()
            .zip(dst_row.as_chunks_mut::<3>().0.iter_mut())
        {
            let r = prev[0].wrapping_add(s[0]);
            let g = prev[1].wrapping_add(s[1]);
            let b = prev[2].wrapping_add(s[2]);
            d[0] = r;
            d[1] = g;
            d[2] = b;
            prev = [r, g, b];
        }
    }
    out
}

fn dpcm_row_unsub_rgb_to_rgba(dpcm: &[u8], width: usize, out_rgba: &mut [u8]) {
    let src_row_bytes = width * 3;
    let dst_row_bytes = width * 4;
    for (src_row, dst_row) in dpcm
        .chunks_exact(src_row_bytes)
        .zip(out_rgba.chunks_exact_mut(dst_row_bytes))
    {
        let mut prev = [0u8; 3];
        for (s, d) in src_row
            .as_chunks::<3>()
            .0
            .iter()
            .zip(dst_row.as_chunks_mut::<4>().0.iter_mut())
        {
            let r = prev[0].wrapping_add(s[0]);
            let g = prev[1].wrapping_add(s[1]);
            let b = prev[2].wrapping_add(s[2]);
            d[0] = r;
            d[1] = g;
            d[2] = b;
            d[3] = 255;
            prev = [r, g, b];
        }
    }
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
    let mut out = Vec::with_capacity(8);
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
    if width == 0
        || height == 0
        || width > MAX_VIDEO_DIM
        || height > MAX_VIDEO_DIM
        || (width as usize).saturating_mul(height as usize) > MAX_VIDEO_PIXELS
    {
        return None;
    }
    let fps = rbsp[7].max(1);
    Some((width, height, fps))
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
        let mut enc = H264Encoder::new(60, 8_000_000);
        let mut dec = H264Decoder::new();

        let f1 = cap.next_frame();
        let e1 = enc.encode(&f1);
        assert!(e1.is_keyframe);
        assert!(e1.annex_b.starts_with(&ANNEX_B_START_CODE));
        assert!(
            e1.annex_b.len() <= 165 * 1024,
            "1:1 1080p IDR frame size {} exceeds 165 KB limit",
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
            e2.annex_b.len() <= 40 * 1024,
            "P frame size {} exceeds 40 KB limit",
            e2.annex_b.len()
        );
        let d2 = dec.decode(&e2.annex_b).expect("P frame must decode");
        assert_eq!(d2.width, 1920);
        assert_eq!(d2.height, 1080);

        enc.force_keyframe();
        let f3 = cap.next_frame();
        let e3 = enc.encode(&f3);
        assert!(e3.is_keyframe);
        assert!(e3.annex_b.len() <= 165 * 1024);
        let d3 = dec.decode(&e3.annex_b).expect("Forced IDR must decode");
        assert_eq!(d3.width, 1920);
        assert_eq!(d3.height, 1080);
    }

    #[test]
    fn test_h265_full_res_1080p_and_unsupported_fallback_to_h264() {
        let mut cap = SyntheticPatternCapture::new(1920, 1080);
        let mut enc = H264Encoder::new_with_codec(60, 8_000_000, VideoCodec::H265);
        assert_eq!(enc.codec(), VideoCodec::H265);

        let f1 = cap.next_frame();
        let e_hevc = enc.encode(&f1);
        assert!(e_hevc.is_keyframe);
        assert!(e_hevc.annex_b.len() <= 165 * 1024);

        let mut dec = H264Decoder::new();
        dec.set_hevc_decode_supported(true);
        let ok_hevc = dec
            .decode_checked(&e_hevc.annex_b)
            .expect("supported HEVC must decode")
            .expect("HEVC IDR must produce frame");
        assert_eq!(ok_hevc.width, 1920);
        assert_eq!(ok_hevc.height, 1080);

        let mut unsupported_dec = H264Decoder::new();
        unsupported_dec.set_hevc_decode_supported(false);
        let err = unsupported_dec.decode_checked(&e_hevc.annex_b).unwrap_err();
        assert!(matches!(err, DecoderError::UnsupportedCodec(_)));

        // Switch encoder to H.264 on error and confirm clean full-res decode.
        enc.set_codec(VideoCodec::H264);
        let f2 = cap.next_frame();
        let e_h264 = enc.encode(&f2);
        assert!(e_h264.is_keyframe);
        let ok_h264 = unsupported_dec
            .decode_checked(&e_h264.annex_b)
            .expect("H.264 fallback must decode on all viewers")
            .expect("H.264 fallback IDR must produce frame");
        assert_eq!(ok_h264.width, 1920);
        assert_eq!(ok_h264.height, 1080);
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
        let frame = RawRgbaFrame {
            width: w,
            height: h,
            rgba,
        };
        let mut enc = H264Encoder::new(60, 8_000_000);
        let mut dec = H264Decoder::new();
        let encoded = enc.encode(&frame);
        let decoded = dec
            .decode(&encoded.annex_b)
            .expect("gray frame must decode");

        for px in decoded.rgba.as_chunks::<4>().0 {
            assert_eq!(px[0], px[1], "neutral gray must not shift green/magenta");
            assert_eq!(px[1], px[2], "neutral gray must not shift purple/blue");
        }
    }

    #[test]
    fn test_decoder_rejects_oversized_sps() {
        assert!(decode_sps(&encode_sps(0, 1080, 60)).is_none());
        assert!(decode_sps(&encode_sps(4097, 64, 60)).is_none());
        assert!(decode_sps(&encode_sps(4096, 4096, 60)).is_none());
        assert!(decode_sps(&encode_sps(3840, 2160, 60)).is_some());

        let mut dec = H264Decoder::new();
        let mut annex_b = Vec::new();
        write_nal(&mut annex_b, NAL_SPS, &encode_sps(65535, 65535, 60));
        write_nal(&mut annex_b, NAL_IDR, &[IDR_MODE_DEFLATE_RGB, 0x00]);

        assert!(dec.decode(&annex_b).is_none());
        assert_eq!(dec.width, 0);
        assert_eq!(dec.height, 0);
        assert!(!dec.has_idr);
    }

    #[test]
    fn test_decoder_rejects_truncated_and_oversized_deflate_payloads() {
        let short_deflate = deflate_fast(&[0u8; 32]).expect("deflate short");
        assert!(inflate_bounded(&short_deflate, MAX_VIDEO_PIXELS * 3 + 1).is_err());
        assert!(inflate_bounded(&short_deflate, MAX_VIDEO_PIXELS * 3).is_err());

        let mut dec = H264Decoder::new();
        let mut truncated_idr = Vec::with_capacity(1 + short_deflate.len());
        truncated_idr.push(IDR_MODE_DEFLATE_RGB);
        truncated_idr.extend_from_slice(&short_deflate);

        let mut truncated_annex_b = Vec::new();
        write_nal(&mut truncated_annex_b, NAL_SPS, &encode_sps(1920, 1080, 60));
        write_nal(&mut truncated_annex_b, NAL_IDR, &truncated_idr);
        assert!(dec.decode(&truncated_annex_b).is_none());
        assert_eq!(dec.width, 0);
        assert_eq!(dec.height, 0);
        assert!(!dec.has_idr);

        let oversized_deflate = deflate_fast(&vec![0xAA; 64 * 64 * 3 + 128]).expect("deflate big");
        let mut oversized_idr = Vec::with_capacity(1 + oversized_deflate.len());
        oversized_idr.push(IDR_MODE_DEFLATE_RGB);
        oversized_idr.extend_from_slice(&oversized_deflate);

        let mut oversized_annex_b = Vec::new();
        write_nal(&mut oversized_annex_b, NAL_SPS, &encode_sps(64, 64, 60));
        write_nal(&mut oversized_annex_b, NAL_IDR, &oversized_idr);
        assert!(dec.decode(&oversized_annex_b).is_none());
        assert_eq!(dec.width, 0);
        assert_eq!(dec.height, 0);

        // Failed keyframe must not corrupt previously decoded dimensions.
        let mut cap = SyntheticPatternCapture::new(64, 64);
        let mut enc = H264Encoder::new(60, 8_000_000);
        let valid = enc.encode(&cap.next_frame());
        assert!(dec.decode(&valid.annex_b).is_some());
        assert_eq!((dec.width, dec.height, dec.has_idr), (64, 64, true));

        assert!(dec.decode(&truncated_annex_b).is_none());
        assert_eq!((dec.width, dec.height, dec.has_idr), (64, 64, true));

        let mut bad_p_annex_b = Vec::new();
        write_nal(&mut bad_p_annex_b, NAL_SLICE, &[1, 1, P_MODE_DEFLATE_TILES]);
        assert!(dec.decode(&bad_p_annex_b).is_none());
    }

    #[test]
    fn test_dynamic_motion_never_leaves_desynchronized_stale_tiles() {
        let w = 1920usize;
        let h = 1080usize;
        let mut rgba1 = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                rgba1[idx] = ((x * 17 + y * 31) & 0x7F) as u8;
                rgba1[idx + 1] = ((x * 23 + y * 13) & 0x3F) as u8;
                rgba1[idx + 2] = 20;
                rgba1[idx + 3] = 255;
            }
        }
        let f1 = RawRgbaFrame {
            width: w as u32,
            height: h as u32,
            rgba: rgba1.clone(),
        };

        let mut enc = H264Encoder::new(60, 8_000_000);
        let mut dec = H264Decoder::new();
        let e1 = enc.encode(&f1);
        assert!(e1.is_keyframe);
        let _ = dec.decode(&e1.annex_b).expect("IDR must decode");

        // Modify ~45% of 64x64 tiles to a bright blue/cyan textured pattern.
        // Previously this hit the 16..=96 tile cap or collapsed to 1 tile, leaving stale squares.
        let tiles_x = w.div_ceil(P_TILE_SIZE);
        let tiles_y = h.div_ceil(P_TILE_SIZE);
        let mut rgba2 = rgba1;
        let mut changed_tiles = Vec::new();
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                if (tx + ty * 3) % 2 == 0 && (tx + ty) % 5 != 0 {
                    changed_tiles.push((tx, ty));
                    let x0 = tx * P_TILE_SIZE;
                    let x1 = (x0 + P_TILE_SIZE).min(w);
                    let y0 = ty * P_TILE_SIZE;
                    let y1 = (y0 + P_TILE_SIZE).min(h);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let idx = (y * w + x) * 4;
                            rgba2[idx] = 10;
                            rgba2[idx + 1] = 210;
                            rgba2[idx + 2] = (220 + ((x ^ y) & 31)) as u8;
                            rgba2[idx + 3] = 255;
                        }
                    }
                }
            }
        }
        assert!(changed_tiles.len() > 180);

        let f2 = RawRgbaFrame {
            width: w as u32,
            height: h as u32,
            rgba: rgba2,
        };
        let e2 = enc.encode(&f2);
        assert!(e2.annex_b.len() <= 165 * 1024);
        let (_, _, d2_rgba) = dec
            .decode_checked_shared(&e2.annex_b)
            .expect("dynamic frame must decode cleanly")
            .expect("dynamic frame must produce output");

        // Verify EVERY changed tile in the decoded output has the new blue/cyan color at its center.
        for (tx, ty) in changed_tiles {
            let cx = (tx * P_TILE_SIZE + P_TILE_SIZE / 2).min(w - 1);
            let cy = (ty * P_TILE_SIZE + P_TILE_SIZE / 2).min(h - 1);
            let idx = (cy * w + cx) * 4;
            assert!(
                d2_rgba[idx + 2] > 160 && d2_rgba[idx] < 80,
                "Tile ({tx}, {ty}) remained stale: RGB=({}, {}, {})",
                d2_rgba[idx],
                d2_rgba[idx + 1],
                d2_rgba[idx + 2]
            );
        }
    }

    #[test]
    fn test_striped_idr_1080p_roundtrip_and_corruption_rejection() {
        let w = 1920usize;
        let h = 1080usize;
        let mut rgba = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                rgba[idx] = ((x * 9 + y * 5) & 0xFF) as u8;
                rgba[idx + 1] = ((x * 3 + y * 11) & 0xFF) as u8;
                rgba[idx + 2] = ((x * 15 + y * 7) & 0xFF) as u8;
                rgba[idx + 3] = 255;
            }
        }
        let frame = RawRgbaFrame {
            width: w as u32,
            height: h as u32,
            rgba,
        };

        let mut enc = H264Encoder::new(60, 8_000_000);
        let mut dec = H264Decoder::new();
        let encoded = enc.encode(&frame);
        assert!(encoded.is_keyframe);
        let decoded = dec
            .decode(&encoded.annex_b)
            .expect("1080p striped IDR must decode");
        assert_eq!(decoded.width, 1920);
        assert_eq!(decoded.height, 1080);

        // Corrupt striped IDR payload (invalid stripe count & truncated stripe length) must be rejected
        // without corrupting existing decoder dimensions/state.
        let mut corrupt_annex_b = Vec::new();
        write_nal(&mut corrupt_annex_b, NAL_SPS, &encode_sps(1920, 1080, 60));
        write_nal(
            &mut corrupt_annex_b,
            NAL_IDR,
            &[IDR_MODE_JPEG_STRIPED, 4, 0, 0, 1, 0, 0xFF],
        );
        assert!(dec.decode(&corrupt_annex_b).is_none());
        assert_eq!((dec.width, dec.height, dec.has_idr), (1920, 1080, true));
    }
}
