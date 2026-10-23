//! Production cooperative receiver; only OS calls and endpoint effects are mocked.
pub use nanami_uapi::os::*;
use nanami_uapi::service::*;
pub use nanami_uapi::{RequestError, Word};
use std::{cell::RefCell, collections::VecDeque};

#[derive(Default)]
struct Effects {
    replies: VecDeque<(Word, Word, Word)>,
    events: VecDeque<nanami_uapi::ipc::ServiceEvent>,
    os_calls: Vec<(Word, Word)>,
    public_receives: usize,
    public_replies: usize,
    waits: usize,
    reply_error: bool,
    polls: usize,
    poll_values: VecDeque<Word>,
    bindings: Vec<Word>,
    order: Vec<&'static str>,
}
thread_local! { static EFFECTS: RefCell<Effects> = RefCell::new(Effects::default()); }

fn call_os_port(
    code: Word,
    operation: Word,
    _: Word,
    _: Word,
    _: Word,
    _: u8,
) -> Result<(Word, Word, Word), RequestError> {
    EFFECTS.with(|effects| {
        let mut effects = effects.borrow_mut();
        effects.os_calls.push((code, operation));
        if code == OS_REQUEST_BINDING_CONTROL && operation == CONTROL_QUIESCED {
            effects.order.push("ack");
        }
        Ok(effects.replies.pop_front().expect("unexpected OS call"))
    })
}
fn pack_service_name_24(_: &str) -> Result<(Word, Word, Word), RequestError> {
    Err(RequestError::InvalidArgument)
}

mod ipc {
    use super::*;
    pub use nanami_uapi::ipc::*;
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn bind_current_thread_notification(slot: Word) -> Result<(), RequestError> {
        assert!([29, 30].contains(&slot));
        EFFECTS.with(|effects| {
            let mut effects = effects.borrow_mut();
            effects.bindings.push(slot);
            if slot == 29 {
                effects.order.push("private");
            } else {
                effects.order.push("event");
            }
        });
        Ok(())
    }
    pub fn notification_wait(slot: Word) -> Result<Word, RequestError> {
        assert!([29, 30].contains(&slot));
        EFFECTS.with(|effects| effects.borrow_mut().waits += 1);
        Ok(BINDING_NOTIFICATION)
    }
    pub fn notification_poll(slot: Word) -> Result<Word, RequestError> {
        assert!([29, 30].contains(&slot));
        EFFECTS.with(|effects| {
            let mut effects = effects.borrow_mut();
            effects.polls += 1;
            Ok(effects.poll_values.pop_front().unwrap_or(0))
        })
    }
    pub fn service_receive_event(port: Word) -> Result<ServiceEvent, RequestError> {
        assert_eq!(port, 28);
        EFFECTS.with(|effects| {
            let mut effects = effects.borrow_mut();
            effects.public_receives += 1;
            Ok(effects.events.pop_front().expect("unexpected receive"))
        })
    }
    pub fn service_reply(port: Word, _: Word, _: Word, _: Word) -> Result<(), RequestError> {
        assert_eq!(port, 28);
        EFFECTS.with(|effects| {
            let mut effects = effects.borrow_mut();
            effects.public_replies += 1;
            if effects.reply_error {
                return Err(RequestError::Protocol);
            }
            Ok(())
        })
    }
}

#[path = "../../nanami/servers/libs/nanami-runtime/src/service.rs"]
mod runtime;

fn request() -> ipc::ServiceEvent {
    ipc::ServiceEvent::Request(ipc::ServiceRequest {
        identifier: SESSION_IDENTIFIER_BIT | 1,
        code: 42,
        arg0: 0,
        arg1: 0,
        arg2: 0,
        arg3: 0,
    })
}

fn configure(replies: &[(Word, Word, Word)], events: &[ipc::ServiceEvent]) {
    EFFECTS.with(|effects| {
        *effects.borrow_mut() = Effects {
            replies: replies.iter().copied().collect(),
            events: events.iter().copied().collect(),
            ..Effects::default()
        }
    });
}

#[test]
fn ordinary_requests_are_direct_and_do_not_poll_alpha_on_every_receive() {
    configure(&[(0, 1, 0), (0, 10, 8)], &[request(), request()]);
    let mut receiver = runtime::Receiver::register(11, 28, 29).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            receiver.receive().unwrap(),
            runtime::ProviderEvent::Request(_)
        ));
        assert!(
            receiver.receive().is_err(),
            "accepted CALL lost its reply responsibility"
        );
        receiver.reply(0, 1, 2).unwrap();
    }
    EFFECTS.with(|effects| {
        let effects = effects.borrow();
        assert_eq!(effects.os_calls.len(), 2);
        assert_eq!(effects.public_receives, 2);
        assert_eq!(effects.public_replies, 2);
    });
}

