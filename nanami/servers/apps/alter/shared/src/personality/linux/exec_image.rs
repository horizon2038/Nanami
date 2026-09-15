use super::{
    align_down_word, align_up_word, load_cached_fork_linux_elf_image, map_request_error,
    personality, read_shm_u16, read_shm_u32, read_shm_u64, read_shm_u8, ElfMetadata, LoadError,
    OsPersonality, Runtime, Word, EINVAL, EIO, ENAMETOOLONG, ENOENT, ENOEXEC, ENOMEM,
    FREEBSD_IMAGE_BASE, LINUX_ELF_HEADER_BYTES, LINUX_IMAGE_BASE, LINUX_PAGE_SIZE, LINUX_PT_LOAD,
    LINUX_PT_TLS, NANAMI_IMAGE_BASE,
};

pub(super) fn spawn_fork_child(
    runtime: &mut Runtime,
    image_name: &[u8],
    pcb_slot: Word,
    personality: OsPersonality,
) -> Result<Word, i32> {
    spawn_rootfs_fork_child(runtime, image_name, pcb_slot, personality)
}

pub(super) fn spawn_rootfs_fork_child(
    runtime: &mut Runtime,
    image_name: &[u8],
    pcb_slot: Word,
    personality: OsPersonality,
) -> Result<Word, i32> {
    let path_len = write_rootfs_image_path(runtime, image_name, personality)?;
    let loaded =
        load_cached_fork_linux_elf_image(runtime, 0, path_len as Word).map_err(map_load_error)?;
    libnanami::request_process_spawn_memory_fault_handler_suspended(
        loaded.address,
        loaded.size,
        4,
        pcb_slot,
    )
    .map_err(map_request_error)
}

pub(super) fn write_rootfs_image_path(
    runtime: &mut Runtime,
    image_name: &[u8],
    personality: OsPersonality,
) -> Result<usize, i32> {
    let prefix = personality::bin_prefix(personality);
    let prefix_len = if image_name.first() == Some(&b'/') {
        0
    } else {
        prefix.len()
    };
    let len = prefix_len
        .checked_add(image_name.len())
        .ok_or(ENAMETOOLONG)?;
    if len == 0 || len as Word > runtime.client_shm_size {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        if prefix_len != 0 {
            ::core::ptr::copy_nonoverlapping(
                prefix.as_ptr(),
                runtime.client_shm as *mut u8,
                prefix_len,
            );
        }
        ::core::ptr::copy_nonoverlapping(
            image_name.as_ptr(),
            (runtime.client_shm as usize + prefix_len) as *mut u8,
            image_name.len(),
        );
    }
    Ok(len)
}

pub(super) fn copy_current_path_to_client_shm(runtime: &mut Runtime, len: Word) -> Result<(), i32> {
    if len == 0 || len > runtime.client_shm_size || len > runtime.posix_shm_size {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            runtime.client_shm as *mut u8,
            len as usize,
        );
    }
    Ok(())
}

pub(super) fn map_load_error(error: LoadError) -> i32 {
    match error {
        LoadError::InvalidArgument => EINVAL,
        LoadError::NotFound => ENOENT,
        LoadError::Io => EIO,
        LoadError::InvalidElf | LoadError::UnsupportedElf => ENOEXEC,
    }
}

pub(super) fn log_execve_load_error(
    runtime: &Runtime,
    pid: Word,
    path_len: Word,
    error: LoadError,
) {
    let path = unsafe {
        ::core::slice::from_raw_parts(
            runtime.client_shm as *const u8,
            ::core::cmp::min(path_len as usize, 96),
        )
    };
    match ::core::str::from_utf8(path) {
        Ok(text) => libnanami::println!(
            "[alter/linux] execve load failed pid={} path={} err={}",
            pid,
            text,
            load_error_name(error)
        ),
        Err(_) => libnanami::println!(
            "[alter/linux] execve load failed pid={} path-len={} err={}",
            pid,
            path_len,
            load_error_name(error)
        ),
    }
}

pub(super) fn load_error_name(error: LoadError) -> &'static str {
    match error {
        LoadError::InvalidArgument => "invalid-argument",
        LoadError::NotFound => "not-found",
        LoadError::Io => "io",
        LoadError::InvalidElf => "invalid-elf",
        LoadError::UnsupportedElf => "unsupported-elf",
    }
}

#[derive(Clone, Copy)]
pub(super) struct CurrentElfMetadata {
    pub(super) entry_point: Word,
    pub(super) program_header_vaddr: Word,
    pub(super) program_header_entry_size: Word,
    pub(super) program_header_count: Word,
    pub(super) tls_vaddr: Word,
    pub(super) tls_file_size: Word,
    pub(super) tls_memory_size: Word,
    pub(super) tls_align: Word,
}

pub(super) fn preferred_exec_image_base(
    personality: OsPersonality,
    metadata: &ElfMetadata,
) -> Word {
    if metadata.elf_type == 3 {
        return if personality == OsPersonality::FreeBsd {
            FREEBSD_IMAGE_BASE
        } else {
            LINUX_IMAGE_BASE
        };
    }
    align_down_word(metadata.first_load.virtual_address, LINUX_PAGE_SIZE)
}

