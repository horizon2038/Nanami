//! Exercise the production loop: no blocking maintenance may delay a completed reply.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::capture_log(format_args!($($arg)*)); };
}
#[path = "../../nanami/servers/libs/nanami-runtime/src/diagnostics.rs"]
pub mod diagnostics;
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
    startup: Vec<String>,
    register_fails: bool,
    interval_fails: bool,
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

fn capture_log(args: std::fmt::Arguments<'_>) {
    let line = args.to_string();
    if line.starts_with("[usb.init] ") {
        STATE.with_borrow_mut(|s| s.startup.push(line));
    } else {
        std::println!("{line}");
    }
}

pub fn register_service_by_name(name: &str, slot: Word) -> Result<(), RequestError> {
    assert_eq!((name, slot), ("usb-service", 20));
    STATE.with_borrow_mut(|s| {
        s.startup.push("rpc:register".into());
        if s.register_fails {
            Err(RequestError::Unavailable)
        } else {
            Ok(())
        }
    })
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
pub mod block {
    pub const BLOCK_DEVICE_REQUEST_READ: usize = 2;
    pub const BLOCK_DEVICE_REQUEST_WRITE: usize = 3;
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
        STATE.with_borrow_mut(|s| {
            s.startup.push("rpc:interval".into());
            if s.interval_fails {
                Err(RequestError::Unavailable)
            } else {
                Ok(())
            }
        })
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
        pub fn start_maintenance(&mut self) {
            STATE.with_borrow_mut(|s| s.startup.push("maintenance-start".into()));
        }
        pub fn print_wait_profile(&mut self) {}
        pub fn profile_clock(&self) -> Option<Word> {
            None
        }
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
                if s.discoveries == 0 {
                    s.startup.push("root-probe".into());
                }
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

fn expected_startup(register_ok: bool, interval_ok: Option<bool>) -> Vec<String> {
    let trace = option_env!("NANAMI_USB_INIT_TRACE") == Some("1");
    let mut expected = Vec::new();
    if trace {
        expected.push("[usb.init] stage=service-register.begin service=usb-service slot=20".into());
    }
    expected.push("rpc:register".into());
    if trace {
        expected.push(format!(
            "[usb.init] stage=service-register.end ok={register_ok}"
        ));
    }
    if let Some(ok) = interval_ok {
        if trace {
            expected.push("[usb.init] stage=timer-interval.begin ms=10 notification=21".into());
        }
        expected.push("rpc:interval".into());
        if trace {
            expected.push(format!("[usb.init] stage=timer-interval.end ok={ok}"));
        }
        if ok {
            expected.push("maintenance-start".into());
            expected.push("root-probe".into());
        }
    }
    expected
}

#[test]
fn startup_records_bracket_each_rpc_once_before_maintenance_and_root_probe() {
    // Multiple requests and maintenance ticks must not repeat startup records.
    run(
        vec![
            ipc::ServiceEvent::Notification {
                identifier: timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
            },
            ipc::ServiceEvent::Request(ipc::ServiceRequest { code: 2 }),
            ipc::ServiceEvent::Notification {
                identifier: timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
            },
        ],
        None,
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.startup, expected_startup(true, Some(true)));
        assert_eq!(s.discoveries, 3);
    });
}

#[test]
fn register_error_is_logged_then_returned_without_timer_or_maintenance() {
    STATE.with_borrow_mut(|s| {
        *s = State {
            register_fails: true,
            ..State::default()
        };
    });
    assert_eq!(
        service::serve(
            &mut [xhci::Controller { irq: Some(32) }],
            &mut input::Input,
            &mut storage::Storage,
            24
        ),
        Err(RequestError::Unavailable)
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.startup, expected_startup(false, None));
        assert_eq!((s.polls, s.acks, s.discoveries, s.received), (0, 0, 0, 0));
    });
}

#[test]
fn interval_error_is_logged_then_returned_without_maintenance_or_root_probe() {
    STATE.with_borrow_mut(|s| {
        *s = State {
            interval_fails: true,
            ..State::default()
        };
    });
    assert_eq!(
        service::serve(
            &mut [xhci::Controller { irq: Some(32) }],
            &mut input::Input,
            &mut storage::Storage,
            24
        ),
        Err(RequestError::Unavailable)
    );
    STATE.with_borrow(|s| {
        assert_eq!(s.startup, expected_startup(true, Some(false)));
        assert_eq!((s.polls, s.acks, s.discoveries, s.received), (0, 0, 0, 0));
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
