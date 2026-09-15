use crate::{mock::*, ports::*, service::*, types::*, *};
use a9n_types::MessageInfo;

fn assert_notification(event: ServiceEvent, expected: Word) {
    assert!(
        matches!(event, ServiceEvent::Notification { identifier, value: 0 } if identifier == expected),
        "{event:?}"
    );
}

#[test]
fn interrupted_call_irq_reaches_service_receive_without_waiting_for_another_irq() {
    let _guard = setup();
    // call_port has received this notification instead of a reply, saved it,
    // and retried the RPC. The IRQ remains masked until the event loop sees it.
    preserve_interrupted_notification(1);
    assert_notification(service_receive_event(SERVICE).unwrap(), 1);
    FAKE.with(|fake| assert!(fake.borrow().operations.is_empty()));
    assert_eq!(notification_poll(BOUND).unwrap(), 0);
}

#[test]
fn deferred_irq_reply_is_sent_before_returning_notification_without_blocking() {
    let _guard = setup();
    preserve_interrupted_notification(1);
    assert_notification(service_reply_receive_event(SERVICE, 0, 42, 99).unwrap(), 1);
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.operations, ["reply"]);
        assert_eq!(fake.replies, [vec![0, 42, 99]]);
    });
    assert_eq!(notification_poll(BOUND).unwrap(), 0);
}

#[test]
fn ordinary_reply_receive_keeps_combined_syscall() {
    let _guard = setup();
    incoming(MessageInfo::normal(true, 0, 0), 73);
    assert!(matches!(
        service_reply_receive_event(SERVICE, 0, 42, 99).unwrap(),
        ServiceEvent::Request(ServiceRequest { identifier: 73, .. })
    ));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.operations, ["reply_receive", "receive part"]);
        assert_eq!(fake.replies, [vec![0, 42, 99]]);
    });
}

#[test]
fn reply_failure_does_not_lose_deferred_notification() {
    let _guard = setup();
    preserve_interrupted_notification(1);
    FAKE.with(|fake| fake.borrow_mut().reply_error = Some(CapabilityError::Other));
    assert!(matches!(
        service_reply_receive_event(SERVICE, 0, 42, 99),
        Err(RequestError::Transport)
    ));
    assert_notification(service_receive_event(SERVICE).unwrap(), 1);
}

#[test]
fn deferred_irq_does_not_change_fault_context_reply() {
    let _guard = setup();
    let registers: Vec<Word> = (0..HARDWARE_CONTEXT_WORDS).map(|i| i + 100).collect();
    preserve_interrupted_notification(4);
    assert_notification(
        service_fault_continue_receive_event(SERVICE, &registers).unwrap(),
        4,
    );
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.operations, ["reply"]);
        assert_eq!(fake.replies, [registers]);
    });
}

#[test]
fn deferred_flags_are_coalesced_and_only_consumed_by_the_bound_port() {
    let _guard = setup();
    preserve_interrupted_notification(1);
    preserve_interrupted_notification(4);
    preserve_interrupted_notification(1);
    FAKE.with(|fake| fake.borrow_mut().notification = 8);
    assert_eq!(notification_wait(26).unwrap(), 8);
    assert_notification(service_receive_event(SERVICE).unwrap(), 5);
    assert_eq!(notification_wait(BOUND).unwrap(), 0);
}

#[test]
fn notification_wait_and_poll_still_consume_deferred_flags_once() {
    let _guard = setup();
    preserve_interrupted_notification(1);
    assert_eq!(notification_wait(BOUND).unwrap(), 1);
    preserve_interrupted_notification(4);
    assert_eq!(notification_poll(BOUND).unwrap(), 4);
    assert_eq!(notification_poll(BOUND).unwrap(), 0);
}

#[test]
fn deferred_notification_does_not_consume_the_next_kernel_message() {
    let _guard = setup();
    preserve_interrupted_notification(2);
    incoming(MessageInfo::normal(true, 0, 0), 73);
    assert_notification(service_receive_event(SERVICE).unwrap(), 2);
    assert!(matches!(
        service_receive_event(SERVICE).unwrap(),
        ServiceEvent::Request(ServiceRequest { identifier: 73, .. })
    ));
    FAKE.with(|fake| assert_eq!(fake.borrow().operations, ["receive"]));
}

#[test]
fn ordinary_fault_reply_receive_keeps_combined_syscall_and_registers() {
    let _guard = setup();
    let registers: Vec<Word> = (0..HARDWARE_CONTEXT_WORDS).map(|i| i + 100).collect();
    incoming(MessageInfo::normal(true, 0, 0), 73);
    assert!(matches!(
        service_fault_continue_receive_event(SERVICE, &registers).unwrap(),
        ServiceEvent::Request(ServiceRequest { identifier: 73, .. })
    ));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.operations, ["reply_receive", "receive part"]);
        assert_eq!(fake.replies, [registers]);
    });
}

#[test]
fn invalid_context_does_not_consume_a_deferred_notification() {
    let _guard = setup();
    preserve_interrupted_notification(4);
    assert!(matches!(
        service_fault_continue_receive_event(SERVICE, &[0; HARDWARE_CONTEXT_WORDS + 1]),
        Err(RequestError::InvalidArgument)
    ));
    assert_notification(service_receive_event(SERVICE).unwrap(), 4);
    FAKE.with(|fake| assert!(fake.borrow().operations.is_empty()));
}
