//! Actual ordinary-read transfer loop with scripted service and copy faults.
type Word = usize;
const EIO: i32 = 5;
const EFAULT: i32 = 14;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_read.rs"]
mod file_read;

#[test]
fn short_file_returns_its_payload_without_another_eof_request() {
    let input = [0x37; 73];
    let mut output = [0xcc; 4096];
    let mut calls = 0;
    let result = file_read::read(4096, 1024, |chunk| {
        calls += 1;
        assert_eq!(chunk, 1024);
        Ok((if calls == 1 { input.len() } else { 0 }, input.as_ptr() as usize))
    }, |offset, source, bytes| {
        output[offset..offset + bytes].copy_from_slice(unsafe {
            core::slice::from_raw_parts(source as *const u8, bytes)
        });
        Ok(())
    });
    assert_eq!(result, Ok(73));
    assert_eq!(&output[..73], &input);
    assert!(output[73..].iter().all(|&byte| byte == 0xcc));
    println!("FILE-SHORT-READ service_calls={calls} returned=73");
    assert_eq!(calls, 1);
}

#[test]
fn a_short_window_returns_before_any_later_service_error() {
    let mut calls = 0;
    let mut copied = Vec::new();
    let result = file_read::read(4096, 1024, |chunk| {
        calls += 1;
        match calls {
            1 => Ok((chunk, 100)),
            2 => Ok((17, 200)),
            _ => Err(EIO),
        }
    }, |offset, source, bytes| {
        copied.push((offset, source, bytes));
        Ok(())
    });
    assert_eq!(result, Ok(1041));
    assert_eq!(calls, 2);
    assert_eq!(copied, [(0, 100, 1024), (1024, 200, 17)]);
}

#[test]
fn full_windows_continue_and_faults_stop_before_later_service_reads() {
    for failure in [None, Some(0), Some(1), Some(2)] {
        let mut calls = 0;
        let mut copies = Vec::new();
        let result = file_read::read(2500, 1024, |chunk| {
            calls += 1;
            Ok((chunk, 100 + calls))
        }, |offset, source, bytes| {
            let index = copies.len();
            copies.push((offset, source, bytes));
            if failure == Some(index) { Err(EFAULT) } else { Ok(()) }
        });
        let expected_calls = failure.map_or(3, |index| index + 1);
        assert_eq!(calls, expected_calls);
        assert_eq!(copies.len(), expected_calls);
        assert_eq!(result, if failure.is_some() { Err(EFAULT) } else { Ok(2500) });
        for (index, &(offset, _, bytes)) in copies.iter().enumerate() {
            assert_eq!((offset, bytes), (1024 * index, (2500 - 1024 * index).min(1024)));
        }
    }
}

#[test]
fn zero_oversized_and_service_error_preserve_the_existing_transfer_policy() {
    let mut copies = 0;
    assert_eq!(file_read::read(0, 1024, |_| panic!("zero read"), |_, _, _| panic!("zero copy")), Ok(0));
    assert_eq!(file_read::read(1, 0, |chunk| { assert_eq!(chunk, 1); Ok((2, 0)) }, |_, _, _| { copies += 1; Ok(()) }), Err(EIO));
    assert_eq!(copies, 0);
    for fail_at in 1..=3 {
        let mut calls = 0;
        let mut copied = 0;
        assert_eq!(file_read::read(3000, 1000, |chunk| {
            calls += 1;
            if calls == fail_at { Err(EIO) } else { Ok((chunk, 0)) }
        }, |_, _, bytes| { copied += bytes; Ok(()) }), Err(EIO));
        assert_eq!(calls, fail_at);
        assert_eq!(copied, (fail_at - 1) * 1000);
    }
}
