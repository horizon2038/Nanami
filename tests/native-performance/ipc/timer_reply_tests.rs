use crate::{init_trace::Trace, mock::*, ports::*, timer_reply::receive_after_reply, types::*, *};
use a9n_types::MessageInfo;

const TRACED: bool = match option_env!("NANAMI_USB_INIT_TRACE") {
    Some(value) => matches!(value.as_bytes(), [b'1']),
    None => false,
};
const SPLIT: bool = TRACED
    && match option_env!("NANAMI_TIMER_SPLIT_REPLY") {
        Some(value) => matches!(value.as_bytes(), [b'1']),
        None => false,
    };

fn selected_trace() -> Trace {
    let mut trace = Trace::new();
    trace.begin(7, PROCESS_SLOT_NOTIFICATION, 10);
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        if TRACED {
            assert_eq!(
                fake.logs,
                ["[timer.init] stage=interval.receive pid=7 notification=21 ms=10"]
            );
        } else {
            assert!(fake.logs.is_empty());
        }
        fake.operations.clear();
        fake.logs.clear();
    });
    trace
}

fn assert_request(event: ServiceEvent, id: Word) {
    assert!(
        matches!(event, ServiceEvent::Request(ServiceRequest { identifier, .. }) if identifier == id)
    );
}

#[test]
fn first_matching_reply_uses_the_selected_path_and_logs_after_reply_before_receive() {
    let _guard = setup();
    let mut trace = selected_trace();
    incoming(MessageInfo::normal(true, 0, 0), 73);
    assert_request(
        receive_after_reply(&mut trace, SERVICE, 0, 42, 99).unwrap(),
        73,
    );
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.replies, [vec![0, 42, 99]]);
        if SPLIT {
            assert_eq!(fake.operations, ["log", "reply", "log", "log", "receive"]);
            assert_eq!(
                fake.logs,
                [
                    "[timer.init] stage=reply-enter status=0x0 path=split",
                    "[timer.init] stage=reply-split.returned ok=true",
                    "[timer.init] stage=receive-enter path=split",
                ]
            );
        } else if TRACED {
            assert_eq!(fake.operations, ["log", "reply_receive", "receive part"]);
            assert_eq!(fake.logs, ["[timer.init] stage=reply-enter status=0x0"]);
        } else {
            assert_eq!(fake.operations, ["reply_receive", "receive part"]);
            assert!(fake.logs.is_empty());
        }
    });
}

#[test]
fn private_delays_and_other_intervals_never_select_the_split_reply() {
    let _guard = setup();
    let mut trace = Trace::new();
    for (slot, delay) in [(26, 10), (PROCESS_SLOT_NOTIFICATION, 2), (0, 0)] {
        trace.begin(7, slot, delay);
        incoming(MessageInfo::normal(true, 0, 0), 73);
        assert_request(
            receive_after_reply(&mut trace, SERVICE, 0, 42, 99).unwrap(),
            73,
        );
    }
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(
            fake.operations,
            [
                "reply_receive",
                "receive part",
                "reply_receive",
                "receive part",
                "reply_receive",
                "receive part",
            ]
        );
        assert!(fake.logs.is_empty());
    });
    // Nonmatching calls did not consume the one-shot selector.
    trace.begin(8, PROCESS_SLOT_NOTIFICATION, 10);
    assert_eq!(trace.reply_enter(0), SPLIT);
}

#[test]
fn no_interval_trace_keeps_the_existing_combined_path() {
    let _guard = setup();
    let mut trace = Trace::new();
    incoming(MessageInfo::normal(true, 0, 0), 73);
    assert_request(
        receive_after_reply(&mut trace, SERVICE, 0, 42, 99).unwrap(),
        73,
    );
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.operations, ["reply_receive", "receive part"]);
        assert!(fake.logs.is_empty());
    });
}

