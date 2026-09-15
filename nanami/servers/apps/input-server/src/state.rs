use super::*;

#[derive(Clone, Copy)]
pub(super) struct EventQueue {
    pub(super) values: [Word; EVENT_QUEUE_CAPACITY],
    pub(super) head: usize,
    pub(super) tail: usize,
    pub(super) count: usize,
}

impl EventQueue {
    pub(super) const fn new() -> Self {
        Self {
            values: [0; EVENT_QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    pub(super) fn push(&mut self, value: Word) {
        if self.count == EVENT_QUEUE_CAPACITY {
            self.head = (self.head + 1) % EVENT_QUEUE_CAPACITY;
            self.count -= 1;
        }
        self.values[self.tail] = value;
        self.tail = (self.tail + 1) % EVENT_QUEUE_CAPACITY;
        self.count += 1;
    }

    pub(super) fn push_with_event_kind(&mut self, event_kind: Word, value: Word) {
        if event_kind == nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE && self.count > 0 {
            let last_index = if self.tail == 0 {
                EVENT_QUEUE_CAPACITY - 1
            } else {
                self.tail - 1
            };
            let last_kind = self.values[last_index] & 0xff;
            if last_kind == nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE {
                // Coalesce mouse move events to keep latest pointer position delta.
                self.values[last_index] = value;
                return;
            }
        }
        self.push(value);
    }

    pub(super) fn pop(&mut self) -> Option<Word> {
        if self.count == 0 {
            return None;
        }
        let value = self.values[self.head];
        self.head = (self.head + 1) % EVENT_QUEUE_CAPACITY;
        self.count -= 1;
        Some(value)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Subscriber {
    pub(super) used: bool,
    pub(super) pid: Word,
    pub(super) event_mask: Word,
    pub(super) notification_descriptor: Word,
    pub(super) shared_queue_local: Word,
    pub(super) shared_queue_peer: Word,
    pub(super) shared_queue_bytes: Word,
    pub(super) shared_mouse_since_notify: usize,
    pub(super) queue: EventQueue,
}

impl Subscriber {
    pub(super) const EMPTY: Self = Self {
        used: false,
        pid: 0,
        event_mask: 0,
        notification_descriptor: 0,
        shared_queue_local: 0,
        shared_queue_peer: 0,
        shared_queue_bytes: 0,
        shared_mouse_since_notify: 0,
        queue: EventQueue::new(),
    };
}

#[derive(Clone, Copy)]
pub(super) struct DriverQueue {
    pub(super) used: bool,
    pub(super) pid: Word,
    pub(super) local_vaddr: Word,
    pub(super) peer_vaddr: Word,
    pub(super) bytes: Word,
}

impl DriverQueue {
    pub(super) const EMPTY: Self = Self {
        used: false,
        pid: 0,
        local_vaddr: 0,
        peer_vaddr: 0,
        bytes: 0,
    };
}

pub(super) struct InputState {
    pub(super) subscribers: [Subscriber; MAX_SUBSCRIBERS],
    pub(super) driver_queues: [DriverQueue; MAX_DRIVER_QUEUES],
    pub(super) keyboard_driver_attached: bool,
    pub(super) keyboard_driver_pid: Word,
    pub(super) mouse_driver_attached: bool,
    pub(super) mouse_driver_pid: Word,
    pub(super) sequence: Word,
    pub(super) published_count: usize,
    pub(super) delivered_count: usize,
}

impl InputState {
    pub(super) const fn new() -> Self {
        Self {
            subscribers: [Subscriber::EMPTY; MAX_SUBSCRIBERS],
            driver_queues: [DriverQueue::EMPTY; MAX_DRIVER_QUEUES],
            keyboard_driver_attached: false,
            keyboard_driver_pid: 0,
            mouse_driver_attached: false,
            mouse_driver_pid: 0,
            sequence: 0,
            published_count: 0,
            delivered_count: 0,
        }
    }
}
