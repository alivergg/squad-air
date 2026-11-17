use crate::ui::{
    theme::{M3_ERROR, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_SUCCESS},
    Message, SquadApp,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{column, container, image as iced_image, row, stack, text},
    Background, Border, Color, ContentFit, Element, Length, Theme,
};

const OVERLAY_AVATAR_SIZE: f32 = 34.0;

pub fn view_overlay(app: &SquadApp) -> Element<'_, Message> {
    let opacity = app.config.overlay_opacity.clamp(0.30, 1.0);
    let speaking_only = app.config.overlay_speaking_only;

    let mut list_col = column![].spacing(6).width(Length::Shrink);
    let mut visible_count = 0usize;

    let local_speaking = app.audio.is_local_speaking();
    let local_muted = app.audio.is_mic_muted() || app.audio.is_deafened();
    if !speaking_only || local_speaking {
        visible_count += 1;
        list_col = list_col.push(overlay_member_row(
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
        list_col = list_col.push(overlay_member_row(
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
        .padding([8, 8])
        .align_x(Horizontal::Left)
        .align_y(Vertical::Center)
        .style(transparent_root_style)
        .into()
}

fn overlay_avatar<'a>(
    avatar_handle: iced_image::Handle,
    is_speaking: bool,
    is_muted: bool,
    opacity: f32,
) -> Element<'a, Message> {
    let base = iced_image(avatar_handle)
        .width(Length::Fixed(OVERLAY_AVATAR_SIZE))
        .height(Length::Fixed(OVERLAY_AVATAR_SIZE))
        .content_fit(ContentFit::Cover);

    let ring_overlay: Element<'a, Message> = if is_muted {
        let badge = container(text("MUTE").size(7).color(Color::from_rgba(
            1.0,
            1.0,
            1.0,
            opacity.max(0.85),
        )))
        .padding([1, 4])
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(
                M3_ERROR.r,
                M3_ERROR.g,
                M3_ERROR.b,
                (opacity * 0.95).clamp(0.65, 0.98),
            ))),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

        container(badge)
            .width(Length::Fixed(OVERLAY_AVATAR_SIZE))
            .height(Length::Fixed(OVERLAY_AVATAR_SIZE))
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(Color::from_rgba(
                    0.08,
                    0.03,
                    0.04,
                    (opacity * 0.58).clamp(0.35, 0.70),
                ))),
                border: Border {
                    color: Color::from_rgba(M3_ERROR.r, M3_ERROR.g, M3_ERROR.b, opacity.max(0.80)),
                    width: 2.5,
                    radius: 999.0.into(),
                },
                ..Default::default()
            })
            .into()
    } else if is_speaking {
        container(text("").size(1))
            .width(Length::Fixed(OVERLAY_AVATAR_SIZE))
            .height(Length::Fixed(OVERLAY_AVATAR_SIZE))
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                border: Border {
                    color: Color::from_rgba(
                        M3_SUCCESS.r,
                        M3_SUCCESS.g,
                        M3_SUCCESS.b,
                        opacity.max(0.85),
                    ),
                    width: 2.5,
                    radius: 999.0.into(),
                },
                ..Default::default()
            })
            .into()
    } else {
        container(text("").size(1))
            .width(Length::Fixed(OVERLAY_AVATAR_SIZE))
            .height(Length::Fixed(OVERLAY_AVATAR_SIZE))
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, opacity * 0.15),
                    width: 1.0,
                    radius: 999.0.into(),
                },
                ..Default::default()
            })
            .into()
    };

    container(stack![base, ring_overlay])
        .width(Length::Fixed(OVERLAY_AVATAR_SIZE))
        .height(Length::Fixed(OVERLAY_AVATAR_SIZE))
        .into()
}

fn overlay_member_row<'a>(
    avatar_handle: iced_image::Handle,
    nickname: &'a str,
    is_speaking: bool,
    is_muted: bool,
    opacity: f32,
) -> Element<'a, Message> {
    let nick_color = if is_muted {
        Color::from_rgba(
            M3_ON_SURFACE_VARIANT.r,
            M3_ON_SURFACE_VARIANT.g,
            M3_ON_SURFACE_VARIANT.b,
            (opacity * 0.78).clamp(0.45, 0.85),
        )
    } else if is_speaking {
        Color::from_rgba(1.0, 1.0, 1.0, opacity.max(0.85))
    } else {
        Color::from_rgba(
            M3_ON_SURFACE.r,
            M3_ON_SURFACE.g,
            M3_ON_SURFACE.b,
            (opacity * 0.92).clamp(0.55, 0.95),
        )
    };

    row![
        overlay_avatar(avatar_handle, is_speaking, is_muted, opacity),
        text(nickname)
            .size(13)
            .color(nick_color)
            .wrapping(text::Wrapping::None)
            .width(Length::Shrink),
    ]
    .spacing(8)
    .align_y(Vertical::Center)
    .into()
}

fn transparent_root_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        ..Default::default()
    }
}
