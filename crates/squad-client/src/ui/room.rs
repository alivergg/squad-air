use crate::net::PeerConnStatus;
use crate::ui::{
    chat::view_chat,
    components::{avatar_circle, mic_level_meter, telemetry_badge},
    stream_view::{view_stream_fullscreen, view_stream_stage},
    theme::{
        canvas_panel_style, floating_dock_style, m3_slider_style, m3_text_input_style,
        peer_card_style, pill_chip_button, pill_danger_button, pill_primary_button,
        pill_success_button, pill_tonal_button, surface_card_style, M3_ERROR, M3_ON_PRIMARY,
        M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS,
    },
    Message, SquadApp,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button, column, container, horizontal_space, row, scrollable, slider, text, text_input,
    },
    Background, Border, Element, Length, Theme,
};
use squad_dsp::MicActivationMode;

pub fn view_room(app: &SquadApp) -> Element<'_, Message> {
    let has_stage = app.watched_peer_id.is_some() || app.local_stream_preview;
    if app.stream_fullscreen && has_stage {
        return view_stream_fullscreen(app);
    }

    let total_members = app.peers.len() + 1;
    let copy_label = if app.invite_copied_flash {
        "Copied Invite!"
    } else {
        "Copy Invite"
    };
    let saved_channel = app
        .config
        .saved_channels
        .iter()
        .find(|c| c.invite_uri == app.active_invite_uri);
    let already_saved = saved_channel.is_some();
    let room_title = saved_channel
        .map(|c| c.name.as_str())
        .unwrap_or("Voice Room");

    let mut left_header = row![].spacing(10).align_y(Vertical::Center);
    if let Ok(invite) = squad_proto::InviteLink::parse(&app.active_invite_uri) {
        left_header = left_header.push(avatar_circle(
            app.avatars.room_handle_for(&invite.room_id),
            40.0,
            false,
        ));
    }
    left_header = left_header
        .push(text(room_title).size(18).color(M3_PRIMARY))
        .push(
            container(
                text(format!("{total_members} Online"))
                    .size(12)
                    .color(M3_SUCCESS),
            )
            .padding([4, 10])
            .style(canvas_panel_style),
        );

    let header = container(
        row![
            left_header,
            horizontal_space(),
            text_input("Room name...", &app.save_channel_name_input)
                .on_input(Message::SaveChannelNameChanged)
                .on_submit(Message::SaveActiveChannel)
                .width(Length::Fixed(165.0))
                .padding([8, 10])
                .size(12)
                .style(m3_text_input_style),
            button(
                text(if already_saved {
                    "Saved ✓"
                } else {
                    "Save Channel"
                })
                .size(12)
            )
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
    let mut self_row = row![
        avatar_circle(
            app.avatars.handle_for(&app.config.client_uuid()),
            42.0,
            local_speaking
        ),
        column![
            text(format!("{} (You)", app.config.nickname)).size(14),
            text(if app.audio.is_mic_muted() {
                "Muted"
            } else if local_speaking {
                "Speaking"
            } else {
                "Connected"
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
    ]
    .spacing(12)
    .align_y(Vertical::Center);

    if app.is_local_sharing {
        let preview_label = if app.local_stream_preview {
            "Hide Preview"
        } else {
            "Preview"
        };
        self_row = self_row.push(
            button(text(preview_label).size(11))
                .padding([6, 12])
                .style(pill_chip_button(app.local_stream_preview))
                .on_press(Message::ToggleLocalStreamPreview),
        );
    }
    self_row = self_row.push(mic_level_meter(
        app.audio.local_mic_level_db(),
        local_speaking,
    ));

    let self_card: Element<'_, Message> = container(self_row)
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

            let mut card_col = column![
                row![
                    avatar_circle(app.avatars.handle_for(&peer_uuid), 42.0, peer.is_speaking),
                    column![
                        text(&peer.nickname).size(14),
                        text(if peer.stream_info.is_some() {
                            "Sharing Screen"
                        } else if peer.is_speaking {
                            "Speaking"
                        } else {
                            "Connected"
                        })
                        .size(11)
                        .color(if peer.is_speaking {
                            M3_SUCCESS
                        } else {
                            M3_ON_SURFACE_VARIANT
                        }),
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
            .spacing(10);

            if let Some(ref stream) = peer.stream_info {
                let is_watching = app.watched_peer_id == Some(peer_id);
                let watch_btn = if is_watching {
                    button(text("Watching").size(11))
                        .padding([5, 10])
                        .style(pill_chip_button(true))
                        .on_press(Message::StopWatchingStream)
                } else if peer.status == PeerConnStatus::Relayed {
                    button(text("P2P/Mesh Only").size(11))
                        .padding([5, 10])
                        .style(pill_tonal_button)
                } else {
                    button(text("Watch").size(11))
                        .padding([5, 10])
                        .style(pill_primary_button)
                        .on_press(Message::WatchPeerStream(peer_id))
                };

                let stream_row = row![
                    container(text("LIVE").size(10).color(M3_ON_PRIMARY))
                        .padding([3, 7])
                        .style(|_t: &Theme| container::Style {
                            background: Some(Background::Color(M3_ERROR)),
                            border: Border {
                                radius: 6.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                    text(&stream.source_title)
                        .size(11)
                        .color(M3_ON_SURFACE_VARIANT),
                    horizontal_space(),
                    watch_btn,
                ]
                .spacing(8)
                .align_y(Vertical::Center);

                card_col = card_col.push(stream_row);
            }

            let card = container(card_col)
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
            text("Voice").size(14).color(M3_ON_SURFACE_VARIANT),
            scrollable(grid_rows).height(Length::Fill),
        ]
        .spacing(10),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(16)
    .style(canvas_panel_style);

    let left_stage: Element<'_, Message> = if has_stage {
        if app.stream_expanded {
            container(view_stream_stage(app))
                .width(Length::FillPortion(6))
                .height(Length::Fill)
                .into()
        } else {
            column![view_stream_stage(app), voice_stage]
                .spacing(12)
                .width(Length::FillPortion(6))
                .height(Length::Fill)
                .into()
        }
    } else {
        container(voice_stage)
            .width(Length::FillPortion(6))
            .height(Length::Fill)
            .into()
    };

    let chat_panel = container(view_chat(app))
        .width(Length::FillPortion(5))
        .height(Length::Fill);

    let main_split = row![left_stage, chat_panel]
        .spacing(14)
        .height(Length::Fill);

    let is_muted = app.audio.is_mic_muted();
    let is_deafened = app.audio.is_deafened();
    let act_mode = app.config.activation_mode;

    let share_btn = if app.is_local_sharing {
        button(
            text(format!(
                "Stop Sharing • {} watching",
                app.local_stream_subscribers
            ))
            .size(12),
        )
        .padding([9, 14])
        .style(pill_danger_button)
        .on_press(Message::StopScreenShare)
    } else {
        button(text("Share Screen").size(12))
            .padding([9, 14])
            .style(pill_tonal_button)
            .on_press(Message::OpenShareModal)
    };

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
            share_btn,
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
