use crate::ui::{
    components::avatar_circle,
    theme::{
        canvas_panel_style, channel_nav_button, m3_text_input_style, pill_danger_button,
        pill_primary_button, pill_tonal_button, sidebar_panel_style, surface_card_style,
        M3_ERROR, M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS,
    },
    Message, SquadApp,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button, column, container, horizontal_space, row, scrollable, text, text_input,
        vertical_space,
    },
    Element, Length,
};

pub fn view_lobby(app: &SquadApp) -> Element<'_, Message> {
    let mut channel_list = column![].spacing(6).width(Length::Fill);
    if app.config.saved_channels.is_empty() {
        channel_list = channel_list.push(
            text("No saved channels yet.\nSave a link below or inside an active room.")
                .size(12)
                .color(M3_ON_SURFACE_VARIANT),
        );
    } else {
        for ch in &app.config.saved_channels {
            let uri_join = ch.invite_uri.clone();
            let uri_del = ch.invite_uri.clone();
            let row_item = row![
                button(
                    text(format!("# {}", ch.name))
                        .size(13)
                        .wrapping(text::Wrapping::None)
                )
                .width(Length::Fill)
                .padding([8, 10])
                .style(channel_nav_button(false))
                .on_press(Message::JoinSavedChannel(uri_join)),
                button(text("×").size(13))
                    .padding([8, 10])
                    .style(pill_danger_button)
                    .on_press(Message::RemoveSavedChannel(uri_del)),
            ]
            .spacing(6)
            .align_y(Vertical::Center);
            channel_list = channel_list.push(row_item);
        }
    }

    let user_footer = container(
        row![
            avatar_circle(&app.config.nickname, false),
            column![
                text(&app.config.nickname).size(13),
                text("Online • 10ms Opus").size(11).color(M3_SUCCESS),
            ]
            .spacing(2)
            .width(Length::Fill),
            button(text("Settings").size(12))
                .padding([8, 12])
                .style(pill_tonal_button)
                .on_press(Message::OpenSettings),
        ]
        .spacing(10)
        .align_y(Vertical::Center),
    )
    .padding(10)
    .width(Length::Fill)
    .style(canvas_panel_style);

    let sidebar = container(
        column![
            column![
                text("SQUAD-AIR").size(20).color(M3_PRIMARY),
                text("E2EE Mesh Voice & Swarm")
                    .size(11)
                    .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(2),
            text("SAVED CHANNELS")
                .size(11)
                .color(M3_ON_SURFACE_VARIANT),
            scrollable(channel_list).height(Length::Fill),
            user_footer,
        ]
        .spacing(14)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fixed(290.0))
    .height(Length::Fill)
    .padding(16)
    .style(sidebar_panel_style);

    let create_card = container(
        column![
            text("Create Encrypted Squad Room").size(18),
            text(
                "Generates a local 256-bit ChaCha20-Poly1305 room key. Zero chat or media bytes ever touch the coordinator."
            )
            .size(13)
            .color(M3_ON_SURFACE_VARIANT),
            vertical_space().height(4),
            button(
                text("Create Room & Copy Invite")
                    .size(14)
                    .align_x(Horizontal::Center)
            )
            .width(Length::Fill)
            .padding([12, 20])
            .style(pill_primary_button)
            .on_press(Message::CreateRoom),
        ]
        .spacing(10),
    )
    .width(Length::Fill)
    .padding(22)
    .style(surface_card_style);

    let join_card = container(
        column![
            text("Join or Bookmark Channel").size(18),
            text("Paste a squad://<room>#<secret> link to connect directly via P2P UDP:")
                .size(13)
                .color(M3_ON_SURFACE_VARIANT),
            row![
                text_input("squad://...", &app.invite_input)
                    .on_input(Message::InviteInputChanged)
                    .on_submit(Message::JoinRoomWithInput)
                    .padding(11)
                    .size(13)
                    .style(m3_text_input_style),
                button(text("Join Now").size(13))
                    .padding([11, 20])
                    .style(pill_primary_button)
                    .on_press(Message::JoinRoomWithInput),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text_input("Channel bookmark name (e.g. CS2 Premier)", &app.save_channel_name_input)
                    .on_input(Message::SaveChannelNameChanged)
                    .on_submit(Message::SaveChannelFromInput)
                    .padding(10)
                    .size(13)
                    .style(m3_text_input_style),
                button(text("Save Channel").size(13))
                    .padding([10, 18])
                    .style(pill_tonal_button)
                    .on_press(Message::SaveChannelFromInput),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
        ]
        .spacing(12),
    )
    .width(Length::Fill)
    .padding(22)
    .style(surface_card_style);

    let mut main_col = column![
        row![
            column![
                text("Welcome Back").size(22),
                text(format!("Server coordinator: {}", app.config.rendezvous_server))
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(2),
            horizontal_space(),
        ]
        .align_y(Vertical::Center),
        create_card,
        join_card,
    ]
    .spacing(16)
    .width(Length::Fill);

    if let Some(ref banner) = app.status_banner {
        main_col = main_col.push(
            container(text(banner).size(13).color(M3_ERROR))
                .padding([10, 14])
                .width(Length::Fill)
                .style(surface_card_style),
        );
    }

    let workspace = container(main_col)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(24)
        .style(canvas_panel_style);

    row![sidebar, workspace]
        .spacing(14)
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
