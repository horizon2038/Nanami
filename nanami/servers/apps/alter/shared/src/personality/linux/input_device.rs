use super::{
    honoka, input, write_target_memory, write_u16, write_u32, write_u64, LinuxFile, LinuxFileKind,
    LinuxSyscallContext, Runtime, Word, EAGAIN, EBADF, EFAULT, EINVAL, ENODEV, ENOTTY,
    LINUX_EVIOCGID, LINUX_EVIOCGVERSION, LINUX_EV_KEY, LINUX_EV_REL, LINUX_EV_SYN,
    LINUX_INPUT_EVENT_BYTES, LINUX_REL_WHEEL, LINUX_REL_X, LINUX_REL_Y, LINUX_SYN_REPORT,
};

pub(super) fn pump_input_events(runtime: &mut Runtime) {
    let source = runtime.input_queue;
    let mut graphics_index = 0usize;
    while graphics_index < runtime.graphics.len() {
        let session = runtime.graphics[graphics_index];
        if session.active && session.input_queue != 0 {
            drain_input_queue(runtime, session.input_queue, graphics_index as Word + 1);
        }
        graphics_index += 1;
    }
    if source != 0 {
        drain_input_queue(runtime, source, 0);
    }
}

pub(super) fn drain_input_queue(runtime: &mut Runtime, source: Word, session_id: Word) {
    let mut queue = honoka::WindowEventQueue::new(source);
    while let Some(packed) = queue.pop() {
        let (kind, code, value0, value1, flags) = input::unpack_input_event(packed);
        match kind {
            input::INPUT_EVENT_KIND_WINDOW_RESIZE if session_id != 0 => {
                if let Some(session) = runtime.graphics.get(session_id as usize - 1) {
                    if let Err(error) = honoka::honoka_resize_viewport(session.honoka_port, session.window_id,
                        value0 as u16 as Word, value1 as u16 as Word) {
                        libnanami::println!("[alter/linux] framebuffer viewport resize failed: {}", error);
                    }
                }
            }
            input::INPUT_EVENT_KIND_KEY => {
                push_keyboard_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_KEY, linux_key_code(code), value0 as i32),
                );
                push_keyboard_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                );
            }
            input::INPUT_EVENT_KIND_MOUSE_MOVE => {
                let (dx, dy) = normalize_mouse_movement(
                    runtime,
                    session_id,
                    value0 as i32,
                    value1 as i32,
                    flags,
                );
                if dx != 0 {
                    push_mouse_event(
                        runtime,
                        session_id,
                        pack_linux_input_event(LINUX_EV_REL, LINUX_REL_X, dx),
                    );
                }
                if dy != 0 {
                    push_mouse_event(
                        runtime,
                        session_id,
                        pack_linux_input_event(LINUX_EV_REL, LINUX_REL_Y, dy),
                    );
                }
                if dx != 0 || dy != 0 {
                    push_mouse_event(
                        runtime,
                        session_id,
                        pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                    );
                }
            }
            input::INPUT_EVENT_KIND_MOUSE_BUTTON => {
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_KEY, linux_mouse_button(code), value0 as i32),
                );
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                );
            }
            input::INPUT_EVENT_KIND_MOUSE_WHEEL => {
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_REL, LINUX_REL_WHEEL, value0 as i32),
                );
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                );
            }
            _ => {}
        }
    }
}

pub(super) fn normalize_mouse_movement(
    runtime: &mut Runtime,
    session_id: Word,
    x: i32,
    y: i32,
    flags: Word,
) -> (i32, i32) {
    if session_id == 0 || (flags & honoka::HONOKA_INPUT_FLAG_ABSOLUTE) == 0 {
        return (x, y);
    }
    let Some(session) = runtime.graphics.get_mut(session_id as usize - 1) else {
        return (0, 0);
    };
    let movement = if session.mouse_position_valid {
        (
            x.saturating_sub(session.mouse_x),
            y.saturating_sub(session.mouse_y),
        )
    } else {
        (0, 0)
    };
    session.mouse_x = x;
    session.mouse_y = y;
    session.mouse_position_valid = true;
    movement
}

pub(super) fn push_keyboard_event(runtime: &mut Runtime, session_id: Word, event: Word) {
    if session_id == 0 {
        runtime.push_keyboard_event(event);
        return;
    }
    let Some(session) = runtime.graphics.get_mut(session_id as usize - 1) else {
        return;
    };
    push_event_ring(
        &mut session.keyboard_events,
        &mut session.keyboard_head,
        &mut session.keyboard_tail,
        event,
    );
}

