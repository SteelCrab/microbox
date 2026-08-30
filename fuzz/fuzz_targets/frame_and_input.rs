#![no_main]

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use libfuzzer_sys::fuzz_target;
use microbox::renderer::{Frame, RenderPlanner};
use microbox::terminal::decode_terminal_event;

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let width = u16::from_le_bytes([data[0], data[1]]).clamp(1, 128) as u32;
    let height = u16::from_le_bytes([data[2], data[3]]).clamp(1, 128) as u32;
    let expected = Frame::rgb_buffer_len(width, height).unwrap();
    let mut pixels = vec![0; expected];
    for (target, source) in pixels.iter_mut().zip(data[4..].iter().cycle()) {
        *target = *source;
    }
    let frame = Frame::new_rgb(width, height, pixels).unwrap();
    let mut planner = RenderPlanner::new(16, 0.35);
    let _ = planner.plan(&frame);

    let code = match data[0] % 8 {
        0 => KeyCode::Char(char::from(data[1].max(32))),
        1 => KeyCode::Enter,
        2 => KeyCode::Esc,
        3 => KeyCode::Left,
        4 => KeyCode::Right,
        5 => KeyCode::Up,
        6 => KeyCode::Down,
        _ => KeyCode::F(data[1] % 40),
    };
    let _ = decode_terminal_event(
        Event::Key(KeyEvent::new(
            code,
            KeyModifiers::from_bits_truncate(data[2]),
        )),
        data[3] & 1 != 0,
    );
    let _ = decode_terminal_event(
        Event::Mouse(MouseEvent {
            kind: match data[0] % 4 {
                0 => MouseEventKind::Down(MouseButton::Left),
                1 => MouseEventKind::Up(MouseButton::Left),
                2 => MouseEventKind::Moved,
                _ => MouseEventKind::ScrollDown,
            },
            column: u16::from(data[1]),
            row: u16::from(data[2]),
            modifiers: KeyModifiers::from_bits_truncate(data[3]),
        }),
        false,
    );
    let _ = decode_terminal_event(
        Event::Paste(String::from_utf8_lossy(data).into_owned()),
        false,
    );
});
