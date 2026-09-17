/// Bound active completion polling to one USB frame. MFINDEX increments every
/// 125 us and wraps at 14 bits. The batch limit also bounds a stopped counter.
pub(super) struct CompletionPoll {
    start: u32,
    batches: usize,
}

impl CompletionPoll {
    pub fn new(mfindex: u32) -> Self {
        Self {
            start: mfindex,
            batches: 256,
        }
    }

    pub fn keep_polling(&mut self, mfindex: u32) -> bool {
        self.batches = if mfindex.wrapping_sub(self.start) & 0x3fff < 8 {
            self.batches.saturating_sub(1)
        } else {
            0
        };
        self.batches != 0
    }
}
