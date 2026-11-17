use crate::hotkeys::{is_macos_accessibility_trusted, is_macos_input_monitoring_trusted};
use crate::ui::{
    components::{avatar_circle, mic_sensitivity_meter},
    theme::{
        m3_menu_style, m3_pick_list_style, m3_slider_style, m3_text_input_style, pill_chip_button,
        pill_danger_button, pill_primary_button, pill_tonal_button, surface_card_style,
        M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS, M3_WARNING,
    },
    HotkeySlot, Message, SquadApp, SYSTEM_DEFAULT_DEVICE,
};
use iced::{
    alignment::Vertical,
    widget::{
        button, column, container, horizontal_space, pick_list, row, scrollable, slider, text,
        text_input,
    },
    Element, Length,
};
use squad_dsp::{JitterProfile, NoiseSuppressionMode};

pub fn view_settings(app: &SquadApp) -> Element<'_, Message> {
    let top_bar = row![
        column![
            text("Settings").size(22).color(M3_PRIMARY),
            text("Changes apply live without disconnecting your call")
                .size(12)
                .color(M3_ON_SURFACE_VARIANT),
        ]
        .spacing(2),
        horizontal_space(),
        button(text("Save & Close").size(13))
            .padding([10, 20])
            .style(pill_primary_button)
            .on_press(Message::CloseSettings),
    ]
    .align_y(Vertical::Center);

    let avatar_preview = avatar_circle(
        app.avatars.handle_for(&app.config.client_uuid()),
        72.0,
        false,
    );

    let profile_section = container(
        column![
            text("Profile").size(16),
            row![
                avatar_preview,
                column![
                    text(if app.config.custom_avatar_hash.is_some() {
                        "Custom Avatar"
                    } else {
                        "Generated Avatar"
                    })
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
                    row![
                        button(text("Choose Image...").size(12))
                            .padding([7, 14])
                            .style(pill_primary_button)
                            .on_press(Message::PickAvatarImage),
                        button(text("Reset Avatar").size(12))
                            .padding([7, 14])
                            .style(pill_tonal_button)
                            .on_press(Message::ResetAvatarIdenticon),
                    ]
                    .spacing(8),
                ]
                .spacing(8),
            ]
            .spacing(16)
            .align_y(Vertical::Center),
            row![
                column![
                    text("Display Name").size(12).color(M3_ON_SURFACE_VARIANT),
                    text_input("Your name", &app.config.nickname)
                        .on_input(Message::NicknameChanged)
                        .padding(10)
                        .size(14)
                        .style(m3_text_input_style),
                ]
                .width(Length::FillPortion(1))
                .spacing(6),
                column![
                    text("Voice Server").size(12).color(M3_ON_SURFACE_VARIANT),
                    text_input("host:port", &app.config.rendezvous_server)
                        .on_input(Message::RendezvousServerChanged)
                        .padding(10)
                        .size(14)
                        .style(m3_text_input_style),
                ]
                .width(Length::FillPortion(1))
                .spacing(6),
            ]
            .spacing(16),
        ]
        .spacing(12),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    let mut input_options = Vec::with_capacity(app.input_devices.len() + 1);
    input_options.push(SYSTEM_DEFAULT_DEVICE.to_string());
    input_options.extend(app.input_devices.iter().cloned());
    let selected_input = Some(
        app.config
            .input_device
            .clone()
            .unwrap_or_else(|| SYSTEM_DEFAULT_DEVICE.to_string()),
    );

    let mut output_options = Vec::with_capacity(app.output_devices.len() + 1);
    output_options.push(SYSTEM_DEFAULT_DEVICE.to_string());
    output_options.extend(app.output_devices.iter().cloned());
    let selected_output = Some(
        app.config
            .output_device
            .clone()
            .unwrap_or_else(|| SYSTEM_DEFAULT_DEVICE.to_string()),
    );

    let devices_section = container(
        column![
            row![
                text("Audio Hardware Devices").size(16),
                horizontal_space(),
                button(text("Refresh Devices").size(12))
                    .padding([6, 14])
                    .style(pill_tonal_button)
                    .on_press(Message::RefreshAudioDevices),
            ]
            .align_y(Vertical::Center),
            row![
                column![
                    text("Input Device (Microphone)")
                        .size(12)
                        .color(M3_ON_SURFACE_VARIANT),
                    pick_list(input_options, selected_input, Message::InputDeviceSelected)
                        .width(Length::Fill)
                        .padding(10)
                        .style(m3_pick_list_style)
                        .menu_style(m3_menu_style),
                ]
                .width(Length::FillPortion(1))
                .spacing(6),
                column![
                    text("Output Device (Speakers / Headset)")
                        .size(12)
                        .color(M3_ON_SURFACE_VARIANT),
                    pick_list(
                        output_options,
                        selected_output,
                        Message::OutputDeviceSelected
                    )
                    .width(Length::Fill)
                    .padding(10)
                    .style(m3_pick_list_style)
                    .menu_style(m3_menu_style),
                ]
                .width(Length::FillPortion(1))
                .spacing(6),
            ]
            .spacing(16),
        ]
        .spacing(12),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    let ns_mode = app.config.ns_mode;
    let aec_on = app.config.aec_enabled;
    let df3_badge = if app.audio.is_df3_available() {
        "Enhanced"
    } else {
        "Enhanced (Auto)"
    };

    let dsp_section = container(
        column![
            text("Voice & Audio Processing").size(16),
            row![
                text("Echo Cancellation:")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                button(text("Off").size(13))
                    .padding([8, 18])
                    .style(pill_chip_button(!aec_on))
                    .on_press(Message::SetAecEnabled(false)),
                button(text("On (Speakers)").size(13))
                    .padding([8, 18])
                    .style(pill_chip_button(aec_on))
                    .on_press(Message::SetAecEnabled(true)),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Noise Suppression:")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                button(text("Off").size(13))
                    .padding([8, 16])
                    .style(pill_chip_button(ns_mode == NoiseSuppressionMode::Off))
                    .on_press(Message::SetNsMode(NoiseSuppressionMode::Off)),
                button(text("Standard").size(13))
                    .padding([8, 16])
                    .style(pill_chip_button(ns_mode == NoiseSuppressionMode::Rnnoise))
                    .on_press(Message::SetNsMode(NoiseSuppressionMode::Rnnoise)),
                button(text(df3_badge).size(13))
                    .padding([8, 16])
                    .style(pill_chip_button(
                        ns_mode == NoiseSuppressionMode::DeepFilterNet3
                    ))
                    .on_press(Message::SetNsMode(NoiseSuppressionMode::DeepFilterNet3)),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Microphone Gain:")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                slider(0.2..=2.5, app.config.input_gain, Message::InputGainChanged)
                    .step(0.05_f32)
                    .style(m3_slider_style),
                text(format!("{:.0}%", app.config.input_gain * 100.0))
                    .size(13)
                    .width(Length::Fixed(52.0)),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
            row![
                text("Microphone Sensitivity:")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                slider(
                    -60.0..=0.0,
                    app.config.vad_threshold_db,
                    Message::VadThresholdChanged
                )
                .step(1.0_f32)
                .style(m3_slider_style),
                text(format!("{:.0} dB", app.config.vad_threshold_db))
                    .size(13)
                    .width(Length::Fixed(52.0)),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
            row![
                text("Live Input Level:")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT)
                    .width(Length::Fixed(330.0)),
                mic_sensitivity_meter(app.audio.local_mic_level_db(), app.config.vad_threshold_db),
                button(
                    text(if app.audio.is_mic_loopback() {
                        "Stop Mic Test"
                    } else {
                        "Hear Yourself"
                    })
                    .size(12)
                )
                .padding([7, 14])
                .style(if app.audio.is_mic_loopback() {
                    pill_danger_button
                } else {
                    pill_tonal_button
                })
                .on_press(Message::ToggleMicLoopback),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
            row![
                text("Latency Profile (Jitter Buffer):")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                row(JitterProfile::ALL.into_iter().map(|profile| {
                    let active = app.config.jitter_profile == profile;
                    let short_label = match profile {
                        JitterProfile::LanUltraLow => "LAN (10 ms)",
                        JitterProfile::LowLatency => "Low (20 ms)",
                        JitterProfile::Balanced => "Balanced (30 ms)",
                        JitterProfile::StableWifi => "Wi-Fi (50 ms)",
                    };
                    button(text(short_label).size(12))
                        .padding([8, 12])
                        .style(pill_chip_button(active))
                        .on_press(Message::SetJitterProfile(profile))
                        .into()
                }))
                .spacing(8),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Voice Bitrate (Opus):")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                row([
                    (24_000, "24 kbps"),
                    (40_000, "40 kbps"),
                    (64_000, "64 kbps"),
                    (96_000, "96 kbps"),
                ]
                .into_iter()
                .map(|(bps, label)| {
                    button(text(label).size(12))
                        .padding([8, 12])
                        .style(pill_chip_button(app.config.opus_bitrate_bps == bps))
                        .on_press(Message::SetOpusBitrate(bps))
                        .into()
                }))
                .spacing(8),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
        ]
        .spacing(14),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    let mut hotkey_col = column![
        text("Keyboard Shortcuts").size(16),
        text("Click any binding and press a key or Mouse3/4/5 (Esc to cancel). Works in fullscreen games.")
            .size(12)
            .color(M3_ON_SURFACE_VARIANT),
        row![
            hotkey_recorder_slot(
                "Push-to-Mute",
                &app.config.hotkey_ptm,
                HotkeySlot::Ptm,
                app.recording_hotkey,
            ),
            hotkey_recorder_slot(
                "Push-to-Talk",
                &app.config.hotkey_ptt,
                HotkeySlot::Ptt,
                app.recording_hotkey,
            ),
        ]
        .spacing(14),
        row![
            hotkey_recorder_slot(
                "Toggle Mic Mute",
                &app.config.hotkey_toggle_mute,
                HotkeySlot::ToggleMute,
                app.recording_hotkey,
            ),
            hotkey_recorder_slot(
                "Toggle Deafen",
                &app.config.hotkey_deafen,
                HotkeySlot::Deafen,
                app.recording_hotkey,
            ),
        ]
        .spacing(14),
    ]
    .spacing(12);

    if cfg!(target_os = "macos") {
        let ax_ok = is_macos_accessibility_trusted();
        let im_ok = is_macos_input_monitoring_trusted();
        let perm_text = format!(
            "macOS Permissions — Accessibility: {} • Input Monitoring: {}",
            if ax_ok { "Granted" } else { "Required" },
            if im_ok { "Granted" } else { "Required" }
        );
        let perm_color = if ax_ok && im_ok {
            M3_SUCCESS
        } else {
            M3_WARNING
        };

        hotkey_col = hotkey_col.push(
            row![
                text(perm_text).size(12).color(perm_color),
                horizontal_space(),
                button(text("Request macOS Permissions").size(12))
                    .padding([7, 14])
                    .style(pill_tonal_button)
                    .on_press(Message::RequestMacosPermissions),
            ]
            .align_y(Vertical::Center),
        );
    }

    let hotkeys_section = container(hotkey_col)
        .width(Length::Fill)
        .padding(20)
        .style(surface_card_style);

    let min_to_tray = app.config.minimize_to_tray;
    let behavior_section = container(
        column![
            text("Behavior & System Tray").size(16).color(M3_PRIMARY),
            row![
                column![
                    text("Minimize to System Tray on Close").size(13),
                    text("Keep your voice call and tray icon running in the background")
                        .size(11)
                        .color(M3_ON_SURFACE_VARIANT),
                ]
                .spacing(2),
                horizontal_space(),
                row![
                    button(text("Off").size(12))
                        .padding([6, 12])
                        .style(pill_chip_button(!min_to_tray))
                        .on_press(Message::SetMinimizeToTray(false)),
                    button(text("On").size(12))
                        .padding([6, 12])
                        .style(pill_chip_button(min_to_tray))
                        .on_press(Message::SetMinimizeToTray(true)),
                ]
                .spacing(6),
            ]
            .align_y(Vertical::Center),
        ]
        .spacing(12),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    let ov_enabled = app.config.overlay_enabled;
    let ov_speaking_only = app.config.overlay_speaking_only;
    let ov_opacity = app.config.overlay_opacity.clamp(0.30, 1.0);
    let ov_pct = (ov_opacity * 100.0).round() as u32;
    let overlay_section = container(
        column![
            text("In-Game Overlay").size(16).color(M3_PRIMARY),
            row![
                column![
                    text("Click-Through HUD Overlay").size(13),
                    text("Show squad members on the left edge over your games")
                        .size(11)
                        .color(M3_ON_SURFACE_VARIANT),
                ]
                .spacing(2),
                horizontal_space(),
                row![
                    button(text("Disabled").size(12))
                        .padding([6, 12])
                        .style(pill_chip_button(!ov_enabled))
                        .on_press(Message::SetOverlayEnabled(false)),
                    button(text("Enabled").size(12))
                        .padding([6, 12])
                        .style(pill_chip_button(ov_enabled))
                        .on_press(Message::SetOverlayEnabled(true)),
                ]
                .spacing(6),
            ]
            .align_y(Vertical::Center),
            row![
                column![
                    text("Display Mode").size(13),
                    text("Show everyone in the squad or only active speakers")
                        .size(11)
                        .color(M3_ON_SURFACE_VARIANT),
                ]
                .spacing(2),
                horizontal_space(),
                row![
                    button(text("All Squad Members").size(12))
                        .padding([6, 12])
                        .style(pill_chip_button(!ov_speaking_only))
                        .on_press(Message::SetOverlaySpeakingOnly(false)),
                    button(text("Speaking Only").size(12))
                        .padding([6, 12])
                        .style(pill_chip_button(ov_speaking_only))
                        .on_press(Message::SetOverlaySpeakingOnly(true)),
                ]
                .spacing(6),
            ]
            .align_y(Vertical::Center),
            row![
                text(format!("Opacity: {ov_pct}%"))
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT)
                    .width(Length::Fixed(110.0)),
                slider(0.30..=1.00, ov_opacity, Message::OverlayOpacityChanged)
                    .step(0.02_f32)
                    .width(Length::Fill)
                    .style(m3_slider_style),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
        ]
        .spacing(12),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    column![
        top_bar,
        scrollable(
            column![
                profile_section,
                devices_section,
                dsp_section,
                hotkeys_section,
                overlay_section,
                behavior_section,
            ]
            .spacing(16)
        )
        .height(Length::Fill),
    ]
    .spacing(16)
    .padding(24)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn hotkey_recorder_slot<'a>(
    label: &'a str,
    binding: &'a str,
    slot: HotkeySlot,
    active_recording: Option<HotkeySlot>,
) -> Element<'a, Message> {
    let is_recording = active_recording == Some(slot);
    let display_txt = if is_recording {
        "Press key or Mouse3/4/5..."
    } else if binding.trim().is_empty() {
        "Unbound (Click to set)"
    } else {
        binding
    };

    let record_btn = button(text(display_txt).size(13))
        .width(Length::Fill)
        .padding([10, 14])
        .style(if is_recording {
            pill_primary_button
        } else {
            pill_tonal_button
        })
        .on_press(Message::StartRecordingHotkey(slot));

    let clear_btn = if is_recording {
        button(text("Cancel").size(12))
            .padding([10, 12])
            .style(pill_tonal_button)
            .on_press(Message::CancelRecordingHotkey)
    } else {
        button(text("×").size(13))
            .padding([10, 12])
            .style(pill_tonal_button)
            .on_press(Message::ClearHotkey(slot))
    };

    column![
        text(label).size(12).color(M3_ON_SURFACE_VARIANT),
        row![record_btn, clear_btn]
            .spacing(6)
            .align_y(Vertical::Center),
    ]
    .width(Length::FillPortion(1))
    .spacing(6)
    .into()
}
