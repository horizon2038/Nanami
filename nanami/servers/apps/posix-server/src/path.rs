use core::ptr;

use libnanami::Word;
use nanami_services::posix::*;

use crate::state::{FdKind, Runtime, MAX_COMPONENTS, PATH_MAX};

pub(crate) fn resolve_client_path(
    runtime: &Runtime,
    session_index: usize,
    path_offset: usize,
    path_len: usize,
) -> Option<([u8; PATH_MAX], usize)> {
    let session = runtime.sessions[session_index];
    if path_len == 0
        || path_offset.checked_add(path_len)? > session.shm_size as usize
        || path_len > PATH_MAX
    {
        return None;
    }
    let input = unsafe {
        core::slice::from_raw_parts((session.shm_local as usize + path_offset) as *const u8, path_len)
    };
    resolve_path(&session.cwd[..session.cwd_len], input)
}

pub(crate) fn resolve_path(cwd: &[u8], arg: &[u8]) -> Option<([u8; PATH_MAX], usize)> {
    let mut raw = [0u8; PATH_MAX];
    let mut raw_len = 0usize;
    if arg.first() == Some(&b'/') {
        raw_len = copy_path_part(&mut raw, raw_len, arg)?;
    } else {
        raw_len = copy_path_part(&mut raw, raw_len, cwd)?;
        if raw_len != 1 {
            raw_len = push_path_byte(&mut raw, raw_len, b'/')?;
        }
        raw_len = copy_path_part(&mut raw, raw_len, arg)?;
    }
    normalize_path(&raw[..raw_len])
}

fn copy_path_part(dst: &mut [u8; PATH_MAX], mut len: usize, src: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < src.len() {
        len = push_path_byte(dst, len, src[i])?;
        i += 1;
    }
    Some(len)
}

fn push_path_byte(dst: &mut [u8; PATH_MAX], len: usize, byte: u8) -> Option<usize> {
    if len >= PATH_MAX {
        return None;
    }
    dst[len] = byte;
    Some(len + 1)
}

fn normalize_path(raw: &[u8]) -> Option<([u8; PATH_MAX], usize)> {
    let mut starts = [0usize; MAX_COMPONENTS];
    let mut lens = [0usize; MAX_COMPONENTS];
    let mut count = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        while i < raw.len() && raw[i] == b'/' {
            i += 1;
        }
        let start = i;
        while i < raw.len() && raw[i] != b'/' {
            i += 1;
        }
        let len = i.saturating_sub(start);
        if len == 0 || (len == 1 && raw[start] == b'.') {
            continue;
        }
        if len == 2 && raw[start] == b'.' && raw[start + 1] == b'.' {
            count = count.saturating_sub(1);
            continue;
        }
        if count >= MAX_COMPONENTS {
            return None;
        }
        starts[count] = start;
        lens[count] = len;
        count += 1;
    }
    let mut out = [0u8; PATH_MAX];
    let mut out_len = 1usize;
    out[0] = b'/';
    let mut component = 0usize;
    while component < count {
        if out_len != 1 {
            out_len = push_path_byte(&mut out, out_len, b'/')?;
        }
        let mut j = 0usize;
        while j < lens[component] {
            out_len = push_path_byte(&mut out, out_len, raw[starts[component] + j])?;
            j += 1;
        }
        component += 1;
    }
    Some((out, out_len))
}

pub(crate) fn write_vfs_path(runtime: &Runtime, offset: usize, path: &[u8]) {
    unsafe {
        ptr::copy_nonoverlapping(
            path.as_ptr(),
            (runtime.vfs_shm as usize + offset) as *mut u8,
            path.len(),
        );
    }
}

pub(crate) fn special_device_kind(path: &[u8]) -> FdKind {
    if bytes_eq(path, b"/dev/null") {
        FdKind::DevNull
    } else if bytes_eq(path, b"/dev/zero") {
        FdKind::DevZero
    } else {
        FdKind::Empty
    }
}

pub(crate) fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub(crate) fn path_basename(path: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut i = 0usize;
    while i < path.len() {
        if path[i] == b'/' {
            start = i + 1;
        }
        i += 1;
    }
    &path[start..]
}

pub(crate) fn vfs_kind_to_posix(kind: Word) -> Word {
    match kind {
        nanami_services::vfs::VFS_FILE_TYPE_REGULAR => POSIX_FILE_TYPE_REGULAR,
        nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => POSIX_FILE_TYPE_DIRECTORY,
        _ => POSIX_FILE_TYPE_UNKNOWN,
    }
}

pub(crate) fn pack_stat(size: Word, kind: Word, major: Word, minor: Word) -> Word {
    (size & POSIX_STAT_SIZE_MASK)
        | ((kind & 0xff) << POSIX_STAT_TYPE_SHIFT)
        | ((major & 0xff) << POSIX_STAT_MAJOR_SHIFT)
        | ((minor & 0xffff) << POSIX_STAT_MINOR_SHIFT)
}
