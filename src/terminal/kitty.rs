use std::io::{self, Write};

use crate::renderer::Frame;

const KITTY_CHUNK_SIZE: usize = 4096;
const DEFAULT_IMAGE_ID: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemoOptions {
    pub width: u32,
    pub height: u32,
}

impl Default for DemoOptions {
    fn default() -> Self {
        Self {
            width: 320,
            height: 180,
        }
    }
}

impl DemoOptions {
    pub fn validate(self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("demo dimensions must be greater than zero".into());
        }
        if self.width > 4096 || self.height > 4096 {
            return Err("demo dimensions must not exceed 4096 pixels".into());
        }
        Ok(())
    }
}

pub struct KittyEncoder<W> {
    output: W,
}

impl<W: Write> KittyEncoder<W> {
    pub fn new(output: W) -> Self {
        Self { output }
    }

    pub fn transmit_rgb(&mut self, frame: &Frame, image_id: u32) -> io::Result<()> {
        self.transmit_rgb_inner(frame, image_id, None)
    }

    pub fn transmit_rgb_placed(
        &mut self,
        frame: &Frame,
        image_id: u32,
        columns: u16,
        rows: u16,
    ) -> io::Result<()> {
        self.transmit_rgb_inner(frame, image_id, Some((columns.max(1), rows.max(1))))
    }

    fn transmit_rgb_inner(
        &mut self,
        frame: &Frame,
        image_id: u32,
        placement: Option<(u16, u16)>,
    ) -> io::Result<()> {
        let encoded = encode_base64(frame.pixels());
        let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(KITTY_CHUNK_SIZE).collect();

        for (index, chunk) in chunks.iter().enumerate() {
            let more = usize::from(index + 1 != chunks.len());
            if index == 0 {
                match placement {
                    Some((columns, rows)) => write!(
                        self.output,
                        "\x1b_Ga=T,f=24,s={},v={},i={},q=1,c={columns},r={rows},m={more};",
                        frame.width(),
                        frame.height(),
                        image_id
                    )?,
                    None => write!(
                        self.output,
                        "\x1b_Ga=T,f=24,s={},v={},i={},q=1,m={more};",
                        frame.width(),
                        frame.height(),
                        image_id
                    )?,
                }
            } else {
                write!(self.output, "\x1b_Gm={more};")?;
            }
            self.output.write_all(chunk)?;
            self.output.write_all(b"\x1b\\")?;
        }
        self.output.flush()
    }

    pub fn delete(&mut self, image_id: u32) -> io::Result<()> {
        write!(self.output, "\x1b_Ga=d,d=I,i={image_id},q=1\x1b\\")?;
        self.output.flush()
    }

    pub fn into_inner(self) -> W {
        self.output
    }
}

pub fn render_demo<W: Write>(output: W, options: DemoOptions) -> io::Result<()> {
    let frame = checkerboard(options.width, options.height);
    let mut encoder = KittyEncoder::new(output);
    encoder.transmit_rgb(&frame, DEFAULT_IMAGE_ID)
}

fn checkerboard(width: u32, height: u32) -> Frame {
    Frame::from_rgb_fn(width, height, |x, y| {
        let light = ((x / 32) + (y / 32)) % 2 == 0;
        if light { [70, 211, 154] } else { [21, 32, 43] }
    })
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(c & 0x3f) as usize] as char
        } else {
            '='
        });
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
    }

    #[test]
    fn emits_chunked_kitty_commands() {
        let frame = Frame::new_rgb(1, 1, vec![255, 0, 0]).unwrap();
        let mut encoder = KittyEncoder::new(Vec::new());
        encoder.transmit_rgb(&frame, 7).unwrap();
        let bytes = encoder.into_inner();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text, "\x1b_Ga=T,f=24,s=1,v=1,i=7,q=1,m=0;/wAA\x1b\\");
    }

    #[test]
    fn includes_terminal_placement_when_requested() {
        let frame = Frame::new_rgb(1, 1, vec![0, 0, 0]).unwrap();
        let mut encoder = KittyEncoder::new(Vec::new());
        encoder.transmit_rgb_placed(&frame, 3, 80, 24).unwrap();
        let text = String::from_utf8(encoder.into_inner()).unwrap();
        assert!(text.starts_with("\x1b_Ga=T,f=24,s=1,v=1,i=3,q=1,c=80,r=24,m=0;"));
    }
}
