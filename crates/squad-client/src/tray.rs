use squad_dsp::{AudioEngineHandle, SoundCue};
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::Notify;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

pub enum TrayAction {
    ShowWindow,
    ToggleMute,
    Quit,
}

pub struct SystemTray {
    _tray: Option<TrayIcon>,
    menu_rx: Mutex<mpsc::Receiver<MenuEvent>>,
    show_item_id: Option<tray_icon::menu::MenuId>,
    mute_item_id: Option<tray_icon::menu::MenuId>,
    quit_item_id: Option<tray_icon::menu::MenuId>,
}

impl SystemTray {
    pub fn new(tray_notify: Arc<Notify>) -> Self {
        let (menu_tx, menu_rx) = mpsc::channel();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = menu_tx.send(event);
            tray_notify.notify_one();
        }));

        let menu = Menu::new();
        let show_item = MenuItem::new("Show Squad-Air", true, None);
        let mute_item = MenuItem::new("Squad-Air: Toggle Mic Mute", true, None);
        let quit_item = MenuItem::new("Quit Squad-Air", true, None);

        let show_id = show_item.id().clone();
        let mute_id = mute_item.id().clone();
        let quit_id = quit_item.id().clone();

        let _ = menu.append(&show_item);
        let _ = menu.append(&mute_item);
        let _ = menu.append(&quit_item);

        let icon = load_custom_tray_icon().or_else(build_procedural_icon);
        let tray = icon.and_then(|ic| {
            TrayIconBuilder::new()
                .with_tooltip("Squad-Air P2P Voice")
                .with_menu(Box::new(menu))
                .with_icon(ic)
                .build()
                .ok()
        });

        Self {
            _tray: tray,
            menu_rx: Mutex::new(menu_rx),
            show_item_id: Some(show_id),
            mute_item_id: Some(mute_id),
            quit_item_id: Some(quit_id),
        }
    }

    pub fn poll_action(&self, audio: &AudioEngineHandle) -> Option<TrayAction> {
        let Ok(rx) = self.menu_rx.lock() else {
            return None;
        };
        while let Ok(event) = rx.try_recv() {
            if Some(&event.id) == self.show_item_id.as_ref() {
                return Some(TrayAction::ShowWindow);
            }
            if Some(&event.id) == self.mute_item_id.as_ref() {
                let next_mute = !audio.is_mic_muted();
                audio.set_mic_muted(next_mute);
                audio.play_cue(if next_mute {
                    SoundCue::Mute
                } else {
                    SoundCue::Unmute
                });
                return Some(TrayAction::ToggleMute);
            }
            if Some(&event.id) == self.quit_item_id.as_ref() {
                return Some(TrayAction::Quit);
            }
        }
        None
    }
}

fn load_custom_tray_icon() -> Option<Icon> {
    let bytes = include_bytes!("../../../assets/icons/icon.png");
    let img = image::load_from_memory(bytes).ok()?;
    let resized = img
        .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    Icon::from_rgba(resized.into_raw(), 64, 64).ok()
}

fn build_procedural_icon() -> Option<Icon> {
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];
    let center = (size as f32) / 2.0;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = (y * size + x) * 4;

            if dist <= 13.5 {
                // Material 3 Primary (#A8C7FA) circle with Dark core (#062E6F)
                if dist <= 5.5 {
                    rgba[idx] = 0x06;
                    rgba[idx + 1] = 0x2E;
                    rgba[idx + 2] = 0x6F;
                    rgba[idx + 3] = 255;
                } else {
                    rgba[idx] = 0xA8;
                    rgba[idx + 1] = 0xC7;
                    rgba[idx + 2] = 0xFA;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
    Icon::from_rgba(rgba, size as u32, size as u32).ok()
}

pub fn release_os_memory() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        unsafe {
            let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }

    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn SetProcessWorkingSetSize(h_process: isize, min_size: usize, max_size: usize) -> i32;
        }
        unsafe {
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }
}
