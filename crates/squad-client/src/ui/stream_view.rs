use crate::net::RemoteStreamInfo;
use crate::ui::theme::{
    canvas_panel_style, m3_slider_style, pill_danger_button, pill_primary_button,
    pill_tonal_button, M3_ERROR, M3_ON_PRIMARY, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT,
    M3_OUTLINE_VARIANT,
};
use crate::ui::{Message, SquadApp};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button, column, container, horizontal_space, image as iced_image, row, slider, stack, text,
    },
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

fn stream_volume_control<'a>(app: &SquadApp) -> Element<'a, Message> {
    let pct = (app.config.stream_volume * 100.0).round() as i32;
    row![
        text("Stream Vol").size(11).color(M3_ON_SURFACE_VARIANT),
        slider(0.0..=200.0, app.config.stream_volume * 100.0, |v| {
            Message::StreamVolumeChanged(v / 100.0)
        })
        .step(1.0_f32)
        .width(Length::Fixed(92.0))
        .style(m3_slider_style),
        text(format!("{pct}%"))
            .size(11)
            .width(Length::Fixed(34.0))
            .color(M3_ON_SURFACE_VARIANT),
    ]
    .spacing(6)
    .align_y(Vertical::Center)
    .into()
}

pub fn view_stream_stage(app: &SquadApp) -> Element<'_, Message> {
    let is_preview = app.local_stream_preview && app.watched_peer_id.is_none();
    if app.watched_peer_id.is_none() && !is_preview {
        return container(text("")).into();
    }

    let (title_text, meta_text, has_audio, badge_label, close_msg) = if is_preview {
        let (_, preview_h) = app
            .capture_targets
            .get(app.selected_capture_target_idx)
            .map(|t| {
                let (w, h) = t.native_dimensions();
                app.share_resolution.target_dimensions(w, h)
            })
            .unwrap_or((1920, 1080));
        (
            "Your Screen Preview".to_string(),
            format!("{}p • {} FPS", preview_h, app.share_fps.as_u8()),
            false,
            "PREVIEW",
            Message::ToggleLocalStreamPreview,
        )
    } else {
        let Some(watched_id) = app.watched_peer_id else {
            return container(text("")).into();
        };
        let watched_peer = app.peers.iter().find(|p| p.peer_id == watched_id);
        let streamer_name = watched_peer
            .map(|p| p.nickname.as_str())
            .unwrap_or("Streamer");
        let meta = watched_peer
            .and_then(|p| p.stream_info.as_ref())
            .map(|info: &RemoteStreamInfo| format!("{}p • {} FPS", info.height, info.fps))
            .unwrap_or_else(|| "Connecting...".to_string());
        let audio_flag = watched_peer
            .and_then(|p| p.stream_info.as_ref())
            .is_some_and(|info| info.has_audio);
        (
            format!("{streamer_name}'s Screen"),
            meta,
            audio_flag,
            "LIVE",
            Message::StopWatchingStream,
        )
    };

    let expand_label = if app.stream_expanded {
        "Collapse"
    } else {
        "Expand"
    };

    let mut top_bar = row![
        container(text(badge_label).size(10).color(M3_ON_PRIMARY))
            .padding([3, 7])
            .style(live_pill_style),
        column![
            text(title_text).size(14).color(M3_ON_SURFACE),
            text(meta_text).size(11).color(M3_ON_SURFACE_VARIANT),
        ]
        .spacing(1),
        horizontal_space(),
    ]
    .spacing(10)
    .align_y(Vertical::Center);

    if has_audio {
        top_bar = top_bar.push(stream_volume_control(app));
    }

    top_bar = top_bar
        .push(
            button(text(expand_label).size(12))
                .padding([7, 12])
                .style(pill_tonal_button)
                .on_press(Message::ToggleStreamExpanded),
        )
        .push(
            button(text("Fullscreen").size(12))
                .padding([7, 12])
                .style(pill_primary_button)
                .on_press(Message::ToggleStreamFullscreen),
        )
        .push(
            button(text("Close").size(12))
                .padding([7, 12])
                .style(pill_danger_button)
                .on_press(close_msg),
        );

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
            text("Starting stream...")
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
    let is_preview = app.local_stream_preview && app.watched_peer_id.is_none();
    if app.watched_peer_id.is_none() && !is_preview {
        return container(text("")).into();
    }

    let (pill_label, has_audio, close_msg) = if is_preview {
        (
            "PREVIEW".to_string(),
            false,
            Message::ToggleLocalStreamPreview,
        )
    } else {
        let Some(watched_id) = app.watched_peer_id else {
            return container(text("")).into();
        };
        let watched_peer = app.peers.iter().find(|p| p.peer_id == watched_id);
        let streamer_name = watched_peer
            .map(|p| p.nickname.as_str())
            .unwrap_or("Streamer");
        let audio_flag = watched_peer
            .and_then(|p| p.stream_info.as_ref())
            .is_some_and(|info| info.has_audio);
        (
            format!("LIVE • {streamer_name}"),
            audio_flag,
            Message::StopWatchingStream,
        )
    };

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
            text("Starting stream...")
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

    let mut controls_row = row![container(text(pill_label).size(11).color(M3_ON_PRIMARY))
        .padding([5, 10])
        .style(live_pill_style),]
    .spacing(8)
    .align_y(Vertical::Center);

    if has_audio {
        controls_row = controls_row.push(stream_volume_control(app));
    }

    controls_row = controls_row
        .push(
            button(text("Exit Fullscreen").size(12))
                .padding([7, 14])
                .style(pill_tonal_button)
                .on_press(Message::ExitStreamFullscreen),
        )
        .push(
            button(text("Close").size(12))
                .padding([7, 12])
                .style(pill_danger_button)
                .on_press(close_msg),
        );

    let floating_controls =
        container(controls_row)
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
