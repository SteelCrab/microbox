use std::error::Error;
use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::protocol::{AgentMessage, ClientMessage, WireDecoder, WireError, write_agent_message};

use super::{ApplicationSpec, NativeError, NativeSession};

const AUTH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub listen: String,
    pub token: String,
    pub application: OsString,
    pub arguments: Vec<OsString>,
    pub width: u16,
    pub height: u16,
    pub fps: u16,
}

pub fn run_agent(config: AgentConfig) -> Result<(), AgentError> {
    if config.token.is_empty() || config.token.len() > 4096 {
        return Err(AgentError::InvalidToken);
    }
    if !(1..=60).contains(&config.fps) {
        return Err(AgentError::InvalidFps(config.fps));
    }
    let listener = TcpListener::bind(&config.listen).map_err(AgentError::Listen)?;
    let spec = ApplicationSpec::new(config.application, config.arguments);
    let mut session = NativeSession::start(&spec, config.width, config.height)?;
    let (mut stream, _) = listener.accept().map_err(AgentError::Accept)?;
    stream.set_nodelay(true).map_err(AgentError::Io)?;
    stream
        .set_read_timeout(Some(AUTH_TIMEOUT))
        .map_err(AgentError::Io)?;
    authenticate(&mut stream, &config.token)?;

    let (width, height) = session.display_size();
    write_agent_message(&mut stream, &AgentMessage::Hello { width, height })?;
    let initial = session.capture()?;
    write_agent_message(&mut stream, &AgentMessage::Frame(initial))?;
    stream.set_read_timeout(None).map_err(AgentError::Io)?;
    stream.set_nonblocking(true).map_err(AgentError::Io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(AgentError::Io)?;

    let running = Arc::new(AtomicBool::new(true));
    let signal_flag = Arc::clone(&running);
    ctrlc::set_handler(move || signal_flag.store(false, Ordering::SeqCst))
        .map_err(|error| AgentError::Signal(error.to_string()))?;
    let interval = Duration::from_secs_f64(1.0 / f64::from(config.fps));
    let mut next_frame = Instant::now() + interval;
    let mut decoder = WireDecoder::default();
    let mut bytes = [0; 64 * 1024];

    while running.load(Ordering::SeqCst) && session.is_running()? {
        loop {
            match stream.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(count) => decoder.push(&bytes[..count])?,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(AgentError::Io(error)),
            }
        }
        while let Some(message) = decoder.next_client()? {
            match message {
                ClientMessage::Input(input) => session.inject(&input)?,
                ClientMessage::Stop => return Ok(()),
                ClientMessage::Authenticate(_) => return Err(AgentError::DuplicateAuth),
            }
        }
        if Instant::now() >= next_frame {
            if session.frame_pending()? {
                write_agent_message(&mut stream, &AgentMessage::Frame(session.capture()?))?;
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
    let _ = write_agent_message(
        &mut stream,
        &AgentMessage::Exit("application exited".into()),
    );
    Ok(())
}

fn authenticate(stream: &mut TcpStream, expected: &str) -> Result<(), AgentError> {
    let mut decoder = WireDecoder::default();
    let mut bytes = [0; 4096];
    loop {
        if let Some(message) = decoder.next_client()? {
            return match message {
                ClientMessage::Authenticate(actual) if constant_time_eq(&actual, expected) => {
                    Ok(())
                }
                ClientMessage::Authenticate(_) => Err(AgentError::Authentication),
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

impl Error for AgentError {}

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

    #[test]
    fn token_comparison_handles_different_lengths() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secrex"));
        assert!(!constant_time_eq("secret", "secret-long"));
    }
}
