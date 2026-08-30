use std::error::Error;
use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::protocol::{
    AgentExit, AgentMessage, ClientMessage, WireDecoder, WireError, write_agent_message,
};

use super::{ApplicationSpec, NativeError, NativeSession};

const AUTH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub listen: String,
    pub token: String,
    pub application: OsString,
    pub arguments: Vec<OsString>,
    pub fps: u16,
}

pub fn run_agent(config: AgentConfig) -> Result<(), AgentError> {
    let debug = AgentDebug::detect();
    if config.token.is_empty() || config.token.len() > 4096 {
        return Err(AgentError::InvalidToken);
    }
    if !(1..=60).contains(&config.fps) {
        return Err(AgentError::InvalidFps(config.fps));
    }
    debug.log(format!("listening address={}", config.listen));
    let listener = TcpListener::bind(&config.listen).map_err(AgentError::Listen)?;
    let (mut stream, peer) = listener.accept().map_err(AgentError::Accept)?;
    debug.log(format!("client accepted peer={peer}"));
    match run_connected_agent(&config, &mut stream, &debug) {
        Ok(()) => Ok(()),
        Err(error) => {
            debug.log(format!("failed error={error}"));
            let _ = write_agent_message(&mut stream, &AgentMessage::Error(error.to_string()));
            Err(error)
        }
    }
}

fn run_connected_agent(
    config: &AgentConfig,
    stream: &mut TcpStream,
    debug: &AgentDebug,
) -> Result<(), AgentError> {
    stream.set_nodelay(true).map_err(AgentError::Io)?;
    stream
        .set_read_timeout(Some(AUTH_TIMEOUT))
        .map_err(AgentError::Io)?;
    let (width, height) = authenticate(stream, &config.token)?;
    debug.log(format!("authenticated display={width}x{height}"));
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(AgentError::InvalidDisplaySize { width, height });
    }
    let spec = ApplicationSpec::new(config.application.clone(), config.arguments.clone());
    debug.log(format!(
        "starting application={:?} arguments={:?}",
        config.application, config.arguments
    ));
    let mut session = NativeSession::start(&spec, width, height)?;
    debug.log(format!(
        "application ready display={}x{} capture={}",
        session.display_size().0,
        session.display_size().1,
        session.capture_method()
    ));

    let (width, height) = session.display_size();
    write_agent_message(stream, &AgentMessage::Hello { width, height })?;
    let _ = session.frame_pending()?;
    let initial = session.capture()?;
    write_agent_message(stream, &AgentMessage::Frame(initial))?;
    debug.log("initial frame sent".into());
    stream.set_read_timeout(None).map_err(AgentError::Io)?;
    stream.set_nonblocking(true).map_err(AgentError::Io)?;

    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .map_err(|error| AgentError::Signal(error.to_string()))?;
    let interval = Duration::from_secs_f64(1.0 / f64::from(config.fps));
    let mut next_frame = Instant::now() + interval;
    let mut decoder = WireDecoder::default();
    let mut bytes = [0; 64 * 1024];
    let mut force_frame = false;

    loop {
        if !running.load(Ordering::SeqCst) {
            let exit = AgentExit::status(true, "agent received a termination signal");
            debug.log(format!("stopping reason={:?}", exit.message));
            write_agent_message(stream, &AgentMessage::Exit(exit))?;
            return Ok(());
        }
        if let Some(status) = session.application_status()? {
            let message = describe_application_exit(&config.application, status);
            let success = status.success();
            debug.log(format!("stopping success={success} reason={message:?}"));
            write_agent_message(
                stream,
                &AgentMessage::Exit(AgentExit::status(success, message)),
            )?;
            return Ok(());
        }
        loop {
            match stream.read(&mut bytes) {
                Ok(0) => {
                    debug.log("client transport closed".into());
                    return Ok(());
                }
                Ok(count) => decoder.push(&bytes[..count])?,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(AgentError::Io(error)),
            }
        }
        while let Some(message) = decoder.next_client()? {
            match message {
                ClientMessage::Input(crate::protocol::InputEvent::Resize { width, height }) => {
                    debug.log(format!("resize requested display={width}x{height}"));
                    session.resize(width, height)?;
                    force_frame = true;
                }
                ClientMessage::Input(input) => {
                    if let Err(error) = session.inject(&input) {
                        debug.log(format!("dropped unsupported input event: {error}"));
                    }
                }
                ClientMessage::Stop => {
                    let exit = AgentExit::status(true, "client requested stop");
                    debug.log(format!("stopping reason={:?}", exit.message));
                    write_agent_message(stream, &AgentMessage::Exit(exit))?;
                    return Ok(());
                }
                ClientMessage::Authenticate { .. } => return Err(AgentError::DuplicateAuth),
            }
        }
        if Instant::now() >= next_frame {
            let damaged = session.frame_pending()?;
            if force_frame || damaged {
                write_agent_message(stream, &AgentMessage::Frame(session.capture()?))?;
                force_frame = false;
            }
            next_frame = Instant::now() + interval;
        } else {
            thread::sleep(
                next_frame
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(5)),
            );
        }
    }
}

