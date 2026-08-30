use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::protocol::{
    AgentMessage, ClientMessage, InputEvent, WireDecoder, WireError, write_client_message,
};
use crate::renderer::Frame;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct FirecrabSession {
    reader: TcpStream,
    writer: TcpStream,
    decoder: WireDecoder,
    latest: Frame,
    pending: bool,
    running: bool,
    width: u16,
    height: u16,
}

impl FirecrabSession {
    pub fn connect(endpoint: &str, token: &str) -> Result<Self, FirecrabError> {
        let addresses = endpoint
            .to_socket_addrs()
            .map_err(|error| FirecrabError::Endpoint(error.to_string()))?
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(FirecrabError::Endpoint("resolved to no addresses".into()));
        }
        let mut last_error = None;
        let mut stream = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let mut reader = stream.ok_or_else(|| {
            FirecrabError::Connect(
                last_error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "connection failed".into()),
            )
        })?;
        reader.set_nodelay(true).map_err(FirecrabError::Io)?;
        reader
            .set_read_timeout(Some(CONNECT_TIMEOUT))
            .map_err(FirecrabError::Io)?;
        reader
            .set_write_timeout(Some(CONNECT_TIMEOUT))
            .map_err(FirecrabError::Io)?;
        let mut writer = reader.try_clone().map_err(FirecrabError::Io)?;
        write_client_message(&mut writer, &ClientMessage::Authenticate(token.into()))?;

        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let mut decoder = WireDecoder::default();
        let mut dimensions = None;
        let mut initial_frame = None;
        while Instant::now() < deadline && (dimensions.is_none() || initial_frame.is_none()) {
            match read_one(&mut reader, &mut decoder)? {
                AgentMessage::Hello { width, height } if width > 0 && height > 0 => {
                    dimensions = Some((width, height));
                }
                AgentMessage::Frame(frame) => initial_frame = Some(frame),
                AgentMessage::Error(message) | AgentMessage::Exit(message) => {
                    return Err(FirecrabError::Agent(message));
                }
                AgentMessage::Hello { .. } => {
                    return Err(FirecrabError::Protocol("zero-sized display".into()));
                }
            }
        }
        let (width, height) = dimensions.ok_or(FirecrabError::HandshakeTimeout)?;
        let latest = initial_frame.ok_or(FirecrabError::HandshakeTimeout)?;
        if (latest.width(), latest.height()) != (u32::from(width), u32::from(height)) {
            return Err(FirecrabError::Protocol(
                "initial frame dimensions do not match hello".into(),
            ));
        }
        reader.set_nonblocking(true).map_err(FirecrabError::Io)?;
        writer
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(FirecrabError::Io)?;
        Ok(Self {
            reader,
            writer,
            decoder,
            latest,
            pending: true,
            running: true,
            width,
            height,
        })
    }

    pub fn capture(&mut self) -> Result<Frame, FirecrabError> {
        self.poll_messages()?;
        self.pending = false;
        Ok(self.latest.clone())
    }

    pub fn frame_pending(&mut self) -> Result<bool, FirecrabError> {
        self.poll_messages()?;
        Ok(self.pending)
    }

    pub fn inject(&mut self, event: &InputEvent) -> Result<(), FirecrabError> {
        write_client_message(&mut self.writer, &ClientMessage::Input(event.clone()))
            .map_err(FirecrabError::Wire)
    }

    pub fn display_size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn is_running(&mut self) -> Result<bool, FirecrabError> {
        self.poll_messages()?;
        Ok(self.running)
    }

    fn poll_messages(&mut self) -> Result<(), FirecrabError> {
        let mut bytes = [0; 64 * 1024];
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => {
                    self.running = false;
                    break;
                }
                Ok(count) => self.decoder.push(&bytes[..count])?,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(FirecrabError::Io(error)),
            }
        }
        while let Some(message) = self.decoder.next_agent()? {
            match message {
                AgentMessage::Frame(frame) => {
                    if (frame.width(), frame.height())
                        != (u32::from(self.width), u32::from(self.height))
                    {
                        return Err(FirecrabError::Protocol(
                            "frame dimensions changed during session".into(),
                        ));
                    }
                    self.latest = frame;
                    self.pending = true;
                }
                AgentMessage::Exit(_) => self.running = false,
                AgentMessage::Error(message) => return Err(FirecrabError::Agent(message)),
                AgentMessage::Hello { .. } => {
                    return Err(FirecrabError::Protocol("duplicate hello".into()));
                }
            }
        }
        Ok(())
    }
}

