//! A bounded, enter-driven io_uring backend. SQ/CQ pages are real shared RAM;
//! unsupported worker/resource-registration features are not advertised.
use super::*;
use crate::state::poll_objects::{IoUring, MemoryFile, Object};
use crate::state::readiness::{PollFd, ReadinessWait, READY_OBJECT};
use alloc::{
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};
use core::sync::atomic::{AtomicU32, Ordering};

const MAX_ENTRIES: u32 = 256;
const CQSIZE: u32 = 1 << 3;
const CLAMP: u32 = 1 << 4;
const NO_SQARRAY: u32 = 1 << 16;
const SQ_ARRAY: Word = 128;
const CQ_HEAD: Word = 32;
const CQ_TAIL: Word = 36;
const IO_LINK: u8 = 1 << 2;
const IO_HARDLINK: u8 = 1 << 3;
const ECANCELED: i32 = 125;
const OPS: &[u8] = &[0, 1, 2, 3, 18, 19, 21, 22, 23, 29, 35, 36, 37, 38, 39, 55];

fn object(runtime: &Runtime, id: Word) -> Result<&IoUring, i32> {
    match runtime.poll_objects.get(id) {
        Some(Object::Ring(ring)) => Ok(ring),
        _ => Err(EBADF),
    }
}

fn object_mut(runtime: &mut Runtime, id: Word) -> Result<&mut IoUring, i32> {
    match runtime.poll_objects.get_mut(id) {
        Some(Object::Ring(ring)) => Ok(ring),
        _ => Err(EBADF),
    }
}

fn ring_id(runtime: &Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::IoUring {
        return Err(EINVAL);
    }
    object(runtime, file.resource)?;
    Ok(file.resource)
}

fn allocate_memory(runtime: &mut Runtime, bytes: Word) -> Result<Word, i32> {
    let id = runtime
        .poll_objects
        .insert(Object::Memory(MemoryFile {
            sysv: None,
            backing: None,
            anonymous: true,
            pages: BTreeMap::new(),
            size: bytes,
            offset: 0,
            status_flags: 0,
            seals: 0,
            maps: Vec::new(),
        }))
        .ok_or(ENOMEM)?;
    for index in 0..bytes.div_ceil(LINUX_PAGE_SIZE) {
        match memfd::page(runtime, id, index) {
            Ok(address) => unsafe {
                core::ptr::write_bytes(address as *mut u8, 0, LINUX_PAGE_SIZE);
            },
            Err(errno) => {
                memfd::collect(runtime, id);
                return Err(errno);
            }
        }
    }
    Ok(id)
}

fn pointer(runtime: &mut Runtime, memory: Word, offset: Word) -> Result<Word, i32> {
    Ok(memfd::page(runtime, memory, offset / LINUX_PAGE_SIZE)? + offset % LINUX_PAGE_SIZE)
}

fn load(runtime: &mut Runtime, memory: Word, offset: Word) -> Result<u32, i32> {
    let address = pointer(runtime, memory, offset)?;
    Ok(unsafe { (&*(address as *const AtomicU32)).load(Ordering::Acquire) })
}

fn store(runtime: &mut Runtime, memory: Word, offset: Word, value: u32) -> Result<(), i32> {
    let address = pointer(runtime, memory, offset)?;
    unsafe {
        (&*(address as *const AtomicU32)).store(value, Ordering::Release);
    }
    Ok(())
}

