extern crate alloc;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/pty.rs"]
mod pty;
use pty::*;

fn pair() -> (Ptys, usize, usize) {
    let mut store = Ptys::new();
    let (id, _) = store.create(0, 2).unwrap();
    let pair = store.get_mut(id).unwrap();
    pair.locked = false;
    let slave = pair.open_slave(2).unwrap();
    (store, id, slave)
}
fn take(pair: &mut Pty, master: bool) -> Vec<u8> {
    let mut out = vec![0; CAPACITY];
    let count = pair.peek(master, &mut out).unwrap();
    pair.consume(master, count);
    out.truncate(count);
    out
}
fn raw(pair: &mut Pty) {
    let mut termios = pair.termios;
    termios[..8].fill(0);
    termios[12..16].fill(0);
    pair.set_termios(termios, true);
}
#[test]
fn locked_slave_and_namespace() {
    let mut store = Ptys::new();
    let (id, _) = store.create(10, 2).unwrap();
    assert_eq!(store.get_mut(id).unwrap().open_slave(2), Err(Error::Locked));
    assert!(store.find(0, 0).is_none());
    assert_eq!(store.find(10, 0).unwrap().id, id);
    let (other, _) = store.create(11, 2).unwrap();
    assert_eq!(store.find(11, 0).unwrap().id, other);
}
#[test]
fn canonical_records_and_partial_reads() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    pair.write(true, b"abc", 0).unwrap();
    assert!(!pair.readable(false));
    assert_eq!(pair.peek(false, &mut [0; 10]), Err(Error::Again));
    pair.write(true, b"\rsecond\n", 0).unwrap();
    let mut first = [0; 2];
    assert_eq!(pair.peek(false, &mut first), Ok(2));
    assert_eq!(&first, b"ab");
    pair.consume(false, 2);
    assert_eq!(take(pair, false), b"c\n");
    assert_eq!(take(pair, false), b"second\n");
    assert_eq!(take(pair, true), b"abc\r\nsecond\r\n");
}
#[test]
fn eof_is_one_record_not_permanent_hangup() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    pair.write(true, b"text\x04\x04later\n", 0).unwrap();
    assert_eq!(take(pair, false), b"text");
    assert_eq!(take(pair, false), b"");
    assert_eq!(take(pair, false), b"later\n");
    assert!(!pair.readable(false));
}
#[test]
fn canonical_editing_and_quoting() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    pair.write(true, b"wrong\x15abc\x7fD one\x17two\x16\x03\n", 0)
        .unwrap();
    assert_eq!(take(pair, false), b"abD two\x03\n");
}
#[test]
fn signal_flush_and_noflsh() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    pair.write(false, b"old", 0).unwrap();
    assert_eq!(pair.write(true, b"old\x03", 0).unwrap().1, 1 << 2);
    assert!(!pair.readable(false));
    assert_eq!(take(pair, true), b"^C");
    let flags = pair.lflag() | NOFLSH;
    pair.termios[12..16].copy_from_slice(&flags.to_le_bytes());
    assert_eq!(pair.write(true, b"kept\x1c\n", 0).unwrap().1, 1 << 3);
    assert_eq!(take(pair, false), b"kept\n");
}
#[test]
fn raw_bytes_and_shared_description_flags() {
    let (mut store, id, slave) = pair();
    let pair = store.get_mut(id).unwrap();
    raw(pair);
    pair.write(true, b"\0\r\x03\xff", 123).unwrap();
    assert_eq!(take(pair, false), b"\0\r\x03\xff");
    assert_eq!(pair.last_input, 123);
    assert_eq!(pair.flags(slave, Some(0x800)), Ok(0x802));
    let another = pair.open_slave(2).unwrap();
    assert_eq!(pair.flags(another, None), Ok(2));
}
#[test]
fn peek_does_not_consume_and_raw_transition_keeps_bytes() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    pair.write(true, b"pending", 0).unwrap();
    let mut value = pair.termios;
    value[12] &= !2;
    pair.set_termios(value, false);
    let mut out = [0; 20];
    assert_eq!(pair.peek(false, &mut out), Ok(7));
    assert_eq!(pair.peek(false, &mut out), Ok(7));
    assert_eq!(take(pair, false), b"pending");
}
#[test]
fn packet_flow_and_flush() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    pair.packet = true;
    pair.write(true, &[19], 0).unwrap();
    assert_eq!(take(pair, true), [4]);
    pair.write(false, b"hello", 0).unwrap();
    assert_eq!(pair.peek(true, &mut [0; 10]), Err(Error::Again));
    pair.write(true, &[17], 0).unwrap();
    assert_eq!(take(pair, true), [8]);
    assert_eq!(take(pair, true), b"\0hello");
    pair.flush(2);
    assert_eq!(take(pair, true), [3]);
}
#[test]
fn slave_and_master_hangup_drain_before_eof() {
    let (mut store, id, slave) = pair();
    store.get_mut(id).unwrap().write(false, b"last", 0).unwrap();
    assert!(!store.close(id, slave));
    let pair = store.get_mut(id).unwrap();
    assert!(pair.master_hup());
    assert_eq!(take(pair, true), b"last");
    assert_eq!(pair.peek(true, &mut [0; 4]), Err(Error::Io));
    let reopened = pair.open_slave(2).unwrap();
    assert!(!pair.master_hup());
    pair.write(true, b"unfinished", 0).unwrap();
    assert!(store.close(id, 1));
    assert!(store.find(0, 0).is_none());
    let pair = store.get_mut(id).unwrap();
    assert_eq!(take(pair, false), b"unfinished");
    assert_eq!(take(pair, false), b"");
    assert_eq!(pair.write(false, b"x", 0), Err(Error::Io));
    store.close(id, reopened);
    assert!(store.get(id).is_err());
}
#[test]
fn bounded_output_recovers_after_drain() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    raw(pair);
    let bytes = vec![b'x'; CAPACITY * 2];
    let done = pair.write(false, &bytes, 0).unwrap().0;
    assert!(done < bytes.len());
    assert_eq!(pair.write(false, b"x", 0), Err(Error::Again));
    assert_eq!(take(pair, true).len(), done);
    assert_eq!(pair.write(false, b"x", 0), Ok((1, 0)));
}
#[test]
fn raw_minimum_readiness_and_canonical_zero_length_record() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    raw(pair);
    pair.termios[17 + VMIN] = 4;
    pair.write(true, b"abc", 0).unwrap();
    assert!(!pair.readable(false));
    pair.termios[17 + VTIME] = 1;
    assert!(pair.readable(false));
    pair.set_termios(default_termios(), true);
    pair.write(true, &[4], 0).unwrap();
    assert!(pair.readable(false));
    assert_eq!(pair.input_len(), 0);
    take(pair, false);
    assert!(!pair.readable(false));
}

