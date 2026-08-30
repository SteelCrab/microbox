#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    Text(String),
    Mouse(MouseEvent),
    Resize { width: u16, height: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub text: Option<String>,
    pub code: u32,
    pub pressed: bool,
    pub modifiers: u8,
}

pub mod modifiers {
    pub const SHIFT: u8 = 1 << 0;
    pub const CONTROL: u8 = 1 << 1;
    pub const ALT: u8 = 1 << 2;
    pub const SUPER: u8 = 1 << 3;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub x: u32,
    pub y: u32,
    pub button: Option<MouseButton>,
    pub kind: MouseKind,
    pub modifiers: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportMapping {
    terminal_columns: u16,
    terminal_rows: u16,
    frame_width: u32,
    frame_height: u32,
}

impl ViewportMapping {
    pub fn new(
        terminal_columns: u16,
        terminal_rows: u16,
        frame_width: u32,
        frame_height: u32,
    ) -> Option<Self> {
        if terminal_columns == 0 || terminal_rows == 0 || frame_width == 0 || frame_height == 0 {
            return None;
        }
        Some(Self {
            terminal_columns,
            terminal_rows,
            frame_width,
            frame_height,
        })
    }

    pub fn map_cell(self, column: u16, row: u16) -> (u32, u32) {
        let column = column.min(self.terminal_columns - 1) as u64;
        let row = row.min(self.terminal_rows - 1) as u64;
        let x = column * self.frame_width as u64 / self.terminal_columns as u64;
        let y = row * self.frame_height as u64 / self.terminal_rows as u64;
        (x as u32, y as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_clamps_terminal_cells() {
        let mapping = ViewportMapping::new(80, 24, 800, 600).unwrap();
        assert_eq!(mapping.map_cell(40, 12), (400, 300));
        assert_eq!(mapping.map_cell(100, 30), (790, 575));
    }
}
