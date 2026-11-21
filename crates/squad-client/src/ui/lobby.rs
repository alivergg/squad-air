use crate::ui::{
    components::avatar_circle,
    theme::{
        canvas_panel_style, channel_nav_button, m3_text_input_style, pill_danger_button,
        pill_primary_button, pill_success_button, pill_tonal_button, sidebar_panel_style,
        surface_card_style, M3_ERROR, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_PRIMARY, M3_SUCCESS,
        M3_WARNING,
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
            let room_id: squad_proto::RoomId = squad_proto::InviteLink::parse(&ch.invite_uri)
                .map(|inv| inv.room_id)
                .unwrap_or_else(|_| {
                    let h = blake3::hash(ch.invite_uri.as_bytes());
                    let mut id = [0u8; 16];
                    id.copy_from_slice(&h.as_bytes()[..16]);
                    id
                });
            let row_item = row![
                button(
                    row![
                        avatar_circle(app.avatars.room_handle_for(&room_id), 26.0, false),
                        text(&ch.name).size(13).wrapping(text::Wrapping::None),
                    ]
                    .spacing(8)
                    .align_y(Vertical::Center),
                )
                .width(Length::Fill)
                .padding([7, 10])
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
            avatar_circle(
                app.avatars.handle_for(&app.config.client_uuid()),
                42.0,
                false
            ),
            column![
                text(&app.config.nickname).size(13),
                text("Online").size(11).color(M3_SUCCESS),
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

    #[cfg_attr(feature = "managed_updates", allow(unused_mut))]
    let mut sidebar_head = column![
        text("SquadAir").size(20).color(M3_PRIMARY),
        text("Private Voice & Screen Share")
            .size(11)
            .color(M3_ON_SURFACE_VARIANT),
    ]
    .spacing(2);

    #[cfg(not(feature = "managed_updates"))]
    if let Some(ref info) = app.available_update {
        sidebar_head = sidebar_head.push(
            container(
                button(
                    text(format!("Update Available ({})", info.tag_name))
                        .size(12)
                        .align_x(Horizontal::Center),
                )
                .width(Length::Fill)
                .padding([8, 12])
                .style(pill_primary_button)
                .on_press(Message::OpenUpdateModal),
            )
            .padding([8, 0]),
        );
    }

    let sidebar = container(
        column![
            sidebar_head,
            text("SAVED ROOMS").size(11).color(M3_ON_SURFACE_VARIANT),
            scrollable(channel_list).height(Length::Fill),
            user_footer,
        ]
        .spacing(14)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fixed(240.0))
    .height(Length::Fill)
    .padding(14)
    .style(sidebar_panel_style);

    let mut join_code_btn = button(
        text(if app.short_code_joining {
            "Connecting..."
        } else {
            "Join Code"
        })
        .size(13),
    )
    .padding([11, 18])
    .style(pill_primary_button);
    if !app.short_code_joining {
        join_code_btn = join_code_btn.on_press(Message::JoinByShortCode);
    }

    let quick_code_card = container(
        column![
            text("Quick Voice Code").size(18),
            text("Enter a 6-digit code (e.g. 482-910) for instant SPAKE2 encrypted join.")
                .size(13)
                .color(M3_ON_SURFACE_VARIANT),
            row![
                text_input("482-910", &app.short_code_input)
                    .on_input(Message::ShortCodeInputChanged)
                    .on_submit(Message::JoinByShortCode)
                    .padding(11)
                    .size(14)
                    .style(m3_text_input_style),
                join_code_btn,
            ]
            .spacing(10)
            .align_y(Vertical::Center),
        ]
        .spacing(10),
    )
    .width(Length::Fill)
    .padding(20)
    .style(surface_card_style);

    let host_btn_label = if app.lan_room_password_input.trim().is_empty() {
        "Host LAN Room"
    } else {
        "Host Locked LAN Room"
    };
    let host_eye_label = if app.lan_host_password_visible {
        "Hide"
    } else {
        "Show"
    };

    let create_card = container(
        column![
            text("Start a Private Room").size(18),
            text(
                "Create an internet room by link or host a zero-config room directly on your LAN."
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
            row![
                text_input("LAN room name (optional)", &app.lan_room_name_input)
                    .on_input(Message::LanRoomNameChanged)
                    .on_submit(Message::HostLanRoom)
                    .width(Length::FillPortion(5))
                    .padding(10)
                    .size(13)
                    .style(m3_text_input_style),
                text_input("Password (optional)", &app.lan_room_password_input)
                    .on_input(Message::LanRoomPasswordChanged)
                    .on_submit(Message::HostLanRoom)
                    .secure(!app.lan_host_password_visible)
                    .width(Length::FillPortion(5))
                    .padding(10)
                    .size(13)
                    .style(m3_text_input_style),
                button(text(host_eye_label).size(12))
                    .padding([10, 12])
                    .style(pill_tonal_button)
                    .on_press(Message::ToggleLanHostPasswordVisible),
            ]
            .spacing(8)
            .align_y(Vertical::Center),
            button(text(host_btn_label).size(13).align_x(Horizontal::Center))
                .width(Length::Fill)
                .padding([10, 18])
                .style(pill_tonal_button)
                .on_press(Message::HostLanRoom),
        ]
        .spacing(10),
    )
    .width(Length::Fill)
    .padding(22)
    .style(surface_card_style);

    let join_card = container(
        column![
            text("Join or Save Room").size(18),
            text("Paste a room invite link to join or add it to your sidebar:")
                .size(13)
                .color(M3_ON_SURFACE_VARIANT),
            row![
                text_input("Paste invite link...", &app.invite_input)
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
                text_input("Room name (e.g. Squad Night)", &app.save_channel_name_input)
                    .on_input(Message::SaveChannelNameChanged)
                    .on_submit(Message::SaveChannelFromInput)
                    .padding(10)
                    .size(13)
                    .style(m3_text_input_style),
                button(text("Save Room").size(13))
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

    let mut lan_list = column![].spacing(8).width(Length::Fill);
    if app.lan_rooms.is_empty() {
        lan_list = lan_list.push(
            text("No LAN rooms discovered right now. Friends hosting on your Wi-Fi / LAN appear here automatically.")
                .size(12)
                .color(M3_ON_SURFACE_VARIANT),
        );
    } else {
        for room in &app.lan_rooms {
            let is_locked = room.requires_password();
            let room_clone = room.clone();
            let subtitle = format!(
                "Host: {}  •  {}/{} online  •  {}",
                room.host_nickname, room.peer_count, room.max_peers, room.host_addr
            );
            let mut title_row = row![text(&room.room_name).size(14)]
                .spacing(8)
                .align_y(Vertical::Center);
            if is_locked {
                title_row = title_row.push(
                    container(text("LOCKED").size(10).color(M3_WARNING))
                        .padding([2, 7])
                        .style(surface_card_style),
                );
            }
            let join_label = if is_locked {
                "Unlock & Join"
            } else {
                "Join LAN"
            };
            let lan_row = container(
                row![
                    avatar_circle(app.avatars.room_handle_for(&room.room_id), 32.0, false),
                    column![
                        title_row,
                        text(subtitle).size(11).color(M3_ON_SURFACE_VARIANT),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button(text(join_label).size(12))
                        .padding([8, 16])
                        .style(pill_primary_button)
                        .on_press(Message::JoinLanRoom(room_clone)),
                ]
                .spacing(10)
                .align_y(Vertical::Center),
            )
            .padding(10)
            .width(Length::Fill)
            .style(canvas_panel_style);
            lan_list = lan_list.push(lan_row);
        }
    }

    let lan_card = container(column![text("LAN Rooms Nearby").size(16), lan_list,].spacing(10))
        .width(Length::Fill)
        .padding(20)
        .style(surface_card_style);

    let mut main_col = column![
        row![
            column![
                text("Welcome Back").size(22),
                text("Ready to talk with your squad")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(2),
            horizontal_space(),
        ]
        .align_y(Vertical::Center),
        quick_code_card,
        create_card,
        join_card,
        lan_card,
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

    let left_workspace = container(scrollable(main_col).height(Length::Fill))
        .width(Length::FillPortion(6))
        .height(Length::Fill)
        .padding(20)
        .style(canvas_panel_style);

    let mut sorted_friend_indices: Vec<usize> = (0..app.friends.len()).collect();
    sorted_friend_indices.sort_by(|&a, &b| {
        let fa = &app.friends[a];
        let fb = &app.friends[b];
        let oa = app.friend_presences.contains_key(&fa.pub_key);
        let ob = app.friend_presences.contains_key(&fb.pub_key);
        ob.cmp(&oa)
            .then_with(|| fa.nickname.to_lowercase().cmp(&fb.nickname.to_lowercase()))
    });
    let online_friends_count = app
        .friends
        .iter()
        .filter(|f| app.friend_presences.contains_key(&f.pub_key))
        .count();

    let mut friends_list = column![].spacing(8).width(Length::Fill);
    if sorted_friend_indices.is_empty() {
        friends_list = friends_list.push(
            container(
                text("No friends added yet.\nClick \"+ Add\" to paste a friend's sqf_ ID, or click \"Copy My ID\" below to share yours.")
                    .size(12)
                    .color(M3_ON_SURFACE_VARIANT),
            )
            .padding(12)
            .width(Length::Fill)
            .style(canvas_panel_style),
        );
    } else {
        for idx in sorted_friend_indices {
            let friend = &app.friends[idx];
            let presence = app.friend_presences.get(&friend.pub_key);
            let online = presence.is_some();
            let display_name = presence
                .filter(|p| !p.nickname.trim().is_empty())
                .map(|p| p.nickname.as_str())
                .unwrap_or(friend.nickname.as_str());
            let (status_text, status_color) = match presence {
                Some(p) if p.in_room => ("In Voice Room", M3_WARNING),
                Some(_) => ("Online", M3_SUCCESS),
                None => ("Offline", M3_ON_SURFACE_VARIANT),
            };

            let mut call_btn = button(text("Call").size(12))
                .padding([7, 12])
                .style(if online {
                    pill_success_button
                } else {
                    pill_tonal_button
                });
            if online {
                call_btn = call_btn.on_press(Message::CallFriend(friend.pub_key));
            }

            let friend_row = container(
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
                        text(status_text).size(11).color(status_color),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    call_btn,
                    button(text("×").size(13))
                        .padding([7, 10])
                        .style(pill_danger_button)
                        .on_press(Message::RemoveFriend(friend.pub_key)),
                ]
                .spacing(8)
                .align_y(Vertical::Center),
            )
            .padding(10)
            .width(Length::Fill)
            .style(canvas_panel_style);

            friends_list = friends_list.push(friend_row);
        }
    }

    let copy_id_label = if app.friend_id_copied_flash {
        "Copied ID!"
    } else {
        "Copy My ID"
    };
    let friends_header = row![
        column![
            text("Friends").size(18).color(M3_ON_SURFACE),
            text(format!("{online_friends_count} online"))
                .size(11)
                .color(if online_friends_count > 0 {
                    M3_SUCCESS
                } else {
                    M3_ON_SURFACE_VARIANT
                }),
        ]
        .spacing(2),
        horizontal_space(),
        button(text("+ Add").size(12))
            .padding([8, 12])
            .style(pill_primary_button)
            .on_press(Message::OpenAddFriendModal),
    ]
    .align_y(Vertical::Center);

    let my_id_strip = container(
        row![
            column![
                text("My X25519 Friend ID").size(12).color(M3_ON_SURFACE),
                text("Zero-server E2EE contact key")
                    .size(10)
                    .color(M3_ON_SURFACE_VARIANT),
            ]
            .spacing(2)
            .width(Length::Fill),
            button(text(copy_id_label).size(12))
                .padding([8, 12])
                .style(if app.friend_id_copied_flash {
                    pill_success_button
                } else {
                    pill_tonal_button
                })
                .on_press(Message::CopyMyFriendId),
        ]
        .spacing(8)
        .align_y(Vertical::Center),
    )
    .padding(10)
    .width(Length::Fill)
    .style(canvas_panel_style);

    let friends_sidebar = container(
        column![
            friends_header,
            scrollable(friends_list).height(Length::Fill),
            my_id_strip,
        ]
        .spacing(12)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::FillPortion(4))
    .height(Length::Fill)
    .padding(16)
    .style(sidebar_panel_style);

    row![sidebar, left_workspace, friends_sidebar]
        .spacing(14)
        .padding(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
