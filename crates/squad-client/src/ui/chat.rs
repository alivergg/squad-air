use crate::net::{BlobTransferState, ChatContentUi, ChatItemUi};
use crate::ui::{
    components::{avatar_circle, with_tooltip},
    icons::{icon, icon_check_animated, IconKind},
    theme::{
        attachment_card_style, canvas_panel_style, chat_bubble_style, copy_flash_button_style,
        m3_text_input_style, pill_danger_button, pill_primary_button, pill_success_button,
        pill_tonal_button, M3_ERROR, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS,
        M3_SURFACE_HIGHEST,
    },
    Message, SquadApp,
};
use iced::{
    alignment::Vertical,
    keyboard,
    widget::{
        button, column, container, horizontal_space, image, row, scrollable, text, text_editor,
        text_input, tooltip::Position,
    },
    Background, Border, Color, ContentFit, Element, Length, Theme,
};
use std::time::Instant;

pub fn view_chat(app: &SquadApp) -> Element<'_, Message> {
    let header = row![
        text("Room Chat").size(15).color(M3_ON_SURFACE_VARIANT),
        horizontal_space(),
        container(text(format!("{} msgs", app.chat_messages.len())).size(11))
            .padding([3, 9])
            .style(attachment_card_style),
    ]
    .align_y(Vertical::Center);

    let mut feed_col = column![].spacing(10).width(Length::Fill);
    if app.chat_messages.is_empty() {
        feed_col = feed_col.push(
            container(
                column![
                    text("No messages yet").size(13),
                    text("Say hello, paste a screenshot, or attach a file.")
                        .size(12)
                        .color(M3_ON_SURFACE_VARIANT),
                ]
                .spacing(4),
            )
            .width(Length::Fill)
            .padding(14)
            .style(attachment_card_style),
        );
    } else {
        for item in &app.chat_messages {
            feed_col = feed_col.push(render_chat_item(app, item));
        }
    }

    let feed = scrollable(container(feed_col).padding(iced::Padding {
        top: 4.0,
        right: 8.0,
        bottom: 4.0,
        left: 0.0,
    }))
    .height(Length::Fill)
    .anchor_bottom();

    let file_btn = if app.has_relayed_peer() {
        with_tooltip(
            button(icon(IconKind::AttachFile, 18.0))
                .padding([10, 11])
                .style(pill_tonal_button),
            "File transfer disabled on Relay — Voice & Chat only",
            Position::Top,
        )
    } else {
        with_tooltip(
            button(icon(IconKind::AttachFile, 18.0))
                .padding([10, 11])
                .style(pill_tonal_button)
                .on_press(Message::PickFileToSend),
            "Attach file",
            Position::Top,
        )
    };

    let composer = row![
        file_btn,
        text_input("Write a message...", &app.chat_input)
            .on_input(Message::ChatInputChanged)
            .on_submit(Message::SendChatText)
            .padding(10)
            .size(13)
            .style(m3_text_input_style),
        with_tooltip(
            button(icon(IconKind::Send, 18.0))
                .padding([10, 12])
                .style(pill_primary_button)
                .on_press(Message::SendChatText),
            "Send message",
            Position::Top,
        ),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    container(
        column![header, feed, composer]
            .spacing(12)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .padding(16)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(canvas_panel_style)
    .into()
}

fn render_chat_item<'a>(app: &'a SquadApp, item: &'a ChatItemUi) -> Element<'a, Message> {
    let now = Instant::now();
    let author_label = if item.is_self {
        format!("{} (You)", item.sender_nickname)
    } else {
        item.sender_nickname.clone()
    };
    let author_color = if item.is_self {
        M3_PRIMARY
    } else {
        iced::Color::WHITE
    };

    let mut meta_items = row![
        avatar_circle(app.avatars.handle_for(&item.sender_uuid), 24.0, false),
        text(author_label).size(12).color(author_color),
        horizontal_space(),
    ]
    .spacing(6)
    .align_y(Vertical::Center);

    if let ChatContentUi::Text(ref body_text) = item.content {
        let copy_progress = app.copy_flash.chat_msg_progress_at(item.msg_id, now);
        let (copy_icon, copy_tip): (Element<'_, Message>, &'static str) = match copy_progress {
            Some(p) => (
                icon_check_animated(13.0, p.check_draw, p.pop_pulse),
                "Copied!",
            ),
            None => (icon(IconKind::ContentCopy, 13.0), "Copy message"),
        };
        let copy_btn = with_tooltip(
            button(copy_icon)
                .padding([4, 6])
                .style(copy_flash_button_style(pill_tonal_button, copy_progress))
                .on_press(Message::CopyChatText(item.msg_id, body_text.clone())),
            copy_tip,
            Position::Top,
        );
        meta_items = meta_items.push(copy_btn);
    }

    let meta_row = meta_items.push(
        text(format_utc_hhmm(item.timestamp_ms))
            .size(11)
            .color(M3_ON_SURFACE_VARIANT),
    );

    let body: Element<'a, Message> = match &item.content {
        ChatContentUi::Text(body_text) => {
            let key = (item.sender_uuid, item.msg_id);
            if let Some(content) = app.chat_text_editors.get(&key) {
                text_editor(content)
                    .on_action(move |action| Message::ChatEditorAction(key, action))
                    .key_binding(read_only_chat_key_binding)
                    .padding(0)
                    .size(13)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                    .style(chat_text_editor_style)
                    .into()
            } else {
                text(body_text).size(13).into()
            }
        }
        ChatContentUi::InlineImage {
            blob_id,
            filename,
            total_size,
            state,
            ..
        } => {
            let caption = text(format!("{filename} ({})", format_bytes(*total_size)))
                .size(11)
                .color(M3_ON_SURFACE_VARIANT);

            if let Some(handle) = app.image_handles.get(blob_id) {
                let bid = *blob_id;
                let clickable_img = button(
                    container(
                        image(handle.clone())
                            .width(Length::Fill)
                            .content_fit(ContentFit::Contain),
                    )
                    .max_height(220.0)
                    .width(Length::Fill),
                )
                .padding(0)
                .width(Length::Fill)
                .style(|_t: &Theme, _status| button::Style {
                    background: None,
                    ..Default::default()
                })
                .on_press(Message::OpenImageLightbox(bid));

                let image_progress = app.copy_flash.image_progress_at(bid, now);
                let (img_copy_icon, img_copy_tip): (Element<'_, Message>, &'static str) =
                    match image_progress {
                        Some(p) => (
                            icon_check_animated(14.0, p.check_draw, p.pop_pulse),
                            "Copied!",
                        ),
                        None => (icon(IconKind::ContentCopy, 14.0), "Copy image"),
                    };
                let footer_row = row![
                    caption,
                    horizontal_space(),
                    with_tooltip(
                        button(img_copy_icon)
                            .padding([5, 7])
                            .style(copy_flash_button_style(pill_tonal_button, image_progress))
                            .on_press(Message::CopyImageToClipboard(bid)),
                        img_copy_tip,
                        Position::Top,
                    ),
                ]
                .spacing(8)
                .align_y(Vertical::Center);

                column![clickable_img, footer_row].spacing(6).into()
            } else {
                let status_line: Element<'a, Message> = match state {
                    BlobTransferState::Downloading {
                        received_chunks,
                        total_chunks,
                    } => {
                        let pct = percent_progress(*received_chunks, *total_chunks);
                        column![
                            text(format!("Receiving inline image... {pct}%"))
                                .size(12)
                                .color(M3_ON_SURFACE_VARIANT),
                            progress_bar(pct),
                        ]
                        .spacing(6)
                        .into()
                    }
                    BlobTransferState::Failed(err) => text(format!("Image transfer failed: {err}"))
                        .size(12)
                        .color(M3_ERROR)
                        .into(),
                    _ => text("Waiting for image chunks...")
                        .size(12)
                        .color(M3_ON_SURFACE_VARIANT)
                        .into(),
                };
                container(column![caption, status_line].spacing(6))
                    .width(Length::Fill)
                    .padding(10)
                    .style(attachment_card_style)
                    .into()
            }
        }
        ChatContentUi::File {
            blob_id,
            filename,
            total_size,
            state,
        } => {
            let bid = *blob_id;
            let file_info = column![
                text(filename).size(13),
                text(format_bytes(*total_size))
                    .size(11)
                    .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(2)
            .width(Length::Fill);

            let action: Element<'a, Message> = match state {
                BlobTransferState::Available => with_tooltip(
                    button(icon(IconKind::Download, 16.0))
                        .padding([7, 10])
                        .style(pill_primary_button)
                        .on_press(Message::DownloadBlob(bid)),
                    "Download file",
                    Position::Top,
                ),
                BlobTransferState::Downloading {
                    received_chunks,
                    total_chunks,
                } => {
                    let pct = percent_progress(*received_chunks, *total_chunks);
                    column![
                        text(format!("{pct}%"))
                            .size(11)
                            .color(M3_ON_SURFACE_VARIANT),
                        progress_bar(pct),
                    ]
                    .width(Length::Fixed(96.0))
                    .spacing(4)
                    .into()
                }
                BlobTransferState::Completed { saved_path } => {
                    let tip = if item.is_self {
                        "Shared — open folder"
                    } else {
                        "Open downloads folder"
                    };
                    with_tooltip(
                        button(icon(IconKind::FolderOpen, 16.0))
                            .padding([7, 10])
                            .style(pill_success_button)
                            .on_press(Message::OpenDownloadsFolder(saved_path.clone())),
                        tip,
                        Position::Top,
                    )
                }
                BlobTransferState::Failed(err) => row![
                    text(err).size(11).color(M3_ERROR),
                    with_tooltip(
                        button(icon(IconKind::Refresh, 15.0))
                            .padding([6, 9])
                            .style(pill_danger_button)
                            .on_press(Message::DownloadBlob(bid)),
                        "Retry download",
                        Position::Top,
                    ),
                ]
                .spacing(6)
                .align_y(Vertical::Center)
                .into(),
            };

            container(
                row![file_info, action]
                    .spacing(10)
                    .align_y(Vertical::Center),
            )
            .width(Length::Fill)
            .padding(10)
            .style(attachment_card_style)
            .into()
        }
    };

    container(column![meta_row, body].spacing(6))
        .width(Length::Fill)
        .padding([10, 12])
        .style(chat_bubble_style(item.is_self))
        .into()
}

