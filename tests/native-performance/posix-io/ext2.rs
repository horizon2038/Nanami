use super::*;
use ipc::ServiceRequest;

const EXT2_S_IFREG: u16 = 0x8000;
const EXT2_S_IFMT: u16 = 0xf000;
const EXT2_S_IFSOCK: u16 = 0xc000;
fn release_unlinked_inode(runtime: &mut Ext2Runtime, _: u32) -> Result<(), RequestError> {
    assert!(runtime.frozen_by.is_none());
    runtime.socket_reclaims += 1;
    Ok(())
}
const EXT2_S_IFDIR: u16 = 0x4000;

// Only the storage/session fixtures are mocked; dispatch checks below are production code.
#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    pid: Word,
    shm_local: Word,
    shm_size: Word,
}
struct DelegatedSession {
    active: bool,
    owner_pid: Word,
    peer_pid: Word,
    shm_local: Word,
    shm_size: Word,
}
#[derive(Clone, Copy)]
struct FileHandle {
    active: bool,
    close_pending: bool,
    owner_pid: Word,
    inode: u32,
    size: u32,
    mode: u16,
    read_grant: read_grants::Grant,
    cursor: Option<Word>,
}
impl FileHandle {
    const EMPTY: Self = Self {
        active: false,
        close_pending: false,
        owner_pid: 0,
        inode: 0,
        size: 0,
        mode: 0,
        read_grant: read_grants::Grant::EMPTY,
        cursor: None,
    };
}
struct Inode {
    size: u32,
}
struct MappedPages(bool);
impl MappedPages {
    fn pins(&self, _: u32) -> bool { self.0 }
}
struct Ext2Runtime {
    next_read_grant: Word,
    handles: [FileHandle; 2],
    delegated_sessions: [DelegatedSession; 1],
    session: Option<ClientSession>,
    written: Vec<u8>,
    writes: usize,
    inode_reads: usize,
    error: Option<Word>,
    frozen_by: Option<Word>,
    socket_reclaims: usize,
    file_versions: file_versions::Versions,
    mapped_pages: MappedPages,
    snapshot_bytes: Option<Vec<u8>>,
    snapshot_reads: usize,
}

#[path = "../../../nanami/servers/core-services/ext2-server/src/file_versions.rs"]
mod file_versions;
#[path = "../../../nanami/servers/core-services/ext2-server/src/file_io.rs"]
mod file_io;
#[path = "../../../nanami/servers/core-services/ext2-server/src/read_grants.rs"]
mod read_grants;
#[path = "../../../nanami/servers/core-services/ext2-server/src/file_cursor.rs"]
mod file_cursor;
#[path = "ext2_cursor.rs"]
mod cursor_tests;

fn file_stat(runtime: &mut Ext2Runtime, file: FileHandle, flags: Word) -> (Word, Word, Word) {
    let identity = if flags == vfs::VFS_STAT_INCLUDE_VERSION {
        file_io::versioned_file_identity(runtime, file)
    } else { file.inode as Word };
    (OS_RESPONSE_OK, identity, (file.size as Word) | (vfs::VFS_FILE_TYPE_REGULAR << vfs::VFS_STAT_TYPE_SHIFT))
}

fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::Status(status) => status,
        _ => OS_RESPONSE_FATAL,
    }
}

mod truncate {
    use super::*;
    pub(super) fn resize_inode(
        runtime: &mut Ext2Runtime,
        inode: u32,
        _: &mut Inode,
        length: usize,
    ) -> Result<(), RequestError> {
        assert_eq!(inode, 7);
        if let Some(error) = runtime.error {
            return Err(RequestError::Status(error));
        }
        runtime.handles[0].size = length as u32;
        Ok(())
    }
}

