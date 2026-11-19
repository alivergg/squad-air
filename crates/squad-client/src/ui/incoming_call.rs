use crate::crypto_identity::format_friend_code;
use crate::net::IncomingFriendCallUi;
use crate::ui::components::avatar_circle;
use crate::ui::theme::{
    canvas_panel_style, m3_text_input_style, pill_danger_button, pill_primary_button,
    pill_success_button, pill_tonal_button, M3_ERROR, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT,
    M3_OUTLINE, M3_PRIMARY, M3_SUCCESS, M3_SURFACE_CONTAINER, M3_WARNING,
};
use crate::ui::{Message, SquadApp};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{button, column, container, horizontal_space, row, scrollable, text, text_input},
    Background, Border, Color, Element, Length, Shadow, Theme, Vector,
};

fn backdrop_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.72))),
        ..Default::default()
    }
}

fn modal_card_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE,
            width: 1.0,
            radius: 24.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.6),
            offset: Vector::new(0.0, 12.0),
            blur_radius: 32.0,
        },
    }
}

pub fn view_incoming_call_modal<'a>(
    app: &'a SquadApp,
    call: &'a IncomingFriendCallUi,
) -> Element<'a, Message> {
    let header_title = if call.is_party_invite {
        "Party Invite"
    } else {
        "Incoming Voice Call"
    };
    let subtitle = if call.is_known_friend {
        "End-to-end encrypted X25519 invite"
    } else {
        "Unknown caller — accepting adds them to your Friends list"
    };
    let subtitle_color = if call.is_known_friend {
        M3_SUCCESS
    } else {
        M3_WARNING
    };

    let actions = row![
        button(text("Decline").size(13).align_x(Horizontal::Center))
            .width(Length::Fill)
            .padding([12, 18])
            .style(pill_danger_button)
            .on_press(Message::DeclineIncomingCall),
        button(text("Accept & Join").size(13).align_x(Horizontal::Center))
            .width(Length::Fill)
            .padding([12, 18])
            .style(pill_success_button)
            .on_press(Message::AcceptIncomingCall),
    ]
    .spacing(12)
    .width(Length::Fill);

    let card = container(
        column![
            text(header_title).size(18).color(M3_PRIMARY),
            avatar_circle(
                app.avatars.handle_for_pub_key(&call.sender_pub_key),
                72.0,
                true
            ),
            text(&call.sender_nickname).size(20).color(M3_ON_SURFACE),
            text(subtitle).size(12).color(subtitle_color),
            actions,
        ]
        .spacing(14)
        .align_x(Horizontal::Center)
        .width(Length::Fill),
    )
    .max_width(400)
    .padding(26)
    .style(modal_card_style);

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(24)
        .style(backdrop_style)
        .into()
}

pub fn view_add_friend_modal(app: &SquadApp) -> Element<'_, Message> {
    let my_code = format_friend_code(&app.config.identity_pub_key(), &app.config.nickname);
    let my_code_preview = if my_code.chars().count() > 42 {
        let prefix: String = my_code.chars().take(39).collect();
        format!("{prefix}...")
    } else {
        my_code
    };
    let copy_label = if app.friend_id_copied_flash {
        "Copied!"
    } else {
        "Copy My ID"
    };

    let mut content = column![
        column![
            text("Add Friend").size(18).color(M3_ON_SURFACE),
            text("Share your cryptographic X25519 Friend ID or paste a friend's ID below.")
                .size(12)
                .color(M3_ON_SURFACE_VARIANT),
        ]
        .spacing(4),
        container(
            row![
                column![
                    text("Your Friend ID").size(11).color(M3_ON_SURFACE_VARIANT),
                    text(my_code_preview).size(12).color(M3_ON_SURFACE),
                ]
                .spacing(2)
                .width(Length::Fill),
                button(text(copy_label).size(12))
                    .padding([8, 12])
                    .style(if app.friend_id_copied_flash {
                        pill_success_button
                    } else {
                        pill_tonal_button
                    })
                    .on_press(Message::CopyMyFriendId),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
        )
        .padding(12)
        .width(Length::Fill)
        .style(canvas_panel_style),
        column![
            text("Friend ID").size(12).color(M3_ON_SURFACE_VARIANT),
            text_input("sqf_...#Nickname", &app.add_friend_code_input)
                .on_input(Message::AddFriendCodeChanged)
                .on_submit(Message::ConfirmAddFriend)
                .padding(11)
                .size(13)
                .style(m3_text_input_style),
        ]
        .spacing(6),
        column![
            text("Display Name (optional)")
                .size(12)
                .color(M3_ON_SURFACE_VARIANT),
            text_input("Nickname override", &app.add_friend_nick_input)
                .on_input(Message::AddFriendNickChanged)
                .on_submit(Message::ConfirmAddFriend)
                .padding(11)
                .size(13)
                .style(m3_text_input_style),
        ]
        .spacing(6),
    ]
    .spacing(14)
    .width(Length::Fill);

    if let Some(ref err) = app.add_friend_error {
        content = content.push(text(err).size(12).color(M3_ERROR));
    }

    let footer = row![
        horizontal_space(),
        button(text("Cancel").size(13))
            .padding([10, 18])
            .style(pill_tonal_button)
            .on_press(Message::CloseAddFriendModal),
        button(text("Add Friend").size(13))
            .padding([10, 20])
            .style(pill_primary_button)
            .on_press(Message::ConfirmAddFriend),
    ]
    .spacing(10)
    .align_y(Vertical::Center);

    content = content.push(footer);

    let card = container(content)
        .max_width(460)
        .padding(24)
        .style(modal_card_style);

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(24)
        .style(backdrop_style)
        .into()
}

