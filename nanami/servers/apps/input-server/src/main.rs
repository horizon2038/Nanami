#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

mod state;
use state::*;

mod distribution;
use distribution::*;

const SLOT_SERVICE_PORT: Word = 20;
const SLOT_NOTIFICATION: Word = libnanami::PROCESS_SLOT_NOTIFICATION;
const SLOT_SUBSCRIBER_NOTIFICATION_BASE: Word = 32;
const MAX_SUBSCRIBERS: usize = 32;
const MAX_DRIVER_QUEUES: usize = 4;
const EVENT_QUEUE_CAPACITY: usize = 64;
const HEARTBEAT_REQ_INTERVAL: usize = 65536;
const SHARED_QUEUE_HEADER_WORDS: usize = nanami_services::input::INPUT_EVENT_QUEUE_HEADER_WORDS;
const SHARED_QUEUE_CAPACITY: usize = nanami_services::input::INPUT_EVENT_QUEUE_CAPACITY;
const INPUT_NOTIFICATION_IDENTIFIER: Word = nanami_services::input::INPUT_NOTIFICATION_IDENTIFIER;
const INPUT_DRIVER_NOTIFICATION_IDENTIFIER: Word =
    nanami_services::input::INPUT_DRIVER_NOTIFICATION_IDENTIFIER;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[input-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    if let Err(e) = libnanami::ipc::init_ipc_tls() {
        return Err(log_error("[input-server] ipc tls init failed: ", e));
    }

    if let Err(e) = nanami_services::registry::register_input_service() {
        return Err(log_error("[input-server] service register failed: ", e));
    }
    libnanami::print!("[input-server] service registered: input-service\n");

    let notification = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    if let Err(e) = libnanami::ipc::bind_current_thread_notification(notification) {
        return Err(log_error("[input-server] bind notification failed: ", e));
    }

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut state = InputState::new();
    let mut pending_status = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;
    let mut request_count = 0usize;

    loop {
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            has_pending_reply = false;
            let received = match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[input-server] reply_receive failed: ", e)),
            };
            received
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[input-server] receive failed: ", e)),
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                request_count = request_count.wrapping_add(1);
                pending_status = handle_request(request, &mut state);
                has_pending_reply = true;
                if (request_count % HEARTBEAT_REQ_INTERVAL) == 0 {
                    libnanami::print!("[input-server] alive requests=");
                    libnanami::print!("{}", request_count);
                    libnanami::print!("\n");
                }
            }
            ServiceEvent::Notification { identifier, value } => {
                if (identifier & INPUT_DRIVER_NOTIFICATION_IDENTIFIER) != 0 {
                    let _ = drain_driver_queues(&mut state);
                } else {
                    libnanami::print!("[input-server] notification id=");
                    libnanami::print!("{}", identifier);
                    libnanami::print!(" value=");
                    libnanami::print!("{:#x}", value);
                    libnanami::print!("\n");
                }
            }
            // usually, this path is unreachable
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                if used_reply_receive {
                    // Reply target may be gone; drop pending reply and continue serving.
                    has_pending_reply = false;
                }
                libnanami::print!("[input-server] fault id=");
                libnanami::print!("{}", identifier);
                libnanami::print!(" reason=");
                libnanami::print!("{:#x}", reason);
                libnanami::print!("\n");
            }
        }
    }
}

