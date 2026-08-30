mod doctor;
mod input;
mod kitty;

pub use doctor::DoctorReport;
pub use input::{TerminalAction, TerminalGuard, poll_action};
pub use kitty::{DemoOptions, KittyEncoder, render_demo};
