use super::{Frame, Rect};

#[derive(Debug, Clone, PartialEq)]
pub enum UpdatePlan {
    Unchanged,
    Full,
    Tiles(Vec<Rect>),
}

pub struct RenderPlanner {
    previous: Option<Frame>,
    tile_size: u32,
    full_threshold: f32,
}

impl RenderPlanner {
    pub fn new(tile_size: u32, full_threshold: f32) -> Self {
        assert!(tile_size > 0, "tile size must be non-zero");
        assert!(
            (0.0..=1.0).contains(&full_threshold),
            "full threshold must be between zero and one"
        );
        Self {
            previous: None,
            tile_size,
            full_threshold,
        }
    }

    pub fn plan(&mut self, current: &Frame) -> UpdatePlan {
        let plan = match &self.previous {
            None => UpdatePlan::Full,
            Some(previous)
                if previous.width() != current.width() || previous.height() != current.height() =>
            {
                UpdatePlan::Full
            }
            Some(previous) => {
                let tiles = current.dirty_tiles(previous, self.tile_size);
                if tiles.is_empty() {
                    UpdatePlan::Unchanged
                } else {
                    let dirty_pixels: u64 = tiles
                        .iter()
                        .map(|rect| u64::from(rect.width) * u64::from(rect.height))
                        .sum();
                    let total_pixels = u64::from(current.width()) * u64::from(current.height());
                    if dirty_pixels as f64 / total_pixels as f64 >= f64::from(self.full_threshold) {
                        UpdatePlan::Full
                    } else {
                        UpdatePlan::Tiles(tiles)
                    }
                }
            }
        };
        self.previous = Some(current.clone());
        plan
    }

    pub fn reset(&mut self) {
        self.previous = None;
    }

    pub fn tile_size(&self) -> u32 {
        self.tile_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(changed: &[(u32, u32)]) -> Frame {
        Frame::from_rgb_fn(8, 8, |x, y| {
            if changed.contains(&(x, y)) {
                [1, 1, 1]
            } else {
                [0, 0, 0]
            }
        })
    }

    #[test]
    fn plans_full_then_unchanged_then_tiles() {
        let mut planner = RenderPlanner::new(4, 0.5);
        assert_eq!(planner.plan(&frame(&[])), UpdatePlan::Full);
        assert_eq!(planner.plan(&frame(&[])), UpdatePlan::Unchanged);
        assert_eq!(
            planner.plan(&frame(&[(1, 1)])),
            UpdatePlan::Tiles(vec![Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            }])
        );
    }

    #[test]
    fn switches_to_full_when_dirty_area_crosses_threshold() {
        let mut planner = RenderPlanner::new(4, 0.5);
        planner.plan(&frame(&[]));
        assert_eq!(planner.plan(&frame(&[(1, 1), (5, 1)])), UpdatePlan::Full);
    }
}
