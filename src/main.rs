use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use microbox::protocol::{DEFAULT_AGENT_PORT, InputEvent, ViewportMapping};
use microbox::renderer::Frame;
use microbox::runtime::{
    AgentConfig, ApplicationSpec, FirecrabSession, NativeSession, OciApplicationSpec, OciSession,
    run_agent,
};
use microbox::session::SessionRegistry;
use microbox::terminal::{
    DemoOptions, DoctorReport, KittyFrameRenderer, RenderOutcome, TerminalAction, TerminalGuard,
    poll_action, render_demo,
};

const MAX_FRAMEBUFFER_DIMENSION: u16 = 4096;
const FALLBACK_CELL_WIDTH: u16 = 8;
const FALLBACK_CELL_HEIGHT: u16 = 16;

const HELP: &str = r#"microbox — GUI applications, without the desktop

Usage:
  microbox doctor
  microbox demo [--width PIXELS] [--height PIXELS]
  microbox run <APPLICATION|OCI_IMAGE> [--runtime native|oci|oci-agent|firecrab] [--fps 1..60] [--stats] [--debug] [-- ARGS...]
  microbox agent <APPLICATION> [--listen ADDRESS] [--fps 1..60] [-- ARGS...]
  microbox ps
  microbox stop <SESSION_ID>
  microbox help

Commands:
  doctor  Inspect terminal capabilities needed by microbox
  demo    Render a generated RGB frame with the Kitty Graphics Protocol
  run     Run one application through a platform-supported GUI runtime
  agent   Serve a GUI application from a Linux guest to a host client
  ps      List running microbox sessions
  stop    Gracefully stop a running session
"#;

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("microbox: {message}");
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
        "agent" => run_agent_command(&args[1..]),
        "ps" => list_sessions(&args[1..]),
        "stop" => stop_session(&args[1..]),
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("microbox {}", env!("CARGO_PKG_VERSION"));
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
    fps: u16,
    stats: bool,
    debug: bool,
    firecrab_endpoint: Option<String>,
}

fn parse_run_options(args: &[String]) -> Result<RunOptions, String> {
    let Some(application) = args.first() else {
        return Err("run requires an application".into());
    };
    let mut runtime = "native";
    let mut runtime_explicit = false;
    let mut fps = 30;
    let mut stats = false;
    let mut debug = false;
    let mut application_arguments = Vec::new();
    let mut firecrab_endpoint = None;
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
            if !matches!(
                runtime,
                "native" | "oci" | "docker" | "oci-agent" | "firecrab"
            ) {
                return Err(format!("unsupported runtime '{runtime}'"));
            }
            runtime_explicit = true;
            index += 2;
            continue;
        }
        if args[index] == "--fps" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| "--fps requires a value".to_string())?;
            fps = value
                .parse::<u16>()
                .map_err(|_| format!("'{value}' is not a valid frame rate"))?;
            if !(1..=60).contains(&fps) {
                return Err("--fps must be between 1 and 60".into());
            }
            index += 2;
            continue;
        }
        if args[index] == "--firecrab-endpoint" {
            firecrab_endpoint = Some(
                args.get(index + 1)
                    .ok_or_else(|| "--firecrab-endpoint requires a value".to_string())?
                    .clone(),
            );
            index += 2;
            continue;
        }
        if args[index] == "--stats" {
            stats = true;
            index += 1;
            continue;
        }
        if args[index] == "--debug" {
            debug = true;
            index += 1;
            continue;
        }
        return Err(format!(
            "unexpected argument '{}'; put application arguments after --",
            args[index]
        ));
    }

    if !runtime_explicit && looks_like_oci_reference(application) {
        runtime = "oci";
    }

    Ok(RunOptions {
        application: application.clone(),
        application_arguments,
        runtime: runtime.into(),
        fps,
        stats,
        debug,
        firecrab_endpoint,
    })
}

