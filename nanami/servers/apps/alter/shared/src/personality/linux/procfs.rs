//! Process-backed procfs. All access is confined to the caller's execution context.
use super::*;
use crate::state::ManagedProcess;
use core::fmt::{self, Write};

const NODE_MASK: Word = 0xffff_ffff;
const ROOT_ENTRIES: Word = 6;

pub(super) fn node(resource: Word) -> Option<VirtualNode> {
    VirtualNode::from_id(resource & NODE_MASK)
}

fn resource(node: VirtualNode, pid: Word) -> Word {
    (pid << 32) | node.id()
}

fn visible(runtime: &Runtime, caller: Word, target: Word) -> Option<&ManagedProcess> {
    let current = runtime.managed_process(caller)?;
    let process = runtime.managed_process(target)?;
    (current.rootfs.leader == process.rootfs.leader).then_some(process)
}

fn decimal(bytes: &[u8]) -> Option<Word> {
    if bytes.is_empty() || bytes[0] == b'0' {
        return None;
    }
    bytes.iter().try_fold(0usize, |number, byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        number
            .checked_mul(10)?
            .checked_add((byte - b'0') as Word)
            .filter(|number| *number <= u32::MAX as Word)
    })
}

pub(super) fn lookup(runtime: &Runtime, caller: Word, path: &[u8]) -> Option<Word> {
    let rest = path.strip_prefix(b"/proc/")?;
    let split = rest
        .iter()
        .position(|byte| *byte == b'/')
        .unwrap_or(rest.len());
    let subject = if &rest[..split] == b"self" {
        runtime.thread_group(caller)
    } else {
        decimal(&rest[..split])?
    };
    visible(runtime, caller, subject)?;
    let suffix = rest.get(split + 1..).unwrap_or_default();
    if suffix == b"task" {
        return Some(resource(VirtualNode::ProcTask, subject));
    }
    let (subject, suffix) = if let Some(task_path) = suffix.strip_prefix(b"task/") {
        let split = task_path
            .iter()
            .position(|byte| *byte == b'/')
            .unwrap_or(task_path.len());
        let tid = decimal(&task_path[..split])?;
        if visible(runtime, caller, tid)?.tgid != runtime.thread_group(subject) {
            return None;
        }
        (tid, task_path.get(split + 1..).unwrap_or_default())
    } else {
        (subject, suffix)
    };
    let node = match suffix {
        b"" => VirtualNode::ProcProcess,
        b"stat" => VirtualNode::ProcStat,
        b"status" => VirtualNode::ProcStatus,
        b"cmdline" => VirtualNode::ProcCmdline,
        b"comm" => VirtualNode::ProcComm,
        b"statm" => VirtualNode::ProcStatm,
        b"exe" => VirtualNode::ProcSelfExe,
        b"net" => return Some(VirtualNode::ProcNet.id()),
        b"net/arp" => return Some(VirtualNode::ProcNetArp.id()),
        _ => return None,
    };
    Some(resource(node, subject))
}

pub(super) fn validate(runtime: &Runtime, caller: Word, value: Word) -> Result<(), i32> {
    let subject = value >> 32;
    if subject != 0 && visible(runtime, caller, subject).is_none() {
        return Err(ENOENT);
    }
    Ok(())
}

pub(super) fn directory_path(value: Word, out: &mut [u8]) -> Option<usize> {
    let mut text = Text::new(out);
    match node(value)? {
        VirtualNode::ProcProcess => write!(text, "/proc/{}", value >> 32).ok()?,
        VirtualNode::ProcTask => write!(text, "/proc/{}/task", value >> 32).ok()?,
        _ => return None,
    }
    Some(text.used)
}

pub(super) struct Entry<'a> {
    pub name: &'a [u8],
    pub resource: Word,
    pub next: Word,
}

