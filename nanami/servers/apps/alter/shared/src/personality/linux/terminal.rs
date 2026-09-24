use super::{
    map_request_error, personality, read_target_memory, sys_evdev_ioctl, sys_framebuffer_ioctl,
    write_target_memory, write_u16, write_u32, write_u8, LinuxFile, LinuxFileKind,
    LinuxSyscallContext, Runtime, Word, EAGAIN, EBADF, EFAULT, EIO, ENOTTY, ESRCH, LINUX_CREAD,
    LINUX_CS8, LINUX_ECHO, LINUX_ECHOE, LINUX_ECHOK, LINUX_ICANON, LINUX_ICRNL, LINUX_IEXTEN,
    LINUX_ISIG, LINUX_IXON, LINUX_ONLCR, LINUX_OPOST, LINUX_TCGETS, LINUX_TCSETS, LINUX_TCSETSF,
    LINUX_TCSETSW, LINUX_TERMINAL_LINE_MAX, LINUX_TERMIOS_BYTES, LINUX_TIOCGWINSZ, LINUX_VEOF,
    LINUX_VEOL, LINUX_VERASE, LINUX_VINTR, LINUX_VKILL, LINUX_VMIN, LINUX_VQUIT, LINUX_VSTART,
    LINUX_VSTOP, LINUX_VSUSP, LINUX_VTIME,
};

pub(super) fn sys_terminal_read(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    sys_terminal_read_now(runtime, pid, user_buffer, len)?.ok_or(EAGAIN)
}

