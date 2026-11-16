use crate::ui::Message;
use iced::window::raw_window_handle::RawWindowHandle;
use iced::{window, Task};

pub fn configure_overlay_window(id: window::Id) -> Task<Message> {
    Task::batch([
        window::enable_mouse_passthrough(id),
        window::run_with_handle(id, |handle| {
            apply_raw_overlay_flags(handle.as_raw());
            Message::OverlayConfigured
        }),
    ])
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
        let ns_view = handle.ns_view.as_ptr() as *mut c_void;
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

        let ns_window = send_ptr(ns_view, sel_registerName(b"window\0".as_ptr() as *const c_char));
        if ns_window.is_null() {
            return;
        }

        let ns_color_cls = objc_getClass(b"NSColor\0".as_ptr() as *const c_char);
        let clear_color = send_ptr(
            ns_color_cls,
            sel_registerName(b"clearColor\0".as_ptr() as *const c_char),
        );

        send_void_bool(
            ns_window,
            sel_registerName(b"setOpaque:\0".as_ptr() as *const c_char),
            false,
        );
        send_void_bool(
            ns_window,
            sel_registerName(b"setHasShadow:\0".as_ptr() as *const c_char),
            false,
        );
        if !clear_color.is_null() {
            send_void_ptr(
                ns_window,
                sel_registerName(b"setBackgroundColor:\0".as_ptr() as *const c_char),
                clear_color,
            );
        }
        send_void_bool(
            ns_window,
            sel_registerName(b"setIgnoresMouseEvents:\0".as_ptr() as *const c_char),
            true,
        );
        // NSStatusWindowLevel = 25 floats cleanly above standard and auxiliary fullscreen spaces.
        send_void_isize(
            ns_window,
            sel_registerName(b"setLevel:\0".as_ptr() as *const c_char),
            25,
        );
        // NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary.
        send_void_usize(
            ns_window,
            sel_registerName(b"setCollectionBehavior:\0".as_ptr() as *const c_char),
            (1 << 0) | (1 << 8),
        );
    }
}

#[cfg(target_os = "windows")]
fn apply_raw_overlay_flags(raw: RawWindowHandle) {
    const GWL_EXSTYLE: i32 = -20;
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
    }

    let RawWindowHandle::Win32(handle) = raw else {
        return;
    };
    let hwnd = handle.hwnd.get();

    unsafe {
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let desired = ex_style
            | WS_EX_LAYERED
            | WS_EX_TRANSPARENT
            | WS_EX_TOPMOST
            | WS_EX_NOACTIVATE
            | WS_EX_TOOLWINDOW;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        let margins = Margins {
            cx_left_width: -1,
            cx_right_width: -1,
            cy_top_height: -1,
            cy_bottom_height: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn apply_raw_overlay_flags(_raw: RawWindowHandle) {}