pub(super) fn directory_entry<'a>(
    runtime: &Runtime,
    caller: Word,
    value: Word,
    cookie: Word,
    name: &'a mut [u8],
) -> Result<Option<Entry<'a>>, i32> {
    validate(runtime, caller, value)?;
    let directory = node(value).ok_or(ENOTDIR)?;
    let subject = value >> 32;
    if directory == VirtualNode::ProcProcess {
        let entries: &[(&[u8], VirtualNode)] = &[
            (b"stat", VirtualNode::ProcStat),
            (b"status", VirtualNode::ProcStatus),
            (b"cmdline", VirtualNode::ProcCmdline),
            (b"comm", VirtualNode::ProcComm),
            (b"statm", VirtualNode::ProcStatm),
            (b"task", VirtualNode::ProcTask),
            (b"exe", VirtualNode::ProcSelfExe),
            (b"net", VirtualNode::ProcNet),
        ];
        return Ok(entries.get(cookie).map(|(name, node)| Entry {
            name,
            resource: if *node == VirtualNode::ProcNet {
                node.id()
            } else {
                resource(*node, subject)
            },
            next: cookie + 1,
        }));
    }
    if directory == VirtualNode::Proc && cookie < ROOT_ENTRIES {
        return Ok(
            virtual_fs::directory_entry(directory, cookie, false).map(|entry| Entry {
                name: entry.name,
                resource: entry.node.id(),
                next: cookie + 1,
            }),
        );
    }
    let first = if directory == VirtualNode::Proc {
        cookie.saturating_sub(ROOT_ENTRIES)
    } else {
        cookie
    };
    let current = runtime.managed_process(caller).ok_or(ESRCH)?;
    let next = runtime
        .managed
        .iter()
        .filter(|process| {
            process.pid != 0
                && process.pid >= first
                && process.rootfs.leader == current.rootfs.leader
                && if directory == VirtualNode::Proc {
                    process.pid == process.tgid
                } else {
                    process.tgid == runtime.thread_group(subject)
                }
        })
        .min_by_key(|process| process.pid);
    let Some(process) = next else {
        return Ok(None);
    };
    let mut text = Text::new(name);
    write!(text, "{}", process.pid).map_err(|_| EIO)?;
    let used = text.used;
    Ok(Some(Entry {
        name: &name[..used],
        resource: resource(VirtualNode::ProcProcess, process.pid),
        next: process.pid
            + 1
            + if directory == VirtualNode::Proc {
                ROOT_ENTRIES
            } else {
                0
            },
    }))
}

// proc stat uses zero st_size, unlike an ordinary file's current text length.
pub(super) fn stat(
    runtime: &Runtime,
    caller: Word,
    value: Word,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    validate(runtime, caller, value)?;
    let node = node(value).ok_or(ENOENT)?;
    let kind = if node.is_directory() {
        posix::POSIX_FILE_TYPE_DIRECTORY
    } else {
        posix::POSIX_FILE_TYPE_REGULAR
    };
    // High inode bits carry POSIX mode in metadata.rs; never pack a PID there.
    let inode = ((value >> 32) * 64 + node.id()) & NODE_MASK;
    Ok((inode, 0, kind, 0, 0))
}

fn state(runtime: &Runtime, process: &ManagedProcess) -> &'static str {
    if process.exited {
        "Z (zombie)"
    } else if process.signal_stopped() {
        "T (stopped)"
    } else if process.signal_waiting
        || process.terminal_read_waiting
        || process.device_read_waiting
        || process.network_waiting
        || process.sleep_waiting
        || process.readiness_wait.is_some()
        || runtime
            .tasks
            .get(process.pid)
            .is_some_and(|task| task.wait.is_some())
    {
        "S (sleeping)"
    } else {
        "R (running)"
    }
}

