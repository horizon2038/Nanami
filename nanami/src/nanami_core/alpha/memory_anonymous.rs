//! Fresh supervised zero mappings: share cold small-page backing under COW.
use super::*;

impl Alpha {
    /// The supervisor holds every VM peer stopped and has revoked its views.
    /// This flag never applies to native callers or initialized file payloads.
    pub(super) fn map_process_anonymous_zero_cow(
        &mut self, pid: usize, base: Option<usize>, pages: usize, permissions: usize,
    ) -> Result<usize, CapabilityError> {
        if !matches!(permissions, 3 | 7) || self.processes.find_entry_by_pid(pid)
            .is_none_or(|entry| !entry.capabilities_sealed || entry.exited || entry.resource_quarantined)
        { return Err(CapabilityError::PermissionDenied); }
        if !cfg!(any(target_arch = "x86_64", test)) || pages < 128
            || option_env!("NANAMI_SHARED_WRITABLE_ZERO") == Some("0")
            || option_env!("NANAMI_SHARED_FILE_ZERO_TAIL") == Some("0")
            || option_env!("NANAMI_EAGER_FILE_ZERO_TAIL") == Some("1")
        {
            return self.map_process_initialized_pages_protected(pid, base, pages, None, permissions);
        }
        // The ordinary constructor tries genuine huge leaves first. Its small
        // readonly fallback already owns and initializes every zero alias.
        let base = self.map_process_initialized_pages_protected(pid, base, pages, None, 1)?;
        let bytes = pages.checked_mul(PAGE_SIZE).ok_or(CapabilityError::InvalidArgument)?;
        let result = (|| {
            let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?;
            let shared = vm.shared_private_pages(base, base + bytes);
            for &va in &shared { vm.mark_private_page_cow_write(va, true); }
            // Allocator headers and early payload stay immediately writable.
            // Cold aliases retain logical WRITE without hardware leaves.
            // Existing faults and checked copyout perform the detach.
            const PREFIX: usize = 16;
            self.handle_process_private_write(OsRequestEvent {
                identifier: self.processes.find_entry_by_pid(pid).ok_or(CapabilityError::Fatal)?.reaper_pid,
                code: 0, arg0: pid, arg1: base, arg2: pages.min(PREFIX) * PAGE_SIZE,
                arg3: nanami_uapi::os::OS_PRIVATE_WRITE_DETACH_DEFERRED,
            })?;
            let cold = self.processes.vm_space(pid).ok_or(CapabilityError::Fatal)?
                .shared_private_pages(base, base + bytes);
            let mut cursor = base;
            for run in cold.chunk_by(|a, b| a.checked_add(PAGE_SIZE) == Some(*b)) {
                if cursor < run[0] {
                    self.handle_mapping_protect_request(OsRequestEvent {
                        identifier: self.processes.find_entry_by_pid(pid).ok_or(CapabilityError::Fatal)?.reaper_pid,
                        code: 0, arg0: pid, arg1: cursor, arg2: run[0] - cursor, arg3: permissions,
                    })?;
                }
                let vm = self.processes.vm_space_mut(pid).ok_or(CapabilityError::Fatal)?;
                for &va in run {
                    if !vm.mapping_deferred(va) || vm.mapping_is_uncertain(va)
                        || !vm.private_page_cow_write(va) { return Err(CapabilityError::Fatal); }
                    vm.set_permissions(va, permissions);
                }
                cursor = run[run.len() - 1] + PAGE_SIZE;
            }
            if cursor < base + bytes {
                // Includes whole genuine huge leaves. Never split a huge
                // interval just to prepare the small hot allocator prefix.
                self.handle_mapping_protect_request(OsRequestEvent {
                    identifier: self.processes.find_entry_by_pid(pid).ok_or(CapabilityError::Fatal)?.reaper_pid,
                    code: 0, arg0: pid, arg1: cursor, arg2: base + bytes - cursor, arg3: permissions,
                })?;
            }
            #[cfg(not(test))]
            if option_env!("NANAMI_WRITABLE_ZERO_DIAG") == Some("1") {
                crate::force_info!("[anon.zero-cow] pid={} base={:#x} pages={} shared-before-prefix={} prefix={} cold={}",
                    pid, base, pages, shared.len(), pages.min(PREFIX), cold.len());
            }
            Ok(base)
        })();
        if result.is_err() {
            // All aliases, including an unpopulated suffix or failed leaf,
            // remain owned until this normal release/ACK path can retire them.
            self.handle_mapping_release_request(OsRequestEvent {
                identifier: pid, code: nanami_uapi::os::OS_REQUEST_MAPPING_RELEASE,
                arg0: base, arg1: bytes, arg2: pid,
                arg3: nanami_uapi::os::OS_MAPPING_RELEASE_ALLOW_HOLES,
            })?;
        }
        result
    }
}