pub(super) fn setup(
    runtime: &mut Runtime,
    pid: Word,
    entries: Word,
    params: Word,
) -> Result<Word, i32> {
    read_target_memory(runtime, pid, params, 120)?;
    let input = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u32, 30) };
    let flags = input[2];
    let requested_cq = input[1];
    if input[7..10].iter().any(|word| *word != 0) || flags & !(CQSIZE | CLAMP | NO_SQARRAY) != 0 {
        return Err(EINVAL);
    }
    if entries == 0 || (entries > MAX_ENTRIES as Word && flags & CLAMP == 0) {
        return Err(EINVAL);
    }
    let entries = (entries.min(MAX_ENTRIES as Word) as u32).next_power_of_two();
    let cq_entries = if flags & CQSIZE != 0 {
        if requested_cq < entries || (requested_cq > MAX_ENTRIES * 2 && flags & CLAMP == 0) {
            return Err(EINVAL);
        }
        requested_cq.min(MAX_ENTRIES * 2).next_power_of_two()
    } else {
        entries * 2
    };
    let array = flags & NO_SQARRAY == 0;
    let cq_offset = align_up_word(SQ_ARRAY + if array { entries as Word * 4 } else { 0 }, 64);
    let ring_bytes = align_up_word(cq_offset + cq_entries as Word * 16, LINUX_PAGE_SIZE);
    let sqe_bytes = align_up_word(entries as Word * 64, LINUX_PAGE_SIZE);
    let memory = allocate_memory(runtime, ring_bytes)?;
    let sqes = match allocate_memory(runtime, sqe_bytes) {
        Ok(id) => id,
        Err(errno) => {
            memfd::collect(runtime, memory);
            return Err(errno);
        }
    };
    let result = (|| {
        store(runtime, memory, 8, entries - 1)?;
        store(runtime, memory, 12, entries)?;
        store(runtime, memory, 40, cq_entries - 1)?;
        store(runtime, memory, 44, cq_entries)?;
        unsafe {
            core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, 120);
        }
        for (index, value) in [
            (0, entries),
            (1, cq_entries),
            (2, flags),
            // SINGLE_MMAP | NODROP. In particular, do not claim RSRC_TAGS.
            (5, 3),
            (10, 0),
            (11, 4),
            (12, 8),
            (13, 12),
            (14, 16),
            (15, 20),
            (16, if array { SQ_ARRAY as u32 } else { 0 }),
            (20, CQ_HEAD as u32),
            (21, CQ_TAIL as u32),
            (22, 40),
            (23, 44),
            (24, 48),
            (25, cq_offset as u32),
            (26, 52),
        ] {
            unsafe {
                ((runtime.posix_shm + index * 4) as *mut u32).write_unaligned(value);
            }
        }
        write_target_memory(runtime, pid, params, 120)?;
        eventfd::allocate(
            runtime,
            pid,
            LinuxFileKind::IoUring,
            Object::Ring(IoUring {
                memory,
                sqes,
                entries,
                cq_entries,
                cq_offset,
                ring_bytes,
                sqe_bytes,
                array,
                head: 0,
                tail: 0,
                cancel_next: false,
                overflow: VecDeque::new(),
            }),
            LINUX_O_CLOEXEC,
        )
    })();
    if result.is_err() {
        memfd::collect(runtime, memory);
        memfd::collect(runtime, sqes);
    }
    result
}

pub(super) fn map(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    prot: Word,
    flags: Word,
    file: LinuxFile,
    offset: Word,
) -> Result<Word, i32> {
    if flags & 3 != 1 {
        return Err(EINVAL);
    }
    let ring = object(runtime, file.resource)?;
    let (id, limit) = match offset {
        0 | 0x0800_0000 => (ring.memory, ring.ring_bytes),
        0x1000_0000 => (ring.sqes, ring.sqe_bytes),
        _ => return Err(EINVAL),
    };
    if bytes > limit {
        return Err(EINVAL);
    }
    memfd::map(
        runtime,
        pid,
        address,
        bytes,
        prot,
        flags,
        LinuxFile {
            kind: LinuxFileKind::MemoryFile,
            posix_fd: id,
            resource: id,
            ..LinuxFile::EMPTY
        },
        0,
    )
}

fn flush(runtime: &mut Runtime, id: Word) -> Result<(), i32> {
    loop {
        let ring = object(runtime, id)?;
        let (memory, tail, count, offset) =
            (ring.memory, ring.tail, ring.cq_entries, ring.cq_offset);
        let head = load(runtime, memory, CQ_HEAD)?;
        if tail.wrapping_sub(head) > count {
            return Err(EINVAL);
        }
        if tail.wrapping_sub(head) == count {
            break;
        }
        let Some((data, result)) = object_mut(runtime, id)?.overflow.pop_front() else {
            break;
        };
        let target = pointer(runtime, memory, offset + (tail & (count - 1)) as Word * 16)?;
        unsafe {
            (target as *mut u64).write_volatile(data);
            ((target + 8) as *mut i32).write_volatile(result);
            ((target + 12) as *mut u32).write_volatile(0);
        }
        let tail = tail.wrapping_add(1);
        object_mut(runtime, id)?.tail = tail;
        store(runtime, memory, CQ_TAIL, tail)?;
        runtime.readiness_changes |= READY_OBJECT;
    }
    let ring = object(runtime, id)?;
    let memory = ring.memory;
    let overflow = !ring.overflow.is_empty();
    store(runtime, memory, 16, if overflow { 2 } else { 0 })
}

