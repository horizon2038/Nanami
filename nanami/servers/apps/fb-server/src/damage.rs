use alloc::vec::Vec;

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
    if previous == source {
        return 0;
    }
    // Compare cache-line-sized spans in RAM, merging adjacent changed spans so
    // moving pictures still use long copies. Partial rows never touch neighbors.
    const SPAN: usize = 64;
    let mut start = None;
    let mut written = 0;
    for (index, (old, new)) in previous
        .chunks_mut(SPAN)
        .zip(source.chunks(SPAN))
        .enumerate()
    {
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
