use super::*;

fn setup() -> (Runtime, Vec<u8>) {
    let runtime = Runtime::new();
    let mut direct = vec![0xa5; 65536];
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        p.direct_pid = 42;
        p.direct_shm = direct.as_mut_ptr() as Word;
    });
    (runtime, direct)
}

fn query(r: &mut Runtime, operation: Word, directory: Word, path: &[u8], flags: Word) -> Result<(Word, Word), i32> {
    namespace::request_path(r, 1, operation, directory, path, &[], flags, 4096)
}

#[test]
fn direct_metadata_remains_fresh_and_uses_the_original_decoder() {
    let (mut r, _direct) = setup();
    let bytes = r.path(b"value");
    let first = namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, bytes, true).unwrap();
    PROVIDER.with(|p| p.borrow_mut().metadata.size = 987);
    let bytes = r.path(b"value");
    let second = namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, bytes, true).unwrap();
    assert_ne!(first, second);
    PROVIDER.with(|p| p.borrow_mut().error = Some(VFS_PATH_ERROR_MISSING));
    let bytes = r.path(b"value");
    assert_eq!(namespace::stat_at(&mut r, 1, LINUX_AT_FDCWD, bytes, true), Err(ENOENT));
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!((p.provider_queries, p.connections, p.attachments), (1, 1, 1));
        assert_eq!((p.direct_calls, p.proxy_calls), (3, 0));
        assert!(p.requests.iter().all(|r| r.flags & VFS_PATH_WITH_METADATA != 0));
    });
}

#[test]
fn root_relative_and_absolute_paths_ignore_only_the_appropriate_directory_fd() {
    let (mut r, _direct) = setup();
    for (directory, path) in [(VFS_PATH_DIRECTORY_ROOT, &b"relative"[..]), (23, &b"/absolute"[..])] {
        query(&mut r, VFS_PATH_RESOLVE, directory, path, VFS_PATH_ALLOW_MISSING).unwrap();
    }
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!((p.direct_calls, p.proxy_calls), (2, 0));
        assert!(p.requests.iter().all(|r| r.directory == VFS_PATH_DIRECTORY_ROOT && r.flags == VFS_PATH_ALLOW_MISSING));
        assert_eq!(p.inputs, [b"relative".to_vec(), b"/absolute".to_vec()]);
    });
}

#[test]
fn directory_fds_open_and_mutations_keep_posix_ownership() {
    let (mut r, _direct) = setup();
    query(&mut r, VFS_PATH_RESOLVE, 9, b"relative", 0).unwrap();
    query(&mut r, VFS_PATH_OPEN_READONLY, VFS_PATH_DIRECTORY_ROOT, b"/file", 0).unwrap();
    query(&mut r, VFS_PATH_CHMOD, VFS_PATH_DIRECTORY_ROOT, b"/file", 0).unwrap();
    query(&mut r, VFS_PATH_SYMLINK, VFS_PATH_DIRECTORY_ROOT, b"/file", 0).unwrap();
    PROVIDER.with(|p| {
        let p = p.borrow();
        assert_eq!((p.provider_queries, p.connections, p.attachments, p.direct_calls, p.proxy_calls), (0, 0, 0, 0, 4));
        assert_eq!(p.requests[0].directory, 9);
        assert!(p.closed.is_empty());
    });
}

#[test]
fn legacy_mismatched_failed_short_overflowing_or_overlapping_channels_fall_back_once() {
    for case in 0..8 {
        let (mut r, _direct) = setup();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            match case {
                0 => p.direct_pid = 0,
                1 => p.registry_pid = 77,
                2 => p.registry_pid = 0,
                3 => p.attach_fails = true,
                4 => p.direct_shm = 0,
                5 => p.direct_size = 65535,
                6 => p.direct_shm = Word::MAX - 10,
                _ => p.direct_shm = r.posix_shm + 4096,
            }
        });
        for _ in 0..2 { query(&mut r, VFS_PATH_RESOLVE, VFS_PATH_DIRECTORY_ROOT, b"/x", 0).unwrap(); }
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!(p.provider_queries, 1, "case {case}");
            assert_eq!((p.direct_calls, p.proxy_calls), (0, 2), "case {case}");
            if case == 1 { assert_eq!(p.attachments, 0); }
            assert_eq!(&r.buffer[..10], b"/lib/value");
        });
    }
}

