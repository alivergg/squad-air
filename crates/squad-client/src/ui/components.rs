use crate::net::{PeerConnStatus, PeerUiSnapshot};
use crate::ui::theme::{
    M3_ERROR, M3_ON_PRIMARY_CONTAINER, M3_ON_SURFACE_VARIANT, M3_OUTLINE_VARIANT,
    M3_PRIMARY_CONTAINER, M3_SUCCESS, M3_SURFACE_HIGHEST, M3_WARNING,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{container, row, text},
    Background, Border, Element, Length, Theme,
};

pub fn avatar_circle<'a, Message: 'a>(nickname: &str, is_speaking: bool) -> Element<'a, Message> {
    let initials: String = nickname
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    container(text(initials).size(15).color(M3_ON_PRIMARY_CONTAINER))
        .width(Length::Fixed(42.0))
        .height(Length::Fixed(42.0))
        .align_x(Horizontal::Center)
        .align_y(Vertical::Center)
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(M3_PRIMARY_CONTAINER)),
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
    let (label, color) = match peer.status {
        PeerConnStatus::Punching => ("PUNCHING NAT...".to_string(), M3_WARNING),
        PeerConnStatus::NatFailed => ("NAT BLOCKED".to_string(), M3_ERROR),
        PeerConnStatus::Connected => {
            let txt = format!("{} ms • {:.1}% loss", peer.rtt_ms, peer.loss_pct);
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

    let bar = container(
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
    });

    row![bar, text(format!("{:.0} dB", db.max(-60.0))).size(12).color(M3_ON_SURFACE_VARIANT)]
        .spacing(8)
        .align_y(Vertical::Center)
        .into()
}