pub(super) fn sys_terminal_read_now(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Option<Word>, i32> {
    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    let chunk = terminal_bounded_len(runtime, len)?;
    if chunk == 0 {
        return Ok(Some(0));
    }
    ensure_terminal_input_notification(runtime, terminal_id)?;

    let canonical = runtime.terminal_canonical(pid).unwrap_or(true);
    let bytes = if canonical {
        drain_terminal_canonical_line(runtime, pid, chunk)?
    } else {
        drain_terminal_raw_input(runtime, terminal_id, chunk)?
    };
    if let Some(bytes) = bytes {
        if bytes != 0 {
            libnanami::request_process_memory_write(pid, user_buffer, runtime.terminal_shm, bytes)
                .map_err(map_request_error)?;
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

pub(super) fn drain_terminal_raw_input(
    runtime: &mut Runtime,
    terminal_id: Word,
    max_len: Word,
) -> Result<Option<Word>, i32> {
    let bytes = nanami_services::terminal::terminal_read_input(
        runtime.terminal_port,
        terminal_id,
        0,
        max_len,
    )
    .map_err(map_request_error)?;
    if bytes == 0 {
        Ok(None)
    } else {
        Ok(Some(bytes))
    }
}

pub(super) fn drain_terminal_canonical_line(
    runtime: &mut Runtime,
    pid: Word,
    max_len: Word,
) -> Result<Option<Word>, i32> {
    fill_terminal_canonical_line(runtime, pid)?;
    Ok(pop_terminal_line(runtime, pid, max_len))
}

fn fill_terminal_canonical_line(runtime: &mut Runtime, pid: Word) -> Result<(), i32> {
    if runtime.managed_process(pid).ok_or(ESRCH)?.terminal_line_ready {
        return Ok(());
    }
    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    loop {
        let bytes = nanami_services::terminal::terminal_read_input(
            runtime.terminal_port,
            terminal_id,
            0,
            1,
        )
        .map_err(map_request_error)?;
        if bytes == 0 {
            return Ok(());
        }
        let byte = unsafe { ::core::ptr::read(runtime.terminal_shm as *const u8) };
        push_terminal_input_byte(runtime, pid, byte)?;
        if runtime.managed_process(pid).ok_or(ESRCH)?.terminal_line_ready {
            return Ok(());
        }
    }
}

pub(super) fn terminal_readable(runtime: &mut Runtime, pid: Word) -> Result<bool, i32> {
    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    ensure_terminal_input_notification(runtime, terminal_id)?;
    if runtime.terminal_canonical(pid).unwrap_or(true) {
        fill_terminal_canonical_line(runtime, pid)?;
        Ok(runtime.managed_process(pid).ok_or(ESRCH)?.terminal_line_ready)
    } else {
        // A zero-length service read reports queued bytes without consuming.
        nanami_services::terminal::terminal_read_input(runtime.terminal_port, terminal_id, 0, 0)
            .map(|bytes| bytes != 0).map_err(map_request_error)
    }
}

pub(super) fn pop_terminal_line(runtime: &mut Runtime, pid: Word, max_len: Word) -> Option<Word> {
    let terminal_shm = runtime.terminal_shm;
    let process = runtime.managed_process_mut(pid)?;
    if !process.terminal_line_ready {
        return None;
    }
    if process.terminal_line_len == 0 {
        process.terminal_line_read = 0;
        process.terminal_line_ready = false;
        return Some(0);
    }
    if process.terminal_line_read >= process.terminal_line_len {
        process.terminal_line_read = 0;
        process.terminal_line_len = 0;
        process.terminal_line_ready = false;
        return None;
    }
    let remaining = process.terminal_line_len - process.terminal_line_read;
    let bytes = ::core::cmp::min(remaining, max_len as usize);
    if bytes == 0 {
        return None;
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            process
                .terminal_line
                .as_ptr()
                .add(process.terminal_line_read),
            terminal_shm as *mut u8,
            bytes,
        );
    }
    process.terminal_line_read += bytes;
    if process.terminal_line_read >= process.terminal_line_len {
        process.terminal_line_read = 0;
        process.terminal_line_len = 0;
        process.terminal_line_ready = false;
    }
    Some(bytes as Word)
}

pub(super) fn push_terminal_input_byte(
    runtime: &mut Runtime,
    pid: Word,
    byte: u8,
) -> Result<(), i32> {
    let Some(process) = runtime.managed_process_mut(pid) else {
        return Err(ESRCH);
    };
    match byte {
        b'\r' | b'\n' => {
            if process.terminal_line_len < LINUX_TERMINAL_LINE_MAX {
                process.terminal_line[process.terminal_line_len] = b'\n';
                process.terminal_line_len += 1;
            }
            process.terminal_line_ready = true;
        }
        0x04 => {
            process.terminal_line_ready = true;
        }
        0x7f | 0x08 => {
            if process.terminal_line_len != 0 {
                process.terminal_line_len -= 1;
            }
        }
        0x20..=0x7e | b'\t' => {
            if process.terminal_line_len + 1 < LINUX_TERMINAL_LINE_MAX {
                process.terminal_line[process.terminal_line_len] = byte;
                process.terminal_line_len += 1;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn ensure_terminal_input_notification(
    runtime: &mut Runtime,
    terminal_id: Word,
) -> Result<(), i32> {
    if runtime.terminal_input_notification_id == terminal_id {
        return Ok(());
    }
    nanami_services::terminal::terminal_attach_input_notification(
        runtime.terminal_port,
        terminal_id,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )
    .map_err(map_request_error)?;
    runtime.terminal_input_notification_id = terminal_id;
    Ok(())
}

pub(super) fn sys_terminal_write(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    let mut done = 0;
    while done < len {
        let chunk = terminal_bounded_len(runtime, len - done)?;
        libnanami::request_process_memory_read(
            pid,
            user_buffer + done,
            runtime.terminal_shm,
            chunk,
        )
        .map_err(map_request_error)?;
        let written = nanami_services::terminal::terminal_write_output(
            runtime.terminal_port,
            terminal_id,
            0,
            chunk,
        )
        .map_err(map_request_error)?;
        if written == 0 {
            return Err(EIO);
        }
        done += written;
        if written < chunk {
            break;
        }
    }
    Ok(done)
}

pub(super) fn sys_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            return sys_evdev_ioctl(runtime, pid, file, request, argument);
        }
        LinuxFileKind::Framebuffer => {
            return sys_framebuffer_ioctl(runtime, pid, file, request, argument);
        }
        LinuxFileKind::Terminal => {}
        _ => return Err(ENOTTY),
    }
    if request == LINUX_TCGETS {
        if argument == 0 {
            return Err(EFAULT);
        }
        let (canonical, echo) = runtime
            .managed_process(pid)
            .map(|process| (process.terminal_canonical, process.terminal_echo))
            .unwrap_or((true, true));
        write_linux_termios(runtime.posix_shm, canonical, echo);
        if let Some(termios) = runtime.managed_process(pid).and_then(|p| p.terminal_termios) {
            unsafe { ::core::ptr::copy_nonoverlapping(termios.as_ptr(), runtime.posix_shm as *mut u8, termios.len()); }
        }
        write_target_memory(runtime, pid, argument, LINUX_TERMIOS_BYTES)?;
        return Ok(0);
    }
    if request == LINUX_TCSETS || request == LINUX_TCSETSW || request == LINUX_TCSETSF {
        if argument == 0 {
            return Err(EFAULT);
        }
        read_target_memory(runtime, pid, argument, LINUX_TERMIOS_BYTES)?;
        let mut termios = [0; 36];
        unsafe { ::core::ptr::copy_nonoverlapping(runtime.posix_shm as *const u8, termios.as_mut_ptr(), termios.len()); }
        let lflag = unsafe { ::core::ptr::read_unaligned((runtime.posix_shm + 12) as *const u32) };
        let oflag = u32::from_ne_bytes(termios[4..8].try_into().unwrap());
        let echo_enabled = (lflag & LINUX_ECHO) != 0;
        let terminal_id = terminal_id_for_pid(runtime, pid)?;
        // Attributes belong to the terminal, not to an individual fork child.
        for process in &mut runtime.managed {
            if process.pid != 0 && process.terminal_id == terminal_id {
                process.terminal_canonical = (lflag & LINUX_ICANON) != 0;
                process.terminal_echo = echo_enabled;
                process.terminal_termios = Some(termios);
                if request == LINUX_TCSETSF {
                    process.terminal_line_len = 0;
                    process.terminal_line_read = 0;
                    process.terminal_line_ready = false;
                }
            }
        }
        nanami_services::terminal::terminal_set_output_crlf(runtime.terminal_port, terminal_id,
            oflag & LINUX_OPOST != 0 && oflag & LINUX_ONLCR != 0).map_err(map_request_error)?;
        if request == LINUX_TCSETSF {
            nanami_services::terminal::terminal_clear(runtime.terminal_port, terminal_id,
                nanami_services::terminal::TERMINAL_CLEAR_INPUT).map_err(map_request_error)?;
        }
        nanami_services::terminal::terminal_set_echo(
            runtime.terminal_port,
            terminal_id,
            echo_enabled,
        )
        .map_err(map_request_error)?;
        return Ok(0);
    }
    if request == LINUX_TIOCGWINSZ {
        if argument == 0 {
            return Err(EFAULT);
        }
        let terminal_id = terminal_id_for_pid(runtime, pid)?;
        let (columns, rows) =
            nanami_services::terminal::terminal_get_size(runtime.terminal_port, terminal_id)
                .map_err(map_request_error)?;
        unsafe {
            write_u16(runtime.posix_shm, rows as u16);
            write_u16(runtime.posix_shm + 2, columns as u16);
            write_u16(runtime.posix_shm + 4, 0);
            write_u16(runtime.posix_shm + 6, 0);
        }
        write_target_memory(runtime, pid, argument, 8)?;
        return Ok(0);
    }
    if request == 0x5414 { // TIOCSWINSZ
        if argument == 0 { return Err(EFAULT); }
        read_target_memory(runtime, pid, argument, 8)?;
        let rows = unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const u16) };
        let cols = unsafe { ::core::ptr::read_unaligned((runtime.posix_shm + 2) as *const u16) };
        let terminal_id = terminal_id_for_pid(runtime, pid)?;
        nanami_services::terminal::terminal_set_size(runtime.terminal_port, terminal_id, cols as Word, rows as Word)
            .map_err(map_request_error)?;
        return Ok(0);
    }
    Ok(0)
}

pub(super) fn write_linux_termios(base: Word, canonical: bool, echo: bool) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_TERMIOS_BYTES as usize);
        write_u32(base, LINUX_ICRNL | LINUX_IXON);
        write_u32(base + 4, LINUX_OPOST | LINUX_ONLCR);
        write_u32(base + 8, LINUX_CREAD | LINUX_CS8);
        let mut lflag = LINUX_ISIG | LINUX_ECHOE | LINUX_ECHOK | LINUX_IEXTEN;
        if canonical {
            lflag |= LINUX_ICANON;
        }
        if echo {
            lflag |= LINUX_ECHO;
        }
        write_u32(base + 12, lflag);
        write_u8(base + 16, 0);
        write_u8(base + 17 + LINUX_VEOF, 4);
        write_u8(base + 17 + LINUX_VEOL, 0);
        write_u8(base + 17 + LINUX_VERASE, 0x7f);
        write_u8(base + 17 + LINUX_VINTR, 3);
        write_u8(base + 17 + LINUX_VKILL, 21);
        write_u8(base + 17 + LINUX_VMIN, 1);
        write_u8(base + 17 + LINUX_VQUIT, 28);
        write_u8(base + 17 + LINUX_VSTART, 17);
        write_u8(base + 17 + LINUX_VSTOP, 19);
        write_u8(base + 17 + LINUX_VSUSP, 26);
        write_u8(base + 17 + LINUX_VTIME, 0);
    }
}

