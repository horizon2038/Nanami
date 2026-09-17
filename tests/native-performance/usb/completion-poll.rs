use super::poll::CompletionPoll;

#[test]
fn fast_poll_covers_one_usb_frame_then_stays_expired() {
    let mut poll = CompletionPoll::new(1234);
    for microframe in 1234..1242 {
        assert!(poll.keep_polling(microframe));
    }
    assert!(!poll.keep_polling(1242));
    // A slow device may take longer than MFINDEX's two-second wrap period.
    assert!(!poll.keep_polling(1234));
}

#[test]
fn fast_poll_handles_microframe_counter_wrap() {
    let mut poll = CompletionPoll::new(0x3ffc);
    assert!(poll.keep_polling(0x3fff));
    assert!(poll.keep_polling(0));
    assert!(poll.keep_polling(3));
    assert!(!poll.keep_polling(4));
}

#[test]
fn stopped_controller_cannot_spin_forever() {
    let mut poll = CompletionPoll::new(0);
    for _ in 1..256 {
        assert!(poll.keep_polling(0));
    }
    assert!(!poll.keep_polling(0));
    assert!(!poll.keep_polling(1));
}
