//! Linux PT_INTERP loading. The Linux dynamic linker, not Alter, resolves DSOs.
use crate::elf::ElfMetadata;
use crate::loader::{load_interpreter_elf_image, LoadError, LoadedElfImage};
use crate::state::Runtime;
use nanami_runtime::Word;

const PAGE: Word = 4096;
const INTERPRETER_BASE: Word = 0x6000_0000;
const MAX_INTERPRETER_BYTES: Word = 16 * 1024 * 1024;

pub struct Interpreter {
    image: LoadedElfImage,
    pub load_bias: Word,
    pub entry: Word,
    base: Word,
    size: Word,
}

pub fn prepare(
    runtime: &mut Runtime,
    executable: &LoadedElfImage,
) -> Result<Option<Interpreter>, LoadError> {
    prepare_in_root(runtime, executable, b"/alter/linux")
}

pub fn prepare_in_root(
    runtime: &mut Runtime,
    executable: &LoadedElfImage,
    root: &[u8],
) -> Result<Option<Interpreter>, LoadError> {
    let meta = &executable.metadata;
    if !meta.has_interpreter {
        return Ok(None);
    }
    let end = meta
        .interpreter_offset
        .checked_add(meta.interpreter_size)
        .ok_or(LoadError::InvalidElf)?;
    if end > executable.size {
        return Err(LoadError::InvalidElf);
    }
    let path = unsafe {
        core::slice::from_raw_parts(
            (executable.address + meta.interpreter_offset) as *const u8,
            meta.interpreter_size as usize,
        )
    };
    if path.last() != Some(&0) || path[..path.len() - 1].contains(&0) || path.first() != Some(&b'/')
    {
        return Err(LoadError::InvalidElf);
    }
    let path = &path[..path.len() - 1];
    // Resolve under the Linux root, never the native Nanami filesystem.
    if path
        .split(|&b| b == b'/')
        .any(|component| component == b"..")
    {
        return Err(LoadError::UnsupportedElf);
    }
    let prefix = root;
    let mut host_path = [0u8; 256];
    let len = prefix
        .len()
        .checked_add(path.len())
        .filter(|&len| len <= host_path.len())
        .ok_or(LoadError::InvalidArgument)?;
    host_path[..prefix.len()].copy_from_slice(prefix);
    host_path[prefix.len()..len].copy_from_slice(path);
    let image = load_interpreter_elf_image(runtime, &host_path[..len], executable.address)?;
    if image.metadata.elf_type != 3 || image.metadata.has_interpreter {
        return Err(LoadError::UnsupportedElf);
    }
    let mut low = Word::MAX;
    let mut high = 0;
    for segment in image
        .metadata
        .segments
        .iter()
        .take(image.metadata.load_segment_count)
    {
        if segment
            .offset
            .checked_add(segment.file_size)
            .filter(|&end| end <= image.size)
            .is_none()
        {
            return Err(LoadError::InvalidElf);
        }
        if segment.memory_size == 0 {
            continue;
        }
        low = low.min(segment.virtual_address & !(PAGE - 1));
        high = high.max(
            segment
                .virtual_address
                .checked_add(segment.memory_size)
                .and_then(|end| end.checked_add(PAGE - 1))
                .ok_or(LoadError::InvalidElf)?
                & !(PAGE - 1),
        );
    }
    let size = high
        .checked_sub(low)
        .filter(|&size| size != 0 && size <= MAX_INTERPRETER_BYTES)
        .ok_or(LoadError::UnsupportedElf)?;
    let main_bias = if meta.elf_type == 3 { 0x400000 } else { 0 };
    for segment in meta.segments.iter().take(meta.load_segment_count) {
        let start = segment
            .virtual_address
            .checked_add(main_bias)
            .ok_or(LoadError::InvalidElf)?;
        let end = start
            .checked_add(segment.memory_size)
            .ok_or(LoadError::InvalidElf)?;
        if start < INTERPRETER_BASE + size && end > INTERPRETER_BASE {
            return Err(LoadError::UnsupportedElf);
        }
    }
    let load_bias = INTERPRETER_BASE
        .checked_sub(low)
        .ok_or(LoadError::UnsupportedElf)?;
    let entry = image
        .metadata
        .entry_point
        .checked_add(load_bias)
        .ok_or(LoadError::InvalidElf)?;
    if entry < INTERPRETER_BASE || entry >= INTERPRETER_BASE + size {
        return Err(LoadError::InvalidElf);
    }
    Ok(Some(Interpreter {
        image,
        load_bias,
        entry,
        base: INTERPRETER_BASE,
        size,
    }))
}

pub fn install(pid: Word, interpreter: &Interpreter) -> Result<(), LoadError> {
    let bytes = unsafe {
        core::slice::from_raw_parts(interpreter.image.address as *const u8, interpreter.image.size as usize)
    };
    if let Some((offset, copied)) = linear_file_prefix(
        bytes, &interpreter.image.metadata, interpreter.base - interpreter.load_bias, interpreter.size,
    ) {
        // One constructor supplies the same zeroed span and ordered file bytes.
        // On failure the caller discards this image; never retry a partial map.
        return nanami_runtime::request_process_map_initialized_at(
            pid, interpreter.base, interpreter.size,
            if copied == 0 { 0 } else { interpreter.image.address + offset }, copied,
        ).map_err(|_| LoadError::Io);
    }
    let (base, size) =
        nanami_runtime::request_process_map_anonymous_at(pid, interpreter.base, interpreter.size)
            .map_err(|_| LoadError::Io)?;
    if base != interpreter.base || size != interpreter.size {
        return Err(LoadError::Io);
    }
    for segment in interpreter
        .image
        .metadata
        .segments
        .iter()
        .take(interpreter.image.metadata.load_segment_count)
    {
        if segment.file_size != 0 {
            nanami_runtime::request_process_memory_write(
                pid,
                interpreter.load_bias + segment.virtual_address,
                interpreter.image.address + segment.offset,
                segment.file_size,
            )
            .map_err(|_| LoadError::Io)?;
        }
    }
    Ok(())
}

