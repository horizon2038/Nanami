use super::*;

fn request(code: Word, pid: Word) -> ServiceRequest {
    ServiceRequest {
        identifier: pid,
        code,
        arg0: 0,
    }
}

#[test]
fn freeze_persists_dirty_blocks_before_acknowledging() {
    let mut r = runtime(vec![1]);
    r.scratch[..BLOCK_SIZE].fill(0x42);
    write_block(&mut r, 1).unwrap();
    assert!(r.block_cache.is_dirty());
    assert_eq!(
        writeback::handle_freeze(request(vfs::VFS_REQUEST_FREEZE, 8), &mut r).0,
        OS_RESPONSE_OK
    );
    assert_eq!(r.frozen_by, Some(8));
    assert!(!r.block_cache.is_dirty());
    FAKE.with(|fake| {
        assert!(fake.borrow().durable[BLOCK_SIZE..2 * BLOCK_SIZE]
            .iter()
            .all(|&b| b == 0x42))
    });
}

#[test]
fn flush_failure_does_not_acknowledge_or_freeze() {
    let mut r = runtime(vec![1]);
    write_block(&mut r, 1).unwrap();
    FAKE.with(|fake| fake.borrow_mut().flush_error = Some(RequestError::Transport));
    assert_ne!(
        writeback::handle_freeze(request(vfs::VFS_REQUEST_FREEZE, 8), &mut r).0,
        OS_RESPONSE_OK
    );
    assert_eq!(r.frozen_by, None);
}

#[test]
fn only_power_service_can_freeze_or_thaw() {
    let mut r = runtime(vec![1]);
    assert!(writeback::check_writable(&r).is_ok());
    assert_eq!(
        writeback::handle_freeze(request(vfs::VFS_REQUEST_FREEZE, 16), &mut r).0,
        OS_RESPONSE_PERMISSION_DENIED
    );
    assert_eq!(r.frozen_by, None);
    assert_eq!(
        writeback::handle_freeze(request(vfs::VFS_REQUEST_FREEZE, 8), &mut r).0,
        OS_RESPONSE_OK
    );
    assert_eq!(
        writeback::handle_freeze(request(vfs::VFS_REQUEST_THAW, 16), &mut r).0,
        OS_RESPONSE_PERMISSION_DENIED
    );
    assert_eq!(r.frozen_by, Some(8));
    assert_eq!(
        writeback::check_writable(&r),
        Err(RequestError::Status(OS_RESPONSE_ILLEGAL_OPERATION))
    );
    assert_eq!(
        writeback::handle_freeze(request(vfs::VFS_REQUEST_THAW, 8), &mut r).0,
        OS_RESPONSE_OK
    );
    assert_eq!(r.frozen_by, None);
    assert!(writeback::check_writable(&r).is_ok());
}
