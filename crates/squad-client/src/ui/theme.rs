use iced::{
    overlay::menu,
    theme::Palette,
    widget::{button, container, pick_list, slider, text_input},
    Background, Border, Color, Shadow, Theme, Vector,
};

const fn rgb_u8(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

// Discord-inspired dark slate palette paired with Material 3 surface hierarchy.
pub const M3_BACKGROUND: Color = rgb_u8(0x1E, 0x1F, 0x22);
pub const M3_SURFACE_CONTAINER: Color = rgb_u8(0x2B, 0x2D, 0x31);
pub const M3_SURFACE_HIGH: Color = rgb_u8(0x31, 0x33, 0x38);
pub const M3_SURFACE_HIGHEST: Color = rgb_u8(0x38, 0x3A, 0x40);
pub const M3_PRIMARY: Color = rgb_u8(0x58, 0x65, 0xF2);
pub const M3_ON_PRIMARY: Color = rgb_u8(0xFF, 0xFF, 0xFF);
pub const M3_PRIMARY_CONTAINER: Color = rgb_u8(0x47, 0x52, 0xC4);
pub const M3_ON_PRIMARY_CONTAINER: Color = rgb_u8(0xE0, 0xE3, 0xFF);
pub const M3_SECONDARY_CONTAINER: Color = rgb_u8(0x3F, 0x41, 0x47);
pub const M3_ON_SECONDARY_CONTAINER: Color = rgb_u8(0xDB, 0xDE, 0xE1);
pub const M3_ON_SURFACE: Color = rgb_u8(0xF2, 0xF3, 0xF5);
pub const M3_ON_SURFACE_VARIANT: Color = rgb_u8(0x94, 0x9B, 0xA4);
pub const M3_OUTLINE: Color = rgb_u8(0x5C, 0x61, 0x6B);
pub const M3_OUTLINE_VARIANT: Color = rgb_u8(0x3F, 0x41, 0x47);
pub const M3_ERROR: Color = rgb_u8(0xF2, 0x3F, 0x43);
pub const M3_DANGER_CONTAINER: Color = rgb_u8(0x4A, 0x1D, 0x21);
pub const M3_SUCCESS: Color = rgb_u8(0x23, 0xA5, 0x5A);
pub const M3_WARNING: Color = rgb_u8(0xF0, 0xB2, 0x32);

pub fn material_dark_theme() -> Theme {
    Theme::custom(
        "SquadAir Discord M3".to_string(),
        Palette {
            background: M3_BACKGROUND,
            text: M3_ON_SURFACE,
            primary: M3_PRIMARY,
            success: M3_SUCCESS,
            danger: M3_ERROR,
        },
    )
}

pub fn root_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_BACKGROUND)),
        text_color: Some(M3_ON_SURFACE),
        ..Default::default()
    }
}

pub fn sidebar_panel_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 14.0.into(),
        },
        ..Default::default()
    }
}

pub fn surface_card_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 14.0.into(),
        },
        ..Default::default()
    }
}

pub fn canvas_panel_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_SURFACE_HIGH)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 14.0.into(),
        },
        ..Default::default()
    }
}

pub fn chat_bubble_style(is_self: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(if is_self {
            rgb_u8(0x2A, 0x2F, 0x45)
        } else {
            M3_SURFACE_CONTAINER
        })),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: if is_self {
                rgb_u8(0x47, 0x52, 0xC4)
            } else {
                M3_OUTLINE_VARIANT
            },
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

pub fn attachment_card_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_BACKGROUND)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

pub fn floating_dock_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 16.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 14.0,
        },
    }
}

pub fn peer_card_style(is_speaking: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        text_color: Some(M3_ON_SURFACE),
        border: Border {
            color: if is_speaking {
                M3_SUCCESS
            } else {
                M3_OUTLINE_VARIANT
            },
            width: if is_speaking { 2.0 } else { 1.0 },
            radius: 14.0.into(),
        },
        ..Default::default()
    }
}

pub fn pill_primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => M3_PRIMARY_CONTAINER,
        button::Status::Disabled => M3_SURFACE_HIGHEST,
        button::Status::Active => M3_PRIMARY,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: M3_ON_PRIMARY,
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn pill_tonal_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => rgb_u8(0x4E, 0x50, 0x58),
        button::Status::Disabled => M3_SURFACE_CONTAINER,
        button::Status::Active => M3_SECONDARY_CONTAINER,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: M3_ON_SECONDARY_CONTAINER,
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn pill_danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => M3_ERROR,
        button::Status::Disabled => M3_SURFACE_HIGHEST,
        button::Status::Active => M3_DANGER_CONTAINER,
    };
    let fg = match status {
        button::Status::Hovered | button::Status::Pressed => M3_ON_PRIMARY,
        _ => M3_ERROR,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn pill_success_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => rgb_u8(0x1E, 0x8E, 0x4E),
        _ => M3_SUCCESS,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: M3_ON_PRIMARY,
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn channel_nav_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| {
        let (bg, fg) = if active {
            (M3_SURFACE_HIGHEST, M3_ON_SURFACE)
        } else {
            let hover_bg = match status {
                button::Status::Hovered | button::Status::Pressed => M3_SURFACE_HIGH,
                _ => Color::TRANSPARENT,
            };
            (hover_bg, M3_ON_SURFACE_VARIANT)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

pub fn pill_chip_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| {
        let (bg, fg, border_color) = if active {
            (M3_PRIMARY_CONTAINER, M3_ON_PRIMARY_CONTAINER, M3_PRIMARY)
        } else {
            let hover_bg = match status {
                button::Status::Hovered | button::Status::Pressed => M3_SURFACE_HIGHEST,
                _ => M3_SURFACE_HIGH,
            };
            (hover_bg, M3_ON_SURFACE_VARIANT, M3_OUTLINE_VARIANT)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: border_color,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    }
}

pub fn m3_text_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused => M3_PRIMARY,
        text_input::Status::Hovered => M3_OUTLINE,
        _ => M3_OUTLINE_VARIANT,
    };
    text_input::Style {
        background: Background::Color(M3_BACKGROUND),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 10.0.into(),
        },
        icon: M3_ON_SURFACE_VARIANT,
        placeholder: M3_OUTLINE,
        value: M3_ON_SURFACE,
        selection: M3_PRIMARY_CONTAINER,
    }
}

pub fn m3_slider_style(_theme: &Theme, _status: slider::Status) -> slider::Style {
    slider::Style {
        rail: slider::Rail {
            backgrounds: (
                Background::Color(M3_PRIMARY),
                Background::Color(M3_SURFACE_HIGHEST),
            ),
            width: 4.0,
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 7.0 },
            background: Background::Color(M3_ON_SURFACE),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

pub fn m3_pick_list_style(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let border_color = match status {
        pick_list::Status::Hovered | pick_list::Status::Opened => M3_PRIMARY,
        _ => M3_OUTLINE_VARIANT,
    };
    pick_list::Style {
        text_color: M3_ON_SURFACE,
        placeholder_color: M3_ON_SURFACE_VARIANT,
        handle_color: M3_ON_SURFACE_VARIANT,
        background: Background::Color(M3_BACKGROUND),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 10.0.into(),
        },
    }
}

pub fn m3_menu_style(_theme: &Theme) -> menu::Style {
    menu::Style {
        background: Background::Color(M3_SURFACE_CONTAINER),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 10.0.into(),
        },
        text_color: M3_ON_SURFACE,
        selected_text_color: M3_ON_PRIMARY,
        selected_background: Background::Color(M3_PRIMARY),
    }
}