pub(super) fn current_elf_metadata_from_loaded(
    metadata: &ElfMetadata,
    image_base: Word,
) -> CurrentElfMetadata {
    let load_bias = if metadata.elf_type == 3 {
        image_base
    } else {
        0
    };
    CurrentElfMetadata {
        entry_point: metadata.entry_point + load_bias,
        program_header_vaddr: if metadata.program_header_vaddr == 0 {
            0
        } else {
            metadata.program_header_vaddr + load_bias
        },
        program_header_entry_size: metadata.program_header_entry_size,
        program_header_count: metadata.program_header_count,
        tls_vaddr: if metadata.tls_vaddr == 0 {
            0
        } else {
            metadata.tls_vaddr + load_bias
        },
        tls_file_size: metadata.tls_file_size,
        tls_memory_size: metadata.tls_memory_size,
        tls_align: metadata.tls_align,
    }
}

pub(super) fn read_current_elf_metadata(
    runtime: &mut Runtime,
    pid: Word,
    preferred_image_base: Word,
) -> Result<CurrentElfMetadata, i32> {
    let candidates = [
        preferred_image_base,
        LINUX_IMAGE_BASE,
        FREEBSD_IMAGE_BASE,
        NANAMI_IMAGE_BASE,
        0,
    ];
    let mut i = 0usize;
    while i < candidates.len() {
        let candidate = candidates[i];
        if i != 0 && candidates[..i].contains(&candidate) {
            i += 1;
            continue;
        }
        if let Some(metadata) = read_current_elf_metadata_at(runtime, pid, candidate)? {
            return Ok(metadata);
        }
        i += 1;
    }
    Err(EINVAL)
}

pub(super) fn read_current_elf_metadata_at(
    runtime: &mut Runtime,
    pid: Word,
    image_base: Word,
) -> Result<Option<CurrentElfMetadata>, i32> {
    if libnanami::request_process_memory_read(
        pid,
        image_base,
        runtime.posix_shm,
        LINUX_ELF_HEADER_BYTES,
    )
    .is_err()
    {
        return Ok(None);
    }
    if read_shm_u8(runtime, 0) != 0x7f
        || read_shm_u8(runtime, 1) != b'E'
        || read_shm_u8(runtime, 2) != b'L'
        || read_shm_u8(runtime, 3) != b'F'
    {
        return Ok(None);
    }
    let phoff = read_shm_u64(runtime, 32);
    let phentsize = read_shm_u16(runtime, 54) as Word;
    let phnum = read_shm_u16(runtime, 56) as Word;
    if phentsize < 56 || phnum == 0 {
        return Err(EINVAL);
    }

    let elf_type = read_shm_u16(runtime, 16);
    let load_bias = if elf_type == 3 { image_base } else { 0 };
    let mut metadata = CurrentElfMetadata {
        entry_point: read_shm_u64(runtime, 24) + load_bias,
        program_header_vaddr: image_base + phoff,
        program_header_entry_size: phentsize,
        program_header_count: phnum,
        tls_vaddr: 0,
        tls_file_size: 0,
        tls_memory_size: 0,
        tls_align: 0,
    };
    let mut i = 0;
    while i < phnum {
        let base = phoff + i * phentsize;
        if base + 56 > LINUX_ELF_HEADER_BYTES {
            break;
        }
        let p_type = read_shm_u32(runtime, base as usize);
        if p_type == LINUX_PT_TLS {
            metadata.tls_vaddr = read_shm_u64(runtime, (base + 16) as usize) + load_bias;
            metadata.tls_file_size = read_shm_u64(runtime, (base + 32) as usize);
            metadata.tls_memory_size = read_shm_u64(runtime, (base + 40) as usize);
            metadata.tls_align = read_shm_u64(runtime, (base + 48) as usize);
        }
        if p_type == LINUX_PT_LOAD {
            let offset = read_shm_u64(runtime, (base + 8) as usize);
            let vaddr = read_shm_u64(runtime, (base + 16) as usize);
            let filesz = read_shm_u64(runtime, (base + 32) as usize);
            if phoff >= offset && phoff < offset.saturating_add(filesz) {
                metadata.program_header_vaddr = vaddr + load_bias + (phoff - offset);
            }
        }
        i += 1;
    }
    Ok(Some(metadata))
}

pub(super) fn install_freebsd_exec_tls(
    runtime: &mut Runtime,
    pid: Word,
    elf: &CurrentElfMetadata,
) -> Result<Word, i32> {
    if elf.tls_memory_size == 0 {
        return Ok(0);
    }
    if elf.tls_file_size > elf.tls_memory_size {
        return Err(EINVAL);
    }
    let total = align_up_word(elf.tls_memory_size + 16, 16);
    if total == 0 || total > runtime.posix_shm_size {
        return Err(ENOMEM);
    }
    let (tls_base, mapped) =
        libnanami::request_process_map_anonymous(pid, total).map_err(map_request_error)?;
    if mapped < total {
        return Err(ENOMEM);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, total as usize);
    }
    if elf.tls_file_size != 0 {
        libnanami::request_process_memory_read(
            pid,
            elf.tls_vaddr,
            runtime.posix_shm,
            elf.tls_file_size,
        )
        .map_err(map_request_error)?;
    }
    let fs_base = tls_base + elf.tls_memory_size;
    unsafe {
        ::core::ptr::write_unaligned(
            (runtime.posix_shm + elf.tls_memory_size) as *mut Word,
            fs_base,
        );
    }
    libnanami::request_process_memory_write(pid, tls_base, runtime.posix_shm, total)
        .map_err(map_request_error)?;
    libnanami::println!(
        "[alter/freebsd] exec tls pid={} base={:#x} fs={:#x} file={:#x} mem={:#x}",
        pid,
        tls_base,
        fs_base,
        elf.tls_file_size,
        elf.tls_memory_size
    );
    Ok(fs_base)
}