fn run_application(options: RunOptions) -> Result<(), String> {
    if !std::io::stdout().is_terminal() {
        return Err("stdout is not a terminal; run `microbox doctor` for details".into());
    }
    if cfg!(target_os = "macos") && options.runtime == "native" {
        return Err(
            "native Linux GUI execution is unavailable on macOS; use an agent-enabled OCI image with `--runtime oci` or use `--runtime firecrab`".into(),
        );
    }

    let debug = options.debug || std::env::var_os("MICROBOX_DEBUG").is_some();
    let debug_started = Instant::now();
    let mut debug_events = Vec::new();
    let initial_geometry = terminal_geometry(None)?;
    if debug {
        eprintln!(
            "microbox debug: host={}/{} runtime={} app={:?} fps={} terminal={}x{} framebuffer={}x{}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            options.runtime,
            options.application,
            options.fps,
            initial_geometry.columns,
            initial_geometry.rows,
            initial_geometry.pixel_width,
            initial_geometry.pixel_height
        );
    }
    let session_application = options.application.clone();
    let session_runtime = match options.runtime.as_str() {
        "docker" => "oci",
        runtime => runtime,
    }
    .to_string();
    let arguments: Vec<OsString> = options
        .application_arguments
        .into_iter()
        .map(OsString::from)
        .collect();
    let mut session = match options.runtime.as_str() {
        "native" => {
            let spec = ApplicationSpec::new(OsString::from(&options.application), arguments);
            GuiSession::Native(Box::new(
                NativeSession::start(
                    &spec,
                    initial_geometry.pixel_width,
                    initial_geometry.pixel_height,
                )
                .map_err(|error| error.to_string())?,
            ))
        }
        "oci" | "docker" | "oci-agent" => {
            let spec = OciApplicationSpec::new(options.application, arguments);
            let start = if options.runtime == "oci-agent" {
                OciSession::start_agent
            } else {
                OciSession::start
            };
            GuiSession::Oci(
                start(
                    &spec,
                    initial_geometry.pixel_width,
                    initial_geometry.pixel_height,
                )
                .map_err(|error| error.to_string())?,
            )
        }
        "firecrab" => {
            let endpoint = options
                .firecrab_endpoint
                .or_else(|| std::env::var("MICROBOX_FIRECRAB_ENDPOINT").ok())
                .ok_or_else(|| {
                    "Firecrab runtime requires --firecrab-endpoint HOST:PORT or MICROBOX_FIRECRAB_ENDPOINT"
                        .to_string()
                })?;
            let token = std::env::var("MICROBOX_AGENT_TOKEN")
                .map_err(|_| "Firecrab runtime requires MICROBOX_AGENT_TOKEN".to_string())?;
            let session = FirecrabSession::connect(
                &endpoint,
                &token,
                initial_geometry.pixel_width,
                initial_geometry.pixel_height,
            )
            .map_err(|error| error.to_string())?;
            GuiSession::Firecrab(session)
        }
        runtime => return Err(format!("unsupported runtime '{runtime}'")),
    };
    record_debug(
        debug,
        debug_started,
        &mut debug_events,
        format!(
            "backend ready display={}x{}",
            session.display_size().0,
            session.display_size().1
        ),
    );

    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .map_err(|error| format!("could not install Ctrl-C handler: {error}"))?;
    let registry = SessionRegistry::discover().map_err(|error| error.to_string())?;
    let registration = registry
        .register(session_application, session_runtime)
        .map_err(|error| format!("could not register session: {error}"))?;
    record_debug(
        debug,
        debug_started,
        &mut debug_events,
        format!(
            "session registered id={} pid={} runtime={}",
            registration.record().id,
            registration.record().pid,
            registration.record().runtime
        ),
    );

    let keyboard_enhanced = DoctorReport::detect().kitty_graphics_likely;
    let terminal_guard = TerminalGuard::enter(keyboard_enhanced)
        .map_err(|error| format!("terminal setup failed: {error}"))?;
    let (mut columns, mut rows) = (initial_geometry.columns, initial_geometry.rows);
    let (mut display_width, mut display_height) = session.display_size();
    let mut mapping = ViewportMapping::new(
        columns,
        rows,
        u32::from(display_width),
        u32::from(display_height),
    )
    .expect("normalized terminal and display dimensions must be non-zero");

    let stdout = std::io::stdout();
    let mut renderer = KittyFrameRenderer::new(stdout.lock(), columns, rows);
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let mut next_frame = Instant::now();
    let mut force_capture = true;
    let mut stats = RenderStats::default();
    let render_result = (|| {
        'session: while running.load(Ordering::SeqCst)
            && session.is_running().map_err(|error| error.to_string())?
        {
            let input_wait = next_frame
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20));
            for action in poll_action(input_wait, keyboard_enhanced)
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
                        let geometry = terminal_geometry(Some((new_columns, new_rows)))?;
                        session
                            .resize(geometry.pixel_width, geometry.pixel_height)
                            .map_err(|error| error.to_string())?;
                        (display_width, display_height) = session.display_size();
                        record_debug(
                            debug,
                            debug_started,
                            &mut debug_events,
                            format!(
                                "resize cells={}x{} framebuffer={}x{}",
                                geometry.columns, geometry.rows, display_width, display_height
                            ),
                        );
                        if let Some(new_mapping) = ViewportMapping::new(
                            geometry.columns,
                            geometry.rows,
                            u32::from(display_width),
                            u32::from(display_height),
                        ) {
                            columns = geometry.columns;
                            rows = geometry.rows;
                            mapping = new_mapping;
                            renderer.resize(columns, rows);
                            force_capture = true;
                            next_frame = Instant::now();
                        }
                    }
                    TerminalAction::Quit => break 'session,
                }
            }

            if Instant::now() >= next_frame {
                stats.polls += 1;
                let damaged = session.frame_pending().map_err(|error| error.to_string())?;
                let frame_pending = force_capture || damaged;
                if frame_pending {
                    let frame = session.capture().map_err(|error| error.to_string())?;
                    let outcome = renderer
                        .render(&frame)
                        .map_err(|error| format!("terminal frame write failed: {error}"))?;
                    stats.observe(outcome);
                    force_capture = false;
                } else {
                    stats.skipped += 1;
                }
                next_frame = Instant::now() + frame_interval;
            } else {
                thread::yield_now();
            }
        }
        Ok(())
    })();
    let _ = renderer.clear();
    drop(renderer);
    drop(terminal_guard);
    record_debug(
        debug,
        debug_started,
        &mut debug_events,
        format!(
            "session finished status={}",
            if render_result.is_ok() { "ok" } else { "error" }
        ),
    );
    if debug {
        for event in &debug_events {
            eprintln!("microbox debug: {event}");
        }
    }
    if options.stats || debug {
        eprintln!("{stats}");
    }
    render_result
}

