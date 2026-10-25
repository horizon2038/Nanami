use super::*;
use crate::state::poll_objects::{EventCounter, Object};
use crate::state::readiness::READY_OBJECT;

pub(super) fn allocate(
    runtime: &mut Runtime,
    pid: Word,
    kind: LinuxFileKind,
    object: Object,
    flags: Word,
) -> Result<Word, i32> {
    let id = runtime.poll_objects.insert(object).ok_or(ENOMEM)?;
    let file = LinuxFile {
        kind,
        posix_fd: id,
        flags: (flags & LINUX_O_NONBLOCK)
            | if flags & LINUX_O_CLOEXEC != 0 {
                LINUX_FD_CLOEXEC
            } else {
                0
            },
        resource: id,
        ..LinuxFile::EMPTY
    };
    match runtime.allocate_linux_file(pid, file, 0) {
        Some(fd) => Ok(fd),
        None => {
            runtime.poll_objects.remove(id);
            Err(EMFILE)
        }
    }
}

pub(super) fn create(
    runtime: &mut Runtime,
    pid: Word,
    value: Word,
    flags: Word,
) -> Result<Word, i32> {
    const SEMAPHORE: Word = 1;
    if flags & !(SEMAPHORE | LINUX_O_CLOEXEC | LINUX_O_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    allocate(
        runtime,
        pid,
        LinuxFileKind::EventFd,
        Object::Counter(EventCounter {
            value: value as u32 as u64,
            semaphore: flags & SEMAPHORE != 0,
            nonblocking: flags & LINUX_O_NONBLOCK != 0,
        }),
        flags,
    )
}

pub(super) fn read(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    bytes: Word,
) -> Result<Word, i32> {
    if bytes < 8 {
        return Err(EINVAL);
    }
    let Some(Object::Counter(counter)) = runtime.poll_objects.get(file.resource) else {
        return Err(EBADF);
    };
    if counter.value == 0 {
        return Err(EAGAIN);
    }
    let value = if counter.semaphore { 1 } else { counter.value };
    unsafe {
        write_u64(runtime.posix_shm, value as Word);
    }
    write_target_memory(runtime, pid, buffer, 8).map_err(|_| EFAULT)?;
    let Some(Object::Counter(counter)) = runtime.poll_objects.get_mut(file.resource) else {
        return Err(EBADF);
    };
    counter.value -= value;
    runtime.readiness_changes |= READY_OBJECT;
    Ok(8)
}

pub(super) fn write(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    bytes: Word,
) -> Result<Word, i32> {
    if bytes < 8 {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, buffer, 8).map_err(|_| EFAULT)?;
    let value = unsafe { (runtime.posix_shm as *const u64).read_unaligned() };
    if value == u64::MAX {
        return Err(EINVAL);
    }
    let Some(Object::Counter(counter)) = runtime.poll_objects.get_mut(file.resource) else {
        return Err(EBADF);
    };
    counter.value = counter
        .value
        .checked_add(value)
        .filter(|value| *value != u64::MAX)
        .ok_or(EAGAIN)?;
    runtime.readiness_changes |= READY_OBJECT;
    Ok(8)
}

pub(super) fn readiness(runtime: &Runtime, file: LinuxFile) -> i16 {
    let Some(Object::Counter(counter)) = runtime.poll_objects.get(file.resource) else {
        return LINUX_POLLNVAL;
    };
    let mut events = 0;
    if counter.value != 0 {
        events |= LINUX_POLLIN | LINUX_POLLRDNORM;
    }
    if counter.value < u64::MAX - 1 {
        events |= LINUX_POLLOUT | LINUX_POLLWRNORM;
    }
    events
}

pub(super) fn nonblocking(runtime: &Runtime, file: LinuxFile) -> bool {
    match runtime.poll_objects.get(file.resource) {
        Some(Object::Counter(counter)) => counter.nonblocking,
        _ => true,
    }
}

pub(super) fn same_file(left: LinuxFile, right: LinuxFile) -> bool {
    left.kind == right.kind && left.posix_fd == right.posix_fd && left.resource == right.resource
}

pub(super) fn release(runtime: &mut Runtime, file: LinuxFile) {
    // Watches do not own an FD reference. dup/fork aliases keep the object
    // alive, while final close removes its allocation (also on failed exec).
    if runtime.managed.iter().any(|process| {
        process.pid != 0 && process.files.iter().any(|other| same_file(*other, file))
    }) {
        return;
    }
    runtime.poll_objects.remove(file.resource);
    runtime.readiness_changes |= READY_OBJECT;
}
