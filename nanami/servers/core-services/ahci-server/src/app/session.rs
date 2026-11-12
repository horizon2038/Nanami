//! Owned client windows and synchronous transfers through one AHCI command slot.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct ClientSession {
    active: bool,
    pid: Word,
    shared_memory: Word,
    shared_size: Word,
    dma_physical: Word,
    peer_memory: Word,
}

impl ClientSession {
    pub(super) const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shared_memory: 0,
        shared_size: 0,
        dma_physical: 0,
        peer_memory: 0,
    };
}

fn attach_dma_memory(request: ServiceRequest, session: &mut ClientSession, runtime: &AhciRuntime) -> Result<(Word, Word), RequestError> {
    let size = shared_window_bytes(request.arg1, runtime.dma_data_bytes);
    if request.identifier == 0 || size == 0 || size > runtime.dma_data_bytes {
        return Err(RequestError::InvalidArgument);
    }
    if session.active && session.pid == request.identifier && session.dma_physical != 0 {
        if session.shared_size != size {
            return Err(RequestError::InvalidArgument);
        }
        return Ok((session.peer_memory, size));
    }
    let mapped = (size + 4095) & !4095;
    let (physical, local) = nanami_runtime::request_dma(mapped)?;
    if local == 0 || (local | physical) & 4095 != 0
        || local.checked_add(mapped).is_none() || physical.checked_add(mapped).is_none()
    {
        return Err(RequestError::Protocol);
    }
    // The peer can address the complete last page, including a short explicit
    // window's padding. Clear it all before publishing any mapping.
    unsafe { ptr::write_bytes(local as *mut u8, 0, mapped); }
    let peer = match nanami_runtime::request_mapping_share(request.identifier, local, mapped) {
        Ok(peer) => peer,
        Err(error) => {
            // No command can refer to this new allocation yet. Alpha retains
            // any uncertain peer mapping/reference if its cleanup failed.
            let _ = nanami_runtime::request_mapping_release(local, mapped);
            return Err(error);
        }
    };
    *session = ClientSession {
        active: true,
        pid: request.identifier,
        shared_memory: local,
        shared_size: size,
        dma_physical: physical,
        peer_memory: peer,
    };
    nanami_runtime::println!("[ahci-server] DMA read window owner={} bytes={}", request.identifier, size);
    Ok((peer, size))
}

fn handle_control(
    request: ServiceRequest,
    session: &mut ClientSession,
    runtime: &AhciRuntime,
) -> (Word, Word, Word) {
    match request.arg0 {
        nanami_client::block::BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = shared_window_bytes(request.arg1, runtime.dma_data_bytes);
            match nanami_runtime::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => {
                    *session = ClientSession {
                        active: true,
                        pid: request.identifier,
                        shared_memory: local,
                        shared_size: size,
                        dma_physical: 0,
                        peer_memory: peer,
                    };
                    (nanami_runtime::OS_RESPONSE_OK, peer, size)
                }
                Err(error) => (status(error), 0, 0),
            }
        }
        nanami_client::block::BLOCK_DEVICE_CONTROL_ATTACH_DMA_READ_MEMORY => {
            match attach_dma_memory(request, session, runtime) {
                Ok((peer, size)) => (nanami_runtime::OS_RESPONSE_OK, peer, size),
                Err(error) => (status(error), 0, 0),
            }
        }
        nanami_client::block::BLOCK_DEVICE_CONTROL_GET_INFO => (
            nanami_runtime::OS_RESPONSE_OK,
            BLOCK_BYTES as Word,
            (runtime.partition_sectors / (BLOCK_BYTES / SECTOR_BYTES) as u64) as Word,
        ),
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_transfer(
    request: ServiceRequest,
    session: &ClientSession,
    runtime: &AhciRuntime,
    write: bool,
) -> Result<(Word, Word, Word), RequestError> {
    if !session.active || session.pid != request.identifier {
        return Ok((nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0));
    }
    let bytes = match transfer_bytes(request.arg1, request.arg2, session.shared_size, runtime.dma_data_bytes) {
        Some(bytes) => bytes,
        None => return Ok((nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)),
    };
    let offset = request.arg2;
    let direct_read = !write && session.dma_physical != 0 && offset & 1 == 0;
    let dma_data = runtime.dma_virtual + DMA_DATA_OFFSET;
    let physical = if direct_read {
        session.dma_physical + offset
    } else {
        runtime.dma_physical + DMA_DATA_OFFSET
    };
    if write {
        unsafe {
            ptr::copy_nonoverlapping(
                (session.shared_memory + offset) as *const u8,
                dma_data as *mut u8,
                bytes,
            );
        }
    }
    match transfer(runtime, request.arg0, bytes, write, physical) {
        Ok(()) => {
            if option_env!("NANAMI_AHCI_WINDOW_DIAG") == Some("1")
                && bytes > nanami_client::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES
            {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static REPORTS: AtomicUsize = AtomicUsize::new(0);
                if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                    nanami_runtime::println!("[ahci.transfer] large bytes={} write={} direct={}", bytes, write, direct_read);
                }
            }
            if !write && !direct_read {
                unsafe {
                    ptr::copy_nonoverlapping(
                        dma_data as *const u8,
                        (session.shared_memory + offset) as *mut u8,
                        bytes,
                    );
                }
            }
            Ok((nanami_runtime::OS_RESPONSE_OK, bytes, 0))
        }
        Err(error) => {
            if matches!(error, RequestError::Transport | RequestError::Protocol) {
                // A failed command may still own its header or data window,
                // including private read/write staging. Stop its DMA before
                // replying or allowing another command to reuse that backing.
                // Failed recovery propagates to serve(), which retains all DMA
                // backing and stops handling requests.
                configure_port(runtime)?;
            }
            Ok((status(error), 0, 0))
        }
    }
}

pub(super) fn handle_request(
    request: ServiceRequest,
    session: &mut ClientSession,
    runtime: &AhciRuntime,
) -> Result<(Word, Word, Word), RequestError> {
    Ok(match request.code {
        nanami_client::block::BLOCK_DEVICE_REQUEST_CONTROL => handle_control(request, session, runtime),
        nanami_client::block::BLOCK_DEVICE_REQUEST_READ => {
            return handle_transfer(request, session, runtime, false);
        }
        nanami_client::block::BLOCK_DEVICE_REQUEST_WRITE => {
            return handle_transfer(request, session, runtime, true);
        }
        nanami_client::block::BLOCK_DEVICE_REQUEST_FLUSH => {
            if !session.active || session.pid != request.identifier {
                return Ok((nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0));
            }
            let result = if runtime.flush_command == 0 {
                Err(RequestError::Unsupported)
            } else {
                prepare_command(runtime, runtime.flush_command, 0, 0, 0, false)
                    .and_then(|()| submit(runtime, 0))
            };
            match result {
                Ok(()) => (nanami_runtime::OS_RESPONSE_OK, 0, 0),
                Err(error) => {
                    if matches!(error, RequestError::Transport | RequestError::Protocol) {
                        configure_port(runtime)?;
                    }
                    (status(error), 0, 0)
                }
            }
        }
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    })
}
