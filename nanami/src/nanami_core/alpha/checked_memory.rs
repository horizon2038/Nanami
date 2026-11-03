//! Authorized process copies; checked requests enforce current page permissions.
use super::*;

impl Alpha {
    pub(super) fn handle_process_memory_copy_request(
        &mut self,
        request: OsRequestEvent,
        write_to_target: bool,
    ) -> Result<(), CapabilityError> {
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let target_va = request.arg1;
        let caller_va = request.arg2;
        let user_access = request.arg3 & nanami_uapi::os::OS_MEMORY_COPY_USER_ACCESS != 0;
        let size_bytes = request.arg3 & !nanami_uapi::os::OS_MEMORY_COPY_USER_ACCESS;
        if caller_pid == 0 || target_pid == 0 || target_va == 0 || caller_va == 0 || size_bytes == 0
        {
            return Err(CapabilityError::InvalidArgument);
        }

        self.authorize_process_memory_copy(caller_pid, target_pid)?;
        self.copy_authorized_process_memory(
            caller_pid,
            target_pid,
            nanami_uapi::os::ProcessMemoryTransfer {
                target: target_va,
                local: caller_va,
                bytes: size_bytes,
            },
            write_to_target,
            user_access,
        )
    }

    pub(super) fn handle_process_memory_vector_request(
        &mut self,
        request: OsRequestEvent,
        write_to_target: bool,
    ) -> Result<(), CapabilityError> {
        use nanami_uapi::os::{ProcessMemoryTransfer, OS_MEMORY_COPY_VECTOR_MAX};
        let caller_pid = request.identifier;
        let target_pid = request.arg0;
        let count = request.arg2;
        if caller_pid == 0
            || target_pid == 0
            || request.arg1 == 0
            || count == 0
            || count > OS_MEMORY_COPY_VECTOR_MAX
            || request.arg3 != 0
        {
            return Err(CapabilityError::InvalidArgument);
        }
        self.authorize_process_memory_copy(caller_pid, target_pid)?;
        // Snapshot all descriptors before writing anything. A destination may
        // overlap the caller's descriptor array, including a self-copy request.
        let mut transfers = [ProcessMemoryTransfer {
            target: 0,
            local: 0,
            bytes: 0,
        }; OS_MEMORY_COPY_VECTOR_MAX];
        let word_bytes = core::mem::size_of::<Word>();
        let record_bytes = 3 * word_bytes;
        let input = self.read_process_memory_into_static_buffer(
            caller_pid,
            request.arg1,
            count * record_bytes,
        )?;
        for (index, transfer) in transfers[..count].iter_mut().enumerate() {
            let offset = index * record_bytes;
            transfer.target =
                Word::from_ne_bytes(input[offset..offset + word_bytes].try_into().unwrap());
            transfer.local = Word::from_ne_bytes(
                input[offset + word_bytes..offset + 2 * word_bytes]
                    .try_into()
                    .unwrap(),
            );
            transfer.bytes = Word::from_ne_bytes(
                input[offset + 2 * word_bytes..offset + record_bytes]
                    .try_into()
                    .unwrap(),
            );
        }
        for transfer in &transfers[..count] {
            if transfer.target == 0
                || transfer.local == 0
                || transfer.bytes == 0
                || transfer.bytes & nanami_uapi::os::OS_MEMORY_COPY_USER_ACCESS != 0
            {
                return Err(CapabilityError::InvalidArgument);
            }
            self.copy_authorized_process_memory(
                caller_pid,
                target_pid,
                *transfer,
                write_to_target,
                true,
            )?;
        }
        Ok(())
    }

    fn authorize_process_memory_copy(
        &self,
        caller_pid: Word,
        target_pid: Word,
    ) -> Result<(), CapabilityError> {
        let target = self
            .processes
            .find_entry_by_pid(target_pid)
            .ok_or(CapabilityError::InvalidArgument)?;
        if target.reaper_pid != caller_pid && target_pid != caller_pid {
            return Err(CapabilityError::PermissionDenied);
        }
        if self.processes.find_entry_by_pid(caller_pid).is_none() {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(())
    }

    fn copy_authorized_process_memory(
        &mut self,
        caller_pid: Word,
        target_pid: Word,
        transfer: nanami_uapi::os::ProcessMemoryTransfer,
        write_to_target: bool,
        user_access: bool,
    ) -> Result<(), CapabilityError> {
        let target_va = transfer.target;
        let caller_va = transfer.local;
        let size_bytes = transfer.bytes;

        if user_access {
            let end = target_va
                .checked_add(size_bytes)
                .ok_or(CapabilityError::InvalidArgument)?;
            let permission = if write_to_target { 2 } else { 1 };
            let vm = self
                .processes
                .vm_space_mut(target_pid)
                .ok_or(CapabilityError::InvalidArgument)?;
            for va in (align_down(target_va, PAGE_SIZE)..end).step_by(PAGE_SIZE) {
                if vm.permissions(va) & permission == 0 {
                    return Err(CapabilityError::PermissionDenied);
                }
            }
        }

        let (source_pid, source_va, destination_pid, destination_va) = if write_to_target {
            (caller_pid, caller_va, target_pid, target_va)
        } else {
            (target_pid, target_va, caller_pid, caller_va)
        };
        self.copy_process_memory(
            source_pid,
            source_va,
            destination_pid,
            destination_va,
            size_bytes,
        )
    }
}
