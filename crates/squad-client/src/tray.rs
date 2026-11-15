use squad_dsp::{AudioEngineHandle, SoundCue};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

pub enum TrayAction {
    ToggleMute,
    Quit,
}

pub struct SystemTray {
    _tray: Option<TrayIcon>,
    mute_item_id: Option<tray_icon::menu::MenuId>,
    quit_item_id: Option<tray_icon::menu::MenuId>,
}

impl SystemTray {
    pub fn new() -> Self {
        let menu = Menu::new();
        let mute_item = MenuItem::new("Squad-Air: Toggle Mic Mute", true, None);
        let quit_item = MenuItem::new("Quit Squad-Air", true, None);

        let mute_id = mute_item.id().clone();
        let quit_id = quit_item.id().clone();

        let _ = menu.append(&mute_item);
        let _ = menu.append(&quit_item);

        let icon = build_procedural_icon();
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
            mute_item_id: Some(mute_id),
            quit_item_id: Some(quit_id),
        }
    }

    pub fn poll_action(&self, audio: &AudioEngineHandle) -> Option<TrayAction> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
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