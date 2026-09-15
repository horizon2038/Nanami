use super::{
    honoka, input, map_request_error, Runtime, Word, ALTER_FB_BYTES, ALTER_FB_HEIGHT,
    ALTER_FB_WIDTH, EINVAL, ENODEV, ENOENT, ENOMEM, ESRCH, SLOT_HONOKA_PRESENT_NOTIFICATION_BASE,
    SLOT_HONOKA_SERVICE, SLOT_INPUT_SERVICE,
};

pub(super) fn graphics_enabled(runtime: &Runtime, pid: Word) -> bool {
    runtime
        .managed_process(pid)
        .map(|process| process.graphics_enabled)
        .unwrap_or(false)
}

pub(super) fn input_resource(runtime: &Runtime, pid: Word, node: Word) -> Result<Word, i32> {
    let session = runtime
        .managed_process(pid)
        .map(|process| process.graphics_session)
        .ok_or(ESRCH)?;
    Ok((session << 32) | node)
}

pub(super) fn ensure_input(runtime: &mut Runtime, pid: Word) -> Result<(), i32> {
    if graphics_enabled(runtime, pid) {
        ensure_graphics_session(runtime, pid)?;
        return Ok(());
    }
    if runtime.input_port != 0 && runtime.input_queue != 0 {
        return Ok(());
    }
    nanami_services::registry::connect_input_service(SLOT_INPUT_SERVICE).map_err(|_| ENODEV)?;
    let port = libnanami::ipc::process_slot_descriptor(SLOT_INPUT_SERVICE);
    let (queue, bytes) = input::input_service_subscribe_shared(
        port,
        input::INPUT_SUBSCRIBE_KEYBOARD | input::INPUT_SUBSCRIBE_MOUSE,
    )
    .map_err(map_request_error)?;
    if queue == 0 || bytes == 0 {
        return Err(ENODEV);
    }
    runtime.input_port = port;
    runtime.input_queue = queue;
    runtime.input_queue_size = bytes;
    Ok(())
}

pub(super) fn ensure_graphics_session(runtime: &mut Runtime, pid: Word) -> Result<Word, i32> {
    if !graphics_enabled(runtime, pid) {
        return Err(ENOENT);
    }
    if let Some(process) = runtime.managed_process(pid) {
        if process.graphics_session != 0 {
            return Ok(process.graphics_session);
        }
    }
    let root_pid = process_tree_root(runtime, pid)?;
    let mut index = 0usize;
    while index < runtime.graphics.len() {
        if runtime.graphics[index].active && runtime.graphics[index].root_pid == root_pid {
            let id = index as Word + 1;
            let _ = runtime.set_graphics_session(pid, id);
            return Ok(id);
        }
        index += 1;
    }
    let Some(index) = runtime.graphics.iter().position(|entry| !entry.active) else {
        return Err(ENOMEM);
    };
    if runtime.honoka_port == 0 {
        runtime.honoka_pid =
            nanami_services::registry::connect_honoka_service_with_pid(SLOT_HONOKA_SERVICE)
                .map_err(|_| ENODEV)?;
        runtime.honoka_port = libnanami::ipc::process_slot_descriptor(SLOT_HONOKA_SERVICE);
    }
    let honoka_pid = runtime.honoka_pid;
    let port = runtime.honoka_port;
    let window = honoka::honoka_create_window_with_title(
        port,
        80 + (index as Word * 32),
        80 + (index as Word * 32),
        ALTER_FB_WIDTH,
        ALTER_FB_HEIGHT,
        b"Alter/Linux fb0",
    )
    .map_err(map_request_error)?;
    let present_slot = SLOT_HONOKA_PRESENT_NOTIFICATION_BASE + index as Word;
    if let Err(error) = libnanami::request_notification_port_copy(
        honoka_pid,
        libnanami::PROCESS_SLOT_NOTIFICATION,
        present_slot,
        honoka::HONOKA_NOTIFICATION_PRESENT | (window & 0xffff_ffff),
    ) {
        let _ = honoka::honoka_destroy_window(port, window);
        return Err(map_request_error(error));
    }
    let present_notification = libnanami::ipc::process_slot_descriptor(present_slot);
    let (input_queue, _) = match honoka::honoka_attach_input_queue(port, window) {
        Ok(attached) => attached,
        Err(error) => {
            let _ = honoka::honoka_destroy_window(port, window);
            return Err(map_request_error(error));
        }
    };
    if let Err(error) = honoka::honoka_attach_input_notification(port, window) {
        let _ = libnanami::request_mapping_release(input_queue, input::INPUT_EVENT_QUEUE_BYTES);
        let _ = honoka::honoka_destroy_window(port, window);
        return Err(map_request_error(error));
    }
    runtime.graphics[index] = crate::state::GraphicsSession {
        active: true,
        root_pid,
        honoka_port: port,
        present_notification,
        window_id: window,
        width: ALTER_FB_WIDTH,
        height: ALTER_FB_HEIGHT,
        damage_queue: 0,
        framebuffer: 0,
        framebuffer_bytes: ALTER_FB_BYTES,
        input_queue,
        keyboard_events: [0; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
        keyboard_head: 0,
        keyboard_tail: 0,
        mouse_events: [0; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
        mouse_head: 0,
        mouse_tail: 0,
        mouse_x: 0,
        mouse_y: 0,
        mouse_position_valid: false,
        guest_pid: 0,
        guest_framebuffer: 0,
        guest_framebuffer_bytes: 0,
    };
    let id = index as Word + 1;
    let _ = runtime.set_graphics_session(pid, id);
    Ok(id)
}

pub(super) fn process_tree_root(runtime: &Runtime, pid: Word) -> Result<Word, i32> {
    let mut current = pid;
    let mut depth = 0usize;
    while depth < runtime.managed.len() {
        let process = runtime.managed_process(current).ok_or(ESRCH)?;
        if process.parent_pid == 0 {
            return Ok(process.pid);
        }
        current = process.parent_pid;
        depth += 1;
    }
    Err(EINVAL)
}

pub(super) fn cleanup_graphics_for_process(runtime: &mut Runtime, pid: Word) {
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    let session_id = process.graphics_session;
    if session_id == 0 {
        return;
    }
    let index = session_id as usize - 1;
    if index >= runtime.graphics.len() || !runtime.graphics[index].active {
        return;
    }
    if runtime.graphics[index].root_pid != pid {
        return;
    }
    if let Some(successor) = runtime.managed.iter().find(|candidate| {
        candidate.pid != 0
            && candidate.pid != pid
            && candidate.graphics_session == session_id
            && !candidate.exited
    }) {
        runtime.graphics[index].root_pid = successor.pid;
        return;
    }
    let session = runtime.graphics[index];
    let _ = honoka::honoka_destroy_window(session.honoka_port, session.window_id);
    if session.input_queue != 0 {
        let _ =
            libnanami::request_mapping_release(session.input_queue, input::INPUT_EVENT_QUEUE_BYTES);
    }
    runtime.graphics[index] = crate::state::GraphicsSession::EMPTY;
}