#[test]
fn signal_and_flow_bytes_work_when_input_or_output_is_full() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    let mut value = pair.termios;
    value[12..16].copy_from_slice(&1u32.to_le_bytes()); // raw, ISIG, no echo
    pair.set_termios(value, true);
    let bytes = vec![b'a'; CAPACITY];
    assert_eq!(pair.write(true, &bytes, 0).unwrap().0, CAPACITY - 1);
    assert_eq!(pair.write(true, &[3], 0), Ok((1, 1 << 2)));
    assert_eq!(pair.input_len(), 0);
    pair.write(false, &bytes, 0).unwrap();
    pair.packet = true;
    pair.packet_status = 0;
    pair.write(true, &[19], 0).unwrap();
    assert_eq!(take(pair, true), [4]);
    pair.write(true, &[17], 0).unwrap();
    assert_eq!(take(pair, true), [8]);
    assert!(take(pair, true).len() > 1);
}

#[test]
fn empty_master_waits_before_first_slave_but_errors_after_last_slave_close() {
    let mut store = Ptys::new();
    let (id, _) = store.create(0, 2).unwrap();
    let pair = store.get_mut(id).unwrap();
    assert!(!pair.master_hup());
    assert_eq!(pair.peek(true, &mut [0; 1]), Err(Error::Again));
    pair.locked = false;
    let slave = pair.open_slave(2).unwrap();
    store.close(id, slave);
    let pair = store.get_mut(id).unwrap();
    assert_eq!(pair.write(true, b"x", 0), Err(Error::Io));
    assert!(!pair.writable(true));
}

#[test]
fn closed_pair_storage_is_reused_without_reusing_object_identity() {
    let mut store = Ptys::new();
    let mut last_id = 0;
    for _ in 0..100 {
        let (id, description) = store.create(10, 2).unwrap();
        assert!(id > last_id);
        assert_eq!(store.get(id).unwrap().number, 0);
        assert!(store.close(id, description));
        assert!(store.entries(10).next().is_none());
        last_id = id;
    }
}

#[test]
fn raw_master_input_readiness_does_not_depend_on_the_output_queue() {
    let (mut store, id, _) = pair();
    let pair = store.get_mut(id).unwrap();
    raw(pair);
    pair.write(false, &vec![b'x'; CAPACITY], 0).unwrap();
    assert!(!pair.writable(false));
    assert!(pair.writable(true));
    assert_eq!(pair.write(true, b"input", 0), Ok((5, 0)));
}
