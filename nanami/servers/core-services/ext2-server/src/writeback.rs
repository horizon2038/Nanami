use super::*;

const WRITEBACK_DELAY_MS: Word = 1000;

pub(super) fn check_writable(runtime: &Ext2Runtime) -> Result<(), RequestError> {
    if runtime.frozen_by.is_some() {
        return Err(RequestError::Status(
            libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        ));
    }
    runtime.block_cache.check_writable()
}

pub(super) fn handle_freeze(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    // Resolve the registered service's PID; the request identifier is minted by
    // Alpha, not supplied by the caller. No shared data buffer is needed here.
    if runtime.writeback.power_pid.is_none() {
        runtime.writeback.power_pid =
            libnanami::connect_service_by_name_with_pid(nanami_services::power::ACPI_SERVICE, 25)
                .ok();
    }
    if runtime.writeback.power_pid != Some(request.identifier) {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    if request.code == nanami_services::vfs::VFS_REQUEST_THAW {
        if runtime.frozen_by == Some(request.identifier) {
            runtime.frozen_by = None;
        }
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    // The server is single-threaded: no write can slip between flush and freeze.
    match flush_blocks(runtime) {
        Ok(()) => {
            runtime.frozen_by = Some(request.identifier);
            libnanami::println!("[ext2-server] storage synchronized and frozen");
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

pub(super) struct Writeback {
    timer: Option<Word>,
    power_pid: Option<Word>,
    pub(super) armed: bool,
}

impl Writeback {
    pub(super) fn new(timer: Option<Word>) -> Self {
        Self {
            timer,
            power_pid: None,
            armed: false,
        }
    }
}

pub(super) fn initialize(runtime: &mut Ext2Runtime) -> Result<(), RequestError> {
    if runtime.writeback.timer.is_none()
        && nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE).is_ok()
    {
        runtime.writeback.timer = Some(libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE));
    }
    if runtime.writeback.timer.is_some() {
        libnanami::ipc::bind_current_thread_notification(libnanami::ipc::process_slot_descriptor(
            libnanami::PROCESS_SLOT_NOTIFICATION,
        ))?;
    }
    Ok(())
}

pub(super) fn after_request(runtime: &mut Ext2Runtime) -> Result<(), RequestError> {
    if !runtime.block_cache.is_dirty()
        || runtime.writeback.armed
        || runtime.block_cache.check_writable().is_err()
    {
        return Ok(());
    }
    if let Some(timer) = runtime.writeback.timer {
        match nanami_services::timer::timer_service_sleep_async_on_notification_milliseconds(
            timer,
            WRITEBACK_DELAY_MS,
            libnanami::PROCESS_SLOT_NOTIFICATION,
        ) {
            Ok(()) => {
                runtime.writeback.armed = true;
                return Ok(());
            }
            Err(error) => {
                log_request_error(
                    "[ext2-server] writeback timer failed; using synchronous writes: ",
                    error,
                );
                runtime.writeback.timer = None;
            }
        }
    }
    // A platform without a timer must not leave dirty data indefinitely.
    flush_blocks(runtime)
}

pub(super) fn handle_sync(
    request: ServiceRequest,
    runtime: &mut Ext2Runtime,
) -> (Word, Word, Word) {
    if find_session(runtime, request.identifier).is_none() {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if request.code == nanami_services::vfs::VFS_REQUEST_FSYNC {
        let Some(handle) = runtime.handles.get(request.arg0) else {
            return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
        };
        if !handle.active || handle.owner_pid != request.identifier {
            return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
        }
    }
    // Initially mount-wide: allocation, directories and inodes share blocks.
    // fdatasync/fsync therefore also persist unrelated dirty files.
    match flush_blocks(runtime) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}
