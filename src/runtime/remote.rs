use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::protocol::{
    AgentExit, AgentMessage, ClientMessage, InputEvent, WireDecoder, WireError,
    write_client_message,
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
    termination: Option<AgentExit>,
    width: u16,
    height: u16,
}

impl FirecrabSession {
    pub fn connect(
        endpoint: &str,
        token: &str,
        width: u16,
        height: u16,
    ) -> Result<Self, FirecrabError> {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(FirecrabError::Protocol(format!(
                "display dimensions must be between 1 and 4096 pixels, got {width}x{height}"
            )));
        }
        Frame::rgb_buffer_len(u32::from(width), u32::from(height))
            .map_err(|error| FirecrabError::Protocol(error.to_string()))?;
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
        write_client_message(
            &mut writer,
            &ClientMessage::Authenticate {
                token: token.into(),
                width,
                height,
            },
        )?;

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
                AgentMessage::Error(message) => {
                    return Err(FirecrabError::Agent(message));
                }
                AgentMessage::Exit(exit) => return Err(FirecrabError::Agent(exit.message)),
                AgentMessage::Hello { .. } => {
                    return Err(FirecrabError::Protocol("zero-sized display".into()));
                }
            }
        }
        let dimensions = dimensions.ok_or(FirecrabError::HandshakeTimeout)?;
        if dimensions != (width, height) {
            return Err(FirecrabError::Protocol(format!(
                "agent opened {}x{} instead of requested {width}x{height}",
                dimensions.0, dimensions.1
            )));
        }
        let latest = initial_frame.ok_or(FirecrabError::HandshakeTimeout)?;
        if (latest.width(), latest.height()) != (u32::from(width), u32::from(height)) {
            return Err(FirecrabError::Protocol(
                "initial frame dimensions do not match hello".into(),
            ));
        }
        reader.set_nonblocking(true).map_err(FirecrabError::Io)?;
        Ok(Self {
            reader,
            writer,
            decoder,
            latest,
            pending: true,
            running: true,
            termination: None,
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

    pub fn resize(&mut self, width: u16, height: u16) -> Result<(), FirecrabError> {
        Frame::rgb_buffer_len(u32::from(width), u32::from(height))
            .map_err(|error| FirecrabError::Protocol(error.to_string()))?;
        write_client_message(
            &mut self.writer,
            &ClientMessage::Input(InputEvent::Resize { width, height }),
        )?;
        self.width = width;
        self.height = height;
        self.pending = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            self.poll_messages()?;
            if self.pending {
                return Ok(());
            }
            if !self.running {
                let reason = self
                    .termination
                    .as_ref()
                    .map(|exit| exit.message.as_str())
                    .unwrap_or("no termination reason was provided");
                return Err(FirecrabError::Agent(format!(
                    "agent exited while resizing: {reason}"
                )));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(FirecrabError::ResizeTimeout { width, height })
    }

    pub fn is_running(&mut self) -> Result<bool, FirecrabError> {
        self.poll_messages()?;
        Ok(self.running)
    }

    pub fn termination(&self) -> Option<&AgentExit> {
        self.termination.as_ref()
    }

    fn poll_messages(&mut self) -> Result<(), FirecrabError> {
        let mut bytes = [0; 64 * 1024];
        let mut end_of_stream = false;
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => {
                    end_of_stream = true;
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
                        continue;
                    }
                    self.latest = frame;
                    self.pending = true;
                }
                AgentMessage::Exit(exit) => {
                    self.termination = Some(exit);
                    self.running = false;
                }
                AgentMessage::Error(message) => {
                    self.termination = Some(AgentExit::status(false, message.clone()));
                    self.running = false;
                    return Err(FirecrabError::Agent(message));
                }
                AgentMessage::Hello { .. } => {
                    return Err(FirecrabError::Protocol("duplicate hello".into()));
                }
            }
        }
        if end_of_stream {
            self.running = false;
            if self.termination.is_none() {
                self.termination = Some(AgentExit::status(
                    false,
                    "agent transport closed without an exit status",
                ));
            }
        }
        Ok(())
    }
}

