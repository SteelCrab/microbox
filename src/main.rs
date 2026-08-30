use std::process::ExitCode;

use micro_gui::terminal::{DemoOptions, DoctorReport, render_demo};

const HELP: &str = r#"micro-gui — GUI applications, without the desktop

Usage:
  micro-gui doctor
  micro-gui demo [--width PIXELS] [--height PIXELS]
  micro-gui run <APPLICATION> [--runtime native|firecrab] [-- ARGS...]
  micro-gui help

Commands:
  doctor  Inspect terminal capabilities needed by micro-gui
  demo    Render a generated RGB frame with the Kitty Graphics Protocol
  run     Reserved for the v0.1 GUI runtime (not connected yet)
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
        "run" => parse_run(&args[1..]),
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

fn parse_run(args: &[String]) -> Result<(), String> {
    let Some(application) = args.first() else {
        return Err("run requires an application".into());
    };
    let mut runtime = "native";
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--" {
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

    Err(format!(
        "'{application}' was not started: the {runtime} display backend is not connected yet; use 'micro-gui demo' to verify terminal rendering"
    ))
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
        let error = parse_run(&["xeyes".into(), "--runtime".into(), "docker".into()]).unwrap_err();
        assert!(error.contains("unsupported runtime"));
    }
}