pub(super) fn read(
    runtime: &mut Runtime,
    caller: Word,
    value: Word,
    offset: Word,
    out: &mut [u8],
) -> Result<usize, i32> {
    validate(runtime, caller, value)?;
    let node = node(value).ok_or(EBADF)?;
    let subject = value >> 32;
    if node == VirtualNode::ProcCmdline {
        return cmdline(runtime, subject, offset, out);
    }
    let mut buffer = [0u8; 2048];
    let mut text = Text::new(&mut buffer);
    if node == VirtualNode::ProcUptime {
        super::clocks::refresh_clock(runtime)?;
        let hz = runtime.monotonic_tick_hz.max(1);
        write!(
            text,
            "{}.{:02} 0.00\n",
            runtime.monotonic_ticks / hz,
            ((runtime.monotonic_ticks % hz) as u128 * 100 / hz as u128) as Word
        )
        .map_err(|_| EIO)?;
    } else {
        let process = visible(runtime, caller, subject).ok_or(ENOENT)?;
        let name = &process.comm[..process
            .comm
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(15)];
        let name = core::str::from_utf8(name).unwrap_or("?");
        let state = state(runtime, process);
        let parent = visible(runtime, caller, process.parent_pid).map_or(0, |parent| parent.tgid);
        let threads = runtime
            .managed
            .iter()
            .filter(|other| other.pid != 0 && other.tgid == process.tgid && !other.exited)
            .count()
            .max(1);
        let virtual_bytes: Word = process
            .mappings
            .iter()
            .filter(|mapping| mapping.base != 0)
            .map(|mapping| mapping.size)
            .sum();
        match node {
            VirtualNode::ProcComm => writeln!(text, "{name}"),
            VirtualNode::ProcStat => {
                let terminal = process.job.controlling_terminal;
                const LINUX_VT_MAJOR: Word = 4;
                const LINUX_PTY_MAJOR: Word = 136;
                let tty = if terminal == 0 {
                    0
                } else if terminal & crate::state::pty::TERMINAL_TAG != 0 {
                    runtime
                        .ptys
                        .get(terminal & !crate::state::pty::TERMINAL_TAG)
                        .map_or(0, |pty| (LINUX_PTY_MAJOR << 8) | pty.number as Word)
                } else {
                    (LINUX_VT_MAJOR << 8) | 1
                };
                let foreground = runtime
                    .terminal_controls
                    .iter()
                    .find(|control| terminal != 0 && control.id == terminal)
                    .map_or(-1, |control| control.foreground as isize);
                // Numeric fields 4..52, indexed relative to field 4 (ppid).
                // CPU/fault/RSS accounting is unavailable: never substitute
                // elapsed wall time or VMA size for CPU time or resident pages.
                let mut fields = [0isize; 49];
                fields[0] = parent as isize;
                fields[1] = process.job.group as isize;
                fields[2] = process.job.session as isize;
                fields[3] = tty as isize;
                fields[4] = foreground;
                fields[18 - 4] = 20; // priority: ordinary nice-0 Linux task.
                fields[20 - 4] = threads as isize;
                fields[22 - 4] = process.started_at as isize;
                fields[23 - 4] = virtual_bytes as isize;
                fields[38 - 4] = LINUX_SIGCHLD as isize;
                fields[52 - 4] = process.exit_status as isize;
                write!(
                    text,
                    "{} ({name}) {}",
                    process.pid,
                    state.as_bytes()[0] as char
                )
                .map_err(|_| EIO)?;
                for field in fields {
                    write!(text, " {field}").map_err(|_| EIO)?;
                }
                writeln!(text)
            }
            VirtualNode::ProcStatus => write!(
                text,
                concat!(
                    "Name:\t{}\nState:\t{}\nTgid:\t{}\nPid:\t{}\nPPid:\t{}\n",
                    "TracerPid:\t0\nUid:\t0\t0\t0\t0\nGid:\t0\t0\t0\t0\n",
                    "FDSize:\t{}\nGroups:\t0\nThreads:\t{}\nVmSize:\t{} kB\n"
                ),
                name,
                state,
                process.tgid,
                process.pid,
                parent,
                LINUX_FD_MAX,
                threads,
                virtual_bytes / 1024
            ),
            VirtualNode::ProcStatm => {
                writeln!(text, "{} 0 0 0 0 0 0", virtual_bytes / LINUX_PAGE_SIZE)
            }
            _ => return Err(EINVAL),
        }
        .map_err(|_| EIO)?;
    }
    let used = text.used;
    let bytes = &buffer[offset.min(used)..used];
    let count = bytes.len().min(out.len());
    out[..count].copy_from_slice(&bytes[..count]);
    Ok(count)
}

fn cmdline(
    runtime: &mut Runtime,
    subject: Word,
    mut skip: Word,
    out: &mut [u8],
) -> Result<usize, i32> {
    let process = runtime.managed_process(subject).ok_or(ENOENT)?;
    if process.exited || process.argument_count == 0 || out.is_empty() {
        return Ok(0);
    }
    let mut pointers = [0usize; ALTER_LAUNCH_MAX_ARGS];
    let argc = process.argument_count.min(pointers.len());
    nanami_runtime::request_process_memory_read_checked(
        subject,
        process.argument_vector,
        runtime.posix_shm,
        argc * 8,
    )
    .map_err(|_| EIO)?;
    unsafe {
        core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const Word,
            pointers.as_mut_ptr(),
            argc,
        );
    }
    let mut used = 0;
    for &pointer in &pointers[..argc] {
        let mut cursor = pointer;
        let mut remaining = LINUX_EXEC_STRING_MAX;
        while remaining != 0 {
            let count = remaining
                .min(128)
                .min(LINUX_PAGE_SIZE - (cursor & (LINUX_PAGE_SIZE - 1)));
            nanami_runtime::request_process_memory_read_checked(
                subject,
                cursor,
                runtime.posix_shm,
                count,
            )
            .map_err(|_| EIO)?;
            let bytes =
                unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, count) };
            let end = bytes.iter().position(|byte| *byte == 0);
            let count = end.map_or(count, |end| end + 1);
            let start = skip.min(count);
            skip -= start;
            let amount = (count - start).min(out.len() - used);
            out[used..used + amount].copy_from_slice(&bytes[start..start + amount]);
            used += amount;
            if used == out.len() {
                return Ok(used);
            }
            if end.is_some() {
                break;
            }
            cursor = cursor.checked_add(count).ok_or(EIO)?;
            remaining -= count;
        }
    }
    Ok(used)
}

struct Text<'a> {
    bytes: &'a mut [u8],
    used: usize,
}
impl<'a> Text<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, used: 0 }
    }
}
impl Write for Text<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self
            .used
            .checked_add(text.len())
            .filter(|end| *end <= self.bytes.len())
            .ok_or(fmt::Error)?;
        self.bytes[self.used..end].copy_from_slice(text.as_bytes());
        self.used = end;
        Ok(())
    }
}
