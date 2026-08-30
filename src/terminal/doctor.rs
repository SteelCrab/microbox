use std::env;
use std::fmt::{self, Display, Formatter};
use std::io::{IsTerminal, stdout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub stdout_is_terminal: bool,
    pub term: Option<String>,
    pub terminal_program: Option<String>,
    pub kitty_graphics_likely: bool,
}

impl DoctorReport {
    pub fn detect() -> Self {
        let term = env::var("TERM").ok();
        let terminal_program = env::var("TERM_PROGRAM").ok();
        let kitty_graphics_likely = env::var_os("KITTY_WINDOW_ID").is_some()
            || term.as_deref() == Some("xterm-kitty")
            || terminal_program.as_deref().is_some_and(|value| {
                matches!(value.to_ascii_lowercase().as_str(), "ghostty" | "wezterm")
            });

        Self {
            stdout_is_terminal: stdout().is_terminal(),
            term,
            terminal_program,
            kitty_graphics_likely,
        }
    }
}

impl Display for DoctorReport {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "microbox doctor")?;
        writeln!(
            formatter,
            "  stdout is a TTY       {}",
            yes_no(self.stdout_is_terminal)
        )?;
        writeln!(
            formatter,
            "  TERM                  {}",
            self.term.as_deref().unwrap_or("(unset)")
        )?;
        writeln!(
            formatter,
            "  terminal program      {}",
            self.terminal_program.as_deref().unwrap_or("(unknown)")
        )?;
        writeln!(
            formatter,
            "  Kitty graphics likely {}",
            yes_no(self.kitty_graphics_likely)
        )?;

        if !self.stdout_is_terminal {
            writeln!(
                formatter,
                "  result                FAIL (stdout is redirected)"
            )
        } else if self.kitty_graphics_likely {
            writeln!(
                formatter,
                "  result                READY for `microbox demo`"
            )
        } else {
            writeln!(
                formatter,
                "  result                UNKNOWN (run `microbox demo` to probe visually)"
            )
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
