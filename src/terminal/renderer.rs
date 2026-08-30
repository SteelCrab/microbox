use std::collections::BTreeSet;
use std::io::{self, Write};

use crate::renderer::{Frame, Rect, RenderPlanner, UpdatePlan};

use super::KittyEncoder;

const BASE_IMAGE_ID: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderOutcome {
    Unchanged,
    Full,
    Tiles(usize),
}

pub struct KittyFrameRenderer<W> {
    encoder: KittyEncoder<W>,
    planner: RenderPlanner,
    active_tiles: BTreeSet<u32>,
    columns: u16,
    rows: u16,
}

impl<W: Write> KittyFrameRenderer<W> {
    pub fn new(output: W, columns: u16, rows: u16) -> Self {
        Self {
            encoder: KittyEncoder::new(output),
            planner: RenderPlanner::new(64, 0.35),
            active_tiles: BTreeSet::new(),
            columns: columns.max(1),
            rows: rows.max(1),
        }
    }

    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.columns = columns.max(1);
        self.rows = rows.max(1);
        self.planner.reset();
    }

    pub fn render(&mut self, frame: &Frame) -> io::Result<RenderOutcome> {
        match self.planner.plan(frame) {
            UpdatePlan::Unchanged => Ok(RenderOutcome::Unchanged),
            UpdatePlan::Full => {
                self.clear_tiles()?;
                self.encoder.transmit_rgb_at(
                    frame,
                    BASE_IMAGE_ID,
                    0,
                    0,
                    self.columns,
                    self.rows,
                )?;
                Ok(RenderOutcome::Full)
            }
            UpdatePlan::Tiles(tiles) => {
                let count = tiles.len();
                for rect in tiles {
                    self.render_tile(frame, rect)?;
                }
                Ok(RenderOutcome::Tiles(count))
            }
        }
    }

    pub fn clear(&mut self) -> io::Result<()> {
        self.clear_tiles()?;
        self.encoder.delete(BASE_IMAGE_ID)
    }

    pub fn into_inner(self) -> W {
        self.encoder.into_inner()
    }

    fn render_tile(&mut self, frame: &Frame, rect: Rect) -> io::Result<()> {
        let tile = frame
            .crop(rect)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let (column, row, columns, rows) = placement(rect, frame, self.columns, self.rows);
        let tiles_per_row = frame.width().div_ceil(self.planner.tile_size());
        let image_id = 2
            + rect.y / self.planner.tile_size() * tiles_per_row
            + rect.x / self.planner.tile_size();
        self.encoder
            .transmit_rgb_at(&tile, image_id, column, row, columns, rows)?;
        self.active_tiles.insert(image_id);
        Ok(())
    }

    fn clear_tiles(&mut self) -> io::Result<()> {
        for image_id in std::mem::take(&mut self.active_tiles) {
            self.encoder.delete(image_id)?;
        }
        Ok(())
    }
}

fn placement(rect: Rect, frame: &Frame, columns: u16, rows: u16) -> (u16, u16, u16, u16) {
    let frame_width = u64::from(frame.width());
    let frame_height = u64::from(frame.height());
    let terminal_columns = u64::from(columns);
    let terminal_rows = u64::from(rows);
    let left = u64::from(rect.x) * terminal_columns / frame_width;
    let top = u64::from(rect.y) * terminal_rows / frame_height;
    let right = (u64::from(rect.x + rect.width) * terminal_columns).div_ceil(frame_width);
    let bottom = (u64::from(rect.y + rect.height) * terminal_rows).div_ceil(frame_height);
    (
        left as u16,
        top as u16,
        (right.saturating_sub(left).max(1)) as u16,
        (bottom.saturating_sub(top).max(1)) as u16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_full_then_nothing_then_a_positioned_tile() {
        let before = Frame::new_rgb(256, 128, vec![0; 256 * 128 * 3]).unwrap();
        let after = Frame::from_rgb_fn(256, 128, |x, y| {
            if x == 1 && y == 1 {
                [1, 1, 1]
            } else {
                [0, 0, 0]
            }
        });
        let mut renderer = KittyFrameRenderer::new(Vec::new(), 80, 24);
        assert_eq!(renderer.render(&before).unwrap(), RenderOutcome::Full);
        assert_eq!(renderer.render(&before).unwrap(), RenderOutcome::Unchanged);
        assert_eq!(renderer.render(&after).unwrap(), RenderOutcome::Tiles(1));
        let output = String::from_utf8(renderer.into_inner()).unwrap();
        assert!(output.contains("i=1"));
        assert!(output.contains("\x1b[1;1H\x1b_Ga=T,f=24,s=64,v=64,i=2"));
    }
}
