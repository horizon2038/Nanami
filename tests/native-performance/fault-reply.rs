//! Production x86_64 reply encoder; Kernel's existing prefix update is modeled.
#![allow(dead_code)]
extern crate self as nanami_runtime;
pub type Word = usize;
type HardwareContext = [Word; 23];
const RAX: usize = 0;
const RIP: usize = 15;
const RFLAGS: usize = 17;
const CARRY: Word = 1;
#[derive(Clone, Copy)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
    program_counter: Word,
}
mod state {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum OsPersonality {
        Linux,
        FreeBsd,
    }
}
use state::OsPersonality;
#[path = "../../nanami/servers/apps/alter/shared/src/common/arch/x86_64/process.rs"]
mod encoder;

fn context(pc: Word) -> LinuxSyscallContext {
    LinuxSyscallContext {
        number: 39,
        args: [0; 6],
        program_counter: pc,
    }
}
fn original() -> HardwareContext {
    core::array::from_fn(|index| 0x1000 + index * 16)
}
fn apply(original: HardwareContext, reply: HardwareContext, count: usize) -> HardwareContext {
    let mut result = original;
    result[..count].copy_from_slice(&reply[..count]);
    result
}

#[test]
fn ordinary_linux_replies_change_only_rax_and_preserve_the_untransmitted_context() {
    let original = original();
    for value in [0, 42, -38, isize::MIN, isize::MAX] {
        let mut reply = original;
        let count = encoder::configure_trapped_syscall_reply(
            &mut reply,
            context(original[RIP]),
            value,
            OsPersonality::Linux,
        );
        assert_eq!(count, 1);
        let actual = apply(original, reply, count);
        assert_eq!(actual[RAX], value as Word);
        assert_eq!(actual[1..], original[1..]);
    }
}

#[test]
fn changed_pc_uses_the_existing_prefix_through_rip() {
    let original = original();
    let mut reply = original;
    let count = encoder::configure_trapped_syscall_reply(
        &mut reply,
        context(0x400002),
        7,
        OsPersonality::Linux,
    );
    assert_eq!(count, RIP + 1);
    let mut expected = original;
    expected[RAX] = 7;
    expected[RIP] = 0x400002;
    assert_eq!(apply(original, reply, count), expected);
}

#[test]
fn signal_and_parked_context_encoding_keeps_the_general_register_prefix() {
    let mut reply = original();
    let pc = reply[RIP];
    assert_eq!(
        encoder::configure_personality_syscall_reply(
            &mut reply,
            context(pc),
            -4,
            OsPersonality::Linux
        ),
        RIP + 1
    );
    assert_eq!(reply[RAX], (-4isize) as Word);
}

#[test]
fn freebsd_replies_still_include_carry_and_clear_it_on_success() {
    let original = original();
    for value in [-9, 5] {
        let mut reply = original;
        let count = encoder::configure_trapped_syscall_reply(
            &mut reply,
            context(original[RIP]),
            value,
            OsPersonality::FreeBsd,
        );
        assert_eq!(count, RFLAGS + 1);
        let actual = apply(original, reply, count);
        assert_eq!(actual[RAX], value.unsigned_abs());
        assert_eq!(actual[RFLAGS] & CARRY, Word::from(value < 0));
        assert_eq!(actual[RFLAGS] & !CARRY, original[RFLAGS] & !CARRY);
        assert_eq!(actual[RFLAGS + 1..], original[RFLAGS + 1..]);
    }
}
