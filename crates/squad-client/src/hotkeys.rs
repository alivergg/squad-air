use squad_dsp::{AudioEngineHandle, MicActivationMode, SoundCue};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::sync::Notify;

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

#[derive(Clone, Copy, Default)]
struct HotkeyBindings {
    toggle_mute: Option<PhysicalInput>,
    ptt: Option<PhysicalInput>,
    ptm: Option<PhysicalInput>,
    deafen: Option<PhysicalInput>,
}

struct HotkeyShared {
    bindings: Mutex<(u64, HotkeyBindings)>,
    generation: AtomicU64,
    active: AtomicBool,
    stop: AtomicBool,
}

pub struct HotkeyController {
    shared: Arc<HotkeyShared>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyController {
    pub fn new(
        toggle_key: &str,
        ptt_key: &str,
        ptm_key: &str,
        deafen_key: &str,
        audio: AudioEngineHandle,
        ui_notify: Arc<Notify>,
    ) -> Self {
        let initial = HotkeyBindings {
            toggle_mute: parse_input(toggle_key),
            ptt: parse_input(ptt_key),
            ptm: parse_input(ptm_key),
            deafen: parse_input(deafen_key),
        };
        let shared = Arc::new(HotkeyShared {
            bindings: Mutex::new((1, initial)),
            generation: AtomicU64::new(1),
            active: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("squad-hotkeys".into())
            .spawn(move || {
                run_hotkey_loop(worker_shared, audio, ui_notify);
            })
            .ok();

        Self { shared, worker }
    }

    pub fn rebind(&self, toggle_key: &str, ptt_key: &str, ptm_key: &str, deafen_key: &str) {
        let next = HotkeyBindings {
            toggle_mute: parse_input(toggle_key),
            ptt: parse_input(ptt_key),
            ptm: parse_input(ptm_key),
            deafen: parse_input(deafen_key),
        };
        let next_gen = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if let Ok(mut guard) = self.shared.bindings.lock() {
            *guard = (next_gen, next);
        }
        if let Some(ref handle) = self.worker {
            handle.thread().unpark();
        }
    }

    pub fn set_active(&self, active: bool) {
        let prev = self.shared.active.swap(active, Ordering::AcqRel);
        if active && !prev {
            if let Some(ref handle) = self.worker {
                handle.thread().unpark();
            }
        }
    }

    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(ref handle) = self.worker {
            handle.thread().unpark();
        }
    }
}

impl Drop for HotkeyController {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn run_hotkey_loop(shared: Arc<HotkeyShared>, audio: AudioEngineHandle, ui_notify: Arc<Notify>) {
    let mut local_gen = 0u64;
    let mut bindings = HotkeyBindings::default();
    let mut prev_toggle_down = false;
    let mut prev_deafen_down = false;
    let mut prev_held = false;

    while !shared.stop.load(Ordering::Acquire) {
        if !shared.active.load(Ordering::Acquire) {
            if prev_held {
                prev_held = false;
                audio.set_hotkey_held(false);
            }
            prev_toggle_down = false;
            prev_deafen_down = false;
            std::thread::park_timeout(Duration::from_millis(250));
            continue;
        }

        let cur_gen = shared.generation.load(Ordering::Acquire);
        if cur_gen != local_gen {
            if let Ok(guard) = shared.bindings.lock() {
                local_gen = guard.0;
                bindings = guard.1;
                prev_toggle_down = false;
                prev_deafen_down = false;
            }
        }

        let toggle_down = bindings.toggle_mute.is_some_and(is_input_pressed);
        if toggle_down && !prev_toggle_down {
            let new_muted = !audio.is_mic_muted();
            audio.set_mic_muted(new_muted);
            audio.play_cue(if new_muted {
                SoundCue::Mute
            } else {
                SoundCue::Unmute
            });
            ui_notify.notify_one();
        }
        prev_toggle_down = toggle_down;

        let deafen_down = bindings.deafen.is_some_and(is_input_pressed);
        if deafen_down && !prev_deafen_down {
            let new_deaf = !audio.is_deafened();
            audio.set_deafened(new_deaf);
            audio.play_cue(if new_deaf {
                SoundCue::Mute
            } else {
                SoundCue::Unmute
            });
            ui_notify.notify_one();
        }
        prev_deafen_down = deafen_down;

        let held = match audio.activation_mode() {
            MicActivationMode::ToggleMute => false,
            MicActivationMode::PushToTalk => bindings.ptt.is_some_and(is_input_pressed),
            MicActivationMode::PushToMute => bindings.ptm.is_some_and(is_input_pressed),
        };
        if held != prev_held {
            prev_held = held;
            audio.set_hotkey_held(held);
            ui_notify.notify_one();
        }

        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn is_supported_hotkey_name(raw: &str) -> bool {
    parse_input(raw).is_some()
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
            PhysicalInput::MacKey(kc) => unsafe { CGEventSourceKeyState(1, kc) != 0 },
            PhysicalInput::MacMouseButton(btn) => unsafe {
                CGEventSourceButtonState(1, btn) != 0
            },
            PhysicalInput::Shift => unsafe {
                CGEventSourceKeyState(1, 0x38) != 0 || CGEventSourceKeyState(1, 0x3C) != 0
            },
            PhysicalInput::Ctrl => unsafe {
                CGEventSourceKeyState(1, 0x3B) != 0 || CGEventSourceKeyState(1, 0x3E) != 0
            },
            PhysicalInput::Alt => unsafe {
                CGEventSourceKeyState(1, 0x3A) != 0 || CGEventSourceKeyState(1, 0x3D) != 0
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
    fn CGEventSourceKeyState(state_id: u32, key: u16) -> u8;
    fn CGEventSourceButtonState(state_id: u32, button: u32) -> u8;
    fn CGPreflightListenEventAccess() -> u8;
    fn CGRequestListenEventAccess() -> u8;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: *const std::ffi::c_void;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> u8;
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
        "metaright" | "superright" | "rwin" => Some(0x36),
        "metaleft" | "superleft" | "lwin" => Some(0x37),
        "shiftleft" | "lshift" => Some(0x38),
        "capslock" => Some(0x39),
        "altleft" | "lalt" => Some(0x3A),
        "controlleft" | "lctrl" => Some(0x3B),
        "shiftright" | "rshift" => Some(0x3C),
        "altright" | "ralt" => Some(0x3D),
        "controlright" | "rctrl" => Some(0x3E),
        "insert" => Some(0x72),
        "home" => Some(0x73),
        "pageup" => Some(0x74),
        "delete" => Some(0x75),
        "end" => Some(0x77),
        "pagedown" => Some(0x79),
        "arrowleft" | "left" => Some(0x7B),
        "arrowright" | "right" => Some(0x7C),
        "arrowdown" | "down" => Some(0x7D),
        "arrowup" | "up" => Some(0x7E),
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
        "insert" => Some(0x2D),
        "delete" => Some(0x2E),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" => Some(0x21),
        "pagedown" => Some(0x22),
        "arrowleft" | "left" => Some(0x25),
        "arrowup" | "up" => Some(0x26),
        "arrowright" | "right" => Some(0x27),
        "arrowdown" | "down" => Some(0x28),
        "lwin" | "superleft" | "metaleft" => Some(0x5B),
        "rwin" | "superright" | "metaright" => Some(0x5C),
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
        unsafe { AXIsProcessTrusted() != 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

pub fn is_macos_input_monitoring_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGPreflightListenEventAccess() != 0 }
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
            let trusted = AXIsProcessTrustedWithOptions(opts) != 0;
            if !opts.is_null() {
                CFRelease(opts);
            }
            trusted
        };
        let input_trusted = unsafe { CGRequestListenEventAccess() != 0 };
        (ax_trusted, input_trusted)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (true, true)
    }
}
