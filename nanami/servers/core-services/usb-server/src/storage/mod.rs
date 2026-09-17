mod discovery;
mod probe;

use crate::{
    input::Input,
    usb::mass_storage,
    xhci::{Controller, Disk},
};
use libnanami::{ipc::ServiceRequest, RequestError, Word};
use nanami_services::block::*;

struct Root {
    controller: usize,
    disk: Disk,
    partition: nanami_gpt::Partition,
}

struct Session {
    pid: Word,
    local: Word,
    peer: Word,
    bytes: Word,
}

pub struct Storage {
    root: Option<Root>,
    session: Option<Session>,
    discovery: Option<discovery::Discovery>,
}

impl Storage {
    pub fn new(manager: Word, discovery_failed: bool) -> Self {
        Self {
            root: None,
            session: None,
            discovery: Some(discovery::Discovery::new(manager, discovery_failed)),
        }
    }

    pub fn request(
        &mut self,
        request: ServiceRequest,
        controllers: &mut [Controller],
        input: &mut Input,
    ) -> (Word, Word, Word) {
        match self.handle(request, controllers, input) {
            Ok((value, extra)) => (libnanami::OS_RESPONSE_OK, value, extra),
            Err(error) => (
                match error {
                    RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
                    RequestError::Status(status) => status,
                    _ => libnanami::OS_RESPONSE_FATAL,
                },
                0,
                0,
            ),
        }
    }

    fn handle(
        &mut self,
        request: ServiceRequest,
        controllers: &mut [Controller],
        input: &mut Input,
    ) -> Result<(Word, Word), RequestError> {
        let root = self.root.as_ref().ok_or(RequestError::Unsupported)?;
        let controller = &mut controllers[root.controller];
        if !controller.disk_present(root.disk) {
            self.root = None;
            libnanami::print!("[usb-server] root device detached; I/O disabled\n");
            return Err(RequestError::Transport);
        }
        if request.code == BLOCK_DEVICE_REQUEST_CONTROL {
            return match request.arg0 {
                BLOCK_DEVICE_CONTROL_GET_INFO => Ok((
                    BLOCK_DEVICE_BLOCK_SIZE,
                    (root.partition.sector_count / 2) as Word,
                )),
                BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY => {
                    let bytes = if request.arg1 == 0 {
                        BLOCK_DEVICE_DEFAULT_SHM_BYTES
                    } else {
                        request.arg1
                    };
                    if bytes == 0 || bytes > BLOCK_DEVICE_DEFAULT_SHM_BYTES {
                        return Err(RequestError::InvalidArgument);
                    }
                    if let Some(session) = &self.session {
                        if session.pid != request.identifier || session.bytes != bytes {
                            return Err(RequestError::Status(
                                libnanami::OS_RESPONSE_PERMISSION_DENIED,
                            ));
                        }
                        return Ok((session.peer, session.bytes));
                    }
                    let (local, peer) =
                        libnanami::request_shared_memory(request.identifier, bytes)?;
                    self.session = Some(Session {
                        pid: request.identifier,
                        local,
                        peer,
                        bytes,
                    });
                    Ok((peer, bytes))
                }
                _ => Err(RequestError::InvalidArgument),
            };
        }
        let session = self
            .session
            .as_ref()
            .filter(|s| s.pid == request.identifier)
            .ok_or(RequestError::InvalidArgument)?;
        if request.code == BLOCK_DEVICE_REQUEST_FLUSH {
            // IMMED=0: the successful CSW is the durability boundary, not just
            // acceptance of the command. Never replay an uncertain write/flush.
            let (ok, bytes) = controller.scsi(
                root.disk,
                &[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                &mut [],
                false,
                input,
            )?;
            return if ok && bytes == 0 {
                Ok((0, 0))
            } else {
                Err(RequestError::Protocol)
            };
        }
        let write = match request.code {
            BLOCK_DEVICE_REQUEST_READ => false,
            BLOCK_DEVICE_REQUEST_WRITE => true,
            _ => return Err(RequestError::InvalidArgument),
        };
        let (lba, bytes) = mass_storage::block_range(
            root.partition.first_lba,
            root.partition.sector_count,
            request.arg0,
            request.arg1,
        )
        .ok_or(RequestError::InvalidArgument)?;
        if request
            .arg2
            .checked_add(bytes)
            .is_none_or(|end| end > session.bytes)
        {
            return Err(RequestError::InvalidArgument);
        }
        let data = unsafe {
            core::slice::from_raw_parts_mut((session.local + request.arg2) as *mut u8, bytes)
        };
        let (cdb, len) = mass_storage::read_write(lba, (bytes / 512) as u32, write)
            .map_err(|_| RequestError::InvalidArgument)?;
        let (ok, transferred) = controller.scsi(root.disk, &cdb[..len], data, !write, input)?;
        if !ok || transferred != bytes {
            return Err(RequestError::Protocol);
        }
        Ok((bytes, 0))
    }
}
