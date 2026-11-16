use crate::net::RemoteStreamInfo;
use crate::ui::theme::{
    canvas_panel_style, pill_danger_button, pill_primary_button, pill_tonal_button, M3_ERROR,
    M3_ON_PRIMARY, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_OUTLINE_VARIANT,
};
use crate::ui::{Message, SquadApp};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{button, column, container, horizontal_space, image as iced_image, row, stack, text},
    Background, Border, Color, ContentFit, Element, Length, Theme,
};

fn live_pill_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_ERROR)),
        text_color: Some(M3_ON_PRIMARY),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn video_canvas_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(0.05, 0.05, 0.07))),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

pub fn view_stream_stage(app: &SquadApp) -> Element<'_, Message> {
    let Some(watched_id) = app.watched_peer_id else {
        return container(text("")).into();
    };

    let watched_peer = app.peers.iter().find(|p| p.peer_id == watched_id);
    let streamer_name = watched_peer
        .map(|p| p.nickname.as_str())
        .unwrap_or("Streamer");

    let meta_text = watched_peer
        .and_then(|p| p.stream_info.as_ref())
        .map(|info: &RemoteStreamInfo| {
            format!(
                "{} • {}x{} • {} FPS • {}",
                info.source_title, info.width, info.height, info.fps, info.encoder_backend
            )
        })
        .unwrap_or_else(|| "Connecting to E2EE stream...".to_string());

    let expand_label = if app.stream_expanded {
        "Collapse"
    } else {
        "Expand"
    };

    let top_bar = row![
        container(text("LIVE").size(10).color(M3_ON_PRIMARY))
            .padding([3, 7])
            .style(live_pill_style),
        column![
            text(format!("{streamer_name}'s Screen"))
                .size(14)
                .color(M3_ON_SURFACE),
            text(meta_text).size(11).color(M3_ON_SURFACE_VARIANT),
        ]
        .spacing(1),
        horizontal_space(),
        button(text(expand_label).size(12))
            .padding([7, 12])
            .style(pill_tonal_button)
            .on_press(Message::ToggleStreamExpanded),
        button(text("Fullscreen (F)").size(12))
            .padding([7, 12])
            .style(pill_primary_button)
            .on_press(Message::ToggleStreamFullscreen),
        button(text("Close Stream").size(12))
            .padding([7, 12])
            .style(pill_danger_button)
            .on_press(Message::StopWatchingStream),
    ]
    .spacing(10)
    .align_y(Vertical::Center);

    let video_area: Element<'_, Message> = if let Some(ref handle) = app.active_stream_frame {
        container(
            iced_image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Contain),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(4)
        .style(video_canvas_style)
        .into()
    } else {
        container(
            text("Waiting for keyframe (H.264 E2EE)...")
                .size(13)
                .color(M3_ON_SURFACE_VARIANT),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(video_canvas_style)
        .into()
    };

    let height = if app.stream_expanded {
        Length::Fill
    } else {
        Length::Fixed(340.0)
    };

    container(column![top_bar, video_area].spacing(10))
        .width(Length::Fill)
        .height(height)
        .padding(12)
        .style(canvas_panel_style)
        .into()
}

pub fn view_stream_fullscreen(app: &SquadApp) -> Element<'_, Message> {
    let Some(watched_id) = app.watched_peer_id else {
        return container(text("")).into();
    };
    let watched_peer = app.peers.iter().find(|p| p.peer_id == watched_id);
    let streamer_name = watched_peer
        .map(|p| p.nickname.as_str())
        .unwrap_or("Streamer");

    let video_bg: Element<'_, Message> = if let Some(ref handle) = app.active_stream_frame {
        container(
            iced_image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Contain),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(Background::Color(Color::BLACK)),
            ..Default::default()
        })
        .into()
    } else {
        container(
            text("Waiting for keyframe (H.264 E2EE)...")
                .size(14)
                .color(M3_ON_SURFACE_VARIANT),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(Background::Color(Color::BLACK)),
            ..Default::default()
        })
        .into()
    };

    let floating_controls = container(
        row![
            container(text(format!("LIVE • {streamer_name}")).size(11).color(M3_ON_PRIMARY))
                .padding([5, 10])
                .style(live_pill_style),
            button(text("Exit Fullscreen (Esc)").size(12))
                .padding([7, 14])
                .style(pill_tonal_button)
                .on_press(Message::ExitStreamFullscreen),
            button(text("Close Stream").size(12))
                .padding([7, 12])
                .style(pill_danger_button)
                .on_press(Message::StopWatchingStream),
        ]
        .spacing(8)
        .align_y(Vertical::Center),
    )
    .padding(10)
    .style(|_t: &Theme| container::Style {
        background: Some(Background::Color(Color::from_rgba(0.08, 0.09, 0.12, 0.72))),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    });

    let top_right = container(floating_controls)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(16)
        .align_x(Horizontal::Right)
        .align_y(Vertical::Top);

    stack![video_bg, top_right].into()
}