#[test]
fn truncate_checks_handle_owner_type_and_propagates_errors() {
    let mut runtime = runtime(&[]);
    let mut request = request(1, 0, 0);
    request.arg1 = 13;
    request.identifier = 99;
    assert_eq!(
        file_io::handle_ftruncate(request, &mut runtime).0,
        OS_RESPONSE_INVALID_DESCRIPTOR
    );
    request.identifier = 10;
    runtime.handles[0].mode = EXT2_S_IFDIR;
    assert_eq!(
        file_io::handle_ftruncate(request, &mut runtime).0,
        OS_RESPONSE_INVALID_ARGUMENT
    );
    assert_eq!(runtime.inode_reads, 0);
    runtime.handles[0].mode = EXT2_S_IFREG;
    runtime.error = Some(OS_RESPONSE_FATAL);
    assert_eq!(
        file_io::handle_ftruncate(request, &mut runtime).0,
        OS_RESPONSE_FATAL
    );
    runtime.error = None;
    assert_eq!(
        file_io::handle_ftruncate(request, &mut runtime).0,
        OS_RESPONSE_OK
    );
    assert_eq!(runtime.handles[0].size, 13);
}

fn find_session(runtime: &Ext2Runtime, pid: Word) -> Option<ClientSession> {
    runtime.session.filter(|s| s.active && s.pid == pid)
}

fn read_inode(runtime: &mut Ext2Runtime, inode: u32) -> Result<Inode, RequestError> {
    assert_eq!(inode, 7);
    runtime.inode_reads += 1;
    Ok(Inode { size: runtime.snapshot_bytes.as_ref().map_or(0, |bytes| bytes.len() as u32) })
}

fn write_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: &mut Inode,
    inode_no: u32,
    offset: usize,
    len: usize,
    input: usize,
) -> Result<usize, Word> {
    runtime.writes += 1;
    assert_eq!(inode_no, 7);
    assert!(input
        .checked_add(len)
        .is_some_and(|end| end <= session.shm_size));
    if let Some(error) = runtime.error {
        return Err(error);
    }
    let bytes =
        unsafe { std::slice::from_raw_parts((session.shm_local + input) as *const u8, len) };
    runtime.written.extend_from_slice(bytes);
    inode.size = (offset + len) as u32;
    Ok(len)
}

fn read_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    inode: Inode,
    offset: usize,
    len: usize,
    out: usize,
) -> Result<usize, Word> {
    let bytes = runtime.snapshot_bytes.as_ref().expect("snapshot read");
    runtime.snapshot_reads += 1;
    assert_eq!(inode.size as usize, bytes.len());
    let len = len.min(bytes.len().saturating_sub(offset));
    if out.checked_add(len).is_none_or(|end| end > session.shm_size) { return Err(OS_RESPONSE_INVALID_ARGUMENT); }
    if let Some(error) = runtime.error { return Err(error); }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr().add(offset), (session.shm_local + out) as *mut u8, len); }
    Ok(len)
}

fn read_directory(
    _runtime: &mut Ext2Runtime,
    _session: ClientSession,
    _inode: Inode,
    _offset: usize,
    _len: usize,
    _out: usize,
    _handle: usize,
) -> Result<(usize, usize), Word> {
    panic!("unexpected directory read")
}

fn runtime(input: &[u8]) -> Ext2Runtime {
    Ext2Runtime {
        next_read_grant: 1,
        handles: [FileHandle {
            active: true,
            close_pending: false,
            owner_pid: 10,
            inode: 7,
            size: 0,
            mode: EXT2_S_IFREG,
            read_grant: read_grants::Grant::EMPTY,
            cursor: None,
        }, FileHandle::EMPTY],
        delegated_sessions: [DelegatedSession {
            active: true,
            owner_pid: 10,
            peer_pid: 16,
            shm_local: input.as_ptr() as Word,
            shm_size: input.len(),
        }],
        // A delegated write must not look up the owner's conventional buffer.
        session: None,
        written: Vec::new(),
        writes: 0,
        inode_reads: 0,
        error: None,
        frozen_by: None,
        socket_reclaims: 0,
        file_versions: file_versions::Versions::new(),
        mapped_pages: MappedPages(false),
        snapshot_bytes: None,
        snapshot_reads: 0,
    }
}

