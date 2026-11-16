use crate::net::{BlobTransferState, ChatContentUi, ChatItemUi};
use crate::ui::{
    components::avatar_circle,
    theme::{
        attachment_card_style, canvas_panel_style, chat_bubble_style, m3_text_input_style,
        pill_danger_button, pill_primary_button, pill_success_button, pill_tonal_button,
        M3_ERROR, M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS, M3_SURFACE_HIGHEST,
    },
    Message, SquadApp,
};
use iced::{
    alignment::Vertical,
    widget::{button, column, container, horizontal_space, image, row, scrollable, text, text_input},
    Background, Border, ContentFit, Element, Length, Theme,
};

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

    let composer = row![
        button(text("+ File").size(12))
            .padding([10, 12])
            .style(pill_tonal_button)
            .on_press(Message::PickFileToSend),
        text_input("Write a message...", &app.chat_input)
            .on_input(Message::ChatInputChanged)
            .on_submit(Message::SendChatText)
            .padding(10)
            .size(13)
            .style(m3_text_input_style),
        button(text("Send").size(12))
            .padding([10, 14])
            .style(pill_primary_button)
            .on_press(Message::SendChatText),
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
        let is_copied = app.copied_msg_id.is_some_and(|(id, _)| id == item.msg_id);
        let copy_label = if is_copied { "Copied ✓" } else { "Copy" };
        let copy_btn = button(text(copy_label).size(10))
            .padding([3, 7])
            .style(if is_copied {
                pill_success_button
            } else {
                pill_tonal_button
            })
            .on_press(Message::CopyChatText(item.msg_id, body_text.clone()));
        meta_items = meta_items.push(copy_btn);
    }

    let meta_row = meta_items.push(
        text(format_utc_hhmm(item.timestamp_ms))
            .size(11)
            .color(M3_ON_SURFACE_VARIANT),
    );

    let body: Element<'a, Message> = match &item.content {
        ChatContentUi::Text(body_text) => text(body_text).size(13).into(),
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

                let footer_row = row![
                    caption,
                    horizontal_space(),
                    button(text("Copy Image").size(10))
                        .padding([4, 8])
                        .style(pill_tonal_button)
                        .on_press(Message::CopyImageToClipboard(bid)),
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
                    BlobTransferState::Failed(err) => {
                        text(format!("Image transfer failed: {err}"))
                            .size(12)
                            .color(M3_ERROR)
                            .into()
                    }
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
                BlobTransferState::Available => button(text("Download").size(12))
                    .padding([7, 12])
                    .style(pill_primary_button)
                    .on_press(Message::DownloadBlob(bid))
                    .into(),
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
                    let label = if item.is_self { "Shared" } else { "Open Folder" };
                    button(text(label).size(12))
                        .padding([7, 12])
                        .style(pill_success_button)
                        .on_press(Message::OpenDownloadsFolder(saved_path.clone()))
                        .into()
                }
                BlobTransferState::Failed(err) => row![
                    text(err).size(11).color(M3_ERROR),
                    button(text("Retry").size(11))
                        .padding([6, 10])
                        .style(pill_danger_button)
                        .on_press(Message::DownloadBlob(bid)),
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
