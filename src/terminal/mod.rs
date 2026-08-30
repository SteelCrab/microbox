mod doctor;
mod input;
mod kitty;
mod renderer;

pub use doctor::DoctorReport;
pub use input::{TerminalAction, TerminalGuard, decode_terminal_event, poll_action};
pub use kitty::{DemoOptions, KittyEncoder, render_demo};
pub use renderer::{KittyFrameRenderer, RenderOutcome};