/// A single file prefix is equivalent only when every LOAD uses one linear
/// translation and every byte outside its file spans is already zero. Keep
/// nonzero padding and reordered/overlapping spans on the ordered write path.
fn linear_file_prefix(
    image: &[u8], metadata: &ElfMetadata, low: Word, size: Word,
) -> Option<(Word, Word)> {
    let mut source_offset = None;
    let mut covered = 0;
    for segment in metadata.segments.iter().take(metadata.load_segment_count) {
        if segment.file_size == 0 { continue; }
        let start = segment.virtual_address.checked_sub(low)?;
        let end = start.checked_add(segment.file_size)?;
        if start < covered || end > size { return None; }
        let offset = segment.offset.checked_sub(start)?;
        if source_offset.is_some_and(|previous| previous != offset) { return None; }
        source_offset = Some(offset);
        let gap_start = offset.checked_add(covered)? as usize;
        let gap_end = offset.checked_add(start)? as usize;
        let file_end = offset.checked_add(end)? as usize;
        // Bounds are checked before reading even a zero-length gap.
        if file_end > image.len() || !zero_filled(image.get(gap_start..gap_end)?) {
            return None;
        }
        covered = end;
    }
    Some((source_offset.unwrap_or(0), covered))
}

// Only owned, initialized image bytes reach this check. Slice chunks keep all
// word loads inside the gap, including unaligned starts and a short tail.
fn zero_filled(bytes: &[u8]) -> bool {
    let mut blocks = bytes.chunks_exact(32);
    for block in &mut blocks {
        let words = u64::from_ne_bytes(block[0..8].try_into().unwrap())
            | u64::from_ne_bytes(block[8..16].try_into().unwrap())
            | u64::from_ne_bytes(block[16..24].try_into().unwrap())
            | u64::from_ne_bytes(block[24..32].try_into().unwrap());
        if words != 0 {
            return false;
        }
    }
    let mut words = blocks.remainder().chunks_exact(8);
    for word in &mut words {
        if u64::from_ne_bytes(word.try_into().unwrap()) != 0 {
            return false;
        }
    }
    words.remainder().iter().all(|byte| *byte == 0)
}

#[cfg(test)]
mod zero_tests {
    extern crate alloc;
    use self::alloc::vec;

    #[test]
    fn word_zero_scan_matches_bytes_at_all_tail_and_poison_positions() {
        for offset in 0..32 {
            for len in [0, 1, 7, 8, 9, 15, 16, 23, 24, 31, 32, 33, 63, 64, 65, 127, 128, 129, 4095, 4096] {
                let mut storage = vec![0xa5; offset + len + 32];
                storage[offset..offset + len].fill(0);
                let check = |storage: &[u8]| {
                    let input = &storage[offset..offset + len];
                    assert_eq!(super::zero_filled(input), input.iter().all(|byte| *byte == 0), "offset={offset} len={len}");
                };
                check(&storage);
                for index in 0..len {
                    storage[offset + index] = 1 << (index % 8);
                    check(&storage);
                    storage[offset + index] = 0;
                }
            }
        }
    }
}

pub fn track(
    runtime: &mut Runtime,
    pid: Word,
    main: &ElfMetadata,
    interpreter: Option<&Interpreter>,
    initial_permissions: bool,
) -> bool {
    // Alpha alone certifies the raw, disjoint, nonzero LOAD layout. Callers
    // consume that acknowledgement before the stopped main image is changed.
    // Keep the ordinary protocol with older Alpha and all exceptional images.
    let bias = if main.elf_type == 3 { 0x400000 } else { 0 };
    for segment in main.segments.iter().take(main.load_segment_count) {
        if segment.memory_size == 0 {
            continue;
        }
        let Some(start) = segment.virtual_address.checked_add(bias) else {
            return false;
        };
        let Some(end) = start
            .checked_add(segment.memory_size)
            .and_then(|end| end.checked_add(PAGE - 1))
        else {
            return false;
        };
        let base = start & !(PAGE - 1);
        let prot = ((segment.flags & 4) >> 2) | (segment.flags & 2) | ((segment.flags & 1) << 2);
        if !runtime.add_mapping(pid, base, (end & !(PAGE - 1)) - base, prot) {
            return false;
        }
        if !initial_permissions && nanami_runtime::request_process_mapping_protect(
            pid,
            base,
            (end & !(PAGE - 1)) - base,
            prot,
        )
        .is_err()
        {
            return false;
        }
    }
    #[cfg(target_os = "none")]
    if initial_permissions && option_env!("NANAMI_INITIAL_RIGHTS_DIAG") == Some("1") {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static REPORTS: AtomicUsize = AtomicUsize::new(0);
        if REPORTS.fetch_add(1, Ordering::Relaxed) < 64 {
            nanami_runtime::println!("[elf.initial-rights] pid={} entry={:#x} loads={} protect-rpcs=0",
                pid, main.entry_point, main.segments.iter().take(main.load_segment_count)
                    .filter(|segment| segment.memory_size != 0).count());
        }
    }
    interpreter
        .map(|image| runtime.add_mapping(pid, image.base, image.size, 7))
        .unwrap_or(true)
}