fn read_only_chat_key_binding(kp: text_editor::KeyPress) -> Option<text_editor::Binding<Message>> {
    if kp.status != text_editor::Status::Focused {
        return None;
    }
    if kp.modifiers.command() {
        return match kp.key.as_ref() {
            keyboard::Key::Character("c" | "C" | "с" | "С") => Some(text_editor::Binding::Copy),
            keyboard::Key::Character("a" | "A" | "ф" | "Ф") => {
                Some(text_editor::Binding::SelectAll)
            }
            _ => None,
        };
    }
    match text_editor::Binding::from_key_press(kp)? {
        text_editor::Binding::Paste
        | text_editor::Binding::Cut
        | text_editor::Binding::Insert(_)
        | text_editor::Binding::Enter
        | text_editor::Binding::Backspace
        | text_editor::Binding::Delete => None,
        other => Some(other),
    }
}

fn chat_text_editor_style(_theme: &Theme, _status: text_editor::Status) -> text_editor::Style {
    text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        },
        icon: Color::TRANSPARENT,
        placeholder: Color::TRANSPARENT,
        value: M3_ON_SURFACE,
        selection: Color {
            a: 0.38,
            ..M3_PRIMARY
        },
    }
}

fn progress_bar<'a>(pct: u32) -> Element<'a, Message> {
    let frac = (pct.min(100) as f32) / 100.0;
    let fill_w = (96.0 * frac).max(4.0);
    container(
        container(text(""))
            .width(Length::Fixed(fill_w))
            .height(Length::Fixed(6.0))
            .style(|_t: &Theme| container::Style {
                background: Some(Background::Color(M3_SUCCESS)),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
    )
    .width(Length::Fixed(96.0))
    .height(Length::Fixed(6.0))
    .style(|_t: &Theme| container::Style {
        background: Some(Background::Color(M3_SURFACE_HIGHEST)),
        border: Border {
            radius: 3.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

fn percent_progress(received: u32, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    ((received as u64 * 100) / total as u64).min(100) as u32
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_utc_hhmm(timestamp_ms: u64) -> String {
    let day_secs = (timestamp_ms / 1000) % 86_400;
    let hours = day_secs / 3600;
    let mins = (day_secs % 3600) / 60;
    format!("{hours:02}:{mins:02} UTC")
}
