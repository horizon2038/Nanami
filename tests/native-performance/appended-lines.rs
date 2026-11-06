#[path = "../negoto/guest/src/appended_lines.rs"]
mod appended_lines;
use appended_lines::complete_appended_lines as appended;

#[test]
fn emits_every_new_complete_line_in_a_single_snapshot() {
    assert_eq!(
        appended(b"ready\n", b"ready\nRTT\nclock\nstats\n"),
        b"RTT\nclock\nstats\n"
    );
}

#[test]
fn unchanged_snapshot_is_not_replayed() {
    assert_eq!(appended(b"ready\npartial", b"ready\npartial"), b"");
}

#[test]
fn partial_line_is_forwarded_only_after_its_newline_arrives() {
    assert_eq!(appended(b"ready\n", b"ready\npar"), b"");
    assert_eq!(
        appended(b"ready\npar", b"ready\npartial\nnext"),
        b"partial\n"
    );
    assert_eq!(
        appended(b"ready\npartial\nnext", b"ready\npartial\nnext\n"),
        b"next\n"
    );
}

#[test]
fn initially_unterminated_log_is_not_lost() {
    assert_eq!(appended(b"", b"init"), b"");
    assert_eq!(appended(b"init", b"initial\n"), b"initial\n");
}

#[test]
fn truncation_restarts_from_the_new_complete_prefix() {
    assert_eq!(appended(b"old\nlong\n", b""), b"");
    assert_eq!(appended(b"old\nlong\n", b"new\n"), b"new\n");
}

#[test]
fn same_length_replacement_restarts() {
    assert_eq!(appended(b"old\n", b"new\n"), b"new\n");
}

#[test]
fn longer_replacement_with_a_changed_prefix_restarts() {
    assert_eq!(appended(b"old\n", b"new\nmore\n"), b"new\nmore\n");
}

#[test]
fn empty_snapshots_and_blank_lines_are_byte_preserving() {
    assert_eq!(appended(b"", b""), b"");
    assert_eq!(appended(b"", b"\n\n"), b"\n\n");
}

#[test]
fn crlf_and_invalid_utf8_do_not_change_byte_offsets() {
    assert_eq!(appended(b"a\r\n", b"a\r\n\xff\r\npart"), b"\xff\r\n");
}

#[test]
fn long_lines_do_not_have_an_artificial_bridge_limit() {
    let mut bytes = vec![b'x'; 8192];
    assert!(appended(b"", &bytes).is_empty());
    bytes.push(b'\n');
    assert_eq!(appended(b"", &bytes), bytes);
}

#[test]
fn arbitrary_snapshot_boundaries_reproduce_the_complete_log_once() {
    let bytes = b"ready\nRTT\nclock0\nclock1\nclock2\nstats\nEND\npartial";
    let expected_end = bytes.iter().rposition(|byte| *byte == b'\n').unwrap() + 1;
    for step in 1..=bytes.len() {
        let mut emitted = Vec::new();
        let mut previous = &bytes[..0];
        for end in (step..bytes.len())
            .step_by(step)
            .chain(core::iter::once(bytes.len()))
        {
            let current = &bytes[..end];
            emitted.extend_from_slice(appended(previous, current));
            previous = current;
        }
        assert_eq!(emitted, bytes[..expected_end], "step={step}");
    }
}
