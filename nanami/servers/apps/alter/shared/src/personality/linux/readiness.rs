use super::{
    keyboard_event_ready, mouse_event_ready, pump_input_events, socket_readiness,
    terminal_readable, LinuxFile, LinuxFileKind, Runtime, Word, EBADF, LINUX_PIPE_BYTES,
    LINUX_POLLERR, LINUX_POLLHUP, LINUX_POLLIN, LINUX_POLLNVAL, LINUX_POLLOUT, LINUX_POLLPRI,
    LINUX_POLLRDNORM, LINUX_POLLWRNORM,
};
use crate::state::readiness::*;

pub(super) fn scan_readiness(
    runtime: &mut Runtime,
    pid: Word,
    wait: &mut ReadinessWait,
) -> Result<Word, i32> {
    let mut ready = 0;
    let mut input_pumped = false;
    wait.sources = 0;
    for entry in &mut wait.entries[..wait.count] {
        entry.revents = if entry.fd < 0 {
            0
        } else if let Some(file) = runtime.linux_file(pid, entry.fd as Word) {
            wait.sources |= match file.kind {
                LinuxFileKind::Terminal => READY_TERMINAL,
                LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => READY_PIPE,
                LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => READY_INPUT,
                LinuxFileKind::SocketUdp
                | LinuxFileKind::SocketTcp
                | LinuxFileKind::SocketTcpListener
                | LinuxFileKind::SocketIcmp => READY_NETWORK,
                _ => 0,
            };
            let available = fd_readiness(
                runtime,
                pid,
                entry.fd as Word,
                file,
                entry.events,
                &mut input_pumped,
            )?;
            // ERR/HUP/NVAL are reported even when no events were requested.
            available & (entry.events | LINUX_POLLERR | LINUX_POLLHUP | LINUX_POLLNVAL)
        } else {
            LINUX_POLLNVAL
        };
        if wait.select && entry.revents & LINUX_POLLNVAL != 0 {
            return Err(EBADF);
        }
        let reported = if wait.select {
            (entry.events & LINUX_POLLIN != 0
                && entry.revents & (LINUX_POLLIN | LINUX_POLLHUP | LINUX_POLLERR) != 0)
                || (entry.events & LINUX_POLLOUT != 0
                    && entry.revents & (LINUX_POLLOUT | LINUX_POLLERR) != 0)
                || (entry.events & LINUX_POLLPRI != 0 && entry.revents & LINUX_POLLPRI != 0)
        } else {
            entry.revents != 0
        };
        ready += Word::from(reported);
    }
    Ok(ready)
}

fn fd_readiness(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    file: LinuxFile,
    events: i16,
    input_pumped: &mut bool,
) -> Result<i16, i32> {
    let read = LINUX_POLLIN | LINUX_POLLRDNORM;
    let write = LINUX_POLLOUT | LINUX_POLLWRNORM;
    Ok(match file.kind {
        LinuxFileKind::Terminal => {
            write
                | if events & read != 0 && terminal_readable(runtime, pid)? {
                    read
                } else {
                    0
                }
        }
        LinuxFileKind::Posix | LinuxFileKind::VirtualFile | LinuxFileKind::Framebuffer => {
            read | write
        }
        LinuxFileKind::VirtualDirectory => read,
        LinuxFileKind::PipeRead => match runtime.pipe(file.posix_fd) {
            Some(pipe) => {
                (if pipe.len != 0 { read } else { 0 })
                    | (if pipe.writers == 0 { LINUX_POLLHUP } else { 0 })
            }
            None => LINUX_POLLNVAL,
        },
        LinuxFileKind::PipeWrite => match runtime.pipe(file.posix_fd) {
            Some(pipe) => {
                (if pipe.len < LINUX_PIPE_BYTES {
                    write
                } else {
                    0
                }) | (if pipe.readers == 0 { LINUX_POLLERR } else { 0 })
            }
            None => LINUX_POLLNVAL,
        },
        LinuxFileKind::SocketNetlink => write | if file.peer_port != 0 { read } else { 0 },
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp => socket_readiness(runtime, pid, fd, file)?,
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            if events & read == 0 {
                return Ok(0);
            }
            if !*input_pumped {
                pump_input_events(runtime);
                *input_pumped = true;
            }
            let ready = if file.kind == LinuxFileKind::EvdevKeyboard {
                keyboard_event_ready(runtime, file.resource >> 32)
            } else {
                mouse_event_ready(runtime, file.resource >> 32)
            };
            if ready {
                read
            } else {
                0
            }
        }
        LinuxFileKind::Empty => LINUX_POLLNVAL,
    })
}
