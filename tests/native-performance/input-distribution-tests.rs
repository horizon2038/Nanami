#![allow(dead_code)]
extern crate self as libnanami;
extern crate self as nanami_services;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
};
pub type Word = usize;
const MAX_SUBSCRIBERS: usize = 32;
const MAX_DRIVER_QUEUES: usize = 4;
const EVENT_QUEUE_CAPACITY: usize = 64;
pub mod input {
    pub const INPUT_DRIVER_KEYBOARD: usize = 1;
    pub const INPUT_DRIVER_MOUSE: usize = 2;
    pub const INPUT_EVENT_KIND_KEY: usize = 1;
    pub const INPUT_EVENT_KIND_MOUSE_MOVE: usize = 2;
    pub const INPUT_EVENT_KIND_MOUSE_BUTTON: usize = 3;
    pub const INPUT_EVENT_KIND_MOUSE_WHEEL: usize = 4;
    pub const INPUT_SUBSCRIBE_KEYBOARD: usize = 1;
    pub const INPUT_SUBSCRIBE_MOUSE: usize = 2;
    pub const INPUT_SUBSCRIBE_ALL: usize = 3;
}
use input::*;
#[derive(Default)]
struct Fake {
    drivers: BTreeMap<Word, VecDeque<Word>>,
    queues: BTreeMap<Word, Vec<Word>>,
    notifications: Vec<Word>,
    consume_during_batch: bool,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
pub mod ipc {
    use super::*;
    pub fn notification_notify(descriptor: Word) -> Result<(), ()> {
        FAKE.with(|fake| fake.borrow_mut().notifications.push(descriptor));
        Ok(())
    }
}
fn pop_shared_event(queue: Word) -> Option<Word> {
    FAKE.with(|fake| {
        fake.borrow_mut()
            .drivers
            .entry(queue)
            .or_default()
            .pop_front()
    })
}
fn push_shared_event_with_kind(queue: Word, _kind: Word, packed: Word) {
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        fake.queues.entry(queue).or_default().push(packed);
        if fake.consume_during_batch {
            fake.queues.get_mut(&queue).unwrap().clear();
        }
    });
}
#[path = "../../nanami/servers/apps/input-server/src/state.rs"]
mod state;
use state::*;
#[path = "../../nanami/servers/apps/input-server/src/distribution.rs"]
mod distribution;
use distribution::*;
#[path = "../../nanami/servers/apps/input-server/src/drivers.rs"]
mod drivers;
use drivers::*;

fn setup() -> InputState {
    FAKE.with(|fake| *fake.borrow_mut() = Fake::default());
    let mut state = InputState::new();
    state.driver_queues[0] = DriverQueue {
        used: true,
        pid: 11,
        event_mask: INPUT_SUBSCRIBE_KEYBOARD,
        local_vaddr: 1,
        peer_vaddr: 0,
        bytes: 4096,
    };
    state.driver_queues[1] = DriverQueue {
        used: true,
        pid: 22,
        event_mask: INPUT_SUBSCRIBE_MOUSE,
        local_vaddr: 2,
        peer_vaddr: 0,
        bytes: 4096,
    };
    for (i, mask) in [
        (0, INPUT_SUBSCRIBE_ALL),
        (1, INPUT_SUBSCRIBE_KEYBOARD),
        (31, INPUT_SUBSCRIBE_MOUSE),
    ] {
        state.subscribers[i] = Subscriber {
            used: true,
            pid: i + 100,
            event_mask: mask,
            shared_queue_local: i + 100,
            notification_descriptor: i + 200,
            ..Subscriber::EMPTY
        };
    }
    state
}
fn driver(queue: Word, events: &[Word]) {
    FAKE.with(|fake| {
        fake.borrow_mut()
            .drivers
            .entry(queue)
            .or_default()
            .extend(events)
    });
}

