use crate::ui::{
    components::avatar_circle,
    theme::{M3_ERROR, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_SUCCESS},
    Message, SquadApp,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{column, container, row, text},
    Background, Border, Color, Element, Length, Theme,
};

pub fn view_overlay(app: &SquadApp) -> Element<'_, Message> {
    let opacity = app.config.overlay_opacity.clamp(0.30, 1.0);
    let speaking_only = app.config.overlay_speaking_only;

    let mut list_col = column![].spacing(6).width(Length::Fixed(226.0));
    let mut visible_count = 0usize;

    let local_speaking = app.audio.is_local_speaking();
    let local_muted = app.audio.is_mic_muted() || app.audio.is_deafened();
    if !speaking_only || local_speaking {
        visible_count += 1;
        list_col = list_col.push(overlay_member_pill(
            app.avatars.handle_for(&app.config.client_uuid()),
            &app.config.nickname,
            local_speaking,
            local_muted,
            opacity,
        ));
    }

    for peer in &app.peers {
        let stats = app.audio.peer_stats(peer.peer_id);
        if speaking_only && !stats.is_speaking {
            continue;
        }
        visible_count += 1;
        list_col = list_col.push(overlay_member_pill(
            app.avatars.handle_for(&peer.client_uuid),
            &peer.nickname,
            stats.is_speaking,
            false,
            opacity,
        ));
    }

    if visible_count == 0 {
        return container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(transparent_root_style)
            .into();
    }

    container(list_col)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([8, 6])
        .align_x(Horizontal::Left)
        .align_y(Vertical::Center)
        .style(transparent_root_style)
        .into()
}

fn overlay_member_pill<'a>(
    avatar_handle: iced::widget::image::Handle,
    nickname: &'a str,
    is_speaking: bool,
    is_muted: bool,
    opacity: f32,
) -> Element<'a, Message> {
    let mut row_items = row![
        avatar_circle(avatar_handle, 34.0, is_speaking),
        text(nickname)
            .size(13)
            .color(if is_speaking {
                Color::from_rgba(1.0, 1.0, 1.0, opacity.max(0.75))
            } else {
                Color::from_rgba(
                    M3_ON_SURFACE.r,
                    M3_ON_SURFACE.g,
                    M3_ON_SURFACE.b,
                    (opacity * 0.95).max(0.65),
                )
            })
            .width(Length::Fill),
    ]
    .spacing(9)
    .align_y(Vertical::Center);

    if is_muted {
        row_items = row_items.push(
            container(
                text("MUTED")
                    .size(9)
                    .color(Color::from_rgba(1.0, 1.0, 1.0, opacity.max(0.8))),
            )
            .padding([2, 6])
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(Color::from_rgba(
                    M3_ERROR.r,
                    M3_ERROR.g,
                    M3_ERROR.b,
                    (opacity * 0.85).max(0.55),
                ))),
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
    } else if is_speaking {
        row_items = row_items.push(
            container(text("").size(1))
                .width(Length::Fixed(7.0))
                .height(Length::Fixed(7.0))
                .style(move |_theme: &Theme| container::Style {
                    background: Some(Background::Color(Color::from_rgba(
                        M3_SUCCESS.r,
                        M3_SUCCESS.g,
                        M3_SUCCESS.b,
                        opacity.max(0.8),
                    ))),
                    border: Border {
                        radius: 999.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    } else {
        row_items = row_items.push(
            container(text("").size(1))
                .width(Length::Fixed(5.0))
                .height(Length::Fixed(5.0))
                .style(move |_theme: &Theme| container::Style {
                    background: Some(Background::Color(Color::from_rgba(
                        M3_ON_SURFACE_VARIANT.r,
                        M3_ON_SURFACE_VARIANT.g,
                        M3_ON_SURFACE_VARIANT.b,
                        opacity * 0.45,
                    ))),
                    border: Border {
                        radius: 999.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    }

    container(row_items)
        .width(Length::Fixed(226.0))
        .padding([6, 10])
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(if is_speaking {
                Color::from_rgba(0.09, 0.14, 0.11, (opacity * 0.88).clamp(0.25, 0.96))
            } else {
                Color::from_rgba(0.08, 0.09, 0.11, (opacity * 0.78).clamp(0.20, 0.90))
            })),
            border: Border {
                color: if is_speaking {
                    Color::from_rgba(
                        M3_SUCCESS.r,
                        M3_SUCCESS.g,
                        M3_SUCCESS.b,
                        opacity.max(0.70),
                    )
                } else {
                    Color::from_rgba(1.0, 1.0, 1.0, opacity * 0.14)
                },
                width: if is_speaking { 1.5 } else { 1.0 },
                radius: 12.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn transparent_root_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        ..Default::default()
    }
}
