use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Write};

use crate::protocol::{InputEvent, KeyEvent, MouseButton, MouseEvent, MouseKind};
use crate::renderer::Frame;

pub const DEFAULT_AGENT_PORT: u16 = 5943;
pub const MAX_WIRE_PAYLOAD: usize = 64 * 1024 * 1024;

const HELLO: u8 = 1;
const FRAME: u8 = 2;
const EXIT: u8 = 3;
const ERROR: u8 = 4;
const AUTH: u8 = 10;
const INPUT_KEY: u8 = 11;
const INPUT_TEXT: u8 = 12;
const INPUT_MOUSE: u8 = 13;
const INPUT_RESIZE: u8 = 14;
const STOP: u8 = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMessage {
    Hello { width: u16, height: u16 },
    Frame(Frame),
    Exit(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    Authenticate(String),
    Input(InputEvent),
    Stop,
}

pub fn write_agent_message(
    writer: &mut impl Write,
    message: &AgentMessage,
) -> Result<(), WireError> {
    let (kind, payload) = match message {
        AgentMessage::Hello { width, height } => {
            let mut payload = Vec::with_capacity(4);
            payload.extend_from_slice(&width.to_be_bytes());
            payload.extend_from_slice(&height.to_be_bytes());
            (HELLO, payload)
        }
        AgentMessage::Frame(frame) => {
            let mut payload = Vec::with_capacity(8 + frame.pixels().len());
            payload.extend_from_slice(&frame.width().to_be_bytes());
            payload.extend_from_slice(&frame.height().to_be_bytes());
            payload.extend_from_slice(frame.pixels());
            (FRAME, payload)
        }
        AgentMessage::Exit(message) => (EXIT, message.as_bytes().to_vec()),
        AgentMessage::Error(message) => (ERROR, message.as_bytes().to_vec()),
    };
    write_packet(writer, kind, &payload)
}

pub fn write_client_message(
    writer: &mut impl Write,
    message: &ClientMessage,
) -> Result<(), WireError> {
    let (kind, payload) = match message {
        ClientMessage::Authenticate(token) => (AUTH, token.as_bytes().to_vec()),
        ClientMessage::Input(InputEvent::Text(text)) => (INPUT_TEXT, text.as_bytes().to_vec()),
        ClientMessage::Input(InputEvent::Key(event)) => {
            let text = event.text.as_deref().unwrap_or("").as_bytes();
            let mut payload = Vec::with_capacity(10 + text.len());
            payload.extend_from_slice(&event.code.to_be_bytes());
            payload.push(u8::from(event.pressed));
            payload.push(event.modifiers);
            payload.extend_from_slice(&(text.len() as u32).to_be_bytes());
            payload.extend_from_slice(text);
            (INPUT_KEY, payload)
        }
        ClientMessage::Input(InputEvent::Mouse(event)) => {
            let mut payload = Vec::with_capacity(11);
            payload.extend_from_slice(&event.x.to_be_bytes());
            payload.extend_from_slice(&event.y.to_be_bytes());
            payload.push(match event.kind {
                MouseKind::Press => 0,
                MouseKind::Release => 1,
                MouseKind::Move => 2,
            });
            payload.push(match event.button {
                Some(MouseButton::Left) => 0,
                Some(MouseButton::Middle) => 1,
                Some(MouseButton::Right) => 2,
                Some(MouseButton::WheelUp) => 3,
                Some(MouseButton::WheelDown) => 4,
                None => u8::MAX,
            });
            payload.push(event.modifiers);
            (INPUT_MOUSE, payload)
        }
        ClientMessage::Input(InputEvent::Resize { width, height }) => {
            let mut payload = Vec::with_capacity(4);
            payload.extend_from_slice(&width.to_be_bytes());
            payload.extend_from_slice(&height.to_be_bytes());
            (INPUT_RESIZE, payload)
        }
        ClientMessage::Stop => (STOP, Vec::new()),
    };
    write_packet(writer, kind, &payload)
}

fn write_packet(writer: &mut impl Write, kind: u8, payload: &[u8]) -> Result<(), WireError> {
    if payload.len() > MAX_WIRE_PAYLOAD || payload.len() > u32::MAX as usize {
        return Err(WireError::PayloadTooLarge(payload.len()));
    }
    writer.write_all(&[kind]).map_err(WireError::Io)?;
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .map_err(WireError::Io)?;
    writer.write_all(payload).map_err(WireError::Io)
}

#[derive(Default)]
pub struct WireDecoder {
    buffer: Vec<u8>,
}

impl WireDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), WireError> {
        let size = self.buffer.len().saturating_add(bytes.len());
        if size > MAX_WIRE_PAYLOAD + 5 {
            return Err(WireError::PayloadTooLarge(size));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn next_packet(&mut self) -> Result<Option<(u8, Vec<u8>)>, WireError> {
        if self.buffer.len() < 5 {
            return Ok(None);
        }
        let length = u32::from_be_bytes(self.buffer[1..5].try_into().unwrap()) as usize;
        if length > MAX_WIRE_PAYLOAD {
            return Err(WireError::PayloadTooLarge(length));
        }
        let packet_length = 5usize
            .checked_add(length)
            .ok_or(WireError::PayloadTooLarge(length))?;
        if self.buffer.len() < packet_length {
            return Ok(None);
        }
        let kind = self.buffer[0];
        let payload = self.buffer[5..packet_length].to_vec();
        self.buffer.drain(..packet_length);
        Ok(Some((kind, payload)))
    }

    pub fn next_agent(&mut self) -> Result<Option<AgentMessage>, WireError> {
        let Some((kind, payload)) = self.next_packet()? else {
            return Ok(None);
        };
        let message = match kind {
            HELLO if payload.len() == 4 => AgentMessage::Hello {
                width: u16::from_be_bytes(payload[0..2].try_into().unwrap()),
                height: u16::from_be_bytes(payload[2..4].try_into().unwrap()),
            },
            FRAME if payload.len() >= 8 => {
                let width = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                let height = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                AgentMessage::Frame(
                    Frame::new_rgb(width, height, payload[8..].to_vec())
                        .map_err(|error| WireError::InvalidMessage(error.to_string()))?,
                )
            }
            EXIT => AgentMessage::Exit(decode_text(payload)?),
            ERROR => AgentMessage::Error(decode_text(payload)?),
            _ => {
                return Err(WireError::InvalidMessage(format!(
                    "agent message type {kind}"
                )));
            }
        };
        Ok(Some(message))
    }

    pub fn next_client(&mut self) -> Result<Option<ClientMessage>, WireError> {
        let Some((kind, payload)) = self.next_packet()? else {
            return Ok(None);
        };
        let message = match kind {
            AUTH => ClientMessage::Authenticate(decode_text(payload)?),
            INPUT_TEXT => ClientMessage::Input(InputEvent::Text(decode_text(payload)?)),
            INPUT_KEY if payload.len() >= 10 => {
                let text_length = u32::from_be_bytes(payload[6..10].try_into().unwrap()) as usize;
                if payload.len() != 10 + text_length {
                    return Err(WireError::InvalidMessage("key text length".into()));
                }
                let text = decode_text(payload[10..].to_vec())?;
                ClientMessage::Input(InputEvent::Key(KeyEvent {
                    code: u32::from_be_bytes(payload[0..4].try_into().unwrap()),
                    pressed: payload[4] != 0,
                    modifiers: payload[5],
                    text: (!text.is_empty()).then_some(text),
                }))
            }
            INPUT_MOUSE if payload.len() == 11 => {
                let kind = match payload[8] {
                    0 => MouseKind::Press,
                    1 => MouseKind::Release,
                    2 => MouseKind::Move,
                    value => return Err(WireError::InvalidMessage(format!("mouse kind {value}"))),
                };
                let button = match payload[9] {
                    0 => Some(MouseButton::Left),
                    1 => Some(MouseButton::Middle),
                    2 => Some(MouseButton::Right),
                    3 => Some(MouseButton::WheelUp),
                    4 => Some(MouseButton::WheelDown),
                    u8::MAX => None,
                    value => {
                        return Err(WireError::InvalidMessage(format!("mouse button {value}")));
                    }
                };
                ClientMessage::Input(InputEvent::Mouse(MouseEvent {
                    x: u32::from_be_bytes(payload[0..4].try_into().unwrap()),
                    y: u32::from_be_bytes(payload[4..8].try_into().unwrap()),
                    kind,
                    button,
                    modifiers: payload[10],
                }))
            }
            INPUT_RESIZE if payload.len() == 4 => ClientMessage::Input(InputEvent::Resize {
                width: u16::from_be_bytes(payload[0..2].try_into().unwrap()),
                height: u16::from_be_bytes(payload[2..4].try_into().unwrap()),
            }),
            STOP if payload.is_empty() => ClientMessage::Stop,
            _ => {
                return Err(WireError::InvalidMessage(format!(
                    "client message type {kind}"
                )));
            }
        };
        Ok(Some(message))
    }
}

fn decode_text(payload: Vec<u8>) -> Result<String, WireError> {
    String::from_utf8(payload).map_err(|_| WireError::InvalidMessage("text is not UTF-8".into()))
}

#[derive(Debug)]
pub enum WireError {
    Io(io::Error),
    PayloadTooLarge(usize),
    InvalidMessage(String),
    Unsupported(&'static str),
}

impl Display for WireError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => Display::fmt(error, formatter),
            Self::PayloadTooLarge(size) => write!(formatter, "wire payload is too large: {size}"),
            Self::InvalidMessage(message) => write!(formatter, "invalid wire message: {message}"),
            Self::Unsupported(message) => write!(formatter, "unsupported wire message: {message}"),
        }
    }
}