pub(super) fn terminal_bounded_len(runtime: &Runtime, len: Word) -> Result<Word, i32> {
    Ok(::core::cmp::min(len, runtime.terminal_shm_size.max(1)))
}

pub(super) fn terminal_id_for_pid(runtime: &Runtime, pid: Word) -> Result<Word, i32> {
    let terminal_id = runtime.terminal_id(pid).ok_or(ESRCH)?;
    if terminal_id == 0 {
        return Err(ENOTTY);
    }
    Ok(terminal_id)
}

pub fn wake_terminal_readers(runtime: &mut Runtime) {
    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.terminal_read_waiting {
            index += 1;
            continue;
        }

        let result = sys_terminal_read_now(
            runtime,
            process.pid,
            process.terminal_read_buffer,
            process.terminal_read_len,
        );
        let return_value = match result {
            Ok(Some(bytes)) => bytes as isize,
            Ok(None) => {
                index += 1;
                continue;
            }
            Err(errno) => -(errno as isize),
        };

        runtime.managed[index].terminal_read_waiting = false;
        runtime.managed[index].terminal_read_buffer = 0;
        runtime.managed[index].terminal_read_len = 0;
        runtime.managed[index].terminal_read_context = LinuxSyscallContext::EMPTY;

        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.terminal_read_context,
            return_value,
            process.personality,
        )
        .is_err()
        {
            libnanami::println!(
                "[alter/{}] terminal wake register write failed pid={} pcb={:#x}",
                personality::name(process.personality),
                process.pid,
                process.pcb
            );
            index += 1;
            continue;
        }
        if let Err(error) = a9n_abi::arch::process_control_block::resume(process.pcb) {
            libnanami::println!(
                "[alter/linux] terminal wake resume failed pid={} pcb={:#x} err={:?}",
                process.pid,
                process.pcb,
                error
            );
        }
        index += 1;
    }
}

pub(super) fn ensure_standard_terminal_fd(runtime: &mut Runtime, pid: Word, fd: Word) {
    if fd > 2 || runtime.linux_file(pid, fd).is_some() {
        return;
    }
    let Some(terminal_id) = runtime.terminal_id(pid) else {
        return;
    };
    if terminal_id == 0 {
        return;
    }
    if runtime.set_linux_file(pid, fd, LinuxFile::terminal()) {
        libnanami::println!(
            "[alter/linux] restored terminal stdio pid={} fd={}",
            pid,
            fd
        );
    }
}