fn record_debug(enabled: bool, started: Instant, events: &mut Vec<String>, message: String) {
    if enabled && events.len() < 1024 {
        events.push(format!("+{:>6}ms {message}", started.elapsed().as_millis()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalGeometry {
    columns: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
}

fn terminal_geometry(cell_override: Option<(u16, u16)>) -> Result<TerminalGeometry, String> {
    let window = crossterm::terminal::window_size()
        .map_err(|error| format!("terminal size query failed: {error}"))?;
    let (columns, rows) = cell_override.unwrap_or((window.columns, window.rows));
    Ok(geometry_from_measurements(
        columns,
        rows,
        window.width,
        window.height,
    ))
}

fn geometry_from_measurements(
    columns: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
) -> TerminalGeometry {
    let columns = columns.max(1);
    let rows = rows.max(1);
    let pixel_width = if pixel_width == 0 {
        columns.saturating_mul(FALLBACK_CELL_WIDTH)
    } else {
        pixel_width
    };
    let pixel_height = if pixel_height == 0 {
        rows.saturating_mul(FALLBACK_CELL_HEIGHT)
    } else {
        pixel_height
    };
    TerminalGeometry {
        columns,
        rows,
        pixel_width: pixel_width.clamp(1, MAX_FRAMEBUFFER_DIMENSION),
        pixel_height: pixel_height.clamp(1, MAX_FRAMEBUFFER_DIMENSION),
    }
}

fn run_agent_command(args: &[String]) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err(
            "the GUI agent runs inside a Linux container or VM; start it in the guest and use the macOS host client to connect".into(),
        );
    }
    let Some(application) = args.first() else {
        return Err("agent requires an application".into());
    };
    let mut listen = format!("0.0.0.0:{DEFAULT_AGENT_PORT}");
    let mut fps = 30;
    let mut arguments = Vec::new();
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--" {
            arguments.extend(args[index + 1..].iter().cloned().map(OsString::from));
            break;
        }
        let numeric = match args[index].as_str() {
            "--fps" => Some(&mut fps),
            _ => None,
        };
        if let Some(target) = numeric {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("{} requires a value", args[index]))?;
            *target = value
                .parse::<u16>()
                .map_err(|_| format!("'{value}' is not a valid number"))?;
            index += 2;
            continue;
        }
        if args[index] == "--listen" {
            listen = args
                .get(index + 1)
                .ok_or_else(|| "--listen requires a value".to_string())?
                .clone();
            index += 2;
            continue;
        }
        return Err(format!("unknown agent option '{}'", args[index]));
    }
    if !(1..=60).contains(&fps) {
        return Err("agent FPS must be between 1 and 60".into());
    }
    let token = std::env::var("MICROBOX_AGENT_TOKEN")
        .map_err(|_| "agent requires MICROBOX_AGENT_TOKEN".to_string())?;
    run_agent(AgentConfig {
        listen,
        token,
        application: OsString::from(application),
        arguments,
        fps,
    })
    .map_err(|error| error.to_string())
}

