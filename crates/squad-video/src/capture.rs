use serde::{Deserialize, Serialize};
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureTarget {
    Monitor {
        id: u64,
        name: String,
        width: u32,
        height: u32,
        is_primary: bool,
    },
    Window {
        id: u64,
        title: String,
        app_name: String,
        width: u32,
        height: u32,
    },
}

impl CaptureTarget {
    pub fn id(&self) -> u64 {
        match self {
            CaptureTarget::Monitor { id, .. } | CaptureTarget::Window { id, .. } => *id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            CaptureTarget::Monitor { name, .. } => name,
            CaptureTarget::Window { title, .. } => title,
        }
    }

    pub fn native_dimensions(&self) -> (u32, u32) {
        match self {
            CaptureTarget::Monitor { width, height, .. }
            | CaptureTarget::Window { width, height, .. } => (*width, *height),
        }
    }

    pub fn is_monitor(&self) -> bool {
        matches!(self, CaptureTarget::Monitor { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamResolution {
    R720p,
    R1080p,
    Native,
}

impl StreamResolution {
    pub fn short_label(&self) -> &'static str {
        match self {
            StreamResolution::R720p => "720p",
            StreamResolution::R1080p => "1080p",
            StreamResolution::Native => "Native",
        }
    }

    /// Computes even-aligned dimensions while preserving aspect ratio.
    pub fn target_dimensions(&self, src_w: u32, src_h: u32) -> (u32, u32) {
        let w = src_w.max(2);
        let h = src_h.max(2);

        let (max_w, max_h) = match self {
            StreamResolution::R720p => (1280u32, 720u32),
            StreamResolution::R1080p => (1920u32, 1080u32),
            StreamResolution::Native => return (align_even(w), align_even(h)),
        };

        if w <= max_w && h <= max_h {
            return (align_even(w), align_even(h));
        }

        let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
        let out_w = ((w as f64 * scale).round() as u32).max(2);
        let out_h = ((h as f64 * scale).round() as u32).max(2);
        (align_even(out_w), align_even(out_h))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum StreamFps {
    Fps30 = 30,
    Fps60 = 60,
}

impl StreamFps {
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum VideoCodec {
    #[default]
    H264 = 0,
    H265 = 1,
}

impl VideoCodec {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::H265,
            _ => Self::H264,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264 (Compatible)",
            VideoCodec::H265 => "H.265 (High Efficiency)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    pub target: CaptureTarget,
    pub resolution: StreamResolution,
    pub fps: StreamFps,
    pub bitrate_bps: u32,
    pub codec: VideoCodec,
    pub share_audio: bool,
}

impl StreamConfig {
    pub fn output_dimensions(&self) -> (u32, u32) {
        let (src_w, src_h) = self.target.native_dimensions();
        self.resolution.target_dimensions(src_w, src_h)
    }
}

#[derive(Debug, Clone)]
pub struct RawRgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[inline]
fn align_even(val: u32) -> u32 {
    (val & !1).max(2)
}

pub struct SyntheticPatternCapture {
    pub width: u32,
    pub height: u32,
    tick: u32,
    buf: Vec<u8>,
}

impl SyntheticPatternCapture {
    pub fn new(width: u32, height: u32) -> Self {
        let w = align_even(width).clamp(2, 4096);
        let h = align_even(height).clamp(2, 4096);
        Self {
            width: w,
            height: h,
            tick: 0,
            buf: vec![0u8; (w as usize) * (h as usize) * 4],
        }
    }

    /// Generates an H.264-friendly color bar pattern with a moving scan marker.
    pub fn next_frame(&mut self) -> RawRgbaFrame {
        let w = self.width as usize;
        let h = self.height as usize;
        let stripe_w = (w / 8).max(1);
        let marker_x = (self.tick as usize * 12) % w.saturating_sub(32).max(1);
        let marker_y = (self.tick as usize * 6) % h.saturating_sub(32).max(1);
        self.tick = self.tick.wrapping_add(1);

        const BARS: [[u8; 3]; 8] = [
            [30, 31, 34],
            [88, 101, 242],
            [35, 165, 90],
            [240, 178, 50],
            [242, 63, 67],
            [71, 82, 196],
            [43, 45, 49],
            [242, 243, 245],
        ];

        for y in 0..h {
            let row_off = y * w * 4;
            let in_marker_y = y >= marker_y && y < marker_y + 24;
            for x in 0..w {
                let p = row_off + x * 4;
                if in_marker_y && x >= marker_x && x < marker_x + 24 {
                    self.buf[p] = 255;
                    self.buf[p + 1] = 255;
                    self.buf[p + 2] = 255;
                    self.buf[p + 3] = 255;
                } else {
                    let bar = BARS[(x / stripe_w).min(7)];
                    self.buf[p] = bar[0];
                    self.buf[p + 1] = bar[1];
                    self.buf[p + 2] = bar[2];
                    self.buf[p + 3] = 255;
                }
            }
        }

        RawRgbaFrame {
            width: self.width,
            height: self.height,
            rgba: self.buf.clone(),
        }
    }
}

pub fn enumerate_capture_targets() -> Vec<CaptureTarget> {
    #[cfg(target_os = "macos")]
    let mut targets = macos_impl::enumerate_targets();

    #[cfg(target_os = "windows")]
    let mut targets = windows_impl::enumerate_targets();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut targets: Vec<CaptureTarget> = Vec::new();

    if !targets.iter().any(CaptureTarget::is_monitor) {
        targets.insert(
            0,
            CaptureTarget::Monitor {
                id: 1,
                name: "Display 1 (Primary)".to_string(),
                width: 1920,
                height: 1080,
                is_primary: true,
            },
        );
    }

    targets
}

pub fn request_screen_capture_permission() {
    #[cfg(target_os = "macos")]
    macos_impl::request_permission();
}

pub struct ScreenCapturer {
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    config: StreamConfig,
    out_width: u32,
    out_height: u32,
    synthetic: SyntheticPatternCapture,
    #[cfg(target_os = "macos")]
    mac: macos_impl::MacCapturer,
    #[cfg(target_os = "windows")]
    win: windows_impl::WinCapturer,
}

impl ScreenCapturer {
    pub fn new(config: StreamConfig) -> Self {
        let (out_width, out_height) = config.output_dimensions();
        debug!(
            "Initializing ScreenCapturer for '{}' -> {}x{} @ {} FPS",
            config.target.title(),
            out_width,
            out_height,
            config.fps.as_u8()
        );
        Self {
            #[cfg(target_os = "macos")]
            mac: macos_impl::MacCapturer::new(out_width, out_height),
            #[cfg(target_os = "windows")]
            win: windows_impl::WinCapturer::new(&config.target, out_width, out_height),
            config,
            out_width,
            out_height,
            synthetic: SyntheticPatternCapture::new(out_width, out_height),
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.out_width, self.out_height)
    }

    pub fn capture_frame(&mut self) -> RawRgbaFrame {
        #[cfg(target_os = "macos")]
        {
            if let Some(frame) =
                self.mac
                    .capture_target(&self.config.target, self.out_width, self.out_height)
            {
                return frame;
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(frame) =
                self.win
                    .capture_target(&self.config.target, self.out_width, self.out_height)
            {
                return frame;
            }
        }

        self.synthetic.next_frame()
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{CaptureTarget, RawRgbaFrame};
    use std::ffi::{c_char, c_void, CStr};

    type CFTypeRef = *const c_void;
    type CFArrayRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFNumberRef = *const c_void;
    type CGDirectDisplayID = u32;
    type CGImageRef = *const c_void;
    type CGContextRef = *const c_void;
    type CGColorSpaceRef = *const c_void;

    const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
    const K_CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW: u32 = 1 << 3;
    const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
    const K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
    const K_CG_WINDOW_IMAGE_NOMINAL_RESOLUTION: u32 = 1 << 4;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_CF_NUMBER_SINT32_TYPE: isize = 3;
    const K_CF_NUMBER_SINT64_TYPE: isize = 4;
    const K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;
    const K_CG_IMAGE_BYTE_ORDER_32_BIG: u32 = 4 << 12;
    const K_CG_INTERPOLATION_LOW: i32 = 1;
    const K_CG_BLEND_MODE_COPY: i32 = 17;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        static kCGWindowNumber: CFStringRef;
        static kCGWindowLayer: CFStringRef;
        static kCGWindowBounds: CFStringRef;
        static kCGWindowOwnerName: CFStringRef;
        static kCGWindowName: CFStringRef;
        static CGRectNull: CGRect;

        fn CGGetActiveDisplayList(
            max_displays: u32,
            active_displays: *mut CGDirectDisplayID,
            display_count: *mut u32,
        ) -> i32;
        fn CGDisplayPixelsWide(display: CGDirectDisplayID) -> usize;
        fn CGDisplayPixelsHigh(display: CGDirectDisplayID) -> usize;
        fn CGDisplayIsMain(display: CGDirectDisplayID) -> u32;
        fn CGDisplayCreateImage(display: CGDirectDisplayID) -> CGImageRef;

        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
        fn CGWindowListCreateImage(
            screen_bounds: CGRect,
            list_option: u32,
            window_id: u32,
            image_option: u32,
        ) -> CGImageRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect)
            -> bool;

        fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
        fn CGColorSpaceRelease(space: CGColorSpaceRef);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGContextSetInterpolationQuality(ctx: CGContextRef, quality: i32);
        fn CGContextSetBlendMode(ctx: CGContextRef, mode: i32);
        fn CGContextDrawImage(ctx: CGContextRef, rect: CGRect, image: CGImageRef);
        fn CGContextRelease(ctx: CGContextRef);
        fn CGImageRelease(image: CGImageRef);

        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: CFArrayRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: isize) -> *const c_void;
        fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;
        fn CFGetTypeID(cf: *const c_void) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFStringGetCString(
            s: CFStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> bool;
        fn CFNumberGetTypeID() -> usize;
        fn CFNumberGetValue(number: CFNumberRef, the_type: isize, value_ptr: *mut c_void) -> bool;
        fn CFRelease(cf: CFTypeRef);
    }

    pub fn request_permission() {
        unsafe {
            if !CGPreflightScreenCaptureAccess() {
                let _ = CGRequestScreenCaptureAccess();
            }
        }
    }

    pub fn enumerate_targets() -> Vec<CaptureTarget> {
        let mut out = Vec::new();
        let mut displays = [0u32; 16];
        let mut display_count = 0u32;

        unsafe {
            if CGGetActiveDisplayList(16, displays.as_mut_ptr(), &mut display_count) == 0 {
                for (idx, &disp_id) in displays[..(display_count as usize).min(16)]
                    .iter()
                    .enumerate()
                {
                    let w = (CGDisplayPixelsWide(disp_id) as u32).max(2);
                    let h = (CGDisplayPixelsHigh(disp_id) as u32).max(2);
                    let is_primary = CGDisplayIsMain(disp_id) != 0;
                    let name = if is_primary {
                        format!("Display {} (Primary)", idx + 1)
                    } else {
                        format!("Display {}", idx + 1)
                    };
                    out.push(CaptureTarget::Monitor {
                        id: disp_id as u64,
                        name,
                        width: w,
                        height: h,
                        is_primary,
                    });
                }
            }

            let opts =
                K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS;
            let list = CGWindowListCopyWindowInfo(opts, 0);
            if !list.is_null() {
                let count = CFArrayGetCount(list);
                for i in 0..count {
                    let dict = CFArrayGetValueAtIndex(list, i);
                    if dict.is_null() {
                        continue;
                    }
                    let Some(layer) = dict_i32(dict, kCGWindowLayer) else {
                        continue;
                    };
                    if layer != 0 {
                        continue;
                    }
                    let Some(wid) = dict_i64(dict, kCGWindowNumber) else {
                        continue;
                    };
                    let owner = dict_string(dict, kCGWindowOwnerName).unwrap_or_default();
                    if owner.is_empty()
                        || owner.eq_ignore_ascii_case("squad-client")
                        || owner.eq_ignore_ascii_case("SquadAir")
                        || owner.eq_ignore_ascii_case("Squad-Air")
                        || owner == "Window Server"
                        || owner == "Dock"
                        || owner == "Control Center"
                        || owner == "Notification Center"
                    {
                        continue;
                    }
                    let bounds_dict = CFDictionaryGetValue(dict, kCGWindowBounds);
                    if bounds_dict.is_null() {
                        continue;
                    }
                    let mut rect = CGRect::default();
                    if !CGRectMakeWithDictionaryRepresentation(bounds_dict, &mut rect) {
                        continue;
                    }
                    if rect.size.width < 120.0 || rect.size.height < 80.0 {
                        continue;
                    }
                    let title_raw = dict_string(dict, kCGWindowName).unwrap_or_default();
                    let title = if title_raw.trim().is_empty() {
                        owner.clone()
                    } else {
                        title_raw
                    };

                    out.push(CaptureTarget::Window {
                        id: wid as u64,
                        title,
                        app_name: owner,
                        width: (rect.size.width.round() as u32).max(2),
                        height: (rect.size.height.round() as u32).max(2),
                    });
                }
                CFRelease(list);
            }
        }

        out
    }

    pub(crate) struct MacCapturer {
        width: u32,
        height: u32,
        color_space: CGColorSpaceRef,
        ctx: CGContextRef,
        backing_rgba: Box<[u8]>,
    }

    unsafe impl Send for MacCapturer {}

    impl MacCapturer {
        pub fn new(out_w: u32, out_h: u32) -> Self {
            let w = out_w.max(2);
            let h = out_h.max(2);
            let stride = (w as usize) * 4;
            let mut backing_rgba = vec![255u8; stride * (h as usize)].into_boxed_slice();

            unsafe {
                let color_space = CGColorSpaceCreateDeviceRGB();
                let ctx = if !color_space.is_null() {
                    let bitmap_info =
                        K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST | K_CG_IMAGE_BYTE_ORDER_32_BIG;
                    let c = CGBitmapContextCreate(
                        backing_rgba.as_mut_ptr() as *mut c_void,
                        w as usize,
                        h as usize,
                        8,
                        stride,
                        color_space,
                        bitmap_info,
                    );
                    if !c.is_null() {
                        CGContextSetInterpolationQuality(c, K_CG_INTERPOLATION_LOW);
                        CGContextSetBlendMode(c, K_CG_BLEND_MODE_COPY);
                    }
                    c
                } else {
                    std::ptr::null()
                };

                Self {
                    width: w,
                    height: h,
                    color_space,
                    ctx,
                    backing_rgba,
                }
            }
        }

        pub fn capture_target(
            &mut self,
            target: &CaptureTarget,
            out_w: u32,
            out_h: u32,
        ) -> Option<RawRgbaFrame> {
            let w = out_w.max(2);
            let h = out_h.max(2);
            if self.width != w || self.height != h || self.ctx.is_null() {
                *self = Self::new(w, h);
            }
            if self.ctx.is_null() {
                return None;
            }

            unsafe {
                let cg_image = match target {
                    CaptureTarget::Monitor { id, .. } => {
                        CGDisplayCreateImage(*id as CGDirectDisplayID)
                    }
                    CaptureTarget::Window { id, .. } => CGWindowListCreateImage(
                        CGRectNull,
                        K_CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW,
                        *id as u32,
                        K_CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING
                            | K_CG_WINDOW_IMAGE_NOMINAL_RESOLUTION,
                    ),
                };
                if cg_image.is_null() {
                    return None;
                }

                let draw_rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: w as f64,
                        height: h as f64,
                    },
                };
                CGContextDrawImage(self.ctx, draw_rect, cg_image);
                CGImageRelease(cg_image);

                Some(RawRgbaFrame {
                    width: w,
                    height: h,
                    rgba: self.backing_rgba.to_vec(),
                })
            }
        }
    }

    impl Drop for MacCapturer {
        fn drop(&mut self) {
            unsafe {
                if !self.ctx.is_null() {
                    CGContextRelease(self.ctx);
                    self.ctx = std::ptr::null();
                }
                if !self.color_space.is_null() {
                    CGColorSpaceRelease(self.color_space);
                    self.color_space = std::ptr::null();
                }
            }
        }
    }

    unsafe fn dict_string(dict: CFDictionaryRef, key: CFStringRef) -> Option<String> {
        let val = CFDictionaryGetValue(dict, key);
        if val.is_null() {
            return None;
        }
        if CFGetTypeID(val) != CFStringGetTypeID() {
            return None;
        }
        let mut buf: [c_char; 512] = [0; 512];
        if CFStringGetCString(
            val,
            buf.as_mut_ptr(),
            buf.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
        ) {
            Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        } else {
            None
        }
    }

    unsafe fn dict_i32(dict: CFDictionaryRef, key: CFStringRef) -> Option<i32> {
        let val = CFDictionaryGetValue(dict, key);
        if val.is_null() {
            return None;
        }
        if CFGetTypeID(val) != CFNumberGetTypeID() {
            return None;
        }
        let mut num = 0i32;
        if CFNumberGetValue(
            val,
            K_CF_NUMBER_SINT32_TYPE,
            &mut num as *mut i32 as *mut c_void,
        ) {
            Some(num)
        } else {
            None
        }
    }

    unsafe fn dict_i64(dict: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
        let val = CFDictionaryGetValue(dict, key);
        if val.is_null() {
            return None;
        }
        if CFGetTypeID(val) != CFNumberGetTypeID() {
            return None;
        }
        let mut num = 0i64;
        if CFNumberGetValue(
            val,
            K_CF_NUMBER_SINT64_TYPE,
            &mut num as *mut i64 as *mut c_void,
        ) {
            Some(num)
        } else {
            None
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{CaptureTarget, RawRgbaFrame};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::Path;
    use std::time::{Duration, Instant};
    use windows::core::Interface;
    use windows::Win32::Foundation::{BOOL, HMODULE, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
        D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
        IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
        DXGI_OUTPUT_DESC,
    };
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC,
        DeleteObject, EnumDisplayMonitors, GdiFlush, GetDC, GetMonitorInfoW, ReleaseDC,
        SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO, MONITORINFOEXW,
        SRCCOPY,
    };
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible,
    };

    const PW_RENDERFULLCONTENT: u32 = 2;
    const MONITORINFOF_PRIMARY: u32 = 1;

    #[link(name = "user32")]
    extern "system" {
        fn PrintWindow(hwnd: HWND, hdcblt: HDC, nflags: u32) -> BOOL;
    }

    struct WinTimerPeriodGuard {
        active: bool,
    }

    impl WinTimerPeriodGuard {
        fn new() -> Self {
            let ok = unsafe { timeBeginPeriod(1) == 0 };
            Self { active: ok }
        }
    }

    impl Drop for WinTimerPeriodGuard {
        fn drop(&mut self) {
            if self.active {
                unsafe {
                    let _ = timeEndPeriod(1);
                }
            }
        }
    }

    struct PersistentGdiDib {
        out_w: u32,
        out_h: u32,
        mem_dc: HDC,
        dib: HBITMAP,
        old_obj: HGDIOBJ,
        bits_ptr: *mut u8,
    }

    impl PersistentGdiDib {
        unsafe fn new(out_w: u32, out_h: u32) -> Option<Self> {
            if out_w == 0 || out_h == 0 || out_w > 4096 || out_h > 4096 {
                return None;
            }
            let screen_dc = GetDC(HWND::default());
            if screen_dc.is_invalid() {
                return None;
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            let _ = ReleaseDC(HWND::default(), screen_dc);
            if mem_dc.is_invalid() {
                return None;
            }

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: out_w as i32,
                    biHeight: -(out_h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut bits_void: *mut c_void = std::ptr::null_mut();
            let Ok(dib) = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits_void, None, 0)
            else {
                let _ = DeleteDC(mem_dc);
                return None;
            };
            if dib.is_invalid() || bits_void.is_null() {
                if !dib.is_invalid() {
                    let _ = DeleteObject(dib);
                }
                let _ = DeleteDC(mem_dc);
                return None;
            }

            let old_obj = SelectObject(mem_dc, dib);
            let _ = SetStretchBltMode(mem_dc, COLORONCOLOR);

            Some(Self {
                out_w,
                out_h,
                mem_dc,
                dib,
                old_obj,
                bits_ptr: bits_void as *mut u8,
            })
        }

        unsafe fn copy_from_dc(
            &mut self,
            src_dc: HDC,
            src_x: i32,
            src_y: i32,
            src_w: i32,
            src_h: i32,
        ) -> Option<RawRgbaFrame> {
            let out_w = self.out_w;
            let out_h = self.out_h;
            let ok = if src_w == out_w as i32 && src_h == out_h as i32 {
                BitBlt(
                    self.mem_dc,
                    0,
                    0,
                    src_w,
                    src_h,
                    src_dc,
                    src_x,
                    src_y,
                    SRCCOPY,
                )
                .is_ok()
            } else {
                StretchBlt(
                    self.mem_dc,
                    0,
                    0,
                    out_w as i32,
                    out_h as i32,
                    src_dc,
                    src_x,
                    src_y,
                    src_w,
                    src_h,
                    SRCCOPY,
                )
                .as_bool()
            };
            if !ok {
                return None;
            }

            let _ = GdiFlush();

            let row_bytes = (out_w as usize) * 4;
            let src_stride = (row_bytes + 3) & !3;
            let src_slice =
                std::slice::from_raw_parts(self.bits_ptr, src_stride * (out_h as usize));
            let mut rgba = vec![0u8; row_bytes * (out_h as usize)];
            for row in 0..out_h as usize {
                let src_row = &src_slice[row * src_stride..row * src_stride + row_bytes];
                let dst_row = &mut rgba[row * row_bytes..(row + 1) * row_bytes];
                for (dst, bgra) in dst_row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src_row.as_chunks::<4>().0.iter())
                {
                    dst[0] = bgra[2];
                    dst[1] = bgra[1];
                    dst[2] = bgra[0];
                    dst[3] = 255;
                }
            }
            Some(RawRgbaFrame {
                width: out_w,
                height: out_h,
                rgba,
            })
        }
    }

    impl Drop for PersistentGdiDib {
        fn drop(&mut self) {
            unsafe {
                if !self.mem_dc.is_invalid() {
                    if !self.old_obj.is_invalid() {
                        SelectObject(self.mem_dc, self.old_obj);
                    }
                    if !self.dib.is_invalid() {
                        let _ = DeleteObject(self.dib);
                    }
                    let _ = DeleteDC(self.mem_dc);
                }
            }
        }
    }

    struct DxgiDuplicator {
        monitor_id: u64,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        dupl: IDXGIOutputDuplication,
        staging: Option<ID3D11Texture2D>,
        staging_w: u32,
        staging_h: u32,
        cached_rgba: Vec<u8>,
    }

    enum DxgiCaptureResult {
        Frame(RawRgbaFrame),
        TimeoutNoCache,
        AccessLost,
    }

    impl DxgiDuplicator {
        unsafe fn new(monitor_id: u64) -> Option<Self> {
            let hmon = HMONITOR(monitor_id as isize);
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut adapter_idx = 0u32;

            while let Ok(adapter1) = factory.EnumAdapters1(adapter_idx) {
                adapter_idx += 1;
                let mut output_idx = 0u32;
                while let Ok(output) = adapter1.EnumOutputs(output_idx) {
                    output_idx += 1;
                    let mut desc = DXGI_OUTPUT_DESC::default();
                    if output.GetDesc(&mut desc).is_err() || desc.Monitor != hmon {
                        continue;
                    }
                    let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                        continue;
                    };
                    let Ok(base_adapter) = adapter1.cast::<IDXGIAdapter>() else {
                        continue;
                    };
                    let mut device: Option<ID3D11Device> = None;
                    let mut context: Option<ID3D11DeviceContext> = None;
                    let mut feature_level = D3D_FEATURE_LEVEL::default();
                    if D3D11CreateDevice(
                        &base_adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        Some(&mut feature_level),
                        Some(&mut context),
                    )
                    .is_err()
                    {
                        continue;
                    }
                    let (Some(device), Some(context)) = (device, context) else {
                        continue;
                    };
                    let Ok(dupl) = output1.DuplicateOutput(&device) else {
                        continue;
                    };
                    return Some(Self {
                        monitor_id,
                        device,
                        context,
                        dupl,
                        staging: None,
                        staging_w: 0,
                        staging_h: 0,
                        cached_rgba: Vec::new(),
                    });
                }
            }
            None
        }

        unsafe fn capture_scaled(&mut self, out_w: u32, out_h: u32) -> DxgiCaptureResult {
            let expected_bytes = (out_w as usize) * (out_h as usize) * 4;
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;

            match self
                .dupl
                .AcquireNextFrame(0, &mut frame_info, &mut resource)
            {
                Ok(()) => {}
                Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    if self.cached_rgba.len() == expected_bytes {
                        return DxgiCaptureResult::Frame(RawRgbaFrame {
                            width: out_w,
                            height: out_h,
                            rgba: self.cached_rgba.clone(),
                        });
                    }
                    return DxgiCaptureResult::TimeoutNoCache;
                }
                Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
                    return DxgiCaptureResult::AccessLost;
                }
                Err(_) => return DxgiCaptureResult::AccessLost,
            }

            let Some(res) = resource else {
                let _ = self.dupl.ReleaseFrame();
                return DxgiCaptureResult::TimeoutNoCache;
            };
            let Ok(src_tex) = res.cast::<ID3D11Texture2D>() else {
                let _ = self.dupl.ReleaseFrame();
                return DxgiCaptureResult::AccessLost;
            };

            let mut src_desc = D3D11_TEXTURE2D_DESC::default();
            src_tex.GetDesc(&mut src_desc);
            let src_w = src_desc.Width;
            let src_h = src_desc.Height;
            if src_w == 0 || src_h == 0 {
                let _ = self.dupl.ReleaseFrame();
                return DxgiCaptureResult::AccessLost;
            }

            if self.staging.is_none() || self.staging_w != src_w || self.staging_h != src_h {
                let staging_desc = D3D11_TEXTURE2D_DESC {
                    Width: src_w,
                    Height: src_h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut new_staging: Option<ID3D11Texture2D> = None;
                if self
                    .device
                    .CreateTexture2D(&staging_desc, None, Some(&mut new_staging))
                    .is_err()
                    || new_staging.is_none()
                {
                    let _ = self.dupl.ReleaseFrame();
                    return DxgiCaptureResult::AccessLost;
                }
                self.staging = new_staging;
                self.staging_w = src_w;
                self.staging_h = src_h;
            }

            let Some(ref staging_tex) = self.staging else {
                let _ = self.dupl.ReleaseFrame();
                return DxgiCaptureResult::AccessLost;
            };

            let Ok(dst_res) = staging_tex.cast::<ID3D11Resource>() else {
                let _ = self.dupl.ReleaseFrame();
                return DxgiCaptureResult::AccessLost;
            };
            let Ok(src_res) = src_tex.cast::<ID3D11Resource>() else {
                let _ = self.dupl.ReleaseFrame();
                return DxgiCaptureResult::AccessLost;
            };

            self.context.CopyResource(&dst_res, &src_res);
            let _ = self.dupl.ReleaseFrame();

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if self
                .context
                .Map(&dst_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .is_err()
                || mapped.pData.is_null()
            {
                return DxgiCaptureResult::AccessLost;
            }

            let pitch = mapped.RowPitch as usize;
            let total_mapped = pitch.saturating_mul(src_h as usize);
            let src_bytes = std::slice::from_raw_parts(mapped.pData as *const u8, total_mapped);

            self.cached_rgba.resize(expected_bytes, 255);
            let dw = out_w as usize;
            let dh = out_h as usize;
            let sw = src_w as usize;
            let sh = src_h as usize;

            if dw == sw && dh == sh {
                let row_bytes = dw * 4;
                for y in 0..dh {
                    let srow = &src_bytes[y * pitch..y * pitch + row_bytes];
                    let drow = &mut self.cached_rgba[y * row_bytes..(y + 1) * row_bytes];
                    for (dst, bgra) in drow
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(srow.as_chunks::<4>().0.iter())
                    {
                        dst[0] = bgra[2];
                        dst[1] = bgra[1];
                        dst[2] = bgra[0];
                        dst[3] = 255;
                    }
                }
            } else {
                for dy in 0..dh {
                    let sy = (dy * sh / dh).min(sh - 1);
                    let srow = &src_bytes[sy * pitch..];
                    let drow = &mut self.cached_rgba[dy * dw * 4..(dy + 1) * dw * 4];
                    for dx in 0..dw {
                        let sx = (dx * sw / dw).min(sw - 1);
                        let sidx = sx * 4;
                        let didx = dx * 4;
                        drow[didx] = srow[sidx + 2];
                        drow[didx + 1] = srow[sidx + 1];
                        drow[didx + 2] = srow[sidx];
                        drow[didx + 3] = 255;
                    }
                }
            }

            self.context.Unmap(&dst_res, 0);

            DxgiCaptureResult::Frame(RawRgbaFrame {
                width: out_w,
                height: out_h,
                rgba: self.cached_rgba.clone(),
            })
        }
    }

    pub(crate) struct WinCapturer {
        _timer_guard: WinTimerPeriodGuard,
        dxgi: Option<DxgiDuplicator>,
        last_dxgi_retry: Instant,
        gdi_dib: Option<PersistentGdiDib>,
    }

    unsafe impl Send for WinCapturer {}

    impl WinCapturer {
        pub fn new(target: &CaptureTarget, out_w: u32, out_h: u32) -> Self {
            let timer_guard = WinTimerPeriodGuard::new();
            let dxgi = match target {
                CaptureTarget::Monitor { id, .. } => unsafe { DxgiDuplicator::new(*id) },
                CaptureTarget::Window { .. } => None,
            };
            let gdi_dib = unsafe { PersistentGdiDib::new(out_w.max(2), out_h.max(2)) };
            Self {
                _timer_guard: timer_guard,
                dxgi,
                last_dxgi_retry: Instant::now(),
                gdi_dib,
            }
        }

        pub fn capture_target(
            &mut self,
            target: &CaptureTarget,
            out_w: u32,
            out_h: u32,
        ) -> Option<RawRgbaFrame> {
            let w = out_w.max(2);
            let h = out_h.max(2);
            unsafe {
                if self
                    .gdi_dib
                    .as_ref()
                    .is_none_or(|d| d.out_w != w || d.out_h != h)
                {
                    self.gdi_dib = PersistentGdiDib::new(w, h);
                }

                match target {
                    CaptureTarget::Monitor { id, .. } => {
                        if self.dxgi.as_ref().is_some_and(|d| d.monitor_id != *id) {
                            self.dxgi = DxgiDuplicator::new(*id);
                            self.last_dxgi_retry = Instant::now();
                        } else if self.dxgi.is_none()
                            && self.last_dxgi_retry.elapsed() >= Duration::from_secs(2)
                        {
                            self.dxgi = DxgiDuplicator::new(*id);
                            self.last_dxgi_retry = Instant::now();
                        }

                        if let Some(ref mut dupl) = self.dxgi {
                            match dupl.capture_scaled(w, h) {
                                DxgiCaptureResult::Frame(frame) => return Some(frame),
                                DxgiCaptureResult::TimeoutNoCache => {}
                                DxgiCaptureResult::AccessLost => {
                                    self.dxgi = None;
                                    self.last_dxgi_retry = Instant::now();
                                }
                            }
                        }

                        let hmon = HMONITOR(*id as isize);
                        let mut info = MONITORINFOEXW::default();
                        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
                        if !GetMonitorInfoW(
                            hmon,
                            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
                        )
                        .as_bool()
                        {
                            return None;
                        }
                        let r = info.monitorInfo.rcMonitor;
                        let src_w = r.right - r.left;
                        let src_h = r.bottom - r.top;
                        if src_w <= 0 || src_h <= 0 {
                            return None;
                        }
                        let screen_dc = GetDC(HWND::default());
                        if screen_dc.is_invalid() {
                            return None;
                        }
                        let frame = self.gdi_dib.as_mut().and_then(|dib| {
                            dib.copy_from_dc(screen_dc, r.left, r.top, src_w, src_h)
                        });
                        if let (Some(ref f), Some(ref mut dupl)) = (&frame, &mut self.dxgi) {
                            dupl.cached_rgba.clear();
                            dupl.cached_rgba.extend_from_slice(&f.rgba);
                        }
                        let _ = ReleaseDC(HWND::default(), screen_dc);
                        frame
                    }
                    CaptureTarget::Window { id, .. } => {
                        let hwnd = HWND(*id as isize);
                        let mut rect = RECT::default();
                        if GetWindowRect(hwnd, &mut rect).is_err() {
                            return None;
                        }
                        let src_w = rect.right - rect.left;
                        let src_h = rect.bottom - rect.top;
                        if src_w <= 0 || src_h <= 0 {
                            return None;
                        }

                        let win_dc = GetDC(hwnd);
                        if win_dc.is_invalid() {
                            return None;
                        }
                        let tmp_dc = CreateCompatibleDC(win_dc);
                        if tmp_dc.is_invalid() {
                            let _ = ReleaseDC(hwnd, win_dc);
                            return None;
                        }
                        let tmp_bmp = CreateCompatibleBitmap(win_dc, src_w, src_h);
                        if tmp_bmp.is_invalid() {
                            let _ = DeleteDC(tmp_dc);
                            let _ = ReleaseDC(hwnd, win_dc);
                            return None;
                        }
                        let old = SelectObject(tmp_dc, tmp_bmp);
                        let printed = PrintWindow(hwnd, tmp_dc, PW_RENDERFULLCONTENT).as_bool()
                            || PrintWindow(hwnd, tmp_dc, 0).as_bool();
                        let src_dc = if printed { tmp_dc } else { win_dc };
                        let frame = self
                            .gdi_dib
                            .as_mut()
                            .and_then(|dib| dib.copy_from_dc(src_dc, 0, 0, src_w, src_h));
                        SelectObject(tmp_dc, old);
                        let _ = DeleteObject(tmp_bmp);
                        let _ = DeleteDC(tmp_dc);
                        let _ = ReleaseDC(hwnd, win_dc);
                        frame
                    }
                }
            }
        }
    }

    pub fn enumerate_targets() -> Vec<CaptureTarget> {
        let mut out = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(enum_monitors_cb),
                LPARAM(&mut out as *mut Vec<CaptureTarget> as isize),
            );
            let _ = EnumWindows(
                Some(enum_windows_cb),
                LPARAM(&mut out as *mut Vec<CaptureTarget> as isize),
            );
        }
        out
    }

    unsafe extern "system" fn enum_monitors_cb(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let targets = &mut *(lparam.0 as *mut Vec<CaptureTarget>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            let r = info.monitorInfo.rcMonitor;
            let w = (r.right - r.left).unsigned_abs().max(2);
            let h = (r.bottom - r.top).unsigned_abs().max(2);
            let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
            let idx = targets.iter().filter(|t| t.is_monitor()).count() + 1;
            let name = if is_primary {
                format!("Display {idx} (Primary)")
            } else {
                format!("Display {idx}")
            };
            targets.push(CaptureTarget::Monitor {
                id: hmon.0 as u64,
                name,
                width: w,
                height: h,
                is_primary,
            });
        }
        BOOL(1)
    }

    unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return BOOL(1);
        }

        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return BOOL(1);
        }

        let mut title_buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut title_buf);
        if len <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&title_buf[..len as usize])
            .trim()
            .to_string();
        if title.is_empty() || title.starts_with("SquadAir") || title.starts_with("Squad-Air") {
            return BOOL(1);
        }

        let mut pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return BOOL(1);
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return BOOL(1);
        }
        let w = (rect.right - rect.left).max(0) as u32;
        let h = (rect.bottom - rect.top).max(0) as u32;
        if w < 120 || h < 80 {
            return BOOL(1);
        }

        let app_name = process_name(pid).unwrap_or_else(|| "Application".to_string());
        let targets = &mut *(lparam.0 as *mut Vec<CaptureTarget>);
        targets.push(CaptureTarget::Window {
            id: hwnd.0 as u64,
            title,
            app_name,
            width: w,
            height: h,
        });
        BOOL(1)
    }

    unsafe fn process_name(pid: u32) -> Option<String> {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        Path::new(&full)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_capture_targets_no_panic_and_has_monitor() {
        let targets = enumerate_capture_targets();
        assert!(!targets.is_empty());
        assert!(targets.iter().any(CaptureTarget::is_monitor));
    }
}
