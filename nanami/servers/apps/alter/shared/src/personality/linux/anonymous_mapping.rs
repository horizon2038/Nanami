//! Fresh anonymous backing can be zeroed with its final permissions.
use super::{map_request_error, sys_munmap, Runtime, Word, ENOMEM, LINUX_PAGE_SIZE};

pub(super) fn map(
    runtime: &mut Runtime, pid: Word, requested: Option<Word>, len: Word, prot: Word,
) -> Result<(Word, Word), i32> {
    let final_permissions = prot != 0 && prot & !7 == 0;
    let cow_size = len.checked_add(LINUX_PAGE_SIZE - 1).map(|n| n & !(LINUX_PAGE_SIZE - 1));
    let cow_zero = cfg!(target_arch = "x86_64") && len >= 128 * LINUX_PAGE_SIZE && matches!(prot, 3 | 7)
        && cow_size.is_some_and(|bytes| requested.is_none_or(|base|
            base != 0 && base & (LINUX_PAGE_SIZE - 1) == 0 && base.checked_add(bytes).is_some()));
    if cow_zero { super::private_memory::mark_shared(runtime, pid); }
    let allocate = || {
    if cow_zero {
        nanami_runtime::request_process_map_anonymous_zero_cow(pid, requested.unwrap_or(0), len, prot)
            .map_err(map_request_error)
    } else
    if final_permissions && requested.is_none() {
        nanami_runtime::request_process_map_anonymous_protected(pid, len, prot)
            .map_err(map_request_error)
    } else if final_permissions {
        let base = requested.unwrap();
        let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)?
            & !(LINUX_PAGE_SIZE - 1);
        nanami_runtime::request_process_map_initialized_protected_at(
            pid, base, mapped, 0, 0, prot,
        ).map_err(map_request_error)?;
        Ok((base, mapped))
    } else {
        if let Some(base) = requested {
            nanami_runtime::request_process_map_anonymous_at(pid, base, len)
        } else {
            nanami_runtime::request_process_map_anonymous(pid, len)
        }.map_err(map_request_error)
    }};
    let (base, mapped) = if cow_zero {
        super::private_memory::with_anonymous_cow(runtime, pid, requested.unwrap_or(0), len, allocate)
    } else {
        super::private_memory::with_mapping_mutation(runtime, pid, requested.unwrap_or(0), len, true, allocate)
    }?;
    if requested.is_some_and(|wanted| base != wanted) || mapped < len {
        let _ = super::private_memory::with_mapping_mutation(runtime, pid, base, mapped.max(len), true, ||
            nanami_runtime::request_process_mapping_release(pid, base, mapped).map_err(map_request_error));
        return Err(ENOMEM);
    }
    if !runtime.add_mapping(pid, base, mapped, prot) {
        super::private_memory::with_mapping_mutation(runtime, pid, base, mapped, true, ||
            nanami_runtime::request_process_mapping_release(pid, base, mapped).map_err(map_request_error))?;
        return Err(ENOMEM);
    }
    if !final_permissions {
        if let Err(error) = super::private_memory::with_mapping_mutation(runtime, pid, base, mapped, false, ||
            nanami_runtime::request_process_mapping_protect(pid, base, mapped, prot).map_err(map_request_error))
        {
            let _ = sys_munmap(runtime, pid, base, mapped);
            return Err(error);
        }
    }
    Ok((base, mapped))
}