#[test]
fn socket_mode_is_not_mistaken_for_a_regular_file_or_directory() {
    let mut runtime = runtime(&[]);
    runtime.session = Some(ClientSession {
        active: true,
        pid: 10,
        shm_local: 0,
        shm_size: 0,
    });
    runtime.handles[0].mode = EXT2_S_IFSOCK;
    let request = request(1, 0, 0);
    assert_eq!(
        file_io::handle_read(request, &mut runtime).0,
        OS_RESPONSE_ILLEGAL_OPERATION
    );
    assert_eq!(
        file_io::handle_read_dir(request, &mut runtime).0,
        OS_RESPONSE_ILLEGAL_OPERATION
    );
    assert_eq!(
        file_io::handle_write_file(request, &mut runtime).0,
        OS_RESPONSE_ILLEGAL_OPERATION
    );
    assert_eq!(runtime.inode_reads, 0);
}

#[test]
fn frozen_close_defers_inode_reclamation_until_thaw() {
    for mode in [EXT2_S_IFSOCK, EXT2_S_IFREG, EXT2_S_IFDIR] {
        let mut runtime = runtime(&[]);
        runtime.handles[0].mode = mode;
        runtime.frozen_by = Some(9);
        let request = request(1, 0, 0);
        assert_eq!(file_io::handle_close(request, &mut runtime).0, OS_RESPONSE_OK);
        assert!(!runtime.handles[0].active);
        assert!(runtime.handles[0].close_pending);
        assert_eq!(runtime.socket_reclaims, 0);
        runtime.frozen_by = None;
        file_io::reclaim_closed_inodes(&mut runtime).unwrap();
        assert!(!runtime.handles[0].close_pending);
        assert_eq!(runtime.socket_reclaims, 1);
    }
}

fn request(delegate: Word, input: Word, len: Word) -> ServiceRequest {
    ServiceRequest {
        identifier: 10,
        code: vfs::VFS_REQUEST_WRITE_DELEGATED,
        arg0: 0,
        arg1: 19,
        arg2: len,
        arg3: (delegate << vfs::VFS_DELEGATE_ID_SHIFT) | input,
    }
}

#[test]
fn versioned_read_checks_identity_size_and_delegate_before_publishing_bytes() {
    let mut output = [0xaa; 32];
    let mut runtime = runtime(&output);
    runtime.delegated_sessions[0].shm_local = output.as_mut_ptr() as Word;
    runtime.handles[0].size = 24;
    runtime.snapshot_bytes = Some((0..24).collect());
    let file = runtime.handles[0];
    let identity = file_io::versioned_file_identity(&mut runtime, file);
    let req = ServiceRequest { code: vfs::VFS_REQUEST_READ_VERSIONED, arg1: identity, ..request(1, 2, 24) };
    assert_eq!(file_io::handle_read_versioned(ServiceRequest {identifier: 11, ..req}, &mut runtime),
        (OS_RESPONSE_INVALID_ARGUMENT, 0, 0));
    runtime.handles[0].owner_pid = 11;
    assert_eq!(file_io::handle_read_versioned(req, &mut runtime), (OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0));
    runtime.handles[0].owner_pid = 10;
    assert_eq!(file_io::handle_read_versioned(ServiceRequest {arg2: 23, ..req}, &mut runtime),
        (OS_RESPONSE_OK, 0, 0));
    runtime.mapped_pages.0 = true;
    assert_eq!(file_io::handle_read_versioned(req, &mut runtime), (OS_RESPONSE_OK, 0, 0));
    runtime.mapped_pages.0 = false;
    assert_eq!(runtime.inode_reads, 0);
    assert_eq!(output, [0xaa; 32]);
    assert_eq!(file_io::handle_read_versioned(req, &mut runtime), (OS_RESPONSE_OK, 24, identity));
    assert_eq!(&output[2..26], runtime.snapshot_bytes.as_deref().unwrap());
    assert_eq!(output[..2], [0xaa; 2]);
    assert_eq!(output[26..], [0xaa; 6]);
    runtime.file_versions.invalidate(7);
    assert_eq!(file_io::handle_read_versioned(req, &mut runtime), (OS_RESPONSE_OK, 0, 0));
    assert_eq!((runtime.inode_reads, runtime.snapshot_reads), (1, 1));
}