#[test]
fn ps2_registration_does_not_replace_usb_input() {
    let mut state = InputState::new();
    assert_eq!(
        register_driver(&mut state, 7, INPUT_DRIVER_KEYBOARD),
        Some(0)
    );
    assert_eq!(register_driver(&mut state, 7, INPUT_DRIVER_MOUSE), Some(0));
    assert_eq!(
        register_driver(&mut state, 9, INPUT_DRIVER_KEYBOARD),
        Some(1)
    );
    assert_eq!(register_driver(&mut state, 9, INPUT_DRIVER_MOUSE), Some(1));
    for pid in [7, 9] {
        for kind in [
            INPUT_EVENT_KIND_KEY,
            INPUT_EVENT_KIND_MOUSE_MOVE,
            INPUT_EVENT_KIND_MOUSE_BUTTON,
        ] {
            assert!(is_authorized_event_from_pid(pid, kind, &state));
        }
        assert!(!is_authorized_event_from_pid(pid, 255, &state));
    }
    assert!(!is_authorized_event_from_pid(
        99,
        INPUT_EVENT_KIND_KEY,
        &state
    ));
}

#[test]
fn registrations_are_bounded_idempotent_and_kind_specific() {
    let mut state = InputState::new();
    assert_eq!(register_driver(&mut state, 0, INPUT_DRIVER_KEYBOARD), None);
    assert_eq!(register_driver(&mut state, 7, 99), None);
    for index in 0..MAX_DRIVER_QUEUES {
        assert_eq!(
            register_driver(&mut state, index + 1, INPUT_DRIVER_KEYBOARD),
            Some(index)
        );
    }
    assert_eq!(register_driver(&mut state, 99, INPUT_DRIVER_MOUSE), None);
    assert_eq!(register_driver(&mut state, 1, INPUT_DRIVER_MOUSE), Some(0));
    assert!(is_authorized_event_from_pid(
        1,
        INPUT_EVENT_KIND_MOUSE_BUTTON,
        &state
    ));
    assert!(!is_authorized_event_from_pid(
        2,
        INPUT_EVENT_KIND_MOUSE_BUTTON,
        &state
    ));
    // Registering before attaching shared memory must not dereference address 0.
    assert_eq!(drain_driver_queues(&mut state), 0);
}

#[test]
fn burst_keeps_event_order_and_masks_but_notifies_each_subscriber_once() {
    let mut state = setup();
    let keys = [0x101, 0x201, 0x301];
    let mouse = [0x102, 0x103];
    driver(1, &keys);
    driver(2, &mouse);
    assert_eq!(drain_driver_queues(&mut state), 10);
    assert_eq!(state.published_count, 5);
    assert_eq!(state.delivered_count, 10);
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.queues[&100], [0x101, 0x201, 0x301, 0x102, 0x103]);
        assert_eq!(fake.queues[&101], keys);
        assert_eq!(fake.queues[&131], mouse);
        assert_eq!(fake.notifications, [200, 201, 231]);
    });
}

#[test]
fn concurrent_consumer_drain_does_not_suppress_final_wakeup() {
    let mut state = setup();
    FAKE.with(|fake| fake.borrow_mut().consume_during_batch = true);
    driver(1, &[0x101, 0x201, 0x301]);
    drain_driver_queues(&mut state);
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(fake.queues[&100].is_empty());
        assert_eq!(fake.notifications, [200, 201]);
    });
}

#[test]
fn unauthorized_and_empty_batches_do_not_notify() {
    let mut state = setup();
    driver(1, &[0x102]); // Mouse event from keyboard driver.
    driver(2, &[0x101]);
    assert_eq!(drain_driver_queues(&mut state), 0);
    assert_eq!(drain_driver_queues(&mut state), 0);
    FAKE.with(|fake| assert!(fake.borrow().notifications.is_empty()));
}

#[test]
fn per_driver_budget_is_unchanged_and_remaining_events_get_another_wakeup() {
    let mut state = setup();
    driver(1, &[0x101; 513]);
    assert_eq!(drain_driver_queues(&mut state), 1024);
    assert_eq!(drain_driver_queues(&mut state), 2);
    FAKE.with(|fake| assert_eq!(fake.borrow().notifications, [200, 201, 200, 201]));
}

#[test]
fn single_request_and_local_queue_delivery_still_notify_immediately() {
    let mut state = setup();
    state.subscribers[0].shared_queue_local = 0;
    assert_eq!(distribute_event(&mut state, INPUT_EVENT_KIND_KEY, 0x101), 2);
    assert_eq!(state.subscribers[0].queue.pop(), Some(0x101));
    FAKE.with(|fake| assert_eq!(fake.borrow().notifications, [200, 201]));
}