pub(super) fn push_mouse_event(runtime: &mut Runtime, session_id: Word, event: Word) {
    if session_id == 0 {
        runtime.push_mouse_event(event);
        return;
    }
    let Some(session) = runtime.graphics.get_mut(session_id as usize - 1) else {
        return;
    };
    push_event_ring(
        &mut session.mouse_events,
        &mut session.mouse_head,
        &mut session.mouse_tail,
        event,
    );
}

pub(super) fn pop_keyboard_event(runtime: &mut Runtime, session_id: Word) -> Option<Word> {
    if session_id == 0 {
        return runtime.pop_keyboard_event();
    }
    let session = runtime.graphics.get_mut(session_id as usize - 1)?;
    pop_event_ring(
        &session.keyboard_events,
        &mut session.keyboard_head,
        session.keyboard_tail,
    )
}

pub(super) fn pop_mouse_event(runtime: &mut Runtime, session_id: Word) -> Option<Word> {
    if session_id == 0 {
        return runtime.pop_mouse_event();
    }
    let session = runtime.graphics.get_mut(session_id as usize - 1)?;
    pop_event_ring(
        &session.mouse_events,
        &mut session.mouse_head,
        session.mouse_tail,
    )
}

pub(super) fn keyboard_event_ready(runtime: &Runtime, session_id: Word) -> bool {
    if session_id == 0 {
        return runtime.keyboard_event_ready();
    }
    runtime
        .graphics
        .get(session_id as usize - 1)
        .map(|session| session.keyboard_head != session.keyboard_tail)
        .unwrap_or(false)
}

pub(super) fn mouse_event_ready(runtime: &Runtime, session_id: Word) -> bool {
    if session_id == 0 {
        return runtime.mouse_event_ready();
    }
    runtime
        .graphics
        .get(session_id as usize - 1)
        .map(|session| session.mouse_head != session.mouse_tail)
        .unwrap_or(false)
}