impl Drop for FirecrabSession {
    fn drop(&mut self) {
        let _ = write_client_message(&mut self.writer, &ClientMessage::Stop);
    }
}

fn read_one(
    reader: &mut TcpStream,
    decoder: &mut WireDecoder,
) -> Result<AgentMessage, FirecrabError> {
    let mut bytes = [0; 64 * 1024];
    loop {
        if let Some(message) = decoder.next_agent()? {
            return Ok(message);
        }
        let count = reader.read(&mut bytes).map_err(FirecrabError::Io)?;
        if count == 0 {
            return Err(FirecrabError::Connect(
                "agent closed during handshake".into(),
            ));
        }
        decoder.push(&bytes[..count])?;
    }
}

#[derive(Debug)]
pub enum FirecrabError {
    Endpoint(String),
    Connect(String),
    HandshakeTimeout,
    Agent(String),
    Protocol(String),
    Io(io::Error),
    Wire(WireError),
}

impl Display for FirecrabError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Endpoint(message) => write!(formatter, "invalid Firecrab endpoint: {message}"),
            Self::Connect(message) => {
                write!(formatter, "could not connect to GUI agent: {message}")
            }
            Self::HandshakeTimeout => write!(formatter, "GUI agent handshake timed out"),
            Self::Agent(message) => write!(formatter, "GUI agent failed: {message}"),
            Self::Protocol(message) => write!(formatter, "GUI agent protocol error: {message}"),
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Wire(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for FirecrabError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Wire(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WireError> for FirecrabError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    use crate::protocol::{AgentMessage, KeyEvent, write_agent_message};

    #[test]
    fn connects_receives_frames_and_sends_input() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let agent = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = WireDecoder::default();
            let mut bytes = [0; 4096];
            loop {
                let count = stream.read(&mut bytes).unwrap();
                decoder.push(&bytes[..count]).unwrap();
                if let Some(ClientMessage::Authenticate(token)) = decoder.next_client().unwrap() {
                    assert_eq!(token, "token");
                    break;
                }
            }
            write_agent_message(
                &mut stream,
                &AgentMessage::Hello {
                    width: 2,
                    height: 1,
                },
            )
            .unwrap();
            write_agent_message(
                &mut stream,
                &AgentMessage::Frame(Frame::new_rgb(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap()),
            )
            .unwrap();
            loop {
                let count = stream.read(&mut bytes).unwrap();
                decoder.push(&bytes[..count]).unwrap();
                if let Some(ClientMessage::Input(InputEvent::Key(event))) =
                    decoder.next_client().unwrap()
                {
                    assert_eq!(event.code, 42);
                    break;
                }
            }
        });

        let mut session = FirecrabSession::connect(&endpoint.to_string(), "token").unwrap();
        assert_eq!(session.display_size(), (2, 1));
        assert_eq!(session.capture().unwrap().pixels(), &[1, 2, 3, 4, 5, 6]);
        session
            .inject(&InputEvent::Key(KeyEvent {
                text: None,
                code: 42,
                pressed: true,
                modifiers: 0,
            }))
            .unwrap();
        agent.join().unwrap();
    }

    #[test]
    fn rejects_unresolvable_endpoint() {
        assert!(matches!(
            FirecrabSession::connect("invalid endpoint", "token"),
            Err(FirecrabError::Endpoint(_))
        ));
    }
}
