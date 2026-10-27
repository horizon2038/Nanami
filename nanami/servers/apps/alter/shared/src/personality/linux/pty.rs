//! PTYs are shared open descriptions, not aliases of the launcher's GUI tty.
use super::*;
use crate::state::pty::{self as state, Error, TERMINAL_TAG};
use crate::state::readiness::{PollFd, ReadinessWait, READY_PTY};

const O_NOCTTY: Word = 0x100;
const TIOCGPTN: Word = 0x80045430;
const TIOCSPTLCK: Word = 0x40045431;
const TIOCGPTLCK: Word = 0x80045439;
const TIOCGPTPEER: Word = 0x5441;
const TIOCPKT: Word = 0x5420;
const TIOCGPKT: Word = 0x80045438;
const FIONREAD: Word = 0x541b;
const TIOCOUTQ: Word = 0x5411;
const TCFLSH: Word = 0x540b;
const TCXONC: Word = 0x540a;
const TCSBRK: Word = 0x5409;

fn errno(error: Error) -> i32 {
    match error {
        Error::Again => EAGAIN,
        Error::Io | Error::Locked => EIO,
        Error::Missing => EBADF,
        Error::Full => ENOMEM,
    }
}

pub(super) fn is_pty(file: LinuxFile) -> bool {
    matches!(
        file.kind,
        LinuxFileKind::PtyMaster | LinuxFileKind::PtySlave
    )
}

fn namespace(runtime: &Runtime, pid: Word) -> Result<Word, i32> {
    Ok(runtime.managed_process(pid).ok_or(ESRCH)?.rootfs.leader)
}

fn make_file(id: Word, description: Word, master: bool, flags: Word) -> LinuxFile {
    LinuxFile {
        kind: if master {
            LinuxFileKind::PtyMaster
        } else {
            LinuxFileKind::PtySlave
        },
        resource: id,
        posix_fd: description,
        flags: if flags & LINUX_O_CLOEXEC != 0 {
            LINUX_FD_CLOEXEC
        } else {
            0
        },
        ..LinuxFile::EMPTY
    }
}

pub(super) fn number(path: &[u8]) -> Option<u32> {
    let name = path.strip_prefix(b"/dev/pts/")?;
    if name.is_empty() || (name.len() > 1 && name[0] == b'0') {
        return None;
    }
    let mut number = 0u32;
    for &byte in name {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number
            .checked_mul(10)?
            .checked_add(u32::from(byte - b'0'))?;
    }
    Some(number)
}

pub(super) fn path_id(runtime: &Runtime, pid: Word, path: &[u8]) -> Result<Word, i32> {
    let number = number(path).ok_or(ENOENT)?;
    runtime
        .ptys
        .find(namespace(runtime, pid)?, number)
        .map(|p| p.id)
        .ok_or(ENOENT)
}

pub(super) fn open_master(runtime: &mut Runtime, pid: Word, flags: Word) -> Result<Word, i32> {
    validate_flags(flags)?;
    let (id, description) = runtime
        .ptys
        .create(namespace(runtime, pid)?, flags & (3 | LINUX_O_NONBLOCK))
        .map_err(errno)?;
    install(runtime, pid, make_file(id, description, true, flags))
}

fn validate_flags(flags: Word) -> Result<(), i32> {
    if flags & LINUX_O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    if flags & LINUX_O_ACCMODE == LINUX_O_ACCMODE || flags & 0x200000 != 0 {
        return Err(EINVAL);
    }
    Ok(())
}

fn install(runtime: &mut Runtime, pid: Word, file: LinuxFile) -> Result<Word, i32> {
    if let Some(fd) = runtime.allocate_linux_file(pid, file, 0) {
        return Ok(fd);
    }
    runtime.ptys.close(file.resource, file.posix_fd);
    Err(EMFILE)
}

pub(super) fn open_slave(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    flags: Word,
) -> Result<Word, i32> {
    validate_flags(flags)?;
    let description = runtime
        .ptys
        .get_mut(id)
        .map_err(errno)?
        .open_slave(flags & (3 | LINUX_O_NONBLOCK))
        .map_err(errno)?;
    let fd = install(runtime, pid, make_file(id, description, false, flags))?;
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    if flags & O_NOCTTY == 0
        && process.job.session == process.tgid
        && process.job.controlling_terminal == 0
    {
        // Failing to acquire an already owned terminal does not fail open().
        let _ = runtime.claim_controlling_terminal(pid, TERMINAL_TAG | id);
    }
    runtime.readiness_changes |= READY_PTY;
    Ok(fd)
}

