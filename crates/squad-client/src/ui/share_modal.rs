use crate::ui::theme::{
    pill_chip_button, pill_primary_button, pill_tonal_button, M3_ON_PRIMARY_CONTAINER,
    M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_OUTLINE, M3_OUTLINE_VARIANT, M3_PRIMARY,
    M3_PRIMARY_CONTAINER, M3_SURFACE_CONTAINER, M3_SURFACE_HIGH, M3_SURFACE_HIGHEST,
};
use crate::ui::{Message, SquadApp};
use iced::{
    alignment::Vertical,
    widget::{button, column, container, horizontal_space, row, scrollable, text},
    Background, Border, Color, Element, Length, Shadow, Theme, Vector,
};
use squad_video::{CaptureTarget, StreamFps, StreamResolution};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareSourceTab {
    Monitors,
    Windows,
}

fn backdrop_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.68))),
        ..Default::default()
    }
}

fn modal_card_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE,
            width: 1.0,
            radius: 24.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
            offset: Vector::new(0.0, 12.0),
            blur_radius: 32.0,
        },
    }
}

fn encoder_badge_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_PRIMARY_CONTAINER)),
        text_color: Some(M3_ON_PRIMARY_CONTAINER),
        border: Border {
            color: M3_PRIMARY,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn target_row_button_style(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| {
        let (bg, border_color) = if selected {
            (M3_PRIMARY_CONTAINER, M3_PRIMARY)
        } else {
            let hover_bg = match status {
                button::Status::Hovered | button::Status::Pressed => M3_SURFACE_HIGHEST,
                _ => M3_SURFACE_HIGH,
            };
            (hover_bg, M3_OUTLINE_VARIANT)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: M3_ON_SURFACE,
            border: Border {
                color: border_color,
                width: if selected { 1.5 } else { 1.0 },
                radius: 10.0.into(),
            },
            ..Default::default()
        }
    }
}

pub fn view_share_modal(app: &SquadApp) -> Element<'_, Message> {
    let header = row![
        column![
            text("Share Your Screen").size(18).color(M3_ON_SURFACE),
            text("End-to-End Encrypted Peer-to-Peer UDP Stream")
                .size(11)
                .color(M3_ON_SURFACE_VARIANT),
        ]
        .spacing(2),
        horizontal_space(),
        container(
            text(format!("GPU Encoder: {}", app.local_encoder_backend))
                .size(11)
                .color(M3_ON_PRIMARY_CONTAINER),
        )
        .padding([6, 10])
        .style(encoder_badge_style),
    ]
    .spacing(12)
    .align_y(Vertical::Center);

    let tabs = row![
        button(text("Screens / Monitors").size(12))
            .padding([8, 12])
            .style(pill_chip_button(app.share_tab == ShareSourceTab::Monitors))
            .on_press(Message::SelectShareTab(ShareSourceTab::Monitors)),
        button(text("Applications & Windows").size(12))
            .padding([8, 12])
            .style(pill_chip_button(app.share_tab == ShareSourceTab::Windows))
            .on_press(Message::SelectShareTab(ShareSourceTab::Windows)),
        horizontal_space(),
        button(text("Refresh").size(12))
            .padding([8, 12])
            .style(pill_tonal_button)
            .on_press(Message::RefreshCaptureTargets),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    let mut list_col = column![].spacing(6);
    let mut visible_count = 0usize;

    for (idx, target) in app.capture_targets.iter().enumerate() {
        let matches_tab = match app.share_tab {
            ShareSourceTab::Monitors => target.is_monitor(),
            ShareSourceTab::Windows => !target.is_monitor(),
        };
        if !matches_tab {
            continue;
        }
        visible_count += 1;
        let is_selected = app.selected_capture_target_idx == idx;

        let (label, sublabel) = match target {
            CaptureTarget::Monitor {
                name,
                width,
                height,
                is_primary,
                ..
            } => (
                name.clone(),
                if *is_primary {
                    format!("{width}x{height} • Primary Display")
                } else {
                    format!("{width}x{height} • Display")
                },
            ),
            CaptureTarget::Window {
                title,
                app_name,
                width,
                height,
                ..
            } => (
                title.clone(),
                format!("{app_name} • {width}x{height}"),
            ),
        };

        let item_btn = button(
            row![
                column![
                    text(label).size(13).color(M3_ON_SURFACE),
                    text(sublabel).size(11).color(M3_ON_SURFACE_VARIANT),
                ]
                .spacing(2),
                horizontal_space(),
                text(if is_selected { "Selected" } else { "Select" })
                    .size(11)
                    .color(if is_selected {
                        M3_ON_PRIMARY_CONTAINER
                    } else {
                        M3_ON_SURFACE_VARIANT
                    }),
            ]
            .align_y(Vertical::Center),
        )
        .width(Length::Fill)
        .padding([10, 12])
        .style(target_row_button_style(is_selected))
        .on_press(Message::SelectCaptureTarget(idx));

        list_col = list_col.push(item_btn);
    }

    if visible_count == 0 {
        list_col = list_col.push(
            container(
                text("No capture targets in this tab. Grant Screen Recording permission if needed and press Refresh.")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
            )
            .padding(14),
        );
    }

    let source_list = container(scrollable(list_col).height(Length::Fixed(200.0)))
        .width(Length::Fill)
        .padding(8)
        .style(|_t: &Theme| container::Style {
            background: Some(Background::Color(iced::Color::from_rgb(
                0x17 as f32 / 255.0,
                0x18 as f32 / 255.0,
                0x1C as f32 / 255.0,
            ))),
            border: Border {
                color: M3_OUTLINE_VARIANT,
                width: 1.0,
                radius: 12.0.into(),
            },
            ..Default::default()
        });

    let resolution_row = row![
        text("Resolution").size(12).color(M3_ON_SURFACE_VARIANT).width(Length::Fixed(80.0)),
        button(text("720p").size(12))
            .padding([7, 12])
            .style(pill_chip_button(app.share_resolution == StreamResolution::R720p))
            .on_press(Message::SetShareResolution(StreamResolution::R720p)),
        button(text("1080p").size(12))
            .padding([7, 12])
            .style(pill_chip_button(app.share_resolution == StreamResolution::R1080p))
            .on_press(Message::SetShareResolution(StreamResolution::R1080p)),
        button(text("Native").size(12))
            .padding([7, 12])
            .style(pill_chip_button(app.share_resolution == StreamResolution::Native))
            .on_press(Message::SetShareResolution(StreamResolution::Native)),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    let fps_row = row![
        text("Frame Rate").size(12).color(M3_ON_SURFACE_VARIANT).width(Length::Fixed(80.0)),
        button(text("30 FPS").size(12))
            .padding([7, 12])
            .style(pill_chip_button(app.share_fps == StreamFps::Fps30))
            .on_press(Message::SetShareFps(StreamFps::Fps30)),
        button(text("60 FPS").size(12))
            .padding([7, 12])
            .style(pill_chip_button(app.share_fps == StreamFps::Fps60))
            .on_press(Message::SetShareFps(StreamFps::Fps60)),
        horizontal_space(),
        text("Bitrate").size(12).color(M3_ON_SURFACE_VARIANT),
        button(text("6 Mbps").size(12))
            .padding([7, 10])
            .style(pill_chip_button(app.share_bitrate_bps == 6_000_000))
            .on_press(Message::SetShareBitrate(6_000_000)),
        button(text("8 Mbps").size(12))
            .padding([7, 10])
            .style(pill_chip_button(app.share_bitrate_bps == 8_000_000))
            .on_press(Message::SetShareBitrate(8_000_000)),
        button(text("12 Mbps").size(12))
            .padding([7, 10])
            .style(pill_chip_button(app.share_bitrate_bps == 12_000_000))
            .on_press(Message::SetShareBitrate(12_000_000)),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    let mut go_live_btn = button(text("Go Live").size(13))
        .padding([10, 18])
        .style(pill_primary_button);
    if !app.capture_targets.is_empty() {
        go_live_btn = go_live_btn.on_press(Message::StartScreenShare);
    }

    let footer = row![
        text("Zero-Waste: encodes only while a peer watches")
            .size(11)
            .color(M3_ON_SURFACE_VARIANT),
        horizontal_space(),
        button(text("Cancel").size(13))
            .padding([10, 16])
            .style(pill_tonal_button)
            .on_press(Message::CloseShareModal),
        go_live_btn,
    ]
    .spacing(10)
    .align_y(Vertical::Center);

    let card = container(
        column![header, tabs, source_list, resolution_row, fps_row, footer].spacing(14),
    )
    .width(Length::Fixed(600.0))
    .padding(22)
    .style(modal_card_style);

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(backdrop_style)
        .into()
}
