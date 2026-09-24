use super::*;
use ipc::ServiceRequest;

const EXT2_S_IFREG: u16 = 0x8000;
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
    owner_pid: Word,
    inode: u32,
    size: u32,
    mode: u16,
}
impl FileHandle {
    const EMPTY: Self = Self {
        active: false,
        owner_pid: 0,
        inode: 0,
        size: 0,
        mode: 0,
    };
}
struct Inode {
    size: u32,
}
struct Ext2Runtime {
    handles: [FileHandle; 1],
    delegated_sessions: [DelegatedSession; 1],
    session: Option<ClientSession>,
    written: Vec<u8>,
    writes: usize,
    inode_reads: usize,
    error: Option<Word>,
}

#[path = "../../../nanami/servers/core-services/ext2-server/src/file_io.rs"]
mod file_io;

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
    Ok(Inode { size: 0 })
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
    _runtime: &mut Ext2Runtime,
    _session: ClientSession,
    _inode: Inode,
    _offset: usize,
    _len: usize,
    _out: usize,
) -> Result<usize, Word> {
    panic!("unexpected read")
}

fn read_directory(
    _runtime: &mut Ext2Runtime,
    _session: ClientSession,
    _inode: Inode,
    _offset: usize,
    _len: usize,
    _out: usize,
) -> Result<(usize, usize), Word> {
    panic!("unexpected directory read")
}

fn runtime(input: &[u8]) -> Ext2Runtime {
    Ext2Runtime {
        handles: [FileHandle {
            active: true,
            owner_pid: 10,
            inode: 7,
            size: 0,
            mode: EXT2_S_IFREG,
        }],
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
