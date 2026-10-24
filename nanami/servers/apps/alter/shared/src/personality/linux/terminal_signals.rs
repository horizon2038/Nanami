//! Convert configured terminal control bytes into foreground-group signals.
use super::*;
use crate::common::signals::Info;
use crate::state::OsPersonality;
use nanami_client::terminal::{terminal_input_controls, TERMINAL_INPUT_FLUSH};

pub(super) fn read_terminal_input(
    runtime: &mut Runtime,
    pid: Word,
    max_len: Word,
) -> Result<Word, i32> {
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let id = process.terminal_id;
    let mut flags = LINUX_ISIG | LINUX_ECHOCTL;
    if process.terminal_echo {
        flags |= LINUX_ECHO;
    }
    let mut controls = [3, 28, 26];
    if let Some(termios) = process.terminal_termios {
        flags = u32::from_ne_bytes(termios[12..16].try_into().unwrap());
        for (target, index) in controls
            .iter_mut()
            .zip([LINUX_VINTR, LINUX_VQUIT, LINUX_VSUSP])
        {
            *target = termios[17 + index];
        }
    }
    if process.personality != OsPersonality::Linux || flags & LINUX_ISIG == 0 {
        return nanami_client::terminal::terminal_read_input(runtime.terminal_port, id, 0, max_len)
            .map_err(map_request_error);
    }
    let filter = terminal_input_controls(
        controls,
        flags & LINUX_NOFLSH == 0,
        flags & (LINUX_ECHO | LINUX_ECHOCTL) == (LINUX_ECHO | LINUX_ECHOCTL),
    );
    let (bytes, events) = nanami_client::terminal::terminal_read_input_filtered(
        runtime.terminal_port,
        id,
        max_len,
        filter,
    )
    .map_err(map_request_error)?;
    if events == 0 {
        return Ok(bytes);
    }
    if filter & TERMINAL_INPUT_FLUSH != 0 {
        for process in &mut runtime.managed {
            if process.pid != 0 && process.terminal_id == id {
                process.terminal_line_len = 0;
                process.terminal_line_read = 0;
                process.terminal_line_ready = false;
            }
        }
    }
    let Some(control) = runtime
        .terminal_controls
        .iter()
        .find(|control| control.id == id)
        .copied()
    else {
        return Ok(bytes);
    };
    for index in 0..runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0
            || process.exited
            || process.personality != OsPersonality::Linux
            || process.job.session != control.session
            || process.job.group != control.foreground
            || process.job.controlling_terminal != id
        {
            continue;
        }
        for (index, signal) in [2, 3, 20].into_iter().enumerate() {
            if events & (1 << index) != 0 {
                let _ = signals_runtime::queue(runtime, process.pid, Info::new(signal, 128, 0));
            }
        }
    }
    Ok(bytes)
}

pub fn handle_terminal_input(runtime: &mut Runtime) {
    // Notification badges can coalesce multiple terminal IDs. Scan each live
    // controlling terminal once, even when none of its programs calls read().
    for index in 0..runtime.terminal_controls.len() {
        let control = runtime.terminal_controls[index];
        if control.id == 0 {
            continue;
        }
        let pid = runtime
            .managed
            .iter()
            .find(|process| {
                process.pid != 0
                    && !process.exited
                    && process.job.session == control.session
                    && process.job.group == control.foreground
                    && process.job.controlling_terminal == control.id
            })
            .map(|process| process.pid);
        if let Some(pid) = pid {
            let _ = read_terminal_input(runtime, pid, 0);
        }
    }
    super::wake_terminal_readers(runtime);
}