impl Drop for FirecrabSession {
    fn drop(&mut self) {
        if self.running {
            let _ = write_client_message(&mut self.writer, &ClientMessage::Stop);
        }
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
    ResizeTimeout { width: u16, height: u16 },
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
            Self::ResizeTimeout { width, height } => {
                write!(
                    formatter,
                    "GUI agent did not resize to {width}x{height} in time"
                )
            }
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
                if let Some(ClientMessage::Authenticate {
                    token,
                    width,
                    height,
                }) = decoder.next_client().unwrap()
                {
                    assert_eq!(token, "token");
                    assert_eq!((width, height), (2, 1));
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
            let mut received_key = false;
            let mut received_resize = false;
            while !received_key || !received_resize {
                while let Some(message) = decoder.next_client().unwrap() {
                    match message {
                        ClientMessage::Input(InputEvent::Key(event)) => {
                            assert_eq!(event.code, 42);
                            received_key = true;
                        }
                        ClientMessage::Input(InputEvent::Resize { width, height }) => {
                            assert_eq!((width, height), (3, 2));
                            write_agent_message(
                                &mut stream,
                                &AgentMessage::Frame(Frame::new_rgb(3, 2, vec![7; 18]).unwrap()),
                            )
                            .unwrap();
                            received_resize = true;
                        }
                        other => panic!("unexpected client message: {other:?}"),
                    }
                }
                if received_key && received_resize {
                    break;
                }
                let count = stream.read(&mut bytes).unwrap();
                decoder.push(&bytes[..count]).unwrap();
            }
        });

        let mut session = FirecrabSession::connect(&endpoint.to_string(), "token", 2, 1).unwrap();
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
        session.resize(3, 2).unwrap();
        assert_eq!(session.display_size(), (3, 2));
        assert_eq!(session.capture().unwrap().pixels(), &[7; 18]);
        agent.join().unwrap();
    }

    #[test]
    fn rejects_unresolvable_endpoint() {
        assert!(matches!(
            FirecrabSession::connect("invalid endpoint", "token", 1, 1),
            Err(FirecrabError::Endpoint(_))
        ));
    }

    #[test]
    fn preserves_structured_agent_exit_status() {
        let (mut session, agent) = connected_test_session(|mut stream| {
            write_agent_message(
                &mut stream,
                &AgentMessage::Exit(AgentExit::status(
                    false,
                    "application terminated by signal 11",
                )),
            )
            .unwrap();
        });
        agent.join().unwrap();

        assert!(!wait_until_stopped(&mut session));
        assert_eq!(
            session.termination(),
            Some(&AgentExit::status(
                false,
                "application terminated by signal 11"
            ))
        );
    }

    #[test]
    fn reports_transport_close_without_exit_status() {
        let (mut session, agent) = connected_test_session(drop);
        agent.join().unwrap();

        assert!(!wait_until_stopped(&mut session));
        assert_eq!(session.termination().unwrap().success, Some(false));
        assert_eq!(
            session.termination().unwrap().message,
            "agent transport closed without an exit status"
        );
    }

    /// The peer thread joining only guarantees it has sent its bytes or
    /// closed its socket, not that the kernel has delivered that to our
    /// nonblocking reader yet. A single `is_running()` check can still see
    /// WouldBlock and race, so poll until it settles instead.
    fn wait_until_stopped(session: &mut FirecrabSession) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let running = session.is_running().unwrap();
            if !running || Instant::now() >= deadline {
                return running;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn connected_test_session(
        finish: impl FnOnce(TcpStream) + Send + 'static,
    ) -> (FirecrabSession, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let agent = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = WireDecoder::default();
            let mut bytes = [0; 4096];
            loop {
                let count = stream.read(&mut bytes).unwrap();
                decoder.push(&bytes[..count]).unwrap();
                if matches!(
                    decoder.next_client().unwrap(),
                    Some(ClientMessage::Authenticate { .. })
                ) {
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
                &AgentMessage::Frame(Frame::new_rgb(2, 1, vec![0; 6]).unwrap()),
            )
            .unwrap();
            finish(stream);
        });
        let session = FirecrabSession::connect(&endpoint.to_string(), "token", 2, 1).unwrap();
        (session, agent)
    }
}
