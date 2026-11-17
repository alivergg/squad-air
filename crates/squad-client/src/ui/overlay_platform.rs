use crate::ui::Message;
use iced::window::raw_window_handle::RawWindowHandle;
use iced::{window, Task};

pub fn configure_overlay_window(id: window::Id) -> Task<Message> {
    window::enable_mouse_passthrough(id).chain(window::run_with_handle(id, |handle| {
        apply_raw_overlay_flags(handle.as_raw());
        Message::OverlayConfigured
    }))
}

#[cfg(target_os = "macos")]
fn apply_raw_overlay_flags(raw: RawWindowHandle) {
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

    let RawWindowHandle::AppKit(handle) = raw else {
        return;
    };

    unsafe {
        let ns_view = handle.ns_view.as_ptr();
        if ns_view.is_null() {
            return;
        }

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

        let ns_window = send_ptr(ns_view, sel_registerName(c"window".as_ptr()));
        if ns_window.is_null() {
            return;
        }

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
    }
}

#[cfg(target_os = "windows")]
fn apply_raw_overlay_flags(raw: RawWindowHandle) {
    use std::ffi::c_void;
    use std::mem::size_of;

    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const WS_POPUP: isize = 0x8000_0000u32 as isize;
    const WS_VISIBLE: isize = 0x1000_0000;
    const WS_CLIPSIBLINGS: isize = 0x0400_0000;
    const STRIP_STYLE_MASK: isize = 0x00CF_0000;
    const STRIP_EXSTYLE_MASK: isize = 0x0006_0301;
    const WS_EX_TOPMOST: isize = 0x0000_0008;
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;

    const DWMWA_NCRENDERING_POLICY: u32 = 2;
    const DWMNCRP_DISABLED: u32 = 1;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_DONOTROUND: u32 = 1;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

    #[repr(C)]
    struct Margins {
        cx_left_width: i32,
        cx_right_width: i32,
        cy_top_height: i32,
        cy_bottom_height: i32,
    }

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
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmExtendFrameIntoClientArea(hwnd: isize, p_mar_inset: *const Margins) -> i32;
        fn DwmSetWindowAttribute(
            hwnd: isize,
            dw_attribute: u32,
            pv_attribute: *const c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    let RawWindowHandle::Win32(handle) = raw else {
        return;
    };
    let hwnd = handle.hwnd.get();

    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let desired_style = (style & !STRIP_STYLE_MASK) | WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS;
        SetWindowLongPtrW(hwnd, GWL_STYLE, desired_style);

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired_ex_style = (ex_style & !STRIP_EXSTYLE_MASK)
            | WS_EX_LAYERED
            | WS_EX_TRANSPARENT
            | WS_EX_TOPMOST
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired_ex_style);

        let margins = Margins {
            cx_left_width: 0,
            cx_right_width: 0,
            cy_top_height: 0,
            cy_bottom_height: 0,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

        let nc_policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            (&nc_policy as *const u32).cast::<c_void>(),
            size_of::<u32>() as u32,
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
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn apply_raw_overlay_flags(_raw: RawWindowHandle) {}
