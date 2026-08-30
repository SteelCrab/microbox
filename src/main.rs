use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use micro_gui::protocol::{InputEvent, ViewportMapping};
use micro_gui::runtime::{ApplicationSpec, NativeSession};
use micro_gui::terminal::{
    DemoOptions, DoctorReport, KittyEncoder, TerminalAction, TerminalGuard, poll_action,
    render_demo,
};

const HELP: &str = r#"micro-gui — GUI applications, without the desktop

Usage:
  micro-gui doctor
  micro-gui demo [--width PIXELS] [--height PIXELS]
  micro-gui run <APPLICATION> [--runtime native|firecrab] [-- ARGS...]
  micro-gui help

Commands:
  doctor  Inspect terminal capabilities needed by micro-gui
  demo    Render a generated RGB frame with the Kitty Graphics Protocol
  run     Run one application on a private Xvfb display (native runtime)
"#;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("micro-gui: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        print!("{HELP}");
        return Ok(());
    };

    match command {
        "doctor" => {
            if args.len() != 1 {
                return Err("doctor does not accept arguments".into());
            }
            print!("{}", DoctorReport::detect());
            Ok(())
        }
        "demo" => {
            let options = parse_demo_options(&args[1..])?;
            render_demo(std::io::stdout().lock(), options).map_err(|error| error.to_string())
        }
        "run" => run_application(parse_run_options(&args[1..])?),
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("micro-gui {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{HELP}")),
    }
}

fn parse_demo_options(args: &[String]) -> Result<DemoOptions, String> {
    let mut options = DemoOptions::default();
    let mut index = 0;

    while index < args.len() {
        let target = match args[index].as_str() {
            "--width" => &mut options.width,
            "--height" => &mut options.height,
            option => return Err(format!("unknown demo option '{option}'")),
        };
        index += 1;
        let value = args
            .get(index)
            .ok_or_else(|| format!("{} requires a value", args[index - 1]))?;
        *target = value
            .parse::<u32>()
            .map_err(|_| format!("'{value}' is not a valid pixel size"))?;
        index += 1;
    }

    options.validate()?;
    Ok(options)
}

#[derive(Debug, PartialEq, Eq)]
struct RunOptions {
    application: String,
    application_arguments: Vec<String>,
    runtime: String,
}

fn parse_run_options(args: &[String]) -> Result<RunOptions, String> {
    let Some(application) = args.first() else {
        return Err("run requires an application".into());
    };
    let mut runtime = "native";
    let mut application_arguments = Vec::new();
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--" {
            application_arguments.extend_from_slice(&args[index + 1..]);
            break;
        }
        if args[index] == "--runtime" {
            runtime = args
                .get(index + 1)
                .ok_or_else(|| "--runtime requires a value".to_string())?;
            if !matches!(runtime, "native" | "firecrab") {
                return Err(format!("unsupported runtime '{runtime}'"));
            }
            index += 2;
            continue;
        }
        return Err(format!(
            "unexpected argument '{}'; put application arguments after --",
            args[index]
        ));
    }

    Ok(RunOptions {
        application: application.clone(),
        application_arguments,
        runtime: runtime.into(),
    })
}

fn run_application(options: RunOptions) -> Result<(), String> {
    if options.runtime == "firecrab" {
        return Err("the firecrab runtime is planned after v0.1 and is not available yet".into());
    }
    if !std::io::stdout().is_terminal() {
        return Err("stdout is not a terminal; run `micro-gui doctor` for details".into());
    }

    let spec = ApplicationSpec::new(
        OsString::from(&options.application),
        options
            .application_arguments
            .into_iter()
            .map(OsString::from),
    );
    let mut session = NativeSession::start(&spec, 640, 360).map_err(|error| error.to_string())?;

    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .map_err(|error| format!("could not install Ctrl-C handler: {error}"))?;

    let keyboard_enhanced = DoctorReport::detect().kitty_graphics_likely;
    let terminal_guard = TerminalGuard::enter(keyboard_enhanced)
        .map_err(|error| format!("terminal setup failed: {error}"))?;
    let (reported_columns, reported_rows) = crossterm::terminal::size()
        .map_err(|error| format!("terminal size query failed: {error}"))?;
    let (mut columns, mut rows) = (reported_columns.max(1), reported_rows.max(1));
    let (display_width, display_height) = session.display_size();
    let mut mapping = ViewportMapping::new(
        columns,
        rows,
        u32::from(display_width),
        u32::from(display_height),
    )
    .expect("normalized terminal and display dimensions must be non-zero");

    let stdout = std::io::stdout();
    let mut encoder = KittyEncoder::new(stdout.lock());
    let frame_interval = Duration::from_millis(100);
    let mut next_frame = Instant::now();
    let render_result = (|| {
        'session: while running.load(Ordering::SeqCst)
            && session.is_running().map_err(|error| error.to_string())?
        {
            for action in poll_action(Duration::from_millis(5), keyboard_enhanced)
                .map_err(|error| format!("terminal input failed: {error}"))?
            {
                match action {
                    TerminalAction::Input(input) => {
                        let input = map_terminal_input(input, mapping);
                        session.inject(&input).map_err(|error| error.to_string())?;
                    }
                    TerminalAction::Resize {
                        columns: new_columns,
                        rows: new_rows,
                    } => {
                        let (new_columns, new_rows) = (new_columns.max(1), new_rows.max(1));
                        if let Some(new_mapping) = ViewportMapping::new(
                            new_columns,
                            new_rows,
                            u32::from(display_width),
                            u32::from(display_height),
                        ) {
                            columns = new_columns;
                            rows = new_rows;
                            mapping = new_mapping;
                            next_frame = Instant::now();
                        }
                    }
                    TerminalAction::Quit => break 'session,
                }
            }

            if Instant::now() >= next_frame {
                let frame = session.capture().map_err(|error| error.to_string())?;
                encoder
                    .transmit_rgb_placed(&frame, 1, columns, rows)
                    .map_err(|error| format!("terminal frame write failed: {error}"))?;
                next_frame = Instant::now() + frame_interval;
            } else {
                thread::yield_now();
            }
        }
        Ok(())
    })();
    let _ = encoder.delete(1);
    drop(encoder);
    drop(terminal_guard);
    render_result
}

fn map_terminal_input(input: InputEvent, mapping: ViewportMapping) -> InputEvent {
    match input {
        InputEvent::Mouse(mut mouse) => {
            (mouse.x, mouse.y) = mapping.map_cell(mouse.x as u16, mouse.y as u16);
            InputEvent::Mouse(mouse)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_demo_dimensions() {
        let options = parse_demo_options(&[
            "--width".into(),
            "640".into(),
            "--height".into(),
            "360".into(),
        ])
        .unwrap();
        assert_eq!(options.width, 640);
        assert_eq!(options.height, 360);
    }

    #[test]
    fn rejects_unknown_runtime() {
        let error =
            parse_run_options(&["xeyes".into(), "--runtime".into(), "docker".into()]).unwrap_err();
        assert!(error.contains("unsupported runtime"));
    }

    #[test]
    fn separates_application_arguments_after_double_dash() {
        let options = parse_run_options(&[
            "viewer".into(),
            "--runtime".into(),
            "native".into(),
            "--".into(),
            "a file.png".into(),
        ])
        .unwrap();
        assert_eq!(
            options,
            RunOptions {
                application: "viewer".into(),
                application_arguments: vec!["a file.png".into()],
                runtime: "native".into(),
            }
        );
    }
}
