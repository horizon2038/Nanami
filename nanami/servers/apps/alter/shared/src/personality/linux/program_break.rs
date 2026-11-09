use super::{
    align_up_word, map_anonymous_tracked, map_anonymous_at_tracked, Runtime, Word, ESRCH,
    LINUX_PROT_READ, LINUX_PROT_WRITE,
};

pub(super) fn sys_brk(runtime: &mut Runtime, pid: Word, requested: Word) -> Result<Word, i32> {
    if runtime.managed_process(pid).ok_or(ESRCH)?.program_break == 0 {
        let (base, mapped) = map_anonymous_tracked(runtime, pid, 4096, LINUX_PROT_READ | LINUX_PROT_WRITE)?;
        let process = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
        process.program_break = base;
        process.mapped_break = base + mapped;
        runtime.sync_program_break(pid);
    }
    let current = runtime.managed_process(pid).ok_or(ESRCH)?.program_break;
    if requested == 0 || requested <= current {
        return Ok(current);
    }
    let mapped = runtime.managed_process(pid).ok_or(ESRCH)?.mapped_break;
    if requested > mapped {
        let extra = align_up_word(requested - mapped, 4096);
        if mapped.checked_add(extra).is_some_and(|end| {
            runtime.managed_process(pid).is_some_and(|process| {
                process.mappings.iter().any(|mapping| {
                    mapping.base != 0 && mapping.size != 0
                        && mapping.base < end && mapping.base.saturating_add(mapping.size) > mapped
                })
            })
        }) {
            // Another mapping owns the required contiguous extension. Avoid
            // generating pages at Alpha's later allocation cursor just to
            // discover that the returned base cannot extend this break.
            return Ok(current);
        }
        // A free extension may lie behind Alpha's automatic cursor after a
        // neighboring mmap was removed. Allocate the required address directly.
        let (_, size) = map_anonymous_at_tracked(runtime, pid, mapped, extra,
            LINUX_PROT_READ | LINUX_PROT_WRITE)?;
        runtime.managed_process_mut(pid).ok_or(ESRCH)?.mapped_break = mapped + size;
    }
    runtime.managed_process_mut(pid).ok_or(ESRCH)?.program_break = requested;
    runtime.sync_program_break(pid);
    Ok(requested)
}