pub fn view_invite_friends_modal(app: &SquadApp) -> Element<'_, Message> {
    let mut sorted_indices: Vec<usize> = (0..app.friends.len()).collect();
    sorted_indices.sort_by(|&a, &b| {
        let fa = &app.friends[a];
        let fb = &app.friends[b];
        let oa = app.friend_presences.contains_key(&fa.pub_key);
        let ob = app.friend_presences.contains_key(&fb.pub_key);
        ob.cmp(&oa)
            .then_with(|| fa.nickname.to_lowercase().cmp(&fb.nickname.to_lowercase()))
    });

    let mut list_col = column![].spacing(8).width(Length::Fill);
    if sorted_indices.is_empty() {
        list_col = list_col.push(
            text("No friends added yet. Add friends in the Lobby using their sqf_ Friend ID.")
                .size(12)
                .color(M3_ON_SURFACE_VARIANT),
        );
    } else {
        for idx in sorted_indices {
            let friend = &app.friends[idx];
            let presence = app.friend_presences.get(&friend.pub_key);
            let online = presence.is_some();
            let display_name = presence
                .filter(|p| !p.nickname.trim().is_empty())
                .map(|p| p.nickname.as_str())
                .unwrap_or(friend.nickname.as_str());
            let (status_label, status_color) = match presence {
                Some(p) if p.in_room => ("In Voice Room", M3_WARNING),
                Some(_) => ("Online", M3_SUCCESS),
                None => ("Offline", M3_ON_SURFACE_VARIANT),
            };
            let invited = app.invited_friends_session.contains(&friend.pub_key);

            let action_btn = if invited {
                button(text("Invited ✓").size(12))
                    .padding([7, 14])
                    .style(pill_success_button)
                    .on_press(Message::InviteFriendToParty(friend.pub_key))
            } else if online {
                button(text("Invite").size(12))
                    .padding([7, 14])
                    .style(pill_primary_button)
                    .on_press(Message::InviteFriendToParty(friend.pub_key))
            } else {
                button(text("Offline").size(12))
                    .padding([7, 14])
                    .style(pill_tonal_button)
            };

            let item_row = container(
                row![
                    avatar_circle(
                        app.avatars.handle_for_pub_key(&friend.pub_key),
                        34.0,
                        online
                    ),
                    column![
                        text(display_name)
                            .size(13)
                            .color(M3_ON_SURFACE)
                            .wrapping(text::Wrapping::None),
                        text(status_label).size(11).color(status_color),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    action_btn,
                ]
                .spacing(10)
                .align_y(Vertical::Center),
            )
            .padding(10)
            .width(Length::Fill)
            .style(canvas_panel_style);

            list_col = list_col.push(item_row);
        }
    }

    let card = container(
        column![
            column![
                text("Invite Friends to Party")
                    .size(18)
                    .color(M3_ON_SURFACE),
                text("Send an end-to-end encrypted room invite to an online friend.")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(4),
            container(scrollable(list_col).height(Length::Fixed(300.0))).width(Length::Fill),
            row![
                horizontal_space(),
                button(text("Close").size(13))
                    .padding([10, 20])
                    .style(pill_tonal_button)
                    .on_press(Message::CloseInviteFriendsModal),
            ]
            .align_y(Vertical::Center),
        ]
        .spacing(14)
        .width(Length::Fill),
    )
    .max_width(460)
    .padding(24)
    .style(modal_card_style);

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(24)
        .style(backdrop_style)
        .into()
}
