use crate::net::{PeerConnStatus, PeerRouteKind, PeerUiSnapshot};
use crate::ui::theme::{
    tooltip_box_style, M3_ERROR, M3_ON_SURFACE_VARIANT, M3_OUTLINE_VARIANT, M3_PRIMARY, M3_SUCCESS,
    M3_SURFACE_HIGHEST, M3_WARNING,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        container, horizontal_space, image as iced_image, row, stack, text, tooltip,
        tooltip::Position,
    },
    Background, Border, ContentFit, Element, Length, Theme,
};

pub fn with_tooltip<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    label: impl iced::widget::text::IntoFragment<'a>,
    position: Position,
) -> Element<'a, Message> {
    tooltip(content, text(label).size(12), position)
        .gap(6.0)
        .padding(8.0)
        .snap_within_viewport(true)
        .style(tooltip_box_style)
        .into()
}

pub fn avatar_circle<'a, Message: 'a>(
    handle: iced_image::Handle,
    size: f32,
    is_speaking: bool,
) -> Element<'a, Message> {
    let pad = if is_speaking { 2.0 } else { 1.0 };
    let img_size = (size - pad * 2.0).max(8.0);
    container(
        iced_image(handle)
            .width(Length::Fixed(img_size))
            .height(Length::Fixed(img_size))
            .content_fit(ContentFit::Cover),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .style(move |_theme: &Theme| container::Style {
        border: Border {
            color: if is_speaking {
                M3_SUCCESS
            } else {
                M3_OUTLINE_VARIANT
            },
            width: if is_speaking { 2.5 } else { 1.0 },
            radius: 999.0.into(),
        },
        ..Default::default()
    })
    .into()
}

