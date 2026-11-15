use squad_dsp::{AudioEngineHandle, MicActivationMode, SoundCue};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhysicalInput {
    #[cfg(target_os = "macos")]
    MacKey(u16),
    #[cfg(target_os = "macos")]
    MacMouseButton(u32),
    #[cfg(target_os = "windows")]
    WinVKey(i32),
    Shift,
    Ctrl,
    Alt,
}

pub struct HotkeyController {
    toggle_mute: Option<PhysicalInput>,
    ptt: Option<PhysicalInput>,
    ptm: Option<PhysicalInput>,
    prev_toggle_down: AtomicBool,
}

impl HotkeyController {
    pub fn new(toggle_key: &str, ptt_key: &str, ptm_key: &str) -> Self {
        let mut controller = Self {
            toggle_mute: None,
            ptt: None,
            ptm: None,
            prev_toggle_down: AtomicBool::new(false),
        };
        controller.rebind(toggle_key, ptt_key, ptm_key);
        controller
    }

    pub fn rebind(&mut self, toggle_key: &str, ptt_key: &str, ptm_key: &str) {
        self.toggle_mute = parse_input(toggle_key);
        self.ptt = parse_input(ptt_key);
        self.ptm = parse_input(ptm_key);
        self.prev_toggle_down.store(false, Ordering::Relaxed);
    }

    /// Samples hardware key/mouse states passively every UI tick without intercepting or blocking OS input.
    pub fn poll_events(&self, audio: &AudioEngineHandle) {
        let toggle_down = self.toggle_mute.is_some_and(is_input_pressed);
        let was_toggle_down = self.prev_toggle_down.swap(toggle_down, Ordering::Relaxed);
        if toggle_down && !was_toggle_down {
            let new_muted = !audio.is_mic_muted();
            audio.set_mic_muted(new_muted);
            audio.play_cue(if new_muted {
                SoundCue::Mute
            } else {
                SoundCue::Unmute
            });
        }

        let held = match audio.activation_mode() {
            MicActivationMode::ToggleMute => false,
            MicActivationMode::PushToTalk => self.ptt.is_some_and(is_input_pressed),
            MicActivationMode::PushToMute => self.ptm.is_some_and(is_input_pressed),
        };
        audio.set_hotkey_held(held);
    }
}

