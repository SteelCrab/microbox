use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use crate::display::{X11Display, X11Error};
use crate::protocol::InputEvent;
use crate::renderer::Frame;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const APPLICATION_WINDOW_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationSpec {
    program: OsString,
    arguments: Vec<OsString>,
}

impl ApplicationSpec {
    pub fn new(
        program: impl Into<OsString>,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Self {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().collect(),
        }
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }
}

pub struct Xvfb {
    _process: ManagedChild,
    display_name: String,
}

impl Xvfb {
    pub fn start(width: u16, height: u16) -> Result<Self, NativeError> {
        if width == 0 || height == 0 {
            return Err(NativeError::InvalidDisplaySize { width, height });
        }

        let screen = format!("{width}x{height}x24");
        let mut command = Command::new("Xvfb");
        command
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                &screen,
                "-nolisten",
                "tcp",
                "-noreset",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = ManagedChild::spawn(&mut command)
            .map_err(|error| NativeError::StartXvfb(error.to_string()))?;

        let stdout = child
            .child
            .stdout
            .take()
            .ok_or(NativeError::MissingXvfbOutput)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
        });

        let line = match receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                child.terminate();
                let _ = reader.join();
                return Err(NativeError::ReadDisplayNumber(error.to_string()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                child.terminate();
                let _ = reader.join();
                return Err(NativeError::XvfbTimeout(STARTUP_TIMEOUT));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                child.terminate();
                let _ = reader.join();
                return Err(NativeError::MissingXvfbOutput);
            }
        };
        let _ = reader.join();

        let display_number = match line.trim().parse::<u16>() {
            Ok(display_number) => display_number,
            Err(_) => {
                child.terminate();
                return Err(NativeError::InvalidDisplayNumber(line.trim().to_string()));
            }
        };

        Ok(Self {
            _process: child,
            display_name: format!(":{display_number}"),
        })
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

pub struct NativeSession {
    application: ManagedChild,
    display: X11Display,
    xvfb: Xvfb,
}

impl NativeSession {
    pub fn start(spec: &ApplicationSpec, width: u16, height: u16) -> Result<Self, NativeError> {
        let xvfb = Xvfb::start(width, height)?;
        let display = connect_with_retry(xvfb.display_name(), STARTUP_TIMEOUT)?;
        let mut command = Command::new(spec.program());
        command
            .args(spec.arguments())
            .env("DISPLAY", xvfb.display_name())
            .env("GDK_BACKEND", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut application =
            ManagedChild::spawn(&mut command).map_err(|error| NativeError::StartApplication {
                program: spec.program().to_string_lossy().into_owned(),
                message: error.to_string(),
            })?;

        let window = match display.wait_for_application_window(APPLICATION_WINDOW_TIMEOUT) {
            Ok(window) => window,
            Err(error) => {
                let early_exit = application.child.try_wait().ok().flatten();
                application.terminate();
                if let Some(status) = early_exit {
                    return Err(NativeError::ApplicationExited(status.to_string()));
                }
                return Err(NativeError::X11(error));
            }
        };
        display.fill_screen(window).map_err(NativeError::X11)?;

        Ok(Self {
            application,
            display,
            xvfb,
        })
    }

    pub fn capture(&mut self) -> Result<Frame, NativeError> {
        self.display.capture().map_err(NativeError::X11)
    }

    pub fn capture_method(&self) -> &'static str {
        self.display.capture_method()
    }

    pub fn frame_pending(&mut self) -> Result<bool, NativeError> {
        self.display.frame_pending().map_err(NativeError::X11)
    }

    pub fn inject(&self, event: &InputEvent) -> Result<(), NativeError> {
        self.display.inject(event).map_err(NativeError::X11)
    }

    pub fn display_size(&self) -> (u16, u16) {
        self.display.size()
    }

    pub fn is_running(&mut self) -> Result<bool, NativeError> {
        self.application
            .child
            .try_wait()
            .map(|status| status.is_none())
            .map_err(|error| NativeError::ApplicationStatus(error.to_string()))
    }

    pub fn display_name(&self) -> &str {
        self.xvfb.display_name()
    }

    #[cfg(test)]
    fn pointer_position(&self) -> Result<(i16, i16), NativeError> {
        self.display.pointer_position().map_err(NativeError::X11)
    }

    #[cfg(test)]
    fn kill_application(&mut self) {
        self.application.terminate();
    }

    #[cfg(test)]
    fn kill_xvfb(&mut self) {
        self.xvfb._process.terminate();
    }
}

fn connect_with_retry(display_name: &str, timeout: Duration) -> Result<X11Display, NativeError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match X11Display::connect(display_name) {
            Ok(display) => return Ok(display),
            Err(error) if std::time::Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(NativeError::X11(error)),
        }
    }
}

struct ManagedChild {
    child: Child,
}

impl ManagedChild {
    fn spawn(command: &mut Command) -> std::io::Result<Self> {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
        command.spawn().map(|child| Self { child })
    }