pub(super) fn flags(
    runtime: &mut Runtime,
    file: LinuxFile,
    update: Option<Word>,
) -> Result<Word, i32> {
    runtime
        .ptys
        .get_mut(file.resource)
        .map_err(errno)?
        .flags(file.posix_fd, update)
        .map_err(errno)
}

pub(super) fn collect(runtime: &mut Runtime, file: LinuxFile) {
    if runtime.managed.iter().any(|p| {
        p.files.iter().any(|f| {
            f.kind == file.kind && f.resource == file.resource && f.posix_fd == file.posix_fd
        })
    }) || runtime.unix_sockets.references(file)
    {
        return;
    }
    let id = TERMINAL_TAG | file.resource;
    if runtime.ptys.close(file.resource, file.posix_fd) {
        hangup_control(runtime, id);
    }
    runtime.readiness_changes |= READY_PTY;
}

fn signal_group(runtime: &mut Runtime, session: Word, group: Word, signal: usize) {
    for index in 0..runtime.managed.len() {
        let p = &runtime.managed[index];
        if p.pid == 0
            || p.exited
            || p.pid != p.tgid
            || p.job.session != session
            || p.job.group != group
        {
            continue;
        }
        let _ = signals_runtime::queue_group(
            runtime,
            p.pid,
            crate::common::signals::Info::new(signal, 128, 0),
        );
    }
}

fn foreground_signal(runtime: &mut Runtime, id: Word, signal: usize) {
    if let Some(control) = runtime
        .terminal_controls
        .iter()
        .find(|c| c.id == id)
        .copied()
    {
        signal_group(runtime, control.session, control.foreground, signal);
    }
}

fn hangup_control(runtime: &mut Runtime, id: Word) {
    foreground_signal(runtime, id, 1);
    foreground_signal(runtime, id, 18);
    for process in &mut runtime.managed {
        if process.job.controlling_terminal == id {
            process.job.controlling_terminal = 0;
        }
    }
    for control in &mut runtime.terminal_controls {
        if control.id == id {
            *control = crate::state::TerminalControl::EMPTY;
        }
    }
}

pub(super) fn session_exit(runtime: &mut Runtime, pid: Word) {
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    let id = process.job.controlling_terminal;
    if id & TERMINAL_TAG == 0
        || process.job.session != process.tgid
        || runtime.has_live_threads(pid)
    {
        return;
    }
    if let Ok(pair) = runtime.ptys.get_mut(id & !TERMINAL_TAG) {
        pair.hangup();
    }
    hangup_control(runtime, id);
    runtime.readiness_changes |= READY_PTY;
}

fn background(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    write: bool,
    setting: bool,
) -> Result<(), i32> {
    if file.kind != LinuxFileKind::PtySlave {
        return Ok(());
    }
    let pair = runtime.ptys.get(file.resource).map_err(errno)?;
    if write && !setting && pair.lflag() & state::TOSTOP == 0 {
        return Ok(());
    }
    let id = pair.terminal_id();
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let job = process.job;
    let Some(control) = runtime.terminal_controls.iter().find(|c| c.id == id) else {
        return Ok(());
    };
    if job.controlling_terminal != id
        || job.session != control.session
        || job.group == control.foreground
    {
        return Ok(());
    }
    let signal = if write { 22 } else { 21 };
    let blocked = process.signals().is_some_and(|s| {
        s.blocked & crate::common::signals::bit(signal) != 0
            || s.action(signal).handler == crate::common::signals::SIG_IGN
    });
    if blocked {
        return if write { Ok(()) } else { Err(EIO) };
    }
    let orphaned = !runtime.managed.iter().any(|member| {
        member.pid != 0
            && !member.exited
            && member.job.session == job.session
            && member.job.group == job.group
            && runtime
                .managed_process(member.parent_pid)
                .is_some_and(|parent| {
                    !parent.exited
                        && parent.job.session == job.session
                        && parent.job.group != job.group
                })
    });
    if orphaned {
        return Err(EIO);
    }
    signal_group(runtime, job.session, job.group, signal);
    Err(EAGAIN)
}

pub(super) fn read(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let bytes = read_to_scratch(runtime, pid, file, len)?;
    if bytes != 0 {
        write_target_memory(runtime, pid, buffer, bytes)?;
    }
    if len != 0 {
        consume(runtime, file, bytes)?;
    }
    Ok(bytes)
}