fn parse_input(raw: &str) -> Option<PhysicalInput> {
    let norm = raw.trim().to_ascii_lowercase();
    if norm.is_empty() {
        return None;
    }

    match norm.as_str() {
        "shift" | "shift_any" => return Some(PhysicalInput::Shift),
        "ctrl" | "control" | "ctrl_any" => return Some(PhysicalInput::Ctrl),
        "alt" | "option" | "alt_any" => return Some(PhysicalInput::Alt),
        _ => {}
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(btn) = mac_mouse_button(&norm) {
            return Some(PhysicalInput::MacMouseButton(btn));
        }
        if let Some(kc) = mac_keycode(&norm) {
            return Some(PhysicalInput::MacKey(kc));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(vk) = win_vkey(&norm) {
            return Some(PhysicalInput::WinVKey(vk));
        }
    }

    None
}

fn is_input_pressed(input: PhysicalInput) -> bool {
    #[cfg(target_os = "macos")]
    {
        match input {
            PhysicalInput::MacKey(kc) => unsafe { CGEventSourceKeyState(1, kc) },
            PhysicalInput::MacMouseButton(btn) => unsafe { CGEventSourceButtonState(1, btn) },
            PhysicalInput::Shift => unsafe {
                CGEventSourceKeyState(1, 0x38) || CGEventSourceKeyState(1, 0x3C)
            },
            PhysicalInput::Ctrl => unsafe {
                CGEventSourceKeyState(1, 0x3B) || CGEventSourceKeyState(1, 0x3E)
            },
            PhysicalInput::Alt => unsafe {
                CGEventSourceKeyState(1, 0x3A) || CGEventSourceKeyState(1, 0x3D)
            },
        }
    }
    #[cfg(target_os = "windows")]
    {
        match input {
            PhysicalInput::WinVKey(vk) => is_win_vk_down(vk),
            PhysicalInput::Shift => is_win_vk_down(0x10),
            PhysicalInput::Ctrl => is_win_vk_down(0x11),
            PhysicalInput::Alt => is_win_vk_down(0x12),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = input;
        false
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceKeyState(state_id: u32, key: u16) -> bool;
    fn CGEventSourceButtonState(state_id: u32, button: u32) -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: *const std::ffi::c_void;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: *const std::ffi::c_void;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFDictionaryCreate(
        allocator: *const std::ffi::c_void,
        keys: *const *const std::ffi::c_void,
        values: *const *const std::ffi::c_void,
        num_values: isize,
        key_callbacks: *const std::ffi::c_void,
        value_callbacks: *const std::ffi::c_void,
    ) -> *const std::ffi::c_void;
    fn CFRelease(cf: *const std::ffi::c_void);
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
}

#[cfg(target_os = "windows")]
fn is_win_vk_down(vkey: i32) -> bool {
    (unsafe { GetAsyncKeyState(vkey) } as u16 & 0x8000) != 0
}

#[cfg(target_os = "macos")]
fn mac_mouse_button(norm: &str) -> Option<u32> {
    match norm {
        "mouse3" | "middlemouse" | "mousemiddle" => Some(2),
        "mouse4" | "xbutton1" => Some(3),
        "mouse5" | "xbutton2" => Some(4),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn mac_keycode(norm: &str) -> Option<u16> {
    match norm {
        "keya" | "a" => Some(0x00),
        "keys" | "s" => Some(0x01),
        "keyd" | "d" => Some(0x02),
        "keyf" | "f" => Some(0x03),
        "keyh" | "h" => Some(0x04),
        "keyg" | "g" => Some(0x05),
        "keyz" | "z" => Some(0x06),
        "keyx" | "x" => Some(0x07),
        "keyc" | "c" => Some(0x08),
        "keyv" | "v" => Some(0x09),
        "keyb" | "b" => Some(0x0B),
        "keyq" | "q" => Some(0x0C),
        "keyw" | "w" => Some(0x0D),
        "keye" | "e" => Some(0x0E),
        "keyr" | "r" => Some(0x0F),
        "keyy" | "y" => Some(0x10),
        "keyt" | "t" => Some(0x11),
        "digit1" | "1" => Some(0x12),
        "digit2" | "2" => Some(0x13),
        "digit3" | "3" => Some(0x14),
        "digit4" | "4" => Some(0x15),
        "digit6" | "6" => Some(0x16),
        "digit5" | "5" => Some(0x17),
        "equal" | "=" => Some(0x18),
        "digit9" | "9" => Some(0x19),
        "digit7" | "7" => Some(0x1A),
        "minus" | "-" => Some(0x1B),
        "digit8" | "8" => Some(0x1C),
        "digit0" | "0" => Some(0x1D),
        "bracketright" | "]" => Some(0x1E),
        "keyo" | "o" => Some(0x1F),
        "keyu" | "u" => Some(0x20),
        "bracketleft" | "[" => Some(0x21),
        "keyi" | "i" => Some(0x22),
        "keyp" | "p" => Some(0x23),
        "enter" | "return" => Some(0x24),
        "keyl" | "l" => Some(0x25),
        "keyj" | "j" => Some(0x26),
        "quote" | "'" => Some(0x27),
        "keyk" | "k" => Some(0x28),
        "semicolon" | ";" => Some(0x29),
        "backslash" | "\\" => Some(0x2A),
        "comma" | "," => Some(0x2B),
        "slash" | "/" => Some(0x2C),
        "keyn" | "n" => Some(0x2D),
        "keym" | "m" => Some(0x2E),
        "period" | "." => Some(0x2F),
        "tab" => Some(0x30),
        "space" | " " => Some(0x31),
        "backquote" | "`" | "tilde" => Some(0x32),
        "backspace" => Some(0x33),
        "escape" | "esc" => Some(0x35),
        "metaright" | "superright" => Some(0x36),
        "metaleft" | "superleft" => Some(0x37),
        "shiftleft" | "lshift" => Some(0x38),
        "capslock" => Some(0x39),
        "altleft" | "lalt" => Some(0x3A),
        "controlleft" | "lctrl" => Some(0x3B),
        "shiftright" | "rshift" => Some(0x3C),
        "altright" | "ralt" => Some(0x3D),
        "controlright" | "rctrl" => Some(0x3E),
        "f1" => Some(0x7A),
        "f2" => Some(0x78),
        "f3" => Some(0x63),
        "f4" => Some(0x76),
        "f5" => Some(0x60),
        "f6" => Some(0x61),
        "f7" => Some(0x62),
        "f8" => Some(0x64),
        "f9" => Some(0x65),
        "f10" => Some(0x6D),
        "f11" => Some(0x67),
        "f12" => Some(0x6F),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn win_vkey(norm: &str) -> Option<i32> {
    match norm {
        "keya" | "a" => Some(0x41),
        "keyb" | "b" => Some(0x42),
        "keyc" | "c" => Some(0x43),
        "keyd" | "d" => Some(0x44),
        "keye" | "e" => Some(0x45),
        "keyf" | "f" => Some(0x46),
        "keyg" | "g" => Some(0x47),
        "keyh" | "h" => Some(0x48),
        "keyi" | "i" => Some(0x49),
        "keyj" | "j" => Some(0x4A),
        "keyk" | "k" => Some(0x4B),
        "keyl" | "l" => Some(0x4C),
        "keym" | "m" => Some(0x4D),
        "keyn" | "n" => Some(0x4E),
        "keyo" | "o" => Some(0x4F),
        "keyp" | "p" => Some(0x50),
        "keyq" | "q" => Some(0x51),
        "keyr" | "r" => Some(0x52),
        "keys" | "s" => Some(0x53),
        "keyt" | "t" => Some(0x54),
        "keyu" | "u" => Some(0x55),
        "keyv" | "v" => Some(0x56),
        "keyw" | "w" => Some(0x57),
        "keyx" | "x" => Some(0x58),
        "keyy" | "y" => Some(0x59),
        "keyz" | "z" => Some(0x5A),
        "digit0" | "0" => Some(0x30),
        "digit1" | "1" => Some(0x31),
        "digit2" | "2" => Some(0x32),
        "digit3" | "3" => Some(0x33),
        "digit4" | "4" => Some(0x34),
        "digit5" | "5" => Some(0x35),
        "digit6" | "6" => Some(0x36),
        "digit7" | "7" => Some(0x37),
        "digit8" | "8" => Some(0x38),
        "digit9" | "9" => Some(0x39),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        "capslock" => Some(0x14),
        "space" | " " => Some(0x20),
        "tab" => Some(0x09),
        "enter" | "return" => Some(0x0D),
        "escape" | "esc" => Some(0x1B),
        "backspace" => Some(0x08),
        "backquote" | "`" | "tilde" => Some(0xC0),
        "minus" | "-" => Some(0xBD),
        "equal" | "=" => Some(0xBB),
        "bracketleft" | "[" => Some(0xDB),
        "bracketright" | "]" => Some(0xDD),
        "backslash" | "\\" => Some(0xDC),
        "semicolon" | ";" => Some(0xBA),
        "quote" | "'" => Some(0xDE),
        "comma" | "," => Some(0xBC),
        "period" | "." => Some(0xBE),
        "slash" | "/" => Some(0xBF),
        "shiftleft" | "lshift" => Some(0xA0),
        "shiftright" | "rshift" => Some(0xA1),
        "controlleft" | "lctrl" => Some(0xA2),
        "controlright" | "rctrl" => Some(0xA3),
        "altleft" | "lalt" => Some(0xA4),
        "altright" | "ralt" => Some(0xA5),
        "mouse3" | "middlemouse" | "mousemiddle" => Some(0x04),
        "mouse4" | "xbutton1" => Some(0x05),
        "mouse5" | "xbutton2" => Some(0x06),
        _ => None,
    }
}

pub fn is_macos_accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

pub fn is_macos_input_monitoring_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGPreflightListenEventAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Triggers native macOS TCC dialogs for Accessibility and Input Monitoring.
pub fn request_macos_permissions() -> (bool, bool) {
    #[cfg(target_os = "macos")]
    {
        let ax_trusted = unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let opts = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const u8 as *const std::ffi::c_void,
                &kCFTypeDictionaryValueCallBacks as *const u8 as *const std::ffi::c_void,
            );
            let trusted = AXIsProcessTrustedWithOptions(opts);
            if !opts.is_null() {
                CFRelease(opts);
            }
            trusted
        };
        let input_trusted = unsafe { CGRequestListenEventAccess() };
        (ax_trusted, input_trusted)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (true, true)
    }
}