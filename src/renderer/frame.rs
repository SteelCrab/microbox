use std::error::Error;
use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Frame {
    pub fn new_rgb(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, FrameError> {
        let expected = pixel_bytes(width, height).ok_or(FrameError::DimensionsOverflow)?;
        if pixels.len() != expected {
            return Err(FrameError::InvalidBufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn from_rgb_fn(
        width: u32,
        height: u32,
        mut pixel: impl FnMut(u32, u32) -> [u8; 3],
    ) -> Self {
        let capacity = pixel_bytes(width, height).expect("frame dimensions overflow");
        let mut pixels = Vec::with_capacity(capacity);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&pixel(x, y));
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn crop(&self, rect: Rect) -> Result<Self, FrameError> {
        let right = rect
            .x
            .checked_add(rect.width)
            .ok_or(FrameError::DimensionsOverflow)?;
        let bottom = rect
            .y
            .checked_add(rect.height)
            .ok_or(FrameError::DimensionsOverflow)?;
        if rect.width == 0 || rect.height == 0 || right > self.width || bottom > self.height {
            return Err(FrameError::InvalidCrop(rect));
        }
        let mut pixels = Vec::with_capacity(pixel_bytes(rect.width, rect.height).unwrap());
        let stride = self.width as usize * 3;
        let row_bytes = rect.width as usize * 3;
        for row in rect.y..bottom {
            let start = row as usize * stride + rect.x as usize * 3;
            pixels.extend_from_slice(&self.pixels[start..start + row_bytes]);
        }
        Self::new_rgb(rect.width, rect.height, pixels)
    }

    pub fn dirty_tiles(&self, previous: &Self, tile_size: u32) -> Vec<Rect> {
        if tile_size == 0 || self.width != previous.width || self.height != previous.height {
            return vec![Rect {
                x: 0,
                y: 0,
                width: self.width,
                height: self.height,
            }];
        }

        let mut dirty = Vec::new();
        for y in (0..self.height).step_by(tile_size as usize) {
            for x in (0..self.width).step_by(tile_size as usize) {
                let width = tile_size.min(self.width - x);
                let height = tile_size.min(self.height - y);
                if self.tile_differs(previous, x, y, width, height) {
                    dirty.push(Rect {
                        x,
                        y,
                        width,
                        height,
                    });
                }
            }
        }
        dirty
    }

    fn tile_differs(&self, previous: &Self, x: u32, y: u32, width: u32, height: u32) -> bool {
        let stride = self.width as usize * 3;
        let row_bytes = width as usize * 3;
        for row in y..y + height {
            let start = row as usize * stride + x as usize * 3;
            if self.pixels[start..start + row_bytes] != previous.pixels[start..start + row_bytes] {
                return true;
            }
        }
        false
    }
}

fn pixel_bytes(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(3)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    DimensionsOverflow,
    InvalidBufferLength { expected: usize, actual: usize },
    InvalidCrop(Rect),
}

impl Display for FrameError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionsOverflow => write!(formatter, "frame dimensions overflow"),
            Self::InvalidBufferLength { expected, actual } => write!(
                formatter,
                "invalid RGB buffer length: expected {expected} bytes, got {actual}"
            ),
            Self::InvalidCrop(rect) => write!(formatter, "invalid frame crop {rect:?}"),
        }
    }
}

impl Error for FrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_buffer_size() {
        assert_eq!(
            Frame::new_rgb(2, 1, vec![0; 5]).unwrap_err(),
            FrameError::InvalidBufferLength {
                expected: 6,
                actual: 5,
            }
        );
    }

    #[test]
    fn identifies_only_changed_tiles() {
        let before = Frame::new_rgb(4, 4, vec![0; 4 * 4 * 3]).unwrap();
        let after = Frame::from_rgb_fn(4, 4, |x, y| {
            if x == 3 && y == 0 {
                [1, 1, 1]
            } else {
                [0, 0, 0]
            }
        });
        assert_eq!(
            after.dirty_tiles(&before, 2),
            vec![Rect {
                x: 2,
                y: 0,
                width: 2,
                height: 2,
            }]
        );
    }

    #[test]
    fn crops_rgb_rows_without_padding() {
        let frame = Frame::from_rgb_fn(3, 2, |x, y| [x as u8, y as u8, 9]);
        let crop = frame
            .crop(Rect {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            })
            .unwrap();
        assert_eq!(crop.pixels(), &[1, 0, 9, 2, 0, 9, 1, 1, 9, 2, 1, 9]);
    }
}