pub(super) fn readv(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    bases: &[Word],
    lens: &[Word],
) -> Result<Word, i32> {
    let len = lens
        .iter()
        .try_fold(0usize, |sum, len| sum.checked_add(*len))
        .ok_or(EINVAL)?;
    let count = read_to_scratch(runtime, pid, file, len)?;
    let mut done = 0;
    for (&base, &len) in bases.iter().zip(lens) {
        let bytes = len.min(count - done);
        if bytes == 0 {
            continue;
        }
        if let Err(error) = super::copyout_from(runtime, pid, base, runtime.posix_shm + done, bytes)
        {
            if done == 0 {
                return Err(error);
            }
            break;
        }
        done += bytes;
    }
    if len != 0 {
        consume(runtime, file, done)?;
    }
    Ok(done)
}

fn consume(runtime: &mut Runtime, file: LinuxFile, bytes: Word) -> Result<(), i32> {
    runtime
        .ptys
        .get_mut(file.resource)
        .map_err(errno)?
        .consume(file.kind == LinuxFileKind::PtyMaster, bytes);
    runtime.readiness_changes |= READY_PTY;
    Ok(())
}

fn read_to_scratch(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    len: Word,
) -> Result<Word, i32> {
    if flags(runtime, file, None)? & LINUX_O_ACCMODE == LINUX_O_WRONLY {
        return Err(EBADF);
    }
    if len == 0 {
        return Ok(0);
    }
    background(runtime, pid, file, false, false)?;
    let nonblocking = flags(runtime, file, None)? & LINUX_O_NONBLOCK != 0;
    let pair = runtime.ptys.get(file.resource).map_err(errno)?;
    let master = file.kind == LinuxFileKind::PtyMaster;
    if !master && !pair.canonical() && !pair.eof() {
        let minimum = usize::from(pair.cc(state::VMIN)).min(len);
        let time = pair.cc(state::VTIME);
        let expired = runtime
            .managed_process(pid)
            .and_then(|p| p.readiness_wait.as_ref())
            .and_then(|w| w.deadline)
            .is_some_and(|deadline| deadline <= runtime.monotonic_ticks);
        if !nonblocking && pair.input_len() < minimum && !expired {
            return Err(EAGAIN);
        }
        if pair.input_len() == 0 {
            if (!nonblocking && minimum == 0 && time == 0) || expired {
                return Ok(0);
            }
            return Err(EAGAIN);
        }
    }
    let count = len.min(runtime.posix_shm_size).min(state::CAPACITY);
    let target = unsafe { core::slice::from_raw_parts_mut(runtime.posix_shm as *mut u8, count) };
    let bytes = pair.peek(master, target).map_err(errno)?;
    Ok(bytes)
}

pub(super) fn write(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if flags(runtime, file, None)? & LINUX_O_ACCMODE == LINUX_O_RDONLY {
        return Err(EBADF);
    }
    if len == 0 {
        return Ok(0);
    }
    background(runtime, pid, file, true, false)?;
    let master = file.kind == LinuxFileKind::PtyMaster;
    let pair = runtime.ptys.get(file.resource).map_err(errno)?;
    if master && !pair.canonical() && pair.cc(state::VTIME) != 0 {
        refresh_clock(runtime)?;
    }
    let count = len.min(runtime.posix_shm_size).min(state::CAPACITY);
    read_target_memory(runtime, pid, buffer, count)?;
    let bytes = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, count) };
    let (done, signals) = runtime
        .ptys
        .get_mut(file.resource)
        .map_err(errno)?
        .write(master, bytes, runtime.monotonic_ticks)
        .map_err(errno)?;
    for signal in [2, 3, 20] {
        if signals & (1 << signal) != 0 {
            foreground_signal(runtime, TERMINAL_TAG | file.resource, signal);
        }
    }
    runtime.readiness_changes |= READY_PTY;
    Ok(done)
}

pub(super) fn readiness(runtime: &Runtime, file: LinuxFile) -> Result<i16, i32> {
    let pair = runtime.ptys.get(file.resource).map_err(errno)?;
    let master = file.kind == LinuxFileKind::PtyMaster;
    let mut result = 0;
    if pair.readable(master) {
        result |= LINUX_POLLIN | LINUX_POLLRDNORM;
    }
    if pair.writable(master) {
        result |= LINUX_POLLOUT | LINUX_POLLWRNORM;
    }
    if (master && pair.master_hup()) || (!master && pair.eof()) {
        result |= LINUX_POLLHUP;
    }
    if !master && pair.eof() {
        result |= LINUX_POLLERR;
    }
    if master && pair.packet && pair.packet_status != 0 {
        result |= LINUX_POLLPRI;
    }
    Ok(result)
}

