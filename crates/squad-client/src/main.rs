#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod crypto_identity;
mod hotkeys;
mod net;
mod tray;
mod ui;
mod updater;

use ui::SquadApp;

#[cfg(target_os = "macos")]
const INFO_PLIST_STR: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.squadair.client</string>
    <key>CFBundleName</key>
    <string>SquadAir</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>SquadAir uses your microphone for end-to-end encrypted P2P voice chat.</string>
</dict>
</plist>
"#;

#[cfg(target_os = "macos")]
const fn build_info_plist() -> [u8; INFO_PLIST_STR.len()] {
    let mut out = [0u8; INFO_PLIST_STR.len()];
    let bytes = INFO_PLIST_STR.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        out[i] = bytes[i];
        i += 1;
    }
    out
}

#[cfg(target_os = "macos")]
#[link_section = "__TEXT,__info_plist"]
#[used]
static INFO_PLIST: [u8; INFO_PLIST_STR.len()] = build_info_plist();

pub fn load_window_icon() -> Option<iced::window::Icon> {
    let bytes = include_bytes!("../../../assets/icons/icon.png");
    let img = image::load_from_memory(bytes).ok()?;
    let resized = img
        .resize_exact(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    iced::window::icon::from_rgba(resized.into_raw(), 64, 64).ok()
}

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "squad_client=info,squad_dsp=info".into()),
        )
        .init();

    #[cfg(target_os = "macos")]
    {
        if !hotkeys::is_macos_accessibility_trusted()
            || !hotkeys::is_macos_input_monitoring_trusted()
        {
            hotkeys::request_macos_permissions();
        }
    }

    iced::daemon(SquadApp::title, SquadApp::update, SquadApp::view)
        .subscription(SquadApp::subscription)
        .theme(SquadApp::theme)
        .style(SquadApp::style)
        .antialiasing(true)
        .run_with(SquadApp::new)
}