impl Error for WireError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_fragmented_frame() {
        let frame = Frame::new_rgb(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        let mut bytes = Vec::new();
        write_agent_message(&mut bytes, &AgentMessage::Frame(frame.clone())).unwrap();
        let mut decoder = WireDecoder::default();
        for byte in bytes {
            decoder.push(&[byte]).unwrap();
        }
        assert_eq!(
            decoder.next_agent().unwrap(),
            Some(AgentMessage::Frame(frame))
        );
    }

    #[test]
    fn round_trips_all_input_kinds() {
        let messages = [
            ClientMessage::Authenticate("secret".into()),
            ClientMessage::Input(InputEvent::Text("한글".into())),
            ClientMessage::Input(InputEvent::Key(KeyEvent {
                text: Some("a".into()),
                code: 42,
                pressed: true,
                modifiers: 3,
            })),
            ClientMessage::Input(InputEvent::Mouse(MouseEvent {
                x: 10,
                y: 20,
                button: Some(MouseButton::Left),
                kind: MouseKind::Press,
                modifiers: 0,
            })),
            ClientMessage::Input(InputEvent::Resize {
                width: 800,
                height: 600,
            }),
            ClientMessage::Stop,
        ];
        let mut bytes = Vec::new();
        for message in &messages {
            write_client_message(&mut bytes, message).unwrap();
        }
        let mut decoder = WireDecoder::default();
        decoder.push(&bytes).unwrap();
        for expected in messages {
            assert_eq!(decoder.next_client().unwrap(), Some(expected));
        }
    }

    #[test]
    fn rejects_oversized_header_before_body_arrives() {
        let mut decoder = WireDecoder::default();
        let too_large = (MAX_WIRE_PAYLOAD as u32 + 1).to_be_bytes();
        decoder
            .push(&[
                FRAME,
                too_large[0],
                too_large[1],
                too_large[2],
                too_large[3],
            ])
            .unwrap();
        assert!(matches!(
            decoder.next_agent(),
            Err(WireError::PayloadTooLarge(_))
        ));
    }
}
