use alloc::vec::Vec;

#[cfg(test)]
static SCANS: [core::sync::atomic::AtomicUsize; 2] = [const { core::sync::atomic::AtomicUsize::new(0) }; 2];
#[cfg(test)]
pub fn take_scan_counts() -> (usize, usize) {
    use core::sync::atomic::Ordering::Relaxed;
    (SCANS[0].swap(0, Relaxed), SCANS[1].swap(0, Relaxed))
}

/// Last published pixels in ordinary RAM. Never read back the hardware aperture.
pub struct Shadow(Vec<u8>);

impl Shadow {
    pub fn new(bytes: usize) -> Option<Self> {
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(bytes).ok()?;
        // Matches fill_screen(), including the stride padding.
        pixels.resize(bytes, 0xff);
        Some(Self(pixels))
    }

    pub fn copy(&mut self, offset: usize, source: &[u8], write: impl FnMut(usize, &[u8])) -> usize {
        copy_changed(&mut self.0[offset..offset + source.len()], source, write)
    }
}

fn copy_changed(previous: &mut [u8], source: &[u8], mut write: impl FnMut(usize, &[u8])) -> usize {
    // Compare cache-line-sized spans in RAM, merging adjacent changed spans so
    // moving pictures still use long copies. Partial rows never touch neighbors.
    const BLOCK: usize = 64 * 1024;
    let mut start = None;
    let mut written = 0;
    // Skip clean blocks throughout the region, including gaps and suffixes
    // after damage. Consecutive dirty blocks use the original fine loop once,
    // retaining long runs without nesting that loop inside every block.
    for (block, new) in source.chunks(BLOCK).enumerate() {
        let base = block * BLOCK;
        #[cfg(test)]
        SCANS[0].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if previous[base..base + new.len()] == *new {
            if let Some(first) = start.take() {
                written += copy_spans(&mut previous[first..base], &source[first..base],
                    |offset, pixels| write(first + offset, pixels));
            }
        } else {
            start.get_or_insert(base);
        }
    }
    if let Some(first) = start {
        if first == 0 {
            // Fully changed frames keep the same fine-loop invocation.
            written += copy_spans(previous, source, write);
        } else {
            written += copy_spans(&mut previous[first..], &source[first..],
                |offset, pixels| write(first + offset, pixels));
        }
    }
    written
}

fn copy_spans(previous: &mut [u8], source: &[u8], mut write: impl FnMut(usize, &[u8])) -> usize {
    const SPAN: usize = 64;
    let mut start = None;
    let mut written = 0;
    for (index, (old, new)) in previous.chunks_mut(SPAN)
        .zip(source.chunks(SPAN)).enumerate()
    {
        #[cfg(test)]
        SCANS[1].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let offset = index * SPAN;
        if old != new {
            start.get_or_insert(offset);
            old.copy_from_slice(new);
        } else if let Some(first) = start.take() {
            write(first, &source[first..offset]);
            written += offset - first;
        }
    }
    if let Some(first) = start {
        write(first, &source[first..]);
        written += source.len() - first;
    }
    written
}