#[test]
fn read_grants_bind_owner_peer_delegate_and_never_reuse_a_closed_handle_token() {
    let mut output = [0xaa; 32];
    let mut r = runtime(&output);
    r.delegated_sessions[0].shm_local = output.as_mut_ptr() as Word;
    assert_eq!(read_grants::issue(&mut r, 0, 11, 1), 0);
    assert_eq!(read_grants::issue(&mut r, 0, 10, 0), 0);
    assert_eq!(read_grants::issue(&mut r, 0, 10, 2), 0);
    let token = read_grants::issue(&mut r, 0, 10, 1);
    assert_ne!(token, 0);
    assert_eq!(read_grants::issue(&mut r, 0, 10, 1), token);
    assert!(read_grants::resolve(&r, 16, token).is_some());
    for peer in [0, 10, 17] { assert!(read_grants::resolve(&r, peer, token).is_none()); }
    for case in 0..4 {
        match case { 0 => r.delegated_sessions[0].active = false,
            1 => r.delegated_sessions[0].owner_pid = 99,
            2 => r.delegated_sessions[0].peer_pid = 99,
            _ => r.handles[0].active = false }
        assert!(read_grants::resolve(&r, 16, token).is_none());
        r.delegated_sessions[0].active = true; r.delegated_sessions[0].owner_pid = 10;
        r.delegated_sessions[0].peer_pid = 16; r.handles[0].active = true;
    }
    let mut reopened = r.handles[0]; reopened.read_grant = read_grants::Grant::EMPTY;
    let req = ServiceRequest { identifier: 10, arg0: 0, ..ServiceRequest::default() };
    assert_eq!(file_io::handle_close(req, &mut r).0, OS_RESPONSE_OK);
    assert!(read_grants::resolve(&r, 16, token).is_none());
    r.handles[0] = reopened;
    let next = read_grants::issue(&mut r, 0, 10, 1);
    assert_ne!(next, token); assert!(read_grants::resolve(&r, 16, token).is_none());
    assert!(read_grants::resolve(&r, 16, next).is_some());
    r.handles[0].read_grant = read_grants::Grant::EMPTY;
    r.next_read_grant = Word::MAX;
    assert_eq!(read_grants::issue(&mut r, 0, 10, 1), 0);
    assert!(r.handles[0].active);
}