#[derive(Clone, Copy)]
struct Submission {
    opcode: u8,
    flags: u8,
    priority: u16,
    fd: Word,
    off: Word,
    addr: Word,
    len: Word,
    op_flags: Word,
    data: u64,
    personality: u16,
    buffer: u16,
    file_index: u32,
}

fn submission(runtime: &mut Runtime, id: Word, index: u32) -> Result<Submission, i32> {
    let ring = object(runtime, id)?;
    let source = pointer(runtime, ring.sqes, index as Word * 64)?;
    let mut bytes = [0u8; 64];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        *byte = unsafe { ((source + offset) as *const u8).read_volatile() };
    }
    let u32_at = |offset| u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let u64_at = |offset| u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
    Ok(Submission {
        opcode: bytes[0],
        flags: bytes[1],
        priority: u16::from_ne_bytes([bytes[2], bytes[3]]),
        fd: u32_at(4) as i32 as isize as Word,
        off: u64_at(8) as Word,
        addr: u64_at(16) as Word,
        len: u32_at(24) as Word,
        op_flags: u32_at(28) as Word,
        data: u64_at(32),
        buffer: u16::from_ne_bytes([bytes[40], bytes[41]]),
        personality: u16::from_ne_bytes([bytes[42], bytes[43]]),
        file_index: u32_at(44),
    })
}

fn execute(runtime: &mut Runtime, pid: Word, sqe: Submission) -> Result<Word, i32> {
    if sqe.flags != 0
        || sqe.priority != 0
        || sqe.personality != 0
        || sqe.buffer != 0
        || sqe.file_index != 0
    {
        return Err(EOPNOTSUPP);
    }
    if matches!(sqe.opcode, 0 | 19 | 29 | 37 | 38 | 55) && sqe.op_flags != 0 {
        return Err(EINVAL);
    }
    match sqe.opcode {
        0 => Ok(0),
        1 | 2 | 22 | 23 => io(runtime, pid, sqe),
        3 => {
            if sqe.op_flags & !1 != 0 {
                return Err(EINVAL);
            }
            sys_fsync(runtime, pid, sqe.fd)
        }
        18 => sys_openat(runtime, pid, sqe.fd, sqe.addr, sqe.op_flags, sqe.len),
        19 => {
            if runtime
                .linux_file(pid, sqe.fd)
                .is_some_and(|file| file.kind == LinuxFileKind::IoUring)
            {
                return Err(EOPNOTSUPP);
            }
            sys_close(runtime, pid, sqe.fd)
        }
        21 => {
            if sqe.op_flags & !(0x100 | 0x800 | 0x1000 | 0x6000) != 0 {
                return Err(EINVAL);
            }
            sys_statx(runtime, pid, sqe.fd, sqe.addr, sqe.op_flags, sqe.off)
        }
        29 => epoll::control(runtime, pid, sqe.fd, sqe.len, sqe.off, sqe.addr),
        35 => {
            if sqe.op_flags != 0 {
                return Err(EOPNOTSUPP);
            }
            sys_renameat(
                runtime,
                pid,
                sqe.fd,
                sqe.addr,
                sqe.len as i32 as isize as Word,
                sqe.off,
            )
        }
        36 => sys_unlinkat(runtime, pid, sqe.fd, sqe.addr, sqe.op_flags),
        37 => sys_mkdirat(runtime, pid, sqe.fd, sqe.addr, sqe.len),
        38 => super::namespace::symlink(runtime, pid, sqe.addr, sqe.fd, sqe.off),
        39 => sys_linkat(
            runtime,
            pid,
            sqe.fd,
            sqe.addr,
            sqe.len as i32 as isize as Word,
            sqe.off,
            sqe.op_flags,
        ),
        55 => truncate::sys_ftruncate(runtime, pid, sqe.fd, sqe.off),
        _ => Err(EOPNOTSUPP),
    }
}

