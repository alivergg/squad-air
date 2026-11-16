use crate::hotkeys::{is_macos_accessibility_trusted, is_macos_input_monitoring_trusted};
use crate::ui::{
    components::{avatar_circle, mic_sensitivity_meter},
    theme::{
        m3_menu_style, m3_pick_list_style, m3_slider_style, m3_text_input_style,
        pill_chip_button, pill_primary_button, pill_tonal_button, surface_card_style,
        M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS, M3_WARNING,
    },
    Message, SquadApp, SYSTEM_DEFAULT_DEVICE,
};
use iced::{
    alignment::Vertical,
    widget::{
        button, column, container, horizontal_space, pick_list, row, scrollable, slider, text,
        text_input,
    },
    Element, Length,
};
use squad_dsp::NoiseSuppressionMode;

pub fn view_settings(app: &SquadApp) -> Element<'_, Message> {
    let top_bar = row![
        column![
            text("Settings & Audio Pipeline").size(22).color(M3_PRIMARY),
            text("Changes apply live without disconnecting your voice mesh")
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

    let avatar_preview = avatar_circle(app.avatars.handle_for(&app.config.client_uuid()), 72.0, false);

    let profile_section = container(
        column![
            text("Profile & Rendezvous Coordinator").size(16),
            row![
                avatar_preview,
                column![
                    text(if app.config.custom_avatar_hash.is_some() {
                        "Custom 128x128 Avatar (P2P Cached)"
                    } else {
                        "5x5 BLAKE3 Identicon (Deterministic)"
                    })
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
                    row![
                        button(text("Choose Image...").size(12))
                            .padding([7, 14])
                            .style(pill_primary_button)
                            .on_press(Message::PickAvatarImage),
                        button(text("Reset to 5x5 Identicon").size(12))
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
                    text("Nickname").size(12).color(M3_ON_SURFACE_VARIANT),
                    text_input("Your nickname", &app.config.nickname)
                        .on_input(Message::NicknameChanged)
                        .padding(10)
                        .size(14)
                        .style(m3_text_input_style),
                ]
                .width(Length::FillPortion(1))
                .spacing(6),
                column![
                    text("Rendezvous Server (UDP host:port)")
                        .size(12)
                        .color(M3_ON_SURFACE_VARIANT),
                    text_input("127.0.0.1:3478", &app.config.rendezvous_server)
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
        "DeepFilterNet3 (ONNX Ready)"
    } else {
        "DeepFilterNet3 (Auto RNNoise Fallback)"
    };

    let dsp_section = container(
        column![
            text("Voice Processing (48 kHz • 10 ms Frame Pipeline)").size(16),
            row![
                text("1. Acoustic Echo Cancellation (WebRTC AEC3):")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                button(text("Off").size(13))
                    .padding([8, 18])
                    .style(pill_chip_button(!aec_on))
                    .on_press(Message::SetAecEnabled(false)),
                button(text("On (Speakers Mode)").size(13))
                    .padding([8, 18])
                    .style(pill_chip_button(aec_on))
                    .on_press(Message::SetAecEnabled(true)),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("2. Neural Noise Suppression:")
                    .size(14)
                    .width(Length::Fixed(330.0)),
                button(text("Off").size(13))
                    .padding([8, 16])
                    .style(pill_chip_button(ns_mode == NoiseSuppressionMode::Off))
                    .on_press(Message::SetNsMode(NoiseSuppressionMode::Off)),
                button(text("RNNoise (nnnoiseless)").size(13))
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
                text("3. Microphone Input Pre-Gain:")
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
                text("4. Microphone Activation Sensitivity (Voice Gate):")
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
                text("Live Input Level (-60 dB .. 0 dB):")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT)
                    .width(Length::Fixed(330.0)),
                mic_sensitivity_meter(
                    app.audio.local_mic_level_db(),
                    app.config.vad_threshold_db
                ),
            ]
            .spacing(12)
            .align_y(Vertical::Center),
        ]
        .spacing(14),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    let mut hotkey_col = column![
        text("Non-Blocking Global Hotkeys (Passive Polling)").size(16),
        text("Keys are never swallowed: typing 'V' in apps or using Push-to-Talk in CS2 continues normally. Supports KeyV, CapsLock, F9, Mouse4, Mouse5.")
            .size(12)
            .color(M3_ON_SURFACE_VARIANT),
        row![
            column![
                text("Push-to-Mute")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
                text_input("KeyV", &app.config.hotkey_ptm)
                    .on_input(Message::HotkeyPtmChanged)
                    .padding(10)
                    .size(14)
                    .style(m3_text_input_style),
            ]
            .width(Length::FillPortion(1))
            .spacing(6),
            column![
                text("Push-to-Talk")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
                text_input("CapsLock", &app.config.hotkey_ptt)
                    .on_input(Message::HotkeyPttChanged)
                    .padding(10)
                    .size(14)
                    .style(m3_text_input_style),
            ]
            .width(Length::FillPortion(1))
            .spacing(6),
            column![
                text("Toggle Mic Mute")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
                text_input("F9", &app.config.hotkey_toggle_mute)
                    .on_input(Message::HotkeyToggleChanged)
                    .padding(10)
                    .size(14)
                    .style(m3_text_input_style),
            ]
            .width(Length::FillPortion(1))
            .spacing(6),
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
        let perm_color = if ax_ok && im_ok { M3_SUCCESS } else { M3_WARNING };

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

    column![
        top_bar,
        scrollable(
            column![
                profile_section,
                devices_section,
                dsp_section,
                hotkeys_section,
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
