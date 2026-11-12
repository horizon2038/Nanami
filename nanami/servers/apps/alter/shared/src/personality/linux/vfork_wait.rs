use super::*;

// clone3 only parks for CLONE_VFORK. Its saved first argument remains the
// original user pointer, so it must never be interpreted as clone's flag word.
pub(super) fn is_vfork_wait(context: LinuxSyscallContext) -> bool {
    context.number == SYS_VFORK || context.number == SYS_CLONE3
        || (context.number == SYS_CLONE && context.args[0] & 0x4000 != 0)
}
