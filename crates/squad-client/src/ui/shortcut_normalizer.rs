use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::event::{self, Event};
use iced::{keyboard, mouse, window, Element, Length, Rectangle, Size, Vector};

/// Matches a logical ASCII letter on Latin layouts (preserving AZERTY/Dvorak),
/// falling back to `physical_key` on non-Latin keyboard layouts or unidentified keys.
pub fn matches_layout_agnostic_key(
    key: &keyboard::Key,
    physical_key: keyboard::key::Physical,
    ascii_lower: u8,
    code: keyboard::key::Code,
) -> bool {
    if let keyboard::Key::Character(ch) = key {
        let bytes = ch.as_bytes();
        if bytes.len() == 1 && bytes[0].is_ascii_alphabetic() {
            return bytes[0].to_ascii_lowercase() == ascii_lower;
        }
    }

    matches!(physical_key, keyboard::key::Physical::Code(c) if c == code)
}

fn map_command_shortcut_char(
    key: &keyboard::Key,
    physical_key: keyboard::key::Physical,
    modifiers: keyboard::Modifiers,
) -> Option<&'static str> {
    if !modifiers.command() || modifiers.alt() {
        return None;
    }

    if let keyboard::Key::Character(ch) = key {
        let bytes = ch.as_bytes();
        if bytes.len() == 1 && bytes[0].is_ascii_alphabetic() {
            return match bytes[0] {
                b'A' => Some("a"),
                b'C' => Some("c"),
                b'V' => Some("v"),
                b'X' => Some("x"),
                _ => None,
            };
        }
    }

    match physical_key {
        keyboard::key::Physical::Code(keyboard::key::Code::KeyA) => Some("a"),
        keyboard::key::Physical::Code(keyboard::key::Code::KeyC) => Some("c"),
        keyboard::key::Physical::Code(keyboard::key::Code::KeyV) => Some("v"),
        keyboard::key::Physical::Code(keyboard::key::Code::KeyX) => Some("x"),
        _ => None,
    }
}

#[derive(Default)]
struct ShortcutNormalizerState {
    remapped_paste_key: Option<keyboard::Key>,
}

fn keys_match_release(pressed: &keyboard::Key, released: &keyboard::Key) -> bool {
    if pressed == released {
        return true;
    }

    match (pressed, released) {
        (keyboard::Key::Character(a), keyboard::Key::Character(b)) => {
            a.to_lowercase() == b.to_lowercase()
        }
        _ => false,
    }
}

fn normalize_event(state: &mut ShortcutNormalizerState, event: Event) -> Event {
    match event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modified_key,
            physical_key,
            location,
            modifiers,
            text,
        }) => {
            if let Some(mapped) = map_command_shortcut_char(&key, physical_key, modifiers) {
                if mapped == "v" {
                    // iced's TextInput clears `is_pasting` only upon receiving KeyReleased("v"),
                    // and KeyReleased lacks `physical_key`, so remember the original logical key.
                    state.remapped_paste_key = Some(key);
                }
                let normalized = keyboard::Key::Character(mapped.into());
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: normalized.clone(),
                    modified_key: normalized,
                    physical_key,
                    location,
                    modifiers,
                    text: None,
                })
            } else {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    modified_key,
                    physical_key,
                    location,
                    modifiers,
                    text,
                })
            }
        }
        Event::Keyboard(keyboard::Event::KeyReleased {
            key,
            location,
            modifiers,
        }) => {
            if state
                .remapped_paste_key
                .as_ref()
                .is_some_and(|saved| keys_match_release(saved, &key))
            {
                state.remapped_paste_key = None;
                Event::Keyboard(keyboard::Event::KeyReleased {
                    key: keyboard::Key::Character("v".into()),
                    location,
                    modifiers,
                })
            } else {
                if matches!(key.as_ref(), keyboard::Key::Character("v")) {
                    state.remapped_paste_key = None;
                }
                Event::Keyboard(keyboard::Event::KeyReleased {
                    key,
                    location,
                    modifiers,
                })
            }
        }
        Event::Window(window::Event::Unfocused) => {
            state.remapped_paste_key = None;
            Event::Window(window::Event::Unfocused)
        }
        other => other,
    }
}

