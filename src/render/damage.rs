//! Damage tracking
//!
//! Accumulates dirty rectangles per frame and forwards to wl_surface.damage_buffer.

use tiny_skia::IntRect;

#[derive(Debug, Clone, Default)]
pub struct DamageTracker {
    pub regions: Vec<IntRect>,
    pub full_redraw: bool,
}

impl DamageTracker {
    pub fn from_scene(scene: &super::scene::SceneGraph) -> Self {
        let mut tracker = Self::default();
        for rect in scene.dirty_rects() {
            tracker.add_rect(rect);
        }
        tracker.optimize();
        tracker
    }

    pub fn add_rect(&mut self, rect: IntRect) {
        self.regions.push(rect);
    }

    pub fn set_full_redraw(&mut self) {
        self.full_redraw = true;
        self.regions.clear();
    }

    pub fn clear(&mut self) {
        self.regions.clear();
        self.full_redraw = false;
    }

    pub fn optimize(&mut self) {
        if self.full_redraw || self.regions.len() < 2 {
            return;
        }

        let mut merged = Vec::with_capacity(self.regions.len());
        for rect in self.regions.drain(..) {
            let mut current = rect;
            let mut index = 0;
            while index < merged.len() {
                if touches_or_overlaps(merged[index], current) {
                    current = union(merged.remove(index), current);
                } else {
                    index += 1;
                }
            }
            merged.push(current);
        }
        self.regions = merged;
    }
}

fn touches_or_overlaps(first: IntRect, second: IntRect) -> bool {
    first.left() <= second.right()
        && second.left() <= first.right()
        && first.top() <= second.bottom()
        && second.top() <= first.bottom()
}

fn union(first: IntRect, second: IntRect) -> IntRect {
    IntRect::from_ltrb(
        first.left().min(second.left()),
        first.top().min(second.top()),
        first.right().max(second.right()),
        first.bottom().max(second.bottom()),
    )
    .expect("union of valid IntRects must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> IntRect {
        IntRect::from_xywh(x, y, width, height).unwrap()
    }

    #[test]
    fn optimize_merges_touching_regions() {
        let mut damage = DamageTracker::default();
        damage.add_rect(rect(0, 0, 10, 10));
        damage.add_rect(rect(10, 0, 5, 10));
        damage.add_rect(rect(40, 40, 2, 2));

        damage.optimize();

        assert_eq!(damage.regions.len(), 2);
        assert!(damage.regions.contains(&rect(0, 0, 15, 10)));
    }
}