#[test]
fn granted_positioned_reads_check_authority_and_bounds_without_changing_an_offset() {
    let mut output = [0xaa; 32]; let mut r = runtime(&output);
    r.delegated_sessions[0].shm_local = output.as_mut_ptr() as Word;
    r.snapshot_bytes = Some((0..24).collect()); r.handles[0].size = 24;
    let token = read_grants::issue(&mut r, 0, 10, 1);
    let req = ServiceRequest { identifier: 16, code: vfs::VFS_REQUEST_READ_GRANTED,
        arg0: token, arg1: 3, arg2: 7, arg3: 5 };
    assert_eq!(file_io::handle_read_granted(ServiceRequest { identifier: 17, ..req }, &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    assert_eq!(r.inode_reads, 0); assert_eq!(output, [0xaa; 32]);
    assert_eq!(file_io::handle_read_granted(req, &mut r), (OS_RESPONSE_OK, 7, 0));
    assert_eq!(&output[5..12], &[3,4,5,6,7,8,9]);
    assert_eq!(&output[..5], &[0xaa; 5]); assert_eq!(&output[12..], &[0xaa; 20]);
    assert_eq!(file_io::handle_read_granted(ServiceRequest { arg3: Word::MAX, ..req }, &mut r).0, OS_RESPONSE_INVALID_ARGUMENT);
    assert_eq!(r.writes, 0);
}

#[test]
fn granted_versions_remain_fresh_and_frozen_close_revokes_read_authority() {
    let mut output = [0xaa; 32]; let mut r = runtime(&output);
    r.delegated_sessions[0].shm_local = output.as_mut_ptr() as Word;
    r.snapshot_bytes = Some((0..24).collect()); r.handles[0].size = 24;
    let token = read_grants::issue(&mut r, 0, 10, 1);
    let stat = ServiceRequest { identifier: 16, arg0: token, arg1: vfs::VFS_STAT_INCLUDE_VERSION, ..ServiceRequest::default() };
    let identity = file_io::handle_fstat_granted(stat, &mut r).1;
    let read = ServiceRequest { identifier: 16, arg0: token, arg1: identity, arg2: 24, arg3: 2, ..ServiceRequest::default() };
    assert_eq!(file_io::handle_read_versioned_granted(read, &mut r), (OS_RESPONSE_OK, 24, identity));
    output.fill(0xaa); r.file_versions.invalidate(7);
    assert_ne!(file_io::handle_fstat_granted(stat, &mut r).1, identity);
    assert_eq!(file_io::handle_read_versioned_granted(read, &mut r), (OS_RESPONSE_OK, 0, 0));
    assert_eq!(output, [0xaa; 32]);
    r.frozen_by = Some(10);
    assert_eq!(file_io::handle_close(ServiceRequest { identifier: 10, arg0: 0, ..ServiceRequest::default() }, &mut r).0, OS_RESPONSE_OK);
    assert_eq!(file_io::handle_fstat_granted(stat, &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
    assert_eq!(file_io::handle_read_versioned_granted(read, &mut r).0, OS_RESPONSE_INVALID_DESCRIPTOR);
}

#[test]
fn delegated_write_reads_peer_buffer_and_updates_owner_handle() {
    let input: Vec<u8> = (0..64).collect();
    let mut runtime = runtime(&input);
    assert_eq!(
        file_io::handle_write_delegated(request(1, 7, 13), &mut runtime),
        (OS_RESPONSE_OK, 13, 0)
    );
    assert_eq!(runtime.written, input[7..20]);
    assert_eq!(runtime.handles[0].size, 32);
    assert_eq!(runtime.writes, 1);
}

#[test]
fn inactive_unowned_and_invalid_delegates_are_rejected_before_storage() {
    let input = [0u8; 32];
    for delegate in [0, 2, vfs::VFS_DELEGATE_VALUE_MASK] {
        let mut runtime = runtime(&input);
        assert_eq!(
            file_io::handle_write_delegated(request(delegate, 0, 1), &mut runtime).0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
        assert_eq!(runtime.inode_reads, 0);
    }
    for (active, owner) in [(false, 10), (true, 99)] {
        let mut runtime = runtime(&input);
        runtime.delegated_sessions[0].active = active;
        runtime.delegated_sessions[0].owner_pid = owner;
        assert_eq!(
            file_io::handle_write_delegated(request(1, 0, 1), &mut runtime).0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
        assert_eq!(runtime.inode_reads, 0);
    }
}

#[test]
fn buffer_delegation_does_not_grant_access_to_another_owners_handle() {
    let input = [0u8; 32];
    let mut runtime = runtime(&input);
    runtime.handles[0].owner_pid = 99;
    assert_eq!(
        file_io::handle_write_delegated(request(1, 0, 1), &mut runtime).0,
        OS_RESPONSE_INVALID_DESCRIPTOR
    );
    assert_eq!(runtime.inode_reads, 0);
}

#[test]
fn delegated_write_rejects_out_of_bounds_and_overflow() {
    let input = [0u8; 32];
    for (offset, len) in [(32, 1), (1, usize::MAX)] {
        let mut runtime = runtime(&input);
        assert_eq!(
            file_io::handle_write_delegated(request(1, offset, len), &mut runtime).0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
        assert_eq!(runtime.inode_reads, 0);
    }
}

#[test]
fn conventional_write_still_uses_the_owners_buffer() {
    let input: Vec<u8> = (0..32).collect();
    let mut runtime = runtime(&input);
    runtime.session = Some(ClientSession {
        active: true,
        pid: 10,
        shm_local: input.as_ptr() as Word,
        shm_size: input.len(),
    });
    let mut request = request(0, 4, 8);
    request.code = vfs::VFS_REQUEST_WRITE;
    assert_eq!(
        file_io::handle_write_file(request, &mut runtime),
        (OS_RESPONSE_OK, 8, 0)
    );
    assert_eq!(runtime.written, input[4..12]);
}

#[test]
fn backend_error_is_returned_without_successful_size_update() {
    let input = [0u8; 32];
    let mut runtime = runtime(&input);
    runtime.error = Some(99);
    assert_eq!(
        file_io::handle_write_delegated(request(1, 0, 4), &mut runtime),
        (99, 0, 0)
    );
    assert_eq!(runtime.writes, 1);
    assert_eq!(runtime.handles[0].size, 0);
    assert!(runtime.written.is_empty());
}