#[test]
fn failed_transport_and_malformed_reply_retry_without_corrupting_inputs() {
    for malformed in [false, true] {
        let (mut r, _direct) = setup();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut();
            p.direct_transport_error = !malformed;
            if malformed { p.direct_reply_bytes = Some(4097); }
        });
        for _ in 0..2 {
            query(&mut r, VFS_PATH_RESOLVE, VFS_PATH_DIRECTORY_ROOT, b"/original", VFS_PATH_FOLLOW_FINAL).unwrap();
            assert_eq!(&r.buffer[..10], b"/lib/value");
        }
        PROVIDER.with(|p| {
            let p = p.borrow();
            assert_eq!((p.direct_calls, p.proxy_calls, p.attachments), (1, 2, 1));
            assert!(p.inputs.iter().all(|p| p == b"/original"));
            assert!(p.requests.iter().all(|r| r.flags == VFS_PATH_FOLLOW_FINAL));
        });
    }
}

#[test]
fn filesystem_errors_do_not_retry_or_publish_output() {
    let (mut r, _direct) = setup();
    r.buffer[..16].fill(0x5a);
    for code in [VFS_PATH_ERROR_MISSING, VFS_PATH_ERROR_IO, VFS_PATH_ERROR_LOOP, VFS_PATH_ERROR_UNSUPPORTED] {
        PROVIDER.with(|p| p.borrow_mut().error = Some(code));
        assert!(query(&mut r, VFS_PATH_RESOLVE, VFS_PATH_DIRECTORY_ROOT, b"/x", 0).is_err());
        assert_eq!(&r.buffer[..16], &[0x5a; 16]);
    }
    PROVIDER.with(|p| assert_eq!((p.borrow().direct_calls, p.borrow().proxy_calls), (4, 0)));
}

#[test]
fn stat_reply_words_are_not_payload_lengths() {
    let (mut r, direct) = setup();
    r.buffer[..16].fill(0x5a);
    PROVIDER.with(|p| p.borrow_mut().metadata.inode = Word::MAX - 1);
    let details = query(&mut r, VFS_PATH_STAT, VFS_PATH_DIRECTORY_ROOT, b"/x", VFS_PATH_FOLLOW_FINAL).unwrap();
    assert_eq!(details.0, Word::MAX - 1);
    assert_eq!(&r.buffer[..16], &[0x5a; 16]);
    assert_eq!(&direct[0xe000..0xe010], &[0xa5; 16]);
    PROVIDER.with(|p| assert_eq!((p.borrow().direct_calls, p.borrow().proxy_calls), (1, 0)));
}

#[test]
fn readlink_copies_exact_payload_including_maximum_output() {
    for bytes in [0, 1, 127, 4096] {
        let (mut r, _direct) = setup();
        PROVIDER.with(|p| {
            let mut p = p.borrow_mut(); p.metadata.file_type = VFS_FILE_TYPE_SYMLINK;
            p.link_target = vec![b'x'; bytes];
        });
        r.buffer[bytes] = 0x5a;
        assert_eq!(query(&mut r, VFS_PATH_READLINK, VFS_PATH_DIRECTORY_ROOT, b"/link", 0).unwrap().0, bytes);
        assert_eq!(&r.buffer[..bytes], vec![b'x'; bytes]);
        assert_eq!(r.buffer[bytes], 0x5a);
    }
}

#[test]
fn invalid_source_extents_fall_back_before_connecting_or_dereferencing() {
    let (mut r, _direct) = setup();
    query(&mut r, VFS_PATH_RESOLVE, VFS_PATH_DIRECTORY_ROOT, b"/x", 0).unwrap();
    let original = unsafe { ((r.posix_shm + 0xc000) as *const PathRequest).read_unaligned() };
    for case in 0..6 {
        let mut request = original;
        let (source, size, descriptor) = match case {
            0 => (0, 65536, 0xc000),
            1 => (Word::MAX - 1, 65536, 0xc000),
            2 => (r.posix_shm, 65536, Word::MAX),
            3 => { request.root_bytes = Word::MAX; (r.posix_shm, 65536, 0xc000) },
            4 => { request.path_offset = Word::MAX; (r.posix_shm, 65536, 0xc000) },
            _ => { request.output_bytes = Word::MAX; (r.posix_shm, 65536, 0xc000) },
        };
        let mut channel = rooted_channel::Channel::new();
        assert!(channel.request(1, source, size, descriptor, request).is_none());
    }
    PROVIDER.with(|p| assert_eq!(p.borrow().provider_queries, 1));
}
