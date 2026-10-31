//! Translate the extensible clone3 ABI into the existing fork/shared-VM paths.
use super::*;

const CLONE_VFORK: Word = 0x4000;
const CLONE_DETACHED: Word = 0x400000;
const EXIT_SIGNAL_MASK: Word = 0xff;
const E2BIG: i32 = 7;

pub(super) fn clone3(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let context = arguments(runtime, pid, context)?;
    sys_fork(runtime, pid, context)
}

fn arguments(
    runtime: &Runtime,
    pid: Word,
    mut context: LinuxSyscallContext,
) -> Result<LinuxSyscallContext, i32> {
    let size = context.args[1];
    if size < 64 {
        return Err(EINVAL);
    }
    if size > LINUX_PAGE_SIZE {
        return Err(E2BIG); // Linux bounds extensible structures to one page.
    }
    read_target_memory(runtime, pid, context.args[0], size)?;
    let bytes = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, size) };
    if bytes
        .get(88..)
        .is_some_and(|tail| tail.iter().any(|byte| *byte != 0))
    {
        return Err(E2BIG);
    }
    let word = |offset: usize| -> Word {
        let mut value = [0; 8];
        if let Some(source) = bytes.get(offset..size.min(offset + 8)) {
            value[..source.len()].copy_from_slice(source);
        }
        u64::from_le_bytes(value) as Word
    };
    let flags = word(0);
    let signal = word(32);
    if flags & (EXIT_SIGNAL_MASK | CLONE_DETACHED) != 0 || signal > 64 {
        return Err(EINVAL);
    }
    // Shared-VM vfork+exec needs address-space detachment, which Alpha does
    // not expose. Preserve libc's ENOSYS fallback, never fake shared memory.
    if flags & CLONE_VFORK != 0
        || flags >> 32 != 0
        || word(64) != 0
        || word(72) != 0
        || word(80) != 0
    {
        return Err(ENOSYS);
    }
    let stack = word(40);
    let stack_size = word(48);
    if (stack == 0) != (stack_size == 0) {
        return Err(EINVAL);
    }
    let stack_top = stack.checked_add(stack_size).ok_or(EINVAL)?;
    // Like Linux access_ok, validate the user address range, not every page's
    // current protection. glibc includes a PROT_NONE guard in this allocation.
    if stack_top > 0x0000_8000_0000_0000 {
        return Err(EFAULT);
    }
    context.number = SYS_CLONE;
    context.args = [flags | signal, stack_top, word(24), 0, 0, 0];
    #[cfg(target_arch = "x86_64")]
    {
        context.args[3] = word(16);
        context.args[4] = word(56);
    }
    #[cfg(target_arch = "aarch64")]
    {
        context.args[3] = word(56);
        context.args[4] = word(16);
    }
    Ok(context)
}