fn io(runtime: &mut Runtime, pid: Word, sqe: Submission) -> Result<Word, i32> {
    if sqe.op_flags != 0 {
        return Err(EOPNOTSUPP);
    }
    let file = runtime.linux_file(pid, sqe.fd).ok_or(EBADF)?;
    // No hidden blocking on pipes/sockets while the personality server owns the
    // syscall. Their asynchronous operations need a separate pending-request engine.
    if !matches!(file.kind, LinuxFileKind::Posix | LinuxFileKind::MemoryFile) {
        return Err(EOPNOTSUPP);
    }
    let write = matches!(sqe.opcode, 2 | 23);
    let mut vectors = Vec::new();
    if matches!(sqe.opcode, 1 | 2) {
        if sqe.len > 1024 {
            return Err(EINVAL);
        }
        vectors.try_reserve(sqe.len).map_err(|_| ENOMEM)?;
        for index in 0..sqe.len {
            let ptr = sqe.addr.checked_add(index * 16).ok_or(EFAULT)?;
            read_target_memory(runtime, pid, ptr, 16)?;
            vectors.push((read_shm_u64(runtime, 0), read_shm_u64(runtime, 8)));
        }
    } else {
        vectors.try_reserve(1).map_err(|_| ENOMEM)?;
        vectors.push((sqe.addr, sqe.len));
    }
    vectors.iter().try_fold(0usize, |sum, (_, bytes)| {
        sum.checked_add(*bytes)
            .filter(|sum| *sum <= i32::MAX as Word)
            .ok_or(EINVAL)
    })?;
    let mut total: Word = 0;
    for (address, bytes) in vectors {
        let result = if sqe.off == Word::MAX {
            if write {
                sys_write(runtime, pid, sqe.fd, address, bytes)
            } else {
                sys_read(runtime, pid, sqe.fd, address, bytes)
            }
        } else {
            sys_positioned_io(
                runtime,
                pid,
                sqe.fd,
                address,
                bytes,
                sqe.off.checked_add(total).ok_or(EINVAL)?,
                write,
            )
        };
        match result {
            Ok(count) => {
                total += count;
                if count < bytes {
                    break;
                }
            }
            Err(_) if total != 0 => break,
            Err(errno) => return Err(errno),
        }
    }
    Ok(total)
}

fn submit(runtime: &mut Runtime, pid: Word, id: Word, count: Word) -> Result<Word, i32> {
    flush(runtime, id)?;
    let ring = object(runtime, id)?;
    let (memory, entries, array, head) = (ring.memory, ring.entries, ring.array, ring.head);
    let tail = load(runtime, memory, 4)?;
    let available = tail.wrapping_sub(head);
    if available > entries {
        return Err(EINVAL);
    }
    let mut done = 0;
    while done < count.min(available as Word) {
        if object(runtime, id)?.overflow.len() >= entries as usize {
            break;
        }
        if object_mut(runtime, id)?.overflow.try_reserve(1).is_err() {
            return if done == 0 { Err(ENOMEM) } else { Ok(done) };
        }
        let head = object(runtime, id)?.head;
        let slot = head & (entries - 1);
        let index = if array {
            load(runtime, memory, SQ_ARRAY + slot as Word * 4)?
        } else {
            slot
        };
        let sqe = if index < entries {
            Some(submission(runtime, id, index)?)
        } else {
            None
        };
        object_mut(runtime, id)?.head = head.wrapping_add(1);
        store(runtime, memory, 0, head.wrapping_add(1))?;
        done += 1;
        let Some(sqe) = sqe else {
            let dropped = load(runtime, memory, 20)?;
            store(runtime, memory, 20, dropped.wrapping_add(1))?;
            continue;
        };
        let cancelled = object(runtime, id)?.cancel_next;
        let result = if cancelled {
            -ECANCELED
        } else {
            match execute(runtime, pid, sqe) {
                Ok(result) => result as i32,
                Err(errno) => -errno,
            }
        };
        // Unsupported link flags must not cause following dependent operations
        // to execute after the rejected head of their chain.
        object_mut(runtime, id)?.cancel_next =
            sqe.flags & (IO_LINK | IO_HARDLINK) != 0 && (cancelled || result < 0);
        object_mut(runtime, id)?
            .overflow
            .push_back((sqe.data, result));
        flush(runtime, id)?;
    }
    if done == 0 && count != 0 && available != 0 {
        return Err(EBUSY);
    }
    Ok(done)
}

