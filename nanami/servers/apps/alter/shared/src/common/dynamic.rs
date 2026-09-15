//! Linux PT_INTERP loading. The Linux dynamic linker, not Alter, resolves DSOs.
use crate::elf::ElfMetadata;
use crate::loader::{load_interpreter_elf_image, LoadError, LoadedElfImage};
use crate::state::Runtime;
use libnanami::Word;

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
    let meta = executable.metadata;
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
    if path.last() != Some(&0)
        || path[..path.len() - 1].contains(&0)
        || path.first() != Some(&b'/')
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
    let prefix = b"/alter/linux";
    let mut host_path = [0u8; 256];
    let len = prefix
        .len()
        .checked_add(path.len())
        .filter(|&len| len <= host_path.len())
        .ok_or(LoadError::InvalidArgument)?;
    host_path[..prefix.len()].copy_from_slice(prefix);
    host_path[prefix.len()..len].copy_from_slice(path);
    let image = load_interpreter_elf_image(runtime, &host_path[..len])?;
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
    let (base, size) =
        libnanami::request_process_map_anonymous_at(pid, interpreter.base, interpreter.size)
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
            libnanami::request_process_memory_write(
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

pub fn track(
    runtime: &mut Runtime,
    pid: Word,
    main: &ElfMetadata,
    interpreter: Option<&Interpreter>,
) -> bool {
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
        let prot =
            ((segment.flags & 4) >> 2) | (segment.flags & 2) | ((segment.flags & 1) << 2);
        if !runtime.add_mapping(pid, base, (end & !(PAGE - 1)) - base, prot) {
            return false;
        }
    }
    interpreter
        .map(|image| runtime.add_mapping(pid, image.base, image.size, 7))
        .unwrap_or(true)
}
