use crate::framebuffer::Rect;

/// Intermediate positions have not been drawn and need no framebuffer writes.
/// Keep the erased and drawn bounds separate: their bounding box can be huge.
#[derive(Default)]
pub struct MotionDamage {
    pending: Option<(Rect, Rect)>,
}

impl MotionDamage {
    pub fn update(&mut self, before: Rect, after: Rect) {
        match &mut self.pending {
            Some((_, latest)) => *latest = after,
            None => self.pending = Some((before, after)),
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn take(&mut self) -> Option<(Rect, Rect)> {
        self.pending.take()
    }
}
