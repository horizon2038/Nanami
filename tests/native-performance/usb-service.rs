//! Exercise the production loop: no blocking maintenance may delay a completed reply.
#![allow(dead_code)]
extern crate self as libnanami;
extern crate self as nanami_services;
pub use std::println;
use std::{cell::RefCell, collections::VecDeque};

macro_rules! io_trace {
    ($($arg:tt)*) => { let _ = format_args!($($arg)*); };
}

pub type Word = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    EndOfScript,
    Unavailable,
}

#[derive(Default)]
struct State {
    events: VecDeque<ipc::ServiceEvent>,
    pending_reply: bool,
    received: usize,
    replied: usize,
    polls: usize,
    acks: usize,
    discoveries: usize,
    connections: usize,
    flushes: usize,
    samples: usize,
}
thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

pub fn register_service_by_name(name: &str, slot: Word) -> Result<(), RequestError> {
    assert_eq!((name, slot), ("usb-service", 20));
    Ok(())
}
pub fn request_nanami_control(property: &str, value: &str) -> Result<(), RequestError> {
    assert_eq!((property, value), ("boot.pc", "sample"));
    STATE.with_borrow_mut(|s| {
        assert!(!s.pending_reply, "diagnostic RPC postponed the block reply");
        s.samples += 1;
    });
    Ok(())
}
pub mod usb {
    pub const USB_SERVICE_REQUEST_INFO: usize = 1;
}
pub mod timer {
    use super::*;
    pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: Word = 1 << 63;
    pub fn timer_service_interval_on_notification_milliseconds(
        timer: Word,
        delay: Word,
        notification: Word,
    ) -> Result<(), RequestError> {
        assert_eq!((timer, delay, notification), (24, 10, 21));
        Ok(())
    }
}
pub mod ipc {
    use super::*;
    pub struct ServiceRequest {
        pub code: Word,
    }
    pub enum ServiceEvent {
        Request(ServiceRequest),
        Notification { identifier: Word },
        Fault { reason: Word },
    }
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn interrupt_ack(_: Word) -> Result<(), RequestError> {
        STATE.with_borrow_mut(|s| {
            assert!(
                !s.pending_reply,
                "IRQ maintenance postponed the block reply"
            );
            s.acks += 1;
        });
        Ok(())
    }
    pub fn service_receive_event(port: Word) -> Result<ServiceEvent, RequestError> {
        assert_eq!(port, 20);
        STATE.with_borrow_mut(|s| {
            assert!(!s.pending_reply, "receive discarded an outstanding reply");
            let event = s.events.pop_front().ok_or(RequestError::EndOfScript)?;
            if matches!(event, ServiceEvent::Request(_)) {
                s.pending_reply = true;
                s.received += 1;
            }
            Ok(event)
        })
    }
    pub fn service_reply_receive_event(
        port: Word,
        status: Word,
        _: Word,
        _: Word,
    ) -> Result<ServiceEvent, RequestError> {
        assert_eq!(status, OS_RESPONSE_OK);
        STATE.with_borrow_mut(|s| {
            assert!(s.pending_reply);
            s.pending_reply = false;
            s.replied += 1;
        });
        service_receive_event(port)
    }
}
pub mod input {
    use super::*;
    pub struct Input;
    impl Input {
        pub fn connected(&self) -> bool {
            false
        }
        pub fn connect(&mut self) -> Result<(), RequestError> {
            STATE.with_borrow_mut(|s| {
                assert!(
                    !s.pending_reply,
                    "input-service RPC postponed the block reply"
                );
                s.connections += 1;
            });
            Err(RequestError::Unavailable)
        }
        pub fn flush(&mut self) {
            STATE.with_borrow_mut(|s| s.flushes += 1);
        }
    }
}
pub mod xhci {
    use super::*;
    pub struct Controller {
        pub irq: Option<Word>,
    }
    impl Controller {
        pub fn poll(&mut self, _: &mut input::Input) {
            STATE.with_borrow_mut(|s| {
                assert!(
                    !s.pending_reply,
                    "HID/hotplug work postponed the block reply"
                );
                s.polls += 1;
            });
        }
        pub fn counts(&self) -> (usize, usize) {
            (3, 2)
        }
    }
}
pub mod storage {
    use super::*;
    pub struct Storage;
    impl Storage {
        pub fn poll(
            &mut self,
            _: &mut [xhci::Controller],
            _: &mut input::Input,
            _: Word,
        ) -> Result<(), RequestError> {
            STATE.with_borrow_mut(|s| {
                assert!(!s.pending_reply, "root probe postponed the block reply");
                s.discoveries += 1;
            });
            Ok(())
        }
        pub fn request(
            &mut self,
            _: ipc::ServiceRequest,
            _: &mut [xhci::Controller],
            _: &mut input::Input,
        ) -> (Word, Word, Word) {
            (OS_RESPONSE_OK, 0x4000, 0)
        }
    }
}

#[path = "../../nanami/servers/core-services/usb-server/src/service.rs"]
mod service;

fn run(events: Vec<ipc::ServiceEvent>, irq: Option<Word>) {
    STATE.with_borrow_mut(|s| {
        *s = State {
            events: events.into(),
            ..State::default()
        }
    });
    assert_eq!(
        service::serve(
            &mut [xhci::Controller { irq }],
            &mut input::Input,
            &mut storage::Storage,
            24
        ),
        Err(RequestError::EndOfScript)
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.received, s.replied);
        assert!(!s.pending_reply);
    });
}

#[test]
fn completed_reads_are_replied_without_hid_or_timer_input() {
    run(
        (0..1000)
            .map(|_| ipc::ServiceEvent::Request(ipc::ServiceRequest { code: 2 }))
            .collect(),
        None,
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.replied, 1000);
        assert_eq!(s.polls, 1); // initial scan only; no notification required to reply
        assert_eq!(s.acks, 0);
    });
}

#[test]
fn deferred_notification_replies_first_then_runs_maintenance() {
    run(
        vec![
            ipc::ServiceEvent::Request(ipc::ServiceRequest { code: 2 }),
            ipc::ServiceEvent::Notification {
                identifier: 1 << 63,
            },
            ipc::ServiceEvent::Request(ipc::ServiceRequest { code: 1 }),
        ],
        Some(32),
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.replied, 2);
        assert_eq!(s.acks, 2);
        assert_eq!(s.polls, 4); // close the IRQ-masked window on each maintenance pass
        assert_eq!(s.connections, 2);
    });
}

#[test]
fn polling_only_maintenance_keeps_discovery_and_input_connection_alive() {
    run(
        (0..3)
            .map(|_| ipc::ServiceEvent::Notification {
                identifier: 1 << 63,
            })
            .collect(),
        None,
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.polls, 4);
        assert_eq!(s.discoveries, 4);
        assert_eq!(s.connections, 4);
    });
}

#[test]
fn diagnostic_samples_are_timer_driven_and_never_delay_a_reply() {
    for (ticks, samples) in [(499, 0), (500, 1), (1000, 2)] {
        let mut events = vec![ipc::ServiceEvent::Request(ipc::ServiceRequest { code: 2 })];
        events.extend((0..ticks).flat_map(|_| {
            [
                ipc::ServiceEvent::Notification {
                    identifier: 1 << 16,
                },
                ipc::ServiceEvent::Notification {
                    identifier: timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
                },
            ]
        }));
        run(events, Some(32));
        STATE.with_borrow(|s| {
            assert_eq!(
                s.samples,
                samples * usize::from(option_env!("NANAMI_BOOT_PC_TRACE") == Some("1"))
            )
        });
    }
}