#[test]
fn second_matching_interval_and_later_replies_keep_the_combined_path() {
    let _guard = setup();
    let mut trace = selected_trace();
    incoming(MessageInfo::normal(true, 0, 0), 73);
    receive_after_reply(&mut trace, SERVICE, 0, 42, 99).unwrap();
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        fake.operations.clear();
        fake.logs.clear();
    });
    trace.begin(9, PROCESS_SLOT_NOTIFICATION, 10);
    incoming(MessageInfo::normal(true, 0, 0), 74);
    assert_request(
        receive_after_reply(&mut trace, SERVICE, 0, 43, 98).unwrap(),
        74,
    );
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.operations, ["reply_receive", "receive part"]);
        assert!(fake.logs.is_empty());
        assert_eq!(fake.replies, [vec![0, 42, 99], vec![0, 43, 98]]);
    });
}

#[test]
fn failed_reply_never_receives_or_retries_and_consumes_the_selector() {
    let _guard = setup();
    let mut trace = selected_trace();
    FAKE.with(|fake| fake.borrow_mut().reply_error = Some(CapabilityError::Other));
    assert!(matches!(
        receive_after_reply(&mut trace, SERVICE, 0, 42, 99),
        Err(RequestError::Transport)
    ));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(fake.replies.is_empty());
        assert!(!fake.operations.iter().any(|op| op.starts_with("receive")));
        if SPLIT {
            assert_eq!(fake.operations, ["log", "reply", "log"]);
            assert_eq!(
                fake.logs.last().unwrap(),
                "[timer.init] stage=reply-split.returned ok=false"
            );
        }
    });
    trace.begin(8, PROCESS_SLOT_NOTIFICATION, 10);
    assert!(!trace.reply_enter(0));
}

#[test]
fn receive_failure_after_reply_is_propagated_without_a_second_reply() {
    let _guard = setup();
    let mut trace = selected_trace();
    FAKE.with(|fake| fake.borrow_mut().receive_error = Some(CapabilityError::Other));
    assert!(matches!(
        receive_after_reply(&mut trace, SERVICE, 0, 42, 99),
        Err(RequestError::Transport)
    ));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.replies, [vec![0, 42, 99]]);
        assert_eq!(
            fake.operations
                .iter()
                .filter(|&&op| op == "reply" || op == "reply_receive")
                .count(),
            1
        );
        if SPLIT {
            assert_eq!(fake.operations, ["log", "reply", "log", "log", "receive"]);
        }
    });
    assert!(!trace.reply_enter(0));
}

#[test]
fn selected_reply_delivers_deferred_irq_without_consuming_the_next_request() {
    let _guard = setup();
    let mut trace = selected_trace();
    preserve_interrupted_notification(4);
    incoming(MessageInfo::normal(true, 0, 0), 73);
    let event = receive_after_reply(&mut trace, SERVICE, 0, 42, 99).unwrap();
    assert!(matches!(
        event,
        ServiceEvent::Notification {
            identifier: 4,
            value: 0
        }
    ));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.replies, [vec![0, 42, 99]]);
        assert_eq!(fake.incoming.len(), 1);
        assert!(!fake.operations.iter().any(|op| op.starts_with("receive")));
        if SPLIT {
            assert_eq!(fake.operations, ["log", "reply", "log", "log"]);
        }
    });
    assert_request(service::service_receive_event(SERVICE).unwrap(), 73);
    assert_eq!(notification_poll(BOUND).unwrap(), 0);
}

#[test]
fn caller_status_and_detail_words_are_unchanged_even_for_a_failed_registration() {
    let _guard = setup();
    let mut trace = selected_trace();
    incoming(MessageInfo::normal(true, 0, 0), 73);
    assert_request(
        receive_after_reply(&mut trace, SERVICE, 3, 17, 29).unwrap(),
        73,
    );
    FAKE.with(|fake| assert_eq!(fake.borrow().replies, [vec![3, 17, 29]]));
}