/// Transparent wrapper widget that normalizes clipboard/selection command shortcuts
/// (`Cmd/Ctrl + A/C/V/X`) across non-English keyboard layouts and Caps Lock.
pub struct ShortcutNormalizer<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

pub fn shortcut_normalizer<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    ShortcutNormalizer {
        content: content.into(),
    }
    .into()
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ShortcutNormalizer<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ShortcutNormalizerState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ShortcutNormalizerState::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let state = tree.state.downcast_mut::<ShortcutNormalizerState>();
        let normalized = normalize_event(state, event);
        self.content.as_widget_mut().on_event(
            &mut tree.children[0],
            normalized,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<ShortcutNormalizer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(wrapper: ShortcutNormalizer<'a, Message, Theme, Renderer>) -> Self {
        Element::new(wrapper)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{
        key::{Code, Physical},
        Location, Modifiers,
    };

    fn cmd_modifier() -> Modifiers {
        #[cfg(target_os = "macos")]
        {
            Modifiers::LOGO
        }
        #[cfg(not(target_os = "macos"))]
        {
            Modifiers::CTRL
        }
    }

    fn key_pressed(
        key: keyboard::Key,
        code: Code,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: Physical::Code(code),
            location: Location::Standard,
            modifiers,
            text: text.map(Into::into),
        })
    }

    fn key_released(key: keyboard::Key, modifiers: Modifiers) -> Event {
        Event::Keyboard(keyboard::Event::KeyReleased {
            key,
            location: Location::Standard,
            modifiers,
        })
    }

    #[test]
    fn russian_paste_press_and_release_normalizes_to_v() {
        let mut state = ShortcutNormalizerState::default();
        let mods = cmd_modifier();

        let pressed = normalize_event(
            &mut state,
            key_pressed(
                keyboard::Key::Character("м".into()),
                Code::KeyV,
                mods,
                Some("м"),
            ),
        );
        match pressed {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                text,
                ..
            }) => {
                assert_eq!(key, keyboard::Key::Character("v".into()));
                assert_eq!(modified_key, keyboard::Key::Character("v".into()));
                assert_eq!(text, None);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let released = normalize_event(
            &mut state,
            key_released(keyboard::Key::Character("м".into()), mods),
        );
        match released {
            Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) => {
                assert_eq!(key, keyboard::Key::Character("v".into()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(state.remapped_paste_key.is_none());
    }

    #[test]
    fn russian_copy_cut_select_all_normalize() {
        let mut state = ShortcutNormalizerState::default();
        let mods = cmd_modifier();

        for (ch, code, expected) in [
            ("с", Code::KeyC, "c"),
            ("ч", Code::KeyX, "x"),
            ("ф", Code::KeyA, "a"),
        ] {
            let out = normalize_event(
                &mut state,
                key_pressed(keyboard::Key::Character(ch.into()), code, mods, Some(ch)),
            );
            match out {
                Event::Keyboard(keyboard::Event::KeyPressed { key, text, .. }) => {
                    assert_eq!(key, keyboard::Key::Character(expected.into()));
                    assert_eq!(text, None);
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    #[test]
    fn non_latin_and_unidentified_paste_keys_normalize() {
        let mods = cmd_modifier();
        for key in [
            keyboard::Key::Character("ω".into()),
            keyboard::Key::Character("ر".into()),
            keyboard::Key::Character("ה".into()),
            keyboard::Key::Character("ㅍ".into()),
            keyboard::Key::Unidentified,
        ] {
            let mut state = ShortcutNormalizerState::default();
            let out = normalize_event(&mut state, key_pressed(key.clone(), Code::KeyV, mods, None));
            match out {
                Event::Keyboard(keyboard::Event::KeyPressed { key: mapped, .. }) => {
                    assert_eq!(mapped, keyboard::Key::Character("v".into()));
                }
                other => panic!("unexpected event: {other:?}"),
            }
            let rel = normalize_event(&mut state, key_released(key, mods));
            match rel {
                Event::Keyboard(keyboard::Event::KeyReleased { key: mapped, .. }) => {
                    assert_eq!(mapped, keyboard::Key::Character("v".into()));
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    #[test]
    fn english_caps_lock_paste_normalizes_press_and_release() {
        let mut state = ShortcutNormalizerState::default();
        let mods = cmd_modifier();

        let pressed = normalize_event(
            &mut state,
            key_pressed(
                keyboard::Key::Character("V".into()),
                Code::KeyV,
                mods,
                Some("V"),
            ),
        );
        match pressed {
            Event::Keyboard(keyboard::Event::KeyPressed { key, text, .. }) => {
                assert_eq!(key, keyboard::Key::Character("v".into()));
                assert_eq!(text, None);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let released = normalize_event(
            &mut state,
            key_released(keyboard::Key::Character("V".into()), mods),
        );
        match released {
            Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) => {
                assert_eq!(key, keyboard::Key::Character("v".into()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn latin_non_qwerty_layouts_preserve_logical_bindings() {
        let mut state = ShortcutNormalizerState::default();
        let mods = cmd_modifier();

        // AZERTY physical KeyA produces logical "q"
        let azerty = normalize_event(
            &mut state,
            key_pressed(
                keyboard::Key::Character("q".into()),
                Code::KeyA,
                mods,
                Some("q"),
            ),
        );
        match azerty {
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                assert_eq!(key, keyboard::Key::Character("q".into()));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // Dvorak physical KeyV produces logical "k"
        let dvorak = normalize_event(
            &mut state,
            key_pressed(
                keyboard::Key::Character("k".into()),
                Code::KeyV,
                mods,
                Some("k"),
            ),
        );
        match dvorak {
            Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => {
                assert_eq!(key, keyboard::Key::Character("k".into()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn alt_gr_diacritics_are_not_remapped() {
        let mut state = ShortcutNormalizerState::default();
        let mods = cmd_modifier() | Modifiers::ALT;

        let out = normalize_event(
            &mut state,
            key_pressed(
                keyboard::Key::Character("ą".into()),
                Code::KeyA,
                mods,
                Some("ą"),
            ),
        );
        match out {
            Event::Keyboard(keyboard::Event::KeyPressed { key, text, .. }) => {
                assert_eq!(key, keyboard::Key::Character("ą".into()));
                assert_eq!(text.as_deref(), Some("ą"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn plain_typing_without_command_is_untouched() {
        let mut state = ShortcutNormalizerState::default();

        let out = normalize_event(
            &mut state,
            key_pressed(
                keyboard::Key::Character("м".into()),
                Code::KeyV,
                Modifiers::default(),
                Some("м"),
            ),
        );
        match out {
            Event::Keyboard(keyboard::Event::KeyPressed { key, text, .. }) => {
                assert_eq!(key, keyboard::Key::Character("м".into()));
                assert_eq!(text.as_deref(), Some("м"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn layout_agnostic_matcher_respects_latin_and_falls_back_for_non_latin() {
        assert!(matches_layout_agnostic_key(
            &keyboard::Key::Character("v".into()),
            Physical::Code(Code::KeyV),
            b'v',
            Code::KeyV,
        ));
        assert!(matches_layout_agnostic_key(
            &keyboard::Key::Character("V".into()),
            Physical::Code(Code::KeyV),
            b'v',
            Code::KeyV,
        ));
        assert!(matches_layout_agnostic_key(
            &keyboard::Key::Character("м".into()),
            Physical::Code(Code::KeyV),
            b'v',
            Code::KeyV,
        ));
        assert!(!matches_layout_agnostic_key(
            &keyboard::Key::Character("k".into()),
            Physical::Code(Code::KeyV),
            b'v',
            Code::KeyV,
        ));
        assert!(!matches_layout_agnostic_key(
            &keyboard::Key::Character("q".into()),
            Physical::Code(Code::KeyA),
            b'a',
            Code::KeyA,
        ));
    }
}
