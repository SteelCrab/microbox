use std::io::{self, Write, stdout};
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseButton as CrosstermMouseButton, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use xkeysym::{Keysym, key};

use crate::protocol::modifiers;
use crate::protocol::{InputEvent, KeyEvent, MouseButton, MouseEvent, MouseKind};

pub struct TerminalGuard {
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    pub fn enter(keyboard_enhanced: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        let enter_result = (|| {
            execute!(
                output,
                EnterAlternateScreen,
                EnableMouseCapture,
                Hide,
                Clear(ClearType::All),
                MoveTo(0, 0)
            )?;
            if keyboard_enhanced {
                execute!(
                    output,
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
                    )
                )?;
            }
            output.flush()
        })();

        if let Err(error) = enter_result {
            let _ = execute!(output, DisableMouseCapture, Show, LeaveAlternateScreen);
            let _ = output.flush();
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self { keyboard_enhanced })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut output = stdout();
        if self.keyboard_enhanced {
            let _ = queue!(output, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(output, DisableMouseCapture, Show, LeaveAlternateScreen);
        let _ = output.flush();
        let _ = disable_raw_mode();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalAction {
    Input(InputEvent),
    Resize { columns: u16, rows: u16 },
    Quit,
}

pub fn poll_action(timeout: Duration, keyboard_enhanced: bool) -> io::Result<Vec<TerminalAction>> {
    if !event::poll(timeout)? {
        return Ok(Vec::new());
    }
    Ok(decode_terminal_event(event::read()?, keyboard_enhanced))
}

pub fn decode_terminal_event(event: Event, keyboard_enhanced: bool) -> Vec<TerminalAction> {
    match event {
        Event::Key(key_event) => {
            if key_event.code == KeyCode::Char('c')
                && key_event.modifiers.contains(KeyModifiers::CONTROL)
                && key_event.kind != KeyEventKind::Release
            {
                return vec![TerminalAction::Quit];
            }
            let Some(keysym) = keycode_to_keysym(key_event.code) else {
                return Vec::new();
            };
            let modifiers = convert_modifiers(key_event.modifiers);
            let key = |pressed| {
                TerminalAction::Input(InputEvent::Key(KeyEvent {
                    text: match key_event.code {
                        KeyCode::Char(character) => Some(character.to_string()),
                        _ => None,
                    },
                    code: keysym,
                    pressed,
                    modifiers,
                }))
            };

            match (keyboard_enhanced, key_event.kind) {
                (_, KeyEventKind::Release) => vec![key(false)],
                (true, KeyEventKind::Press | KeyEventKind::Repeat) => vec![key(true)],
                (false, KeyEventKind::Press | KeyEventKind::Repeat) => {
                    vec![key(true), key(false)]
                }
            }
        }
        Event::Mouse(mouse_event) => {
            let modifiers = convert_modifiers(mouse_event.modifiers);
            let mouse = |kind, button| {
                TerminalAction::Input(InputEvent::Mouse(MouseEvent {
                    x: u32::from(mouse_event.column),
                    y: u32::from(mouse_event.row),
                    button,
                    kind,
                    modifiers,
                }))
            };
            match mouse_event.kind {
                MouseEventKind::Down(button) => {
                    vec![mouse(MouseKind::Press, Some(convert_button(button)))]
                }
                MouseEventKind::Up(button) => {
                    vec![mouse(MouseKind::Release, Some(convert_button(button)))]
                }
                MouseEventKind::Drag(button) => {
                    vec![mouse(MouseKind::Move, Some(convert_button(button)))]
                }
                MouseEventKind::Moved => vec![mouse(MouseKind::Move, None)],
                MouseEventKind::ScrollUp => vec![
                    mouse(MouseKind::Press, Some(MouseButton::WheelUp)),
                    mouse(MouseKind::Release, Some(MouseButton::WheelUp)),
                ],
                MouseEventKind::ScrollDown => vec![
                    mouse(MouseKind::Press, Some(MouseButton::WheelDown)),
                    mouse(MouseKind::Release, Some(MouseButton::WheelDown)),
                ],
                MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => Vec::new(),
            }
        }
        Event::Resize(columns, rows) => vec![TerminalAction::Resize { columns, rows }],
        _ => Vec::new(),
    }
}

fn convert_button(button: CrosstermMouseButton) -> MouseButton {
    match button {
        CrosstermMouseButton::Left => MouseButton::Left,
        CrosstermMouseButton::Middle => MouseButton::Middle,
        CrosstermMouseButton::Right => MouseButton::Right,
    }
}

fn convert_modifiers(value: KeyModifiers) -> u8 {
    let mut result = 0;
    if value.contains(KeyModifiers::SHIFT) {
        result |= modifiers::SHIFT;
    }
    if value.contains(KeyModifiers::CONTROL) {
        result |= modifiers::CONTROL;
    }
    if value.contains(KeyModifiers::ALT) {
        result |= modifiers::ALT;
    }
    if value.intersects(KeyModifiers::SUPER | KeyModifiers::META | KeyModifiers::HYPER) {
        result |= modifiers::SUPER;
    }
    result
}

fn keycode_to_keysym(code: KeyCode) -> Option<u32> {
    let raw = match code {
        KeyCode::Char(character) => Keysym::from_char(character).raw(),
        KeyCode::Backspace => key::BackSpace,
        KeyCode::Enter => key::Return,
        KeyCode::Left => key::Left,
        KeyCode::Right => key::Right,
        KeyCode::Up => key::Up,
        KeyCode::Down => key::Down,
        KeyCode::Home => key::Home,
        KeyCode::End => key::End,
        KeyCode::PageUp => key::Page_Up,
        KeyCode::PageDown => key::Page_Down,
        KeyCode::Tab => key::Tab,
        KeyCode::BackTab => key::ISO_Left_Tab,
        KeyCode::Delete => key::Delete,
        KeyCode::Insert => key::Insert,
        KeyCode::F(number @ 1..=35) => key::F1 + u32::from(number - 1),
        KeyCode::Esc => key::Escape,
        KeyCode::CapsLock => key::Caps_Lock,
        KeyCode::ScrollLock => key::Scroll_Lock,
        KeyCode::NumLock => key::Num_Lock,
        KeyCode::PrintScreen => key::Print,
        KeyCode::Pause => key::Pause,
        KeyCode::Menu => key::Menu,
        KeyCode::KeypadBegin => key::KP_Begin,
        KeyCode::Null | KeyCode::F(_) | KeyCode::Media(_) | KeyCode::Modifier(_) => return None,
    };
    Some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent as CrosstermKeyEvent, MouseEvent as CrosstermMouseEvent};

    #[test]
    fn fallback_key_press_synthesizes_release() {
        let actions = decode_terminal_event(
            Event::Key(CrosstermKeyEvent::new(
                KeyCode::Char('a'),
                KeyModifiers::NONE,
            )),
            false,
        );
        assert_eq!(actions.len(), 2);
        assert!(matches!(
            &actions[0],
            TerminalAction::Input(InputEvent::Key(KeyEvent { pressed: true, .. }))
        ));
        assert!(matches!(
            &actions[1],
            TerminalAction::Input(InputEvent::Key(KeyEvent { pressed: false, .. }))
        ));
    }

    #[test]
    fn control_c_is_reserved_for_session_exit() {
        let actions = decode_terminal_event(
            Event::Key(CrosstermKeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )),
            false,
        );
        assert_eq!(actions, vec![TerminalAction::Quit]);
    }

    #[test]
    fn mouse_coordinates_are_preserved_as_cells() {
        let actions = decode_terminal_event(
            Event::Mouse(CrosstermMouseEvent {
                kind: MouseEventKind::Down(CrosstermMouseButton::Left),
                column: 12,
                row: 7,
                modifiers: KeyModifiers::NONE,
            }),
            false,
        );
        assert!(matches!(
            &actions[0],
            TerminalAction::Input(InputEvent::Mouse(MouseEvent {
                x: 12,
                y: 7,
                kind: MouseKind::Press,
                button: Some(MouseButton::Left),
                ..
            }))
        ));
    }
}