fn completions(runtime: &mut Runtime, id: Word) -> Result<u32, i32> {
    flush(runtime, id)?;
    let ring = object(runtime, id)?;
    let (memory, tail, entries) = (ring.memory, ring.tail, ring.cq_entries);
    let head = load(runtime, memory, CQ_HEAD)?;
    let count = tail.wrapping_sub(head);
    if count > entries {
        return Err(EINVAL);
    }
    Ok(count)
}

pub(super) fn enter(
    runtime: &mut Runtime,
    pid: Word,
    mut context: LinuxSyscallContext,
) -> EmulationAction {
    let result = (|| {
        let id = ring_id(runtime, pid, context.args[0])?;
        if context.args[3] & !1 != 0 || context.args[2] > object(runtime, id)?.cq_entries as Word {
            return Err(EINVAL);
        }
        let mask = poll_timeout::temporary_signal_mask(runtime, pid, context)?;
        let submitted = submit(runtime, pid, id, context.args[1])?;
        if context.args[3] & 1 == 0 || completions(runtime, id)? as Word >= context.args[2] {
            return Ok(Some(submitted));
        }
        // Keep the submitted count for completion; never resubmit an SQE on wake.
        context.args[1] = submitted;
        let process = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
        if let Some(mask) = mask {
            let state = process.signals_mut();
            state.suspend_mask = Some(state.blocked);
            state.blocked = mask;
        }
        process.readiness_wait = Some(ReadinessWait {
            context,
            entries: [PollFd::EMPTY; LINUX_FD_MAX],
            count: 0,
            sources: READY_OBJECT,
            deadline: None,
            select: false,
        });
        Ok(None)
    })();
    match result {
        Ok(Some(value)) => EmulationAction::Return(value as isize),
        Ok(None) => EmulationAction::Park,
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

pub(super) fn retry(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let id = ring_id(runtime, pid, context.args[0])?;
    if completions(runtime, id)? as Word >= context.args[2] {
        return Ok(context.args[1]);
    }
    Err(EAGAIN)
}

pub(super) fn register(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    op: Word,
    arg: Word,
    count: Word,
) -> Result<Word, i32> {
    ring_id(runtime, pid, fd)?;
    if op != 8 {
        return Err(EOPNOTSUPP);
    }
    if count > 256 {
        return Err(EINVAL);
    }
    let bytes = 16 + count * 8;
    read_target_memory(runtime, pid, arg, bytes)?;
    if unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes) }
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(EINVAL);
    }
    unsafe {
        core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, bytes);
        *(runtime.posix_shm as *mut u8) = 55;
        *((runtime.posix_shm + 1) as *mut u8) = count.min(56) as u8;
        for index in 0..count.min(56) {
            let address = runtime.posix_shm + 16 + index * 8;
            *(address as *mut u8) = index as u8;
            if OPS.contains(&(index as u8)) {
                ((address + 2) as *mut u16).write_unaligned(1);
            }
        }
    }
    write_target_memory(runtime, pid, arg, bytes)?;
    Ok(0)
}

pub(super) fn readiness(runtime: &mut Runtime, id: Word) -> Result<i16, i32> {
    let mut events = if completions(runtime, id)? != 0 {
        LINUX_POLLIN | LINUX_POLLRDNORM
    } else {
        0
    };
    let ring = object(runtime, id)?;
    let (memory, head, entries) = (ring.memory, ring.head, ring.entries);
    if load(runtime, memory, 4)?.wrapping_sub(head) < entries {
        events |= LINUX_POLLOUT | LINUX_POLLWRNORM;
    }
    Ok(events)
}

pub(super) fn release(runtime: &mut Runtime, file: LinuxFile) {
    let backing = object(runtime, file.resource)
        .map(|ring| (ring.memory, ring.sqes))
        .ok();
    eventfd::release(runtime, file);
    if runtime.poll_objects.get(file.resource).is_none() {
        if let Some((memory, sqes)) = backing {
            memfd::collect(runtime, memory);
            memfd::collect(runtime, sqes);
        }
    }
}