pub(super) fn park(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<bool, i32> {
    if !matches!(
        context.number,
        SYS_READ | SYS_READV | SYS_WRITE | SYS_WRITEV | SYS_IOCTL
    ) {
        return Ok(false);
    }
    let Some(file) = runtime.linux_file(pid, context.args[0]) else {
        return Ok(false);
    };
    if !is_pty(file) {
        return Ok(false);
    }
    let job_stop = runtime
        .managed_process(pid)
        .and_then(|p| p.signals())
        .is_some_and(|s| {
            s.pending_mask() & (crate::common::signals::bit(21) | crate::common::signals::bit(22))
                != 0
        });
    if flags(runtime, file, None)? & LINUX_O_NONBLOCK != 0 && !job_stop {
        return Ok(false);
    }
    let mut entries = [PollFd::EMPTY; LINUX_FD_MAX];
    entries[0] = PollFd {
        fd: context.args[0] as i32,
        events: if matches!(context.number, SYS_READ | SYS_READV) {
            LINUX_POLLIN
        } else {
            LINUX_POLLOUT
        },
        revents: 0,
    };
    refresh_clock(runtime)?;
    let mut wait = ReadinessWait {
        context,
        entries,
        count: 1,
        sources: READY_PTY,
        deadline: None,
        select: false,
    };
    update_deadline(runtime, pid, &mut wait);
    runtime
        .managed_process_mut(pid)
        .ok_or(ESRCH)?
        .readiness_wait = Some(wait);
    if let Err(error) = arm_clock_timer(runtime) {
        runtime
            .managed_process_mut(pid)
            .ok_or(ESRCH)?
            .readiness_wait = None;
        return Err(error);
    }
    Ok(true)
}

pub(super) fn update_deadline(runtime: &Runtime, pid: Word, wait: &mut ReadinessWait) {
    if !matches!(wait.context.number, SYS_READ | SYS_READV) {
        return;
    }
    let Some(file) = runtime.linux_file(pid, wait.context.args[0]) else {
        return;
    };
    if file.kind != LinuxFileKind::PtySlave {
        return;
    }
    let Ok(pair) = runtime.ptys.get(file.resource) else {
        return;
    };
    if pair.canonical() || pair.cc(state::VTIME) == 0 {
        wait.deadline = None;
        return;
    }
    let duration = (runtime.monotonic_tick_hz as u128 * u128::from(pair.cc(state::VTIME)))
        .div_ceil(10)
        .min(Word::MAX as u128) as Word;
    if pair.cc(state::VMIN) == 0 {
        if wait.deadline.is_none() {
            wait.deadline = Some(runtime.monotonic_ticks.saturating_add(duration));
        }
    } else if pair.input_len() != 0 {
        // Existing input starts its timer when read begins, not retroactively
        // at arrival. Later bytes extend that deadline; unrelated wakeups do
        // not restart it or shorten it to an old arrival timestamp.
        wait.deadline = Some(match wait.deadline {
            Some(deadline) => deadline.max(pair.last_input.saturating_add(duration)),
            None => runtime
                .monotonic_ticks
                .max(pair.last_input)
                .saturating_add(duration),
        });
    } else {
        wait.deadline = None;
    }
}

pub(super) fn retry(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Option<Result<Word, i32>> {
    if !matches!(
        context.number,
        SYS_READ | SYS_READV | SYS_WRITE | SYS_WRITEV | SYS_IOCTL
    ) {
        return None;
    }
    let waiting_on_pty = runtime
        .managed_process(pid)
        .and_then(|p| p.readiness_wait.as_ref())
        .is_some_and(|w| w.sources == READY_PTY);
    let file = match runtime.linux_file(pid, context.args[0]) {
        Some(file) if is_pty(file) => file,
        _ if waiting_on_pty => return Some(Err(EBADF)),
        _ => return None,
    };
    if context.number == SYS_IOCTL {
        return Some(ioctl(runtime, pid, file, context.args[1], context.args[2]));
    }
    super::pipe_wait::retry(runtime, pid, context)
}

fn read_i32(runtime: &mut Runtime, pid: Word, argument: Word) -> Result<i32, i32> {
    read_target_memory(runtime, pid, argument, 4)?;
    Ok(unsafe { core::ptr::read_unaligned(runtime.posix_shm as *const i32) })
}

pub(super) fn ioctl(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    let request = request as u32 as Word;
    let master = file.kind == LinuxFileKind::PtyMaster;
    let id = TERMINAL_TAG | file.resource;
    if request == TIOCGPTPEER {
        if !master {
            return Err(ENOTTY);
        }
        if argument & !(LINUX_O_ACCMODE | LINUX_O_NONBLOCK | LINUX_O_CLOEXEC | O_NOCTTY) != 0 {
            return Err(EINVAL);
        }
        return open_slave(runtime, pid, file.resource, argument);
    }
    if matches!(
        request,
        TIOCGPTN | TIOCSPTLCK | TIOCGPTLCK | TIOCPKT | TIOCGPKT | 0x40045436
    ) && !master
    {
        return Err(ENOTTY);
    }
    let setting = matches!(
        request,
        LINUX_TCSETS | LINUX_TCSETSW | LINUX_TCSETSF | LINUX_TIOCSPGRP | TCFLSH | TCXONC | TCSBRK
    );
    if setting {
        background(runtime, pid, file, true, true)?;
    }
    if master && matches!(request, LINUX_TIOCGPGRP | LINUX_TIOCGSID) {
        let control = runtime
            .terminal_controls
            .iter()
            .find(|c| c.id == id)
            .ok_or(ENOTTY)?;
        let value = if request == LINUX_TIOCGPGRP {
            control.foreground
        } else {
            control.session
        };
        write_u32_to_target(runtime, pid, argument, value as u32)?;
        return Ok(0);
    }
    if !master {
        if request == LINUX_TIOCSCTTY && runtime.ptys.get(file.resource).map_err(errno)?.eof() {
            return Err(EIO);
        }
        if request == LINUX_TIOCNOTTY {
            let control = *runtime
                .controlling_terminal(pid, id)
                .map_err(job_control::errno)?;
            if runtime.thread_group(pid) == control.session {
                foreground_signal(runtime, id, 1);
                foreground_signal(runtime, id, 18);
            }
        }
        if let Some(result) =
            job_control::terminal_ioctl_for_id(runtime, pid, id, request, argument)
        {
            runtime.readiness_changes |= READY_PTY;
            return result;
        }
    }
    match request {
        TIOCGPTN | TIOCGPTLCK | TIOCGPKT | FIONREAD | TIOCOUTQ | 0x5424 => {
            let pair = runtime.ptys.get(file.resource).map_err(errno)?;
            let value = match request {
                TIOCGPTN => pair.number,
                TIOCGPTLCK => u32::from(pair.locked),
                TIOCGPKT => u32::from(pair.packet),
                FIONREAD if master => pair.output_len() as u32,
                FIONREAD => pair.input_len() as u32,
                TIOCOUTQ if master => 0,
                TIOCOUTQ => pair.output_len() as u32,
                _ => 0, // N_TTY line discipline.
            };
            write_u32_to_target(runtime, pid, argument, value)?;
        }
        TIOCSPTLCK | TIOCPKT => {
            let value = read_i32(runtime, pid, argument)?;
            let pair = runtime.ptys.get_mut(file.resource).map_err(errno)?;
            if request == TIOCSPTLCK {
                pair.locked = value != 0;
            } else {
                pair.packet = value != 0;
                pair.packet_status = 0;
            }
        }
        LINUX_TCGETS => {
            let value = runtime.ptys.get(file.resource).map_err(errno)?.termios;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    value.as_ptr(),
                    runtime.posix_shm as *mut u8,
                    value.len(),
                );
            }
            write_target_memory(runtime, pid, argument, 36)?;
        }
        LINUX_TCSETS | LINUX_TCSETSW | LINUX_TCSETSF => {
            read_target_memory(runtime, pid, argument, 36)?;
            let mut value = [0; 36];
            unsafe {
                core::ptr::copy_nonoverlapping(
                    runtime.posix_shm as *const u8,
                    value.as_mut_ptr(),
                    36,
                );
            }
            let pair = runtime.ptys.get_mut(file.resource).map_err(errno)?;
            if request != LINUX_TCSETS && pair.output_len() != 0 {
                return Err(EAGAIN);
            }
            pair.set_termios(value, request == LINUX_TCSETSF);
        }
        LINUX_TIOCGWINSZ => {
            let value = runtime.ptys.get(file.resource).map_err(errno)?.winsize;
            for (index, item) in value.into_iter().enumerate() {
                unsafe {
                    write_u16(runtime.posix_shm + index * 2, item);
                }
            }
            write_target_memory(runtime, pid, argument, 8)?;
        }
        LINUX_TIOCSWINSZ => {
            read_target_memory(runtime, pid, argument, 8)?;
            let mut value = [0u16; 4];
            for (index, item) in value.iter_mut().enumerate() {
                *item = unsafe {
                    core::ptr::read_unaligned((runtime.posix_shm + index * 2) as *const u16)
                };
            }
            let pair = runtime.ptys.get_mut(file.resource).map_err(errno)?;
            let changed = pair.winsize != value;
            pair.winsize = value;
            if changed {
                foreground_signal(runtime, id, 28);
            }
        }
        TCFLSH => {
            if argument > 2 {
                return Err(EINVAL);
            }
            runtime
                .ptys
                .get_mut(file.resource)
                .map_err(errno)?
                .flush(argument);
        }
        TCXONC => {
            if argument > 3 {
                return Err(EINVAL);
            }
            let pair = runtime.ptys.get_mut(file.resource).map_err(errno)?;
            if argument < 2 {
                pair.flow(argument == 0);
            } else {
                // TCIOFF/TCION transmit the configured byte to the peer.
                let byte = pair.cc(if argument == 2 { 9 } else { 8 });
                pair.write(false, &[byte], runtime.monotonic_ticks)
                    .map_err(errno)?;
            }
        }
        TCSBRK => {
            if argument == 0 {
                return Err(EOPNOTSUPP);
            }
            if runtime.ptys.get(file.resource).map_err(errno)?.output_len() != 0 {
                return Err(EAGAIN);
            }
        }
        0x5423 => {
            if read_i32(runtime, pid, argument)? != 0 {
                return Err(EINVAL);
            }
        }
        0x40045436 => {
            if !matches!(argument, 2 | 3 | 20) {
                return Err(EINVAL);
            }
            foreground_signal(runtime, id, argument);
        }
        _ => return Err(ENOTTY),
    }
    runtime.readiness_changes |= READY_PTY;
    Ok(0)
}