#[test]
fn quiesce_is_acknowledged_only_after_reply_and_old_receiver_retires() {
    configure(
        &[
            (0, 1, 0),
            (0, 10, 8),
            (0, 10, 8 | 2),
            (0, 0, 0),
            (0, 10, 8 | 3),
            (OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        ],
        &[
            request(),
            ipc::ServiceEvent::Notification {
                identifier: BINDING_NOTIFICATION,
                value: 0,
            },
        ],
    );
    let mut receiver = runtime::Receiver::register(11, 28, 29).unwrap();
    assert!(matches!(
        receiver.receive().unwrap(),
        runtime::ProviderEvent::Request(_)
    ));
    assert!(receiver.receive().is_err());
    receiver.reply(0, 1, 2).unwrap();
    assert!(matches!(
        receiver.receive().unwrap(),
        runtime::ProviderEvent::Retired
    ));
    EFFECTS.with(|effects| {
        let effects = effects.borrow();
        assert_eq!(effects.waits, 1);
        assert_eq!(effects.public_replies, 1);
        assert_eq!(
            effects
                .os_calls
                .iter()
                .filter(|&&call| call == (OS_REQUEST_BINDING_CONTROL, CONTROL_QUIESCED))
                .count(),
            1
        );
    });
}

#[test]
fn uncertain_reply_does_not_replay_work_or_acknowledge_quiescence() {
    configure(&[(0, 1, 0), (0, 10, 8)], &[request()]);
    let mut receiver = runtime::Receiver::register(11, 28, 29).unwrap();
    receiver.receive().unwrap();
    EFFECTS.with(|effects| effects.borrow_mut().reply_error = true);
    assert!(receiver.reply(0, 1, 2).is_err());
    assert!(receiver.receive().is_err());
    EFFECTS.with(|effects| {
        let effects = effects.borrow();
        assert_eq!(effects.os_calls.len(), 2);
        assert_eq!(effects.public_receives, 1);
        assert_eq!(effects.public_replies, 1);
    });
}

#[test]
fn queued_call_stream_cannot_starve_quiescence_beyond_one_batch() {
    configure(
        &[
            (0, 1, 0),
            (0, 10, 8),
            (0, 10, 8 | 2),
            (0, 0, 0),
            (OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        ],
        &vec![request(); 32],
    );
    EFFECTS.with(|effects| {
        effects
            .borrow_mut()
            .poll_values
            .extend([0, BINDING_NOTIFICATION])
    });
    let mut receiver = runtime::Receiver::register(11, 28, 29).unwrap();
    for _ in 0..32 {
        assert!(matches!(
            receiver.receive().unwrap(),
            runtime::ProviderEvent::Request(_)
        ));
        receiver.reply(0, 1, 2).unwrap();
    }
    assert!(matches!(
        receiver.receive().unwrap(),
        runtime::ProviderEvent::Retired
    ));
    EFFECTS.with(|effects| {
        let effects = effects.borrow();
        assert_eq!(effects.public_receives, 32);
        assert_eq!(effects.public_replies, 32);
        assert_eq!(effects.polls, 2);
        assert_eq!(
            effects
                .os_calls
                .iter()
                .filter(|&&call| call == (OS_REQUEST_BINDING_CONTROL, CONTROL_QUIESCED))
                .count(),
            1
        );
    });
}

#[test]
fn event_work_is_direct_and_does_not_query_alpha_on_each_notification() {
    configure(
        &[(0, 1, 0), (0, 10, 8)],
        &[
            ipc::ServiceEvent::Notification {
                identifier: SERVICE_WORK_NOTIFICATION,
                value: 0,
            },
            ipc::ServiceEvent::Notification {
                identifier: SERVICE_WORK_NOTIFICATION,
                value: 0,
            },
        ],
    );
    let mut receiver = runtime::Receiver::register_buffered_events(11, 28, 29, 30).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            receiver.receive().unwrap(),
            runtime::ProviderEvent::Event(ipc::ServiceEvent::Notification {
                identifier: SERVICE_WORK_NOTIFICATION,
                ..
            })
        ));
    }
    EFFECTS.with(|effects| {
        let effects = effects.borrow();
        assert_eq!(effects.os_calls.len(), 2);
        assert_eq!(effects.bindings, [29, 30]);
        assert_eq!(effects.public_receives, 2);
    });
}

#[test]
fn event_alias_is_unbound_before_ack_and_rebound_after_abort() {
    configure(
        &[
            (0, 1, 0),
            (0, 10, 8),
            (0, 10, 8 | 2),
            (0, 0, 0),
            (0, 10, 8 | 3),
            (0, 10, 8),
        ],
        &[
            ipc::ServiceEvent::Notification {
                identifier: SERVICE_RESOURCE_NOTIFICATION,
                value: 0,
            },
            ipc::ServiceEvent::Notification {
                identifier: BINDING_NOTIFICATION | SERVICE_WORK_NOTIFICATION,
                value: 0,
            },
            ipc::ServiceEvent::Notification {
                identifier: SERVICE_RESOURCE_NOTIFICATION | SERVICE_WORK_NOTIFICATION,
                value: 0,
            },
        ],
    );
    let mut receiver = runtime::Receiver::register_buffered_events(11, 28, 29, 30).unwrap();
    assert!(matches!(
        receiver.receive().unwrap(),
        runtime::ProviderEvent::Event(_)
    ));
    assert!(
        matches!(receiver.receive().unwrap(), runtime::ProviderEvent::Event(
        ipc::ServiceEvent::Notification { identifier, .. }
    ) if identifier == SERVICE_RESOURCE_NOTIFICATION | SERVICE_WORK_NOTIFICATION)
    );
    EFFECTS.with(|effects| {
        let effects = effects.borrow();
        assert_eq!(effects.bindings, [29, 30, 29, 30]);
        assert_eq!(
            effects.order,
            ["private", "event", "private", "ack", "event"]
        );
        assert_eq!(effects.waits, 1);
    });
}