    fn terminate(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let process_group = Pid::from_raw(self.child.id() as i32);
            let _ = killpg(process_group, Signal::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[derive(Debug)]
pub enum NativeError {
    InvalidDisplaySize { width: u16, height: u16 },
    StartXvfb(String),
    MissingXvfbOutput,
    ReadDisplayNumber(String),
    InvalidDisplayNumber(String),
    XvfbTimeout(Duration),
    StartApplication { program: String, message: String },
    ApplicationExited(String),
    ApplicationStatus(String),
    X11(X11Error),
}

impl Display for NativeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDisplaySize { width, height } => {
                write!(formatter, "invalid Xvfb display size {width}x{height}")
            }
            Self::StartXvfb(message) => write!(formatter, "could not start Xvfb: {message}"),
            Self::MissingXvfbOutput => write!(formatter, "Xvfb did not report a display number"),
            Self::ReadDisplayNumber(message) => {
                write!(formatter, "could not read Xvfb display number: {message}")
            }
            Self::InvalidDisplayNumber(value) => {
                write!(formatter, "Xvfb returned invalid display number '{value}'")
            }
            Self::XvfbTimeout(timeout) => write!(
                formatter,
                "Xvfb did not start within {:.1}s",
                timeout.as_secs_f32()
            ),
            Self::StartApplication { program, message } => {
                write!(formatter, "could not start '{program}': {message}")
            }
            Self::ApplicationExited(status) => {
                write!(
                    formatter,
                    "application exited before mapping a window ({status})"
                )
            }
            Self::ApplicationStatus(message) => {
                write!(formatter, "could not read application status: {message}")
            }
            Self::X11(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for NativeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::X11(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static XVFB_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_xvfb() -> std::sync::MutexGuard<'static, ()> {
        XVFB_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn rejects_empty_display_size_without_starting_xvfb() {
        assert!(matches!(
            Xvfb::start(0, 100),
            Err(NativeError::InvalidDisplaySize { .. })
        ));
    }

    #[test]
    fn application_spec_keeps_arguments_separate() {
        let spec = ApplicationSpec::new("viewer", [OsString::from("a file.png")]);
        assert_eq!(spec.program(), OsStr::new("viewer"));
        assert_eq!(spec.arguments(), [OsString::from("a file.png")]);
    }

    #[test]
    #[ignore = "requires Xvfb and xeyes"]
    fn captures_xeyes_frame() {
        let _test_lock = lock_xvfb();
        let spec = ApplicationSpec::new("xeyes", []);
        let mut session = NativeSession::start(&spec, 320, 180).unwrap();
        assert_eq!(session.capture_method(), "MIT-SHM");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let frame = session.capture().unwrap();
            assert_eq!((frame.width(), frame.height()), (320, 180));
            if frame.pixels().iter().any(|&byte| byte != 0) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "xeyes did not draw a non-black frame"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    #[ignore = "requires Xvfb and xeyes"]
    fn observes_application_crash_and_cleans_up() {
        let _test_lock = lock_xvfb();
        let spec = ApplicationSpec::new("xeyes", []);
        let mut session = NativeSession::start(&spec, 320, 180).unwrap();
        session.kill_application();
        assert!(!session.is_running().unwrap());
    }

    #[test]
    #[ignore = "requires Xvfb and xeyes"]
    fn reports_xvfb_crash() {
        let _test_lock = lock_xvfb();
        let spec = ApplicationSpec::new("xeyes", []);
        let mut session = NativeSession::start(&spec, 320, 180).unwrap();
        session.kill_xvfb();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if session.capture().is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "capture remained healthy after Xvfb termination"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    #[ignore = "requires Xvfb and mousepad"]
    fn smoke_tests_mousepad() {
        let _test_lock = lock_xvfb();
        if !program_available("mousepad")
            || std::env::var_os("MICRO_GUI_GTK_SMOKE").as_deref() != Some(OsStr::new("1"))
        {
            return;
        }
        let spec = ApplicationSpec::new("mousepad", [OsString::from("--disable-server")]);
        assert_application_draws(spec);
    }

    #[test]
    #[ignore = "requires Xvfb and xmessage"]
    fn smoke_tests_xmessage() {
        let _test_lock = lock_xvfb();
        if !program_available("xmessage") {
            return;
        }
        let spec = ApplicationSpec::new("xmessage", [OsString::from("micro-gui smoke test")]);
        assert_application_draws(spec);
    }

    fn assert_application_draws(spec: ApplicationSpec) {
        let mut session = NativeSession::start(&spec, 320, 180).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let frame = session.capture().unwrap();
            if frame.pixels().iter().any(|&byte| byte != 0) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "application did not draw a non-black frame"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn program_available(program: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
        })
    }

    #[test]
    #[ignore = "requires Xvfb and xeyes"]
    fn injects_pointer_and_keyboard_events() {
        use crate::protocol::{KeyEvent, MouseEvent, MouseKind};

        let _test_lock = lock_xvfb();
        let spec = ApplicationSpec::new("xeyes", []);
        let session = NativeSession::start(&spec, 320, 180).unwrap();
        session
            .inject(&InputEvent::Mouse(MouseEvent {
                x: 123,
                y: 45,
                button: None,
                kind: MouseKind::Move,
                modifiers: 0,
            }))
            .unwrap();
        assert_eq!(session.pointer_position().unwrap(), (123, 45));

        for kind in [MouseKind::Press, MouseKind::Release] {
            session
                .inject(&InputEvent::Mouse(MouseEvent {
                    x: 123,
                    y: 45,
                    button: Some(crate::protocol::MouseButton::Left),
                    kind,
                    modifiers: 0,
                }))
                .unwrap();
        }

        for pressed in [true, false] {
            session
                .inject(&InputEvent::Key(KeyEvent {
                    text: Some("a".into()),
                    code: xkeysym::Keysym::from_char('a').raw(),
                    pressed,
                    modifiers: 0,
                }))
                .unwrap();
        }
    }
}