pub(super) fn push_event_ring(
    events: &mut [Word; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
    head: &mut usize,
    tail: &mut usize,
    event: Word,
) {
    let next = (*tail + 1) % events.len();
    if next == *head {
        *head = (*head + 1) % events.len();
    }
    events[*tail] = event;
    *tail = next;
}

pub(super) fn pop_event_ring(
    events: &[Word; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
    head: &mut usize,
    tail: usize,
) -> Option<Word> {
    if *head == tail {
        return None;
    }
    let event = events[*head];
    *head = (*head + 1) % events.len();
    Some(event)
}

pub(super) fn sys_evdev_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if len < LINUX_INPUT_EVENT_BYTES {
        return Err(EINVAL);
    }
    let len = len.min(runtime.posix_shm_size);
    if user_buffer == 0 || runtime.posix_shm == 0 || len < LINUX_INPUT_EVENT_BYTES {
        return Err(EFAULT);
    }
    pump_input_events(runtime);
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let session_id = file.resource >> 32;
    let mut written = 0usize;
    while written + LINUX_INPUT_EVENT_BYTES as usize <= len as usize {
        let packed = match file.kind {
            LinuxFileKind::EvdevKeyboard => pop_keyboard_event(runtime, session_id),
            LinuxFileKind::EvdevMouse => pop_mouse_event(runtime, session_id),
            _ => return Err(ENODEV),
        };
        let Some(packed) = packed else { break };
        let (event_type, code, value) = unpack_linux_input_event(packed);
        write_input_event(runtime.posix_shm + written as Word, event_type, code, value);
        written += LINUX_INPUT_EVENT_BYTES as usize;
    }
    if written == 0 {
        return Err(EAGAIN);
    }
    write_target_memory(runtime, pid, user_buffer, written as Word)?;
    Ok(written as Word)
}

pub(super) fn pack_linux_input_event(event_type: u16, code: u16, value: i32) -> Word {
    event_type as Word | ((code as Word) << 16) | (((value as u32) as Word) << 32)
}

pub(super) fn unpack_linux_input_event(event: Word) -> (u16, u16, i32) {
    (
        event as u16,
        (event >> 16) as u16,
        (event >> 32) as u32 as i32,
    )
}

pub(super) fn write_input_event(base: Word, event_type: u16, code: u16, value: i32) {
    unsafe {
        write_u64(base, 0);
        write_u64(base + 8, 0);
        write_u16(base + 16, event_type);
        write_u16(base + 18, code);
        write_u32(base + 20, value as u32);
    }
}

pub(super) fn linux_key_code(code: Word) -> u16 {
    match code {
        0x11c => 96,
        0x11d => 97,
        0x135 => 98,
        0x138 => 100,
        0x147 => 102,
        0x148 => 103,
        0x149 => 104,
        0x14b => 105,
        0x14d => 106,
        0x14f => 107,
        0x150 => 108,
        0x151 => 109,
        0x152 => 110,
        0x153 => 111,
        _ => (code & 0x7f) as u16,
    }
}

pub(super) fn linux_mouse_button(code: Word) -> u16 {
    match code {
        1 => 0x110,
        2 => 0x111,
        3 => 0x112,
        _ => 0x110,
    }
}

pub(super) fn sys_evdev_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    if argument == 0 {
        return Err(EFAULT);
    }
    if request == LINUX_EVIOCGVERSION {
        unsafe { write_u32(runtime.posix_shm, 0x0001_0001) };
        write_target_memory(runtime, pid, argument, 4)?;
        return Ok(0);
    }
    if request == LINUX_EVIOCGID {
        unsafe {
            write_u16(runtime.posix_shm, 0x0011);
            write_u16(runtime.posix_shm + 2, 0);
            write_u16(runtime.posix_shm + 4, 0);
            write_u16(runtime.posix_shm + 6, 1);
        }
        write_target_memory(runtime, pid, argument, 8)?;
        return Ok(0);
    }

    let direction = (request >> 30) & 0x3;
    let ioctl_type = (request >> 8) & 0xff;
    let number = request & 0xff;
    let requested_len = ((request >> 16) & 0x3fff).min(runtime.posix_shm_size);
    if direction == 2 && ioctl_type == 0x45 && number == 0x06 {
        let name = if file.kind == LinuxFileKind::EvdevKeyboard {
            b"Nanami PS/2 Keyboard\0" as &[u8]
        } else {
            b"Nanami PS/2 Mouse\0" as &[u8]
        };
        let bytes = requested_len.min(name.len() as Word);
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                name.as_ptr(),
                runtime.posix_shm as *mut u8,
                bytes as usize,
            );
        }
        write_target_memory(runtime, pid, argument, bytes)?;
        return Ok(bytes);
    }
    if direction == 2 && ioctl_type == 0x45 && (0x20..=0x3f).contains(&number) {
        let bytes = requested_len;
        unsafe { ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, bytes as usize) };
        let event_type = number - 0x20;
        if event_type == 0 && bytes != 0 {
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_EV_SYN as Word);
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_EV_KEY as Word);
            if file.kind == LinuxFileKind::EvdevMouse {
                set_bitmap_bit(runtime.posix_shm, bytes, LINUX_EV_REL as Word);
            }
        } else if event_type == LINUX_EV_KEY as Word {
            if file.kind == LinuxFileKind::EvdevKeyboard {
                let mut bit = 0;
                while bit <= 0xff {
                    set_bitmap_bit(runtime.posix_shm, bytes, bit);
                    bit += 1;
                }
            } else {
                set_bitmap_bit(runtime.posix_shm, bytes, 0x110);
                set_bitmap_bit(runtime.posix_shm, bytes, 0x111);
                set_bitmap_bit(runtime.posix_shm, bytes, 0x112);
            }
        } else if event_type == LINUX_EV_REL as Word && file.kind == LinuxFileKind::EvdevMouse {
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_REL_X as Word);
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_REL_Y as Word);
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_REL_WHEEL as Word);
        }
        write_target_memory(runtime, pid, argument, bytes)?;
        return Ok(bytes);
    }
    Err(ENOTTY)
}

pub(super) fn set_bitmap_bit(base: Word, bytes: Word, bit: Word) {
    let byte = bit / 8;
    if byte >= bytes {
        return;
    }
    unsafe {
        let pointer = (base + byte) as *mut u8;
        *pointer |= 1 << (bit & 7);
    }
}

pub fn wake_device_readers(runtime: &mut Runtime) {
    pump_input_events(runtime);
    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.device_read_waiting {
            index += 1;
            continue;
        }
        let result = sys_evdev_read(
            runtime,
            process.pid,
            process.device_read_fd,
            process.device_read_buffer,
            process.device_read_len,
        );
        let return_value = match result {
            Err(EAGAIN) => {
                index += 1;
                continue;
            }
            Ok(bytes) => bytes as isize,
            Err(errno) => -(errno as isize),
        };
        runtime.managed[index].device_read_waiting = false;
        runtime.managed[index].device_read_fd = 0;
        runtime.managed[index].device_read_buffer = 0;
        runtime.managed[index].device_read_len = 0;
        runtime.managed[index].device_read_context = LinuxSyscallContext::EMPTY;
        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.device_read_context,
            return_value,
            process.personality,
        )
        .is_ok()
        {
            let _ = a9n_abi::arch::process_control_block::resume(process.pcb);
        }
        index += 1;
    }
}
