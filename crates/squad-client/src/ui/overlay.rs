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

fn effective_widget_opacity(configured: f32) -> f32 {
    #[cfg(target_os = "macos")]
    {
        // Window-wide alpha is applied natively via NSWindow setAlphaValue:.
        let _ = configured;
        1.0
    }
    #[cfg(not(target_os = "macos"))]
    {
        configured.clamp(0.30, 1.0)
    }
}

fn scaled_alpha(opacity: f32, weight: f32) -> f32 {
    (opacity.clamp(0.30, 1.0) * weight).clamp(0.0, 1.0)
}

pub fn view_overlay(app: &SquadApp) -> Element<'_, Message> {
    let opacity = effective_widget_opacity(app.config.overlay_opacity);
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
        .content_fit(ContentFit::Cover)
        .opacity(opacity);

    let ring_overlay: Element<'a, Message> = if is_muted {
        let badge = container(text("MUTE").size(7).color(Color::from_rgba(
            1.0,
            1.0,
            1.0,
            scaled_alpha(opacity, 1.0),
        )))
        .padding([1, 4])
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(
                M3_ERROR.r,
                M3_ERROR.g,
                M3_ERROR.b,
                scaled_alpha(opacity, 0.95),
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
                    scaled_alpha(opacity, 0.58),
                ))),
                border: Border {
                    color: Color::from_rgba(
                        M3_ERROR.r,
                        M3_ERROR.g,
                        M3_ERROR.b,
                        scaled_alpha(opacity, 0.95),
                    ),
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
                        scaled_alpha(opacity, 1.0),
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
                    color: Color::from_rgba(1.0, 1.0, 1.0, scaled_alpha(opacity, 0.18)),
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
            scaled_alpha(opacity, 0.78),
        )
    } else if is_speaking {
        Color::from_rgba(1.0, 1.0, 1.0, scaled_alpha(opacity, 1.0))
    } else {
        Color::from_rgba(
            M3_ON_SURFACE.r,
            M3_ON_SURFACE.g,
            M3_ON_SURFACE.b,
            scaled_alpha(opacity, 0.92),
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

#[cfg(test)]
mod tests {
    use super::scaled_alpha;

    #[test]
    fn scaled_alpha_preserves_linear_range() {
        assert!((scaled_alpha(0.30, 1.0) - 0.30).abs() < 1e-6);
        assert!((scaled_alpha(0.50, 0.58) - 0.29).abs() < 1e-6);
        assert!((scaled_alpha(0.10, 1.0) - 0.30).abs() < 1e-6);
        assert!((scaled_alpha(1.25, 1.0) - 1.0).abs() < 1e-6);
    }
}