fn describe_application_exit(
    application: &std::ffi::OsStr,
    status: std::process::ExitStatus,
) -> String {
    let application = application.to_string_lossy();
    if let Some(code) = status.code() {
        return format!("application {application:?} exited with code {code}");
    }
    if let Some(signal) = status.signal() {
        let core = if status.core_dumped() {
            " (core dumped)"
        } else {
            ""
        };
        return format!("application {application:?} terminated by signal {signal}{core}");
    }
    format!("application {application:?} exited with unknown status {status}")
}

struct AgentDebug {
    enabled: bool,
    started: Instant,
}

impl AgentDebug {
    fn detect() -> Self {
        Self {
            enabled: std::env::var_os("MICROBOX_DEBUG").is_some(),
            started: Instant::now(),
        }
    }

    fn log(&self, message: String) {
        if self.enabled {
            eprintln!(
                "microbox agent debug: +{:>6}ms {message}",
                self.started.elapsed().as_millis()
            );
        }
    }
}

fn authenticate(stream: &mut TcpStream, expected: &str) -> Result<(u16, u16), AgentError> {
    let mut decoder = WireDecoder::default();
    let mut bytes = [0; 4096];
    loop {
        if let Some(message) = decoder.next_client()? {
            return match message {
                ClientMessage::Authenticate {
                    token,
                    width,
                    height,
                } if constant_time_eq(&token, expected) => Ok((width, height)),
                ClientMessage::Authenticate { .. } => Err(AgentError::Authentication),
                _ => Err(AgentError::AuthenticationRequired),
            };
        }
        let count = stream.read(&mut bytes).map_err(AgentError::Io)?;
        if count == 0 {
            return Err(AgentError::AuthenticationRequired);
        }
        decoder.push(&bytes[..count])?;
    }
}

fn constant_time_eq(actual: &str, expected: &str) -> bool {
    let actual = actual.as_bytes();
    let expected = expected.as_bytes();
    let mut difference = actual.len() ^ expected.len();
    let length = actual.len().max(expected.len());
    for index in 0..length {
        difference |= usize::from(
            actual.get(index).copied().unwrap_or(0) ^ expected.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

#[derive(Debug)]
pub enum AgentError {
    InvalidToken,
    InvalidFps(u16),
    InvalidDisplaySize { width: u16, height: u16 },
    Listen(io::Error),
    Accept(io::Error),
    Io(io::Error),
    Wire(WireError),
    Native(NativeError),
    Authentication,
    AuthenticationRequired,
    DuplicateAuth,
    Signal(String),
}

impl Display for AgentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken => write!(formatter, "agent token must contain 1 to 4096 bytes"),
            Self::InvalidFps(fps) => {
                write!(formatter, "agent FPS must be between 1 and 60, got {fps}")
            }
            Self::InvalidDisplaySize { width, height } => write!(
                formatter,
                "agent display dimensions must be between 1 and 4096 pixels, got {width}x{height}"
            ),
            Self::Listen(error) => write!(formatter, "could not bind GUI agent: {error}"),
            Self::Accept(error) => write!(formatter, "could not accept GUI client: {error}"),
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Wire(error) => Display::fmt(error, formatter),
            Self::Native(error) => Display::fmt(error, formatter),
            Self::Authentication => write!(formatter, "GUI agent authentication failed"),
            Self::AuthenticationRequired => {
                write!(formatter, "GUI agent requires authentication first")
            }
            Self::DuplicateAuth => write!(formatter, "GUI agent received duplicate authentication"),
            Self::Signal(message) => write!(
                formatter,
                "could not install agent signal handler: {message}"
            ),
        }
    }
}

impl Error for AgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Listen(error) | Self::Accept(error) | Self::Io(error) => Some(error),
            Self::Wire(error) => Some(error),
            Self::Native(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WireError> for AgentError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

impl From<NativeError> for AgentError {
    fn from(error: NativeError) -> Self {
        Self::Native(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn token_comparison_handles_different_lengths() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secrex"));
        assert!(!constant_time_eq("secret", "secret-long"));
    }

    #[test]
    fn describes_application_exit_codes_and_signals() {
        let code = Command::new("sh").args(["-c", "exit 7"]).status().unwrap();
        assert_eq!(
            describe_application_exit(std::ffi::OsStr::new("test-app"), code),
            "application \"test-app\" exited with code 7"
        );

        let signal = Command::new("sh")
            .args(["-c", "kill -TERM $$"])
            .status()
            .unwrap();
        assert_eq!(
            describe_application_exit(std::ffi::OsStr::new("test-app"), signal),
            "application \"test-app\" terminated by signal 15"
        );
    }
}