pub(super) fn stat(
    runtime: &Runtime,
    id: Word,
    master: bool,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let pair = runtime.ptys.get(id).map_err(errno)?;
    let major = if master {
        5
    } else {
        136 + pair.number as Word / 256
    };
    let minor = if master { 2 } else { pair.number as Word % 256 };
    Ok((
        0x10000 + id * 2 + Word::from(master),
        0,
        posix::POSIX_FILE_TYPE_CHAR_DEVICE,
        major,
        minor,
    ))
}

pub(super) fn getdents(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    count: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let ns = namespace(runtime, pid)?;
    let mut out = 0;
    let mut cursor = file.offset;
    loop {
        // Cursor is the next numeric name, so deleting another PTY cannot
        // shift directory offsets or cause an entry to be skipped.
        let Some(pair) = runtime
            .ptys
            .entries(ns)
            .filter(|p| p.number as Word >= cursor)
            .min_by_key(|p| p.number)
        else {
            break;
        };
        let number = pair.number;
        let inode = 0x10000 + pair.id * 2;
        let mut name = [0u8; 10];
        let mut start = name.len();
        let mut n = number;
        loop {
            start -= 1;
            name[start] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        let name = &name[start..];
        let size = (19 + name.len() + 1 + 7) & !7;
        if out + size > count.min(runtime.posix_shm_size) {
            if out == 0 {
                return Err(EINVAL);
            }
            break;
        }
        let base = runtime.posix_shm + out;
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, size);
            write_u64(base, inode);
            write_u64(base + 8, number as Word + 1);
            write_u16(base + 16, size as u16);
            core::ptr::write((base + 18) as *mut u8, LINUX_DT_CHR as u8);
            core::ptr::copy_nonoverlapping(name.as_ptr(), (base + 19) as *mut u8, name.len());
        }
        out += size;
        cursor = number as Word + 1;
    }
    if out != 0 {
        write_target_memory(runtime, pid, buffer, out)?;
    }
    file.offset = cursor;
    runtime.set_linux_file(pid, fd, file);
    Ok(out)
}