pub fn telemetry_badge<'a, Message: 'a>(peer: &PeerUiSnapshot) -> Element<'a, Message> {
    let (label, color) = match (peer.status, peer.route) {
        (PeerConnStatus::Punching, _) | (_, PeerRouteKind::Connecting) => {
            ("Connecting...".to_string(), M3_WARNING)
        }
        (PeerConnStatus::NatFailed, _) | (_, PeerRouteKind::NatFailed) => {
            ("NAT Failed".to_string(), M3_ERROR)
        }
        (PeerConnStatus::MeshRelayed, _) | (_, PeerRouteKind::MeshRelayed) => {
            let txt = if peer.loss_pct >= 0.5 {
                format!(
                    "Mesh • {} ms • +{}ms buf • {:.1}%",
                    peer.rtt_ms, peer.jitter_ms, peer.loss_pct
                )
            } else {
                format!("Mesh • {} ms • +{}ms buf", peer.rtt_ms, peer.jitter_ms)
            };
            let col = if peer.rtt_ms < 80 && peer.loss_pct < 1.0 {
                M3_SUCCESS
            } else if peer.rtt_ms < 150 && peer.loss_pct < 5.0 {
                M3_WARNING
            } else {
                M3_ERROR
            };
            (txt, col)
        }
        (PeerConnStatus::Relayed, _) | (_, PeerRouteKind::Relayed) => {
            let txt = if peer.loss_pct >= 0.5 {
                format!(
                    "Relay • {} ms • +{}ms buf • {:.1}%",
                    peer.rtt_ms, peer.jitter_ms, peer.loss_pct
                )
            } else {
                format!("Relay • {} ms • +{}ms buf", peer.rtt_ms, peer.jitter_ms)
            };
            let col = if peer.loss_pct >= 5.0 {
                M3_ERROR
            } else {
                M3_WARNING
            };
            (txt, col)
        }
        (PeerConnStatus::Connected, route) => {
            let prefix = match route {
                PeerRouteKind::LanDirect => "LAN",
                _ => "P2P",
            };
            let txt = if peer.loss_pct >= 0.5 {
                format!(
                    "{prefix} • {} ms • +{}ms buf • {:.1}%",
                    peer.rtt_ms, peer.jitter_ms, peer.loss_pct
                )
            } else {
                format!("{prefix} • {} ms • +{}ms buf", peer.rtt_ms, peer.jitter_ms)
            };
            let col = if peer.rtt_ms < 60 && peer.loss_pct < 1.0 {
                M3_SUCCESS
            } else if peer.rtt_ms < 120 && peer.loss_pct < 5.0 {
                M3_WARNING
            } else {
                M3_ERROR
            };
            (txt, col)
        }
    };

    container(text(label).size(12).color(color))
        .padding([4, 10])
        .style(|_theme: &Theme| container::Style {
            background: Some(Background::Color(M3_SURFACE_HIGHEST)),
            border: Border {
                radius: 999.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

pub fn mic_level_meter<'a, Message: 'a>(db: f32, is_speaking: bool) -> Element<'a, Message> {
    let normalized = ((db + 60.0) / 60.0).clamp(0.04, 1.0);
    let fill_width = (normalized * 110.0) as u16;

    container(
        container(text(""))
            .width(Length::Fixed(fill_width as f32))
            .height(Length::Fixed(8.0))
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(if is_speaking {
                    M3_SUCCESS
                } else {
                    M3_ON_SURFACE_VARIANT
                })),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
    )
    .width(Length::Fixed(110.0))
    .height(Length::Fixed(8.0))
    .style(|_theme: &Theme| container::Style {
        background: Some(Background::Color(M3_SURFACE_HIGHEST)),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

pub fn mic_sensitivity_meter<'a, Message: 'a>(db: f32, threshold_db: f32) -> Element<'a, Message> {
    let gate_active = db >= threshold_db;
    let level_norm = ((db + 60.0) / 60.0).clamp(0.01, 0.99);
    let fill_portion = (level_norm * 1000.0).round() as u16;
    let empty_portion = 1000u16.saturating_sub(fill_portion).max(1);

    let thresh_norm = ((threshold_db + 60.0) / 60.0).clamp(0.01, 0.99);
    let left_portion = (thresh_norm * 1000.0).round() as u16;
    let right_portion = 1000u16.saturating_sub(left_portion).max(1);

    let track = container(
        row![
            container(text(""))
                .width(Length::FillPortion(fill_portion))
                .height(Length::Fixed(12.0))
                .style(move |_theme: &Theme| container::Style {
                    background: Some(Background::Color(if gate_active {
                        M3_SUCCESS
                    } else {
                        M3_ON_SURFACE_VARIANT
                    })),
                    border: Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            horizontal_space().width(Length::FillPortion(empty_portion)),
        ]
        .width(Length::Fill)
        .height(Length::Fixed(12.0)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(12.0))
    .style(|_theme: &Theme| container::Style {
        background: Some(Background::Color(M3_SURFACE_HIGHEST)),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let marker_overlay = row![
        horizontal_space().width(Length::FillPortion(left_portion)),
        container(text(""))
            .width(Length::Fixed(2.5))
            .height(Length::Fixed(14.0))
            .style(|_theme: &Theme| container::Style {
                background: Some(Background::Color(M3_PRIMARY)),
                ..Default::default()
            }),
        horizontal_space().width(Length::FillPortion(right_portion)),
    ]
    .width(Length::Fill)
    .align_y(Vertical::Center);

    let bar_with_marker = container(stack![
        container(track)
            .width(Length::Fill)
            .height(Length::Fixed(14.0))
            .align_y(Vertical::Center),
        marker_overlay,
    ])
    .width(Length::Fill)
    .height(Length::Fixed(14.0));

    let gate_label = if gate_active {
        "Gate Open"
    } else {
        "Gate Closed"
    };
    let gate_color = if gate_active {
        M3_SUCCESS
    } else {
        M3_ON_SURFACE_VARIANT
    };

    row![
        bar_with_marker,
        text(format!("{:.0} dB", db.clamp(-60.0, 0.0)))
            .size(12)
            .width(Length::Fixed(48.0))
            .align_x(Horizontal::Right)
            .color(M3_ON_SURFACE_VARIANT),
        text(gate_label)
            .size(12)
            .width(Length::Fixed(78.0))
            .align_x(Horizontal::Right)
            .color(gate_color),
    ]
    .spacing(10)
    .width(Length::Fill)
    .align_y(Vertical::Center)
    .into()
}