fn list_sessions(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("ps does not accept arguments".into());
    }
    let registry = SessionRegistry::discover().map_err(|error| error.to_string())?;
    let records = registry.list().map_err(|error| error.to_string())?;
    println!("{:<18} {:<10} {:<8} APP", "ID", "RUNTIME", "PID");
    for record in records {
        println!(
            "{:<18} {:<10} {:<8} {}",
            record.id,
            clean_table_cell(&record.runtime),
            record.pid,
            clean_table_cell(&record.application)
        );
    }
    Ok(())
}

fn stop_session(args: &[String]) -> Result<(), String> {
    let [id] = args else {
        return Err("stop requires exactly one session id".into());
    };
    let registry = SessionRegistry::discover().map_err(|error| error.to_string())?;
    let record = registry.stop(id).map_err(|error| error.to_string())?;
    println!("stop requested for {} ({})", record.id, record.application);
    Ok(())
}

fn clean_table_cell(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn looks_like_oci_reference(application: &str) -> bool {
    if application.starts_with('/')
        || application.starts_with("./")
        || application.starts_with("../")
    {
        return false;
    }
    application.contains('/') || application.contains(':') || application.contains('@')
}

enum GuiSession {
    Native(Box<NativeSession>),
    Oci(OciSession),
    Firecrab(FirecrabSession),
}

impl GuiSession {
    fn capture(&mut self) -> Result<Frame, String> {
        match self {
            Self::Native(session) => session.capture().map_err(|error| error.to_string()),
            Self::Oci(session) => session.capture().map_err(|error| error.to_string()),
            Self::Firecrab(session) => session.capture().map_err(|error| error.to_string()),
        }
    }

    fn frame_pending(&mut self) -> Result<bool, String> {
        match self {
            Self::Native(session) => session.frame_pending().map_err(|error| error.to_string()),
            Self::Oci(session) => session.frame_pending().map_err(|error| error.to_string()),
            Self::Firecrab(session) => session.frame_pending().map_err(|error| error.to_string()),
        }
    }

    fn inject(&mut self, event: &InputEvent) -> Result<(), String> {
        match self {
            Self::Native(session) => session.inject(event).map_err(|error| error.to_string()),
            Self::Oci(session) => session.inject(event).map_err(|error| error.to_string()),
            Self::Firecrab(session) => session.inject(event).map_err(|error| error.to_string()),
        }
    }

    fn resize(&mut self, width: u16, height: u16) -> Result<(), String> {
        match self {
            Self::Native(session) => session
                .resize(width, height)
                .map_err(|error| error.to_string()),
            Self::Oci(session) => session
                .resize(width, height)
                .map_err(|error| error.to_string()),
            Self::Firecrab(session) => session
                .resize(width, height)
                .map_err(|error| error.to_string()),
        }
    }

    fn display_size(&self) -> (u16, u16) {
        match self {
            Self::Native(session) => session.display_size(),
            Self::Oci(session) => session.display_size(),
            Self::Firecrab(session) => session.display_size(),
        }
    }

    fn is_running(&mut self) -> Result<bool, String> {
        match self {
            Self::Native(session) => session.is_running().map_err(|error| error.to_string()),
            Self::Oci(session) => session.is_running().map_err(|error| error.to_string()),
            Self::Firecrab(session) => session.is_running().map_err(|error| error.to_string()),
        }
    }
}

#[derive(Default)]
struct RenderStats {
    polls: u64,
    captured: u64,
    full: u64,
    tile_frames: u64,
    tiles: u64,
    unchanged: u64,
    skipped: u64,
}

impl RenderStats {
    fn observe(&mut self, outcome: RenderOutcome) {
        self.captured += 1;
        match outcome {
            RenderOutcome::Unchanged => self.unchanged += 1,
            RenderOutcome::Full => self.full += 1,
            RenderOutcome::Tiles(count) => {
                self.tile_frames += 1;
                self.tiles += count as u64;
            }
        }
    }
}

impl std::fmt::Display for RenderStats {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "microbox render stats: polls={}, captured={}, full={}, tile_frames={}, tiles={}, unchanged={}, skipped={}",
            self.polls,
            self.captured,
            self.full,
            self.tile_frames,
            self.tiles,
            self.unchanged,
            self.skipped
        )
    }
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
    fn uses_terminal_pixels_and_cell_fallback_for_dynamic_geometry() {
        assert_eq!(
            geometry_from_measurements(100, 30, 1920, 1080),
            TerminalGeometry {
                columns: 100,
                rows: 30,
                pixel_width: 1920,
                pixel_height: 1080,
            }
        );
        assert_eq!(
            geometry_from_measurements(100, 30, 0, 0),
            TerminalGeometry {
                columns: 100,
                rows: 30,
                pixel_width: 800,
                pixel_height: 480,
            }
        );
        assert_eq!(
            geometry_from_measurements(0, 0, u16::MAX, u16::MAX).pixel_width,
            4096
        );
    }

    #[test]
    fn rejects_unknown_runtime() {
        let error =
            parse_run_options(&["xeyes".into(), "--runtime".into(), "unknown".into()]).unwrap_err();
        assert!(error.contains("unsupported runtime"));
    }

    #[test]
    fn accepts_oci_runtime_aliases() {
        for runtime in ["oci", "docker", "oci-agent"] {
            let options =
                parse_run_options(&["example/gui:1".into(), "--runtime".into(), runtime.into()])
                    .unwrap();
            assert_eq!(options.runtime, runtime);
        }
    }

    #[test]
    fn infers_oci_runtime_from_registry_reference() {
        let options = parse_run_options(&["ghcr.io/example/gui:1".into()]).unwrap();
        assert_eq!(options.runtime, "oci");
        assert!(!looks_like_oci_reference("./target/debug/gui"));
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
                fps: 30,
                stats: false,
                debug: false,
                firecrab_endpoint: None,
            }
        );
    }

    #[test]
    fn parses_frame_rate_and_stats() {
        let options = parse_run_options(&[
            "xeyes".into(),
            "--fps".into(),
            "45".into(),
            "--stats".into(),
        ])
        .unwrap();
        assert_eq!(options.fps, 45);
        assert!(options.stats);
    }

    #[test]
    fn enables_detailed_debug_output() {
        let options = parse_run_options(&["xeyes".into(), "--debug".into()]).unwrap();
        assert!(options.debug);
    }

    #[test]
    fn sanitizes_session_table_cells() {
        assert_eq!(clean_table_cell("app\nname\t1"), "app name 1");
    }
}
