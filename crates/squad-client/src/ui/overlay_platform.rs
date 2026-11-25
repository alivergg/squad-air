use crate::ui::Message;
use iced::window::raw_window_handle::RawWindowHandle;
use iced::{window, Task};

#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_POPUP: u32 = 0x8000_0000;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_CLIPSIBLINGS: u32 = 0x0400_0000;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_STRIP_STYLE_MASK: u32 = 0x00CF_0000;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_STRIP_EXSTYLE_MASK: u32 = 0x0006_0301;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_EX_TOPMOST: u32 = 0x0000_0008;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_EX_TRANSPARENT: u32 = 0x0000_0020;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_EX_LAYERED: u32 = 0x0008_0000;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_WS_EX_NOACTIVATE: u32 = 0x0800_0000;
#[cfg(any(target_os = "windows", test))]
pub(crate) const WIN32_REQUIRED_EX_STYLES: u32 = WIN32_WS_EX_LAYERED
    | WIN32_WS_EX_TRANSPARENT
    | WIN32_WS_EX_TOPMOST
    | WIN32_WS_EX_NOACTIVATE
    | WIN32_WS_EX_TOOLWINDOW;

pub(crate) fn should_use_iced_mouse_passthrough(is_windows: bool) -> bool {
    !is_windows
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn sanitize_overlay_style(raw_style: u32) -> u32 {
    (raw_style & !WIN32_STRIP_STYLE_MASK) | WIN32_WS_POPUP | WIN32_WS_CLIPSIBLINGS
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn sanitize_overlay_ex_style(raw_ex_style: u32) -> u32 {
    (raw_ex_style & !WIN32_STRIP_EXSTYLE_MASK) | WIN32_REQUIRED_EX_STYLES
}

pub fn configure_overlay_window(id: window::Id, opacity: f32) -> Task<Message> {
    let apply_task = window::run_with_handle(id, move |handle| {
        apply_raw_overlay_flags(handle.as_raw(), opacity);
        Message::OverlayConfigured
    });
    if should_use_iced_mouse_passthrough(cfg!(target_os = "windows")) {
        window::enable_mouse_passthrough(id).chain(apply_task)
    } else {
        apply_task
    }
}

pub fn set_overlay_opacity(id: window::Id, opacity: f32) -> Task<Message> {
    #[cfg(target_os = "macos")]
    {
        window::run_with_handle(id, move |handle| {
            apply_macos_overlay_opacity(handle.as_raw(), opacity);
            Message::OverlayConfigured
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, opacity);
        Task::none()
    }
}

#[cfg(target_os = "macos")]
unsafe fn macos_ns_window(raw: RawWindowHandle) -> Option<*mut std::ffi::c_void> {
    use std::ffi::{c_char, c_void};
    use std::mem::transmute;

    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    let RawWindowHandle::AppKit(handle) = raw else {
        return None;
    };
    let ns_view = handle.ns_view.as_ptr();
    if ns_view.is_null() {
        return None;
    }

    let send_ptr: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        transmute(objc_msgSend as *const ());
    let ns_window = send_ptr(ns_view, sel_registerName(c"window".as_ptr()));
    (!ns_window.is_null()).then_some(ns_window)
}

#[cfg(target_os = "macos")]
unsafe fn apply_macos_ns_window_alpha(ns_window: *mut std::ffi::c_void, opacity: f32) {
    use std::ffi::{c_char, c_void};
    use std::mem::transmute;

    #[link(name = "objc")]
    extern "C" {
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    let clamped = opacity.clamp(0.30, 1.0) as f64;
    let send_void_f64: unsafe extern "C" fn(*mut c_void, *mut c_void, f64) =
        transmute(objc_msgSend as *const ());
    send_void_f64(
        ns_window,
        sel_registerName(c"setAlphaValue:".as_ptr()),
        clamped,
    );
}

#[cfg(target_os = "macos")]
fn apply_macos_overlay_opacity(raw: RawWindowHandle, opacity: f32) {
    unsafe {
        if let Some(ns_window) = macos_ns_window(raw) {
            apply_macos_ns_window_alpha(ns_window, opacity);
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_raw_overlay_flags(raw: RawWindowHandle, opacity: f32) {
    use std::ffi::{c_char, c_void};
    use std::mem::transmute;

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe {
        let Some(ns_window) = macos_ns_window(raw) else {
            return;
        };

        let send_ptr: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            transmute(objc_msgSend as *const ());
        let send_void_bool: unsafe extern "C" fn(*mut c_void, *mut c_void, bool) =
            transmute(objc_msgSend as *const ());
        let send_void_ptr: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
            transmute(objc_msgSend as *const ());
        let send_void_isize: unsafe extern "C" fn(*mut c_void, *mut c_void, isize) =
            transmute(objc_msgSend as *const ());
        let send_void_usize: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) =
            transmute(objc_msgSend as *const ());

        let ns_color_cls = objc_getClass(c"NSColor".as_ptr());
        let clear_color = send_ptr(ns_color_cls, sel_registerName(c"clearColor".as_ptr()));

        send_void_bool(ns_window, sel_registerName(c"setOpaque:".as_ptr()), false);
        send_void_bool(
            ns_window,
            sel_registerName(c"setHasShadow:".as_ptr()),
            false,
        );
        if !clear_color.is_null() {
            send_void_ptr(
                ns_window,
                sel_registerName(c"setBackgroundColor:".as_ptr()),
                clear_color,
            );
        }
        send_void_bool(
            ns_window,
            sel_registerName(c"setIgnoresMouseEvents:".as_ptr()),
            true,
        );
        // NSStatusWindowLevel = 25 floats cleanly above standard and auxiliary fullscreen spaces.
        send_void_isize(ns_window, sel_registerName(c"setLevel:".as_ptr()), 25);
        // NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary.
        send_void_usize(
            ns_window,
            sel_registerName(c"setCollectionBehavior:".as_ptr()),
            (1 << 0) | (1 << 8),
        );
        apply_macos_ns_window_alpha(ns_window, opacity);
    }
}

#[cfg(target_os = "windows")]
fn apply_raw_overlay_flags(raw: RawWindowHandle, _opacity: f32) {
    use std::ffi::c_void;
    use std::mem::size_of;

    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SW_SHOWNOACTIVATE: i32 = 4;
    const HTTRANSPARENT: isize = -1;

    const WM_STYLECHANGING: u32 = 0x007C;
    const WM_NCDESTROY: u32 = 0x0082;
    const WM_NCCALCSIZE: u32 = 0x0083;
    const WM_NCHITTEST: u32 = 0x0084;
    const WM_NCPAINT: u32 = 0x0085;
    const WM_NCACTIVATE: u32 = 0x0086;

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_DONOTROUND: u32 = 1;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    const OVERLAY_SUBCLASS_ID: usize = 0x51A0_0001;

    #[repr(C)]
    struct StyleStruct {
        _style_old: u32,
        style_new: u32,
    }

    type SubclassProc = Option<
        unsafe extern "system" fn(
            hwnd: isize,
            u_msg: u32,
            w_param: usize,
            l_param: isize,
            u_id_subclass: usize,
            dw_ref_data: usize,
        ) -> isize,
    >;

    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, n_index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, n_index: i32, dw_new_long: isize) -> isize;
        fn SetWindowPos(
            hwnd: isize,
            hwnd_insert_after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            u_flags: u32,
        ) -> i32;
        fn ShowWindow(hwnd: isize, n_cmd_show: i32) -> i32;
    }

    #[link(name = "comctl32")]
    extern "system" {
        fn SetWindowSubclass(
            hwnd: isize,
            pfn_subclass: SubclassProc,
            u_id_subclass: usize,
            dw_ref_data: usize,
        ) -> i32;
        fn RemoveWindowSubclass(
            hwnd: isize,
            pfn_subclass: SubclassProc,
            u_id_subclass: usize,
        ) -> i32;
        fn DefSubclassProc(hwnd: isize, u_msg: u32, w_param: usize, l_param: isize) -> isize;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            dw_attribute: u32,
            pv_attribute: *const c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    unsafe extern "system" fn overlay_subclass_proc(
        hwnd: isize,
        u_msg: u32,
        w_param: usize,
        l_param: isize,
        u_id_subclass: usize,
        _dw_ref_data: usize,
    ) -> isize {
        match u_msg {
            WM_NCCALCSIZE => 0,
            WM_NCACTIVATE => 1,
            WM_NCPAINT => 0,
            WM_NCHITTEST => HTTRANSPARENT,
            WM_STYLECHANGING if l_param != 0 => {
                let ss = &mut *(l_param as *mut StyleStruct);
                let index = w_param as i32;
                if index == GWL_STYLE {
                    ss.style_new = sanitize_overlay_style(ss.style_new);
                    0
                } else if index == GWL_EXSTYLE {
                    ss.style_new = sanitize_overlay_ex_style(ss.style_new);
                    0
                } else {
                    DefSubclassProc(hwnd, u_msg, w_param, l_param)
                }
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(overlay_subclass_proc), u_id_subclass);
                DefSubclassProc(hwnd, u_msg, w_param, l_param)
            }
            _ => DefSubclassProc(hwnd, u_msg, w_param, l_param),
        }
    }

    let RawWindowHandle::Win32(handle) = raw else {
        return;
    };
    let hwnd = handle.hwnd.get();

    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(overlay_subclass_proc), OVERLAY_SUBCLASS_ID, 0);

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, sanitize_overlay_style(style) as isize);

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            sanitize_overlay_ex_style(ex_style) as isize,
        );

        let corner_pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner_pref as *const u32).cast::<c_void>(),
            size_of::<u32>() as u32,
        );

        let border_none = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&border_none as *const u32).cast::<c_void>(),
            size_of::<u32>() as u32,
        );

        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn apply_raw_overlay_flags(_raw: RawWindowHandle, _opacity: f32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_overlay_style_strips_caption_and_enforces_popup() {
        let with_caption_hidden = 0x00CF_0000;
        let sanitized_hidden = sanitize_overlay_style(with_caption_hidden);
        assert_eq!(sanitized_hidden & WIN32_STRIP_STYLE_MASK, 0);
        assert_ne!(sanitized_hidden & WIN32_WS_POPUP, 0);
        assert_ne!(sanitized_hidden & WIN32_WS_CLIPSIBLINGS, 0);
        assert_eq!(sanitized_hidden & 0x1000_0000, 0);

        let with_visible = 0x10CF_0000;
        let sanitized_visible = sanitize_overlay_style(with_visible);
        assert_eq!(sanitized_visible & WIN32_STRIP_STYLE_MASK, 0);
        assert_ne!(sanitized_visible & WIN32_WS_POPUP, 0);
        assert_ne!(sanitized_visible & WIN32_WS_CLIPSIBLINGS, 0);
        assert_ne!(sanitized_visible & 0x1000_0000, 0);
    }

    #[test]
    fn sanitize_overlay_ex_style_strips_edges_and_enforces_overlay_flags() {
        let dirty = WIN32_STRIP_EXSTYLE_MASK;
        let sanitized = sanitize_overlay_ex_style(dirty);
        assert_eq!(sanitized & WIN32_STRIP_EXSTYLE_MASK, 0);
        assert_eq!(
            sanitized & WIN32_REQUIRED_EX_STYLES,
            WIN32_REQUIRED_EX_STYLES
        );
    }

    #[test]
    fn windows_skips_iced_mouse_passthrough() {
        assert!(!should_use_iced_mouse_passthrough(true));
        assert!(should_use_iced_mouse_passthrough(false));
    }
}