fn handle_request(request: ServiceRequest, state: &mut InputState) -> (Word, Word, Word) {
    match request.code {
        nanami_services::input::INPUT_SERVICE_REQUEST_SUBSCRIBE => {
            let mask = if request.arg0 == 0 {
                nanami_services::input::INPUT_SUBSCRIBE_ALL
            } else {
                request.arg0
            };
            if let Some(index) = find_or_alloc_subscriber(state, request.identifier) {
                state.subscribers[index].event_mask = mask;
                if state.subscribers[index].notification_descriptor == 0 {
                    match attach_subscriber_notification(index, request.identifier) {
                        Ok(descriptor) => {
                            state.subscribers[index].notification_descriptor = descriptor;
                            libnanami::print!("[input-server] subscriber notify attached pid=");
                            libnanami::print!("{}", request.identifier);
                            libnanami::print!(" slot=");
                            libnanami::print!(
                                "{}",
                                SLOT_SUBSCRIBER_NOTIFICATION_BASE + index as Word
                            );
                            libnanami::print!(" desc=");
                            libnanami::print!("{:#x}", descriptor);
                            libnanami::print!("\n");
                        }
                        Err(e) => {
                            log_request_error(
                                "[input-server] subscriber notify attach failed: ",
                                e,
                            );
                            return (map_request_error_to_status(e), 0, 0);
                        }
                    }
                }
                libnanami::print!("[input-server] subscribe pid=");
                libnanami::print!("{}", request.identifier);
                libnanami::print!(" mask=");
                libnanami::print!("{:#x}", mask);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, mask, 0)
            } else {
                (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0)
            }
        }
        nanami_services::input::INPUT_SERVICE_REQUEST_SUBSCRIBE_SHARED => {
            let mask = if request.arg0 == 0 {
                nanami_services::input::INPUT_SUBSCRIBE_ALL
            } else {
                request.arg0
            };
            if let Some(index) = find_or_alloc_subscriber(state, request.identifier) {
                state.subscribers[index].event_mask = mask;
                if state.subscribers[index].shared_queue_local == 0 {
                    match attach_shared_event_queue(request.identifier) {
                        Ok((local_vaddr, peer_vaddr, size_bytes)) => {
                            init_shared_event_queue(local_vaddr);
                            state.subscribers[index].shared_queue_local = local_vaddr;
                            state.subscribers[index].shared_queue_peer = peer_vaddr;
                            state.subscribers[index].shared_queue_bytes = size_bytes;
                            libnanami::print!("[input-server] subscriber queue attached pid=");
                            libnanami::print!("{}", request.identifier);
                            libnanami::print!(" local=");
                            libnanami::print!("{:#x}", local_vaddr);
                            libnanami::print!(" peer=");
                            libnanami::print!("{:#x}", peer_vaddr);
                            libnanami::print!(" bytes=");
                            libnanami::print!("{:#x}", size_bytes);
                            libnanami::print!("\n");
                        }
                        Err(e) => {
                            log_request_error("[input-server] subscriber queue attach failed: ", e);
                            return (map_request_error_to_status(e), 0, 0);
                        }
                    }
                }
                if state.subscribers[index].notification_descriptor == 0 {
                    match attach_subscriber_notification(index, request.identifier) {
                        Ok(descriptor) => {
                            state.subscribers[index].notification_descriptor = descriptor;
                            libnanami::print!("[input-server] subscriber notify attached pid=");
                            libnanami::print!("{}", request.identifier);
                            libnanami::print!(" slot=");
                            libnanami::print!(
                                "{}",
                                SLOT_SUBSCRIBER_NOTIFICATION_BASE + index as Word
                            );
                            libnanami::print!(" desc=");
                            libnanami::print!("{:#x}", descriptor);
                            libnanami::print!("\n");
                        }
                        Err(e) => {
                            log_request_error(
                                "[input-server] subscriber notify attach failed: ",
                                e,
                            );
                            return (map_request_error_to_status(e), 0, 0);
                        }
                    }
                }
                libnanami::print!("[input-server] subscribe-shared pid=");
                libnanami::print!("{}", request.identifier);
                libnanami::print!(" mask=");
                libnanami::print!("{:#x}", mask);
                libnanami::print!("\n");
                (
                    libnanami::OS_RESPONSE_OK,
                    state.subscribers[index].shared_queue_peer,
                    state.subscribers[index].shared_queue_bytes,
                )
            } else {
                (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0)
            }
        }
        nanami_services::input::INPUT_SERVICE_REQUEST_READ_EVENT => {
            if let Some(index) = find_subscriber(state, request.identifier) {
                if let Some(event) = state.subscribers[index].queue.pop() {
                    (libnanami::OS_RESPONSE_OK, 1, event)
                } else {
                    (libnanami::OS_RESPONSE_OK, 0, 0)
                }
            } else {
                (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0)
            }
        }
        nanami_services::input::INPUT_SERVICE_REQUEST_ATTACH_DRIVER => match request.arg0 {
            nanami_services::input::INPUT_DRIVER_KEYBOARD => {
                state.keyboard_driver_attached = true;
                state.keyboard_driver_pid = request.identifier;
                libnanami::print!("[input-server] keyboard driver attached pid=");
                libnanami::print!("{}", request.identifier);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, 0, 0)
            }
            nanami_services::input::INPUT_DRIVER_MOUSE => {
                state.mouse_driver_attached = true;
                state.mouse_driver_pid = request.identifier;
                libnanami::print!("[input-server] mouse driver attached pid=");
                libnanami::print!("{}", request.identifier);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, 0, 0)
            }
            _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        nanami_services::input::INPUT_SERVICE_REQUEST_ATTACH_DRIVER_SHARED => {
            if request.arg0 != nanami_services::input::INPUT_DRIVER_KEYBOARD
                && request.arg0 != nanami_services::input::INPUT_DRIVER_MOUSE
            {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            match find_or_attach_driver_queue(state, request.identifier) {
                Some(index) => {
                    libnanami::print!("[input-server] driver queue attached pid=");
                    libnanami::print!("{}", request.identifier);
                    libnanami::print!(" local=");
                    libnanami::print!("{:#x}", state.driver_queues[index].local_vaddr);
                    libnanami::print!(" peer=");
                    libnanami::print!("{:#x}", state.driver_queues[index].peer_vaddr);
                    libnanami::print!(" bytes=");
                    libnanami::print!("{:#x}", state.driver_queues[index].bytes);
                    libnanami::print!("\n");
                    (
                        libnanami::OS_RESPONSE_OK,
                        state.driver_queues[index].peer_vaddr,
                        state.driver_queues[index].bytes,
                    )
                }
                None => (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
            }
        }
        nanami_services::input::INPUT_SERVICE_REQUEST_PUBLISH_EVENT => {
            if !is_authorized_driver(request, state) {
                libnanami::print!("[input-server] publish denied id=");
                libnanami::print!("{}", request.identifier);
                libnanami::print!(" kind=");
                libnanami::print!("{}", request.arg0);
                libnanami::print!(" kpid=");
                libnanami::print!("{}", state.keyboard_driver_pid);
                libnanami::print!(" mpid=");
                libnanami::print!("{}", state.mouse_driver_pid);
                libnanami::print!("\n");
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }

            let event_kind = request.arg0;
            let code = request.arg1;
            let value0 = request.arg2 as u16 as i16;
            let value1 = request.arg3 as u16 as i16;
            let packed = nanami_services::input::pack_input_event(
                event_kind,
                code,
                value0,
                value1,
                state.sequence & 0xff,
            );
            state.sequence = state.sequence.wrapping_add(1);

            let delivered = distribute_event(state, event_kind, packed);
            state.published_count = state.published_count.wrapping_add(1);
            if state.published_count <= 16 || (state.published_count & 0xff) == 0 {
                libnanami::print!("[input-server] delivered kind=");
                libnanami::print!("{}", event_kind);
                libnanami::print!(" count=");
                libnanami::print!("{}", delivered);
                libnanami::print!("\n");
            }
            if delivered == 0 {
                libnanami::print!("[input-server] publish no-subscriber kind=");
                libnanami::print!("{}", event_kind);
                libnanami::print!(" code=");
                libnanami::print!("{:#x}", code);
                libnanami::print!("\n");
            }
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn is_authorized_driver(request: ServiceRequest, state: &InputState) -> bool {
    match request.arg0 {
        nanami_services::input::INPUT_EVENT_KIND_KEY => {
            state.keyboard_driver_attached && request.identifier == state.keyboard_driver_pid
        }
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON
        | nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE
        | nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL => {
            state.mouse_driver_attached && request.identifier == state.mouse_driver_pid
        }
        _ => false,
    }
}

fn is_authorized_event_from_pid(pid: Word, event_kind: Word, state: &InputState) -> bool {
    match event_kind {
        nanami_services::input::INPUT_EVENT_KIND_KEY => {
            state.keyboard_driver_attached && pid == state.keyboard_driver_pid
        }
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON
        | nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE
        | nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL => {
            state.mouse_driver_attached && pid == state.mouse_driver_pid
        }
        _ => false,
    }
}

fn find_subscriber(state: &InputState, pid: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_SUBSCRIBERS {
        if state.subscribers[i].used && state.subscribers[i].pid == pid {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_or_alloc_subscriber(state: &mut InputState, pid: Word) -> Option<usize> {
    if let Some(index) = find_subscriber(state, pid) {
        return Some(index);
    }

    let mut i = 0usize;
    while i < MAX_SUBSCRIBERS {
        if !state.subscribers[i].used {
            state.subscribers[i] = Subscriber {
                used: true,
                pid,
                event_mask: nanami_services::input::INPUT_SUBSCRIBE_ALL,
                notification_descriptor: 0,
                shared_queue_local: 0,
                shared_queue_peer: 0,
                shared_queue_bytes: 0,
                shared_mouse_since_notify: 0,
                queue: EventQueue::new(),
            };
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_or_attach_driver_queue(state: &mut InputState, pid: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_DRIVER_QUEUES {
        if state.driver_queues[i].used && state.driver_queues[i].pid == pid {
            return Some(i);
        }
        i += 1;
    }

    i = 0;
    while i < MAX_DRIVER_QUEUES {
        if !state.driver_queues[i].used {
            match attach_shared_event_queue(pid) {
                Ok((local_vaddr, peer_vaddr, size_bytes)) => {
                    init_shared_event_queue(local_vaddr);
                    state.driver_queues[i] = DriverQueue {
                        used: true,
                        pid,
                        local_vaddr,
                        peer_vaddr,
                        bytes: size_bytes,
                    };
                    return Some(i);
                }
                Err(e) => {
                    log_request_error("[input-server] driver queue attach failed: ", e);
                    return None;
                }
            }
        }
        i += 1;
    }
    None
}

fn attach_shared_event_queue(pid: Word) -> Result<(Word, Word, Word), RequestError> {
    let size = nanami_services::input::INPUT_EVENT_QUEUE_BYTES;
    let (local_vaddr, peer_vaddr) = libnanami::request_shared_memory(pid, size)?;
    Ok((local_vaddr, peer_vaddr, size))
}

fn init_shared_event_queue(base: Word) {
    write_shared_word(base, 0, nanami_services::input::INPUT_EVENT_QUEUE_MAGIC);
    write_shared_word(base, 1, SHARED_QUEUE_CAPACITY as Word);
    write_shared_word(base, 2, 0);
    write_shared_word(base, 3, 0);
    write_shared_word(base, 4, 0);
}

fn push_shared_event_with_kind(base: Word, _event_kind: Word, packed: Word) {
    let capacity = read_shared_word(base, 1) as usize;
    if capacity == 0 || capacity > SHARED_QUEUE_CAPACITY {
        return;
    }
    let head = (read_shared_word(base, 2) as usize) % capacity;
    let tail = (read_shared_word(base, 3) as usize) % capacity;
    // This queue is single-producer. Published slots must not be rewritten,
    // because consumers claim head independently with CAS.
    let next_tail = (tail + 1) % capacity;
    if next_tail == head {
        let dropped = read_shared_word(base, 4).wrapping_add(1);
        write_shared_word(base, 4, dropped);
        return;
    }
    write_shared_word(base, SHARED_QUEUE_HEADER_WORDS + tail, packed);
    write_shared_word(base, 3, next_tail as Word);
}

fn pop_shared_event(base: Word) -> Option<Word> {
    if read_shared_word(base, 0) != nanami_services::input::INPUT_EVENT_QUEUE_MAGIC {
        return None;
    }
    let capacity = read_shared_word(base, 1) as usize;
    if capacity == 0 || capacity > SHARED_QUEUE_CAPACITY {
        return None;
    }
    // SPMC-safe consumer path: only the consumer that wins the CAS owns
    // the event. Losing consumers retry without modifying the queue.
    loop {
        let head = (read_shared_word(base, 2) as usize) % capacity;
        let tail = (read_shared_word(base, 3) as usize) % capacity;
        if head == tail {
            return None;
        }
        let value = read_shared_word(base, SHARED_QUEUE_HEADER_WORDS + head);
        let next_head = (head + 1) % capacity;
        if compare_exchange_shared_word(base, 2, head as Word, next_head as Word) {
            return Some(value);
        }
    }
}

fn read_shared_word(base: Word, index: usize) -> Word {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr).load(Ordering::SeqCst) as Word
    }
}

fn write_shared_word(base: Word, index: usize, value: Word) {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr).store(value as usize, Ordering::SeqCst);
    }
}

fn compare_exchange_shared_word(base: Word, index: usize, current: Word, next: Word) -> bool {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr)
            .compare_exchange(
                current as usize,
                next as usize,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }
}

const fn word_offset(index: usize) -> Word {
    (index * core::mem::size_of::<Word>()) as Word
}

fn attach_subscriber_notification(index: usize, pid: Word) -> Result<Word, RequestError> {
    let destination_slot = SLOT_SUBSCRIBER_NOTIFICATION_BASE + index as Word;
    libnanami::request_notification_port_copy(
        pid,
        libnanami::PROCESS_SLOT_NOTIFICATION,
        destination_slot,
        INPUT_NOTIFICATION_IDENTIFIER,
    )?;
    Ok(libnanami::ipc::process_slot_descriptor(destination_slot))
}

fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

fn log_error(prefix: &str, err: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

libnanami::nanami_entry!(nanami_main);
