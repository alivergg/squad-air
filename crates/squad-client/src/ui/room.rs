use crate::ui::{
    chat::view_chat,
    components::{avatar_circle, mic_level_meter, telemetry_badge},
    theme::{
        canvas_panel_style, floating_dock_style, m3_slider_style, m3_text_input_style,
        peer_card_style, pill_chip_button, pill_danger_button, pill_primary_button,
        pill_success_button, pill_tonal_button, surface_card_style, M3_ERROR,
        M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS,
    },
    Message, SquadApp,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button, column, container, horizontal_space, row, scrollable, slider, text, text_input,
    },
    Element, Length,
};
use squad_dsp::MicActivationMode;

pub fn view_room(app: &SquadApp) -> Element<'_, Message> {
    let total_members = app.peers.len() + 1;
    let copy_label = if app.invite_copied_flash {
        "Copied Invite!"
    } else {
        "Copy Invite"
    };
    let already_saved = app
        .config
        .saved_channels
        .iter()
        .any(|c| c.invite_uri == app.active_invite_uri);

    let header = container(
        row![
            column![
                row![
                    text("# squad-voice").size(18).color(M3_PRIMARY),
                    container(
                        text(format!("{total_members}/10 Online • E2EE"))
                            .size(12)
                            .color(M3_SUCCESS)
                    )
                    .padding([4, 10])
                    .style(canvas_panel_style),
                ]
                .spacing(10)
                .align_y(Vertical::Center),
                text(format!(
                    "Slot #{} • Server {}",
                    app.local_peer_id, app.config.rendezvous_server
                ))
                .size(11)
                .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(3),
            horizontal_space(),
            text_input("Channel name...", &app.save_channel_name_input)
                .on_input(Message::SaveChannelNameChanged)
                .on_submit(Message::SaveActiveChannel)
                .width(Length::Fixed(165.0))
                .padding([8, 10])
                .size(12)
                .style(m3_text_input_style),
            button(text(if already_saved { "Saved ✓" } else { "Save Channel" }).size(12))
                .padding([9, 14])
                .style(if already_saved {
                    pill_success_button
                } else {
                    pill_tonal_button
                })
                .on_press(Message::SaveActiveChannel),
            button(text(copy_label).size(12))
                .padding([9, 14])
                .style(pill_primary_button)
                .on_press(Message::CopyInviteUri),
            button(text("Settings").size(12))
                .padding([9, 14])
                .style(pill_tonal_button)
                .on_press(Message::OpenSettings),
            button(text("Leave").size(12))
                .padding([9, 14])
                .style(pill_danger_button)
                .on_press(Message::LeaveRoom),
        ]
        .spacing(8)
        .align_y(Vertical::Center),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(surface_card_style);

    let local_speaking = app.audio.is_local_speaking();
    let self_card: Element<'_, Message> = container(
        row![
            avatar_circle(&app.config.nickname, local_speaking),
            column![
                text(format!("{} (You)", app.config.nickname)).size(14),
                text(if app.audio.is_mic_muted() {
                    "Microphone Muted"
                } else if local_speaking {
                    "Transmitting 10ms Opus"
                } else {
                    "DTX Standby (0 kbps)"
                })
                .size(12)
                .color(if local_speaking {
                    M3_SUCCESS
                } else {
                    M3_ON_SURFACE_VARIANT
                }),
            ]
            .spacing(3),
            horizontal_space(),
            mic_level_meter(app.audio.local_mic_level_db(), local_speaking),
        ]
        .spacing(12)
        .align_y(Vertical::Center),
    )
    .width(Length::Fill)
    .padding(14)
    .style(peer_card_style(local_speaking))
    .into();

    let mut grid_rows = column![self_card].spacing(10);
    for chunk in app.peers.chunks(2) {
        let mut r = row![].spacing(10);
        for peer in chunk {
            let vol_gain = app.config.peer_volume(&peer.client_uuid);
            let vol_pct = (vol_gain * 100.0).round() as i32;
            let peer_id = peer.peer_id;
            let peer_uuid = peer.client_uuid;

            let card = container(
                column![
                    row![
                        avatar_circle(&peer.nickname, peer.is_speaking),
                        column![
                            text(&peer.nickname).size(14),
                            text(format!("Peer #{}", peer.peer_id))
                                .size(11)
                                .color(M3_ON_SURFACE_VARIANT),
                        ]
                        .spacing(2),
                        horizontal_space(),
                        telemetry_badge(peer),
                    ]
                    .spacing(10)
                    .align_y(Vertical::Center),
                    row![
                        text("Vol").size(12).color(M3_ON_SURFACE_VARIANT),
                        slider(0.0..=200.0, vol_gain * 100.0, move |v| {
                            Message::PeerVolumeChanged {
                                peer_id,
                                client_uuid: peer_uuid,
                                gain: v / 100.0,
                            }
                        })
                        .step(1.0_f32)
                        .style(m3_slider_style),
                        text(format!("{vol_pct}%"))
                            .size(12)
                            .width(Length::Fixed(40.0))
                            .align_x(Horizontal::Right),
                    ]
                    .spacing(8)
                    .align_y(Vertical::Center),
                ]
                .spacing(10),
            )
            .width(Length::FillPortion(1))
            .padding(14)
            .style(peer_card_style(peer.is_speaking));

            r = r.push(card);
        }
        if chunk.len() == 1 {
            r = r.push(horizontal_space().width(Length::FillPortion(1)));
        }
        grid_rows = grid_rows.push(r);
    }

    let voice_stage = container(
        column![
            text("VOICE STAGE").size(13).color(M3_ON_SURFACE_VARIANT),
            scrollable(grid_rows).height(Length::Fill),
        ]
        .spacing(10),
    )
    .width(Length::FillPortion(6))
    .height(Length::Fill)
    .padding(16)
    .style(canvas_panel_style);

    let chat_panel = container(view_chat(app))
        .width(Length::FillPortion(5))
        .height(Length::Fill);

    let main_split = row![voice_stage, chat_panel]
        .spacing(14)
        .height(Length::Fill);

    let is_muted = app.audio.is_mic_muted();
    let is_deafened = app.audio.is_deafened();
    let act_mode = app.config.activation_mode;

    let bottom_dock = container(
        row![
            button(text(if is_muted { "Unmute Mic" } else { "Mute Mic" }).size(12))
                .padding([9, 16])
                .style(if is_muted {
                    pill_danger_button
                } else {
                    pill_tonal_button
                })
                .on_press(Message::ToggleMicMute),
            button(text(if is_deafened { "Undeafen" } else { "Deafen" }).size(12))
                .padding([9, 16])
                .style(if is_deafened {
                    pill_danger_button
                } else {
                    pill_tonal_button
                })
                .on_press(Message::ToggleDeafen),
            text("Input Mode:").size(12).color(M3_ON_SURFACE_VARIANT),
            button(text("Push-to-Mute").size(12))
                .padding([8, 12])
                .style(pill_chip_button(act_mode == MicActivationMode::PushToMute))
                .on_press(Message::SetActivationMode(MicActivationMode::PushToMute)),
            button(text("Push-to-Talk").size(12))
                .padding([8, 12])
                .style(pill_chip_button(act_mode == MicActivationMode::PushToTalk))
                .on_press(Message::SetActivationMode(MicActivationMode::PushToTalk)),
            button(text("Open Mic").size(12))
                .padding([8, 12])
                .style(pill_chip_button(act_mode == MicActivationMode::ToggleMute))
                .on_press(Message::SetActivationMode(MicActivationMode::ToggleMute)),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .padding([10, 16])
    .style(floating_dock_style);

    let mut root_col = column![header, main_split].spacing(14);
    if let Some(ref banner) = app.status_banner {
        root_col = root_col.push(text(banner).size(12).color(M3_ERROR));
    }
    root_col = root_col.push(
        container(bottom_dock)
            .width(Length::Fill)
            .align_x(Horizontal::Center),
    );

    root_col
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
