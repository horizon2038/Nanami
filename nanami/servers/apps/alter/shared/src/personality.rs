use libnanami::Word;

use crate::common::process::LinuxSyscallContext;
use crate::common::state::{OsPersonality, Runtime};

#[path = "personality/freebsd.rs"]
pub mod freebsd;
#[path = "personality/linux.rs"]
pub mod linux;

pub trait Personality {
    const ID: OsPersonality;
    const NAME: &'static str;
    const ROOT: &'static [u8];
    const BIN_PREFIX: &'static [u8];

    fn dispatch_syscall(
        runtime: &mut Runtime,
        native_pid: Word,
        context: LinuxSyscallContext,
    ) -> linux::EmulationAction;
}

pub struct Linux;

impl Personality for Linux {
    const ID: OsPersonality = OsPersonality::Linux;
    const NAME: &'static str = "linux";
    const ROOT: &'static [u8] = b"/alter/linux";
    const BIN_PREFIX: &'static [u8] = b"/alter/linux/bin/";

    #[inline]
    fn dispatch_syscall(
        runtime: &mut Runtime,
        native_pid: Word,
        context: LinuxSyscallContext,
    ) -> linux::EmulationAction {
        linux::dispatch_syscall(runtime, native_pid, context)
    }
}

pub struct FreeBsd;

impl Personality for FreeBsd {
    const ID: OsPersonality = OsPersonality::FreeBsd;
    const NAME: &'static str = "freebsd";
    const ROOT: &'static [u8] = b"/alter/freebsd";
    const BIN_PREFIX: &'static [u8] = b"/alter/freebsd/bin/";

    #[inline]
    fn dispatch_syscall(
        runtime: &mut Runtime,
        native_pid: Word,
        context: LinuxSyscallContext,
    ) -> linux::EmulationAction {
        freebsd::dispatch_syscall(runtime, native_pid, context)
    }
}

#[inline]
pub fn dispatch_syscall(
    personality: OsPersonality,
    runtime: &mut Runtime,
    native_pid: Word,
    context: LinuxSyscallContext,
) -> linux::EmulationAction {
    match personality {
        OsPersonality::Linux => Linux::dispatch_syscall(runtime, native_pid, context),
        OsPersonality::FreeBsd => FreeBsd::dispatch_syscall(runtime, native_pid, context),
    }
}

#[inline]
pub fn root(personality: OsPersonality) -> &'static [u8] {
    match personality {
        OsPersonality::Linux => Linux::ROOT,
        OsPersonality::FreeBsd => FreeBsd::ROOT,
    }
}

#[inline]
pub fn bin_prefix(personality: OsPersonality) -> &'static [u8] {
    match personality {
        OsPersonality::Linux => Linux::BIN_PREFIX,
        OsPersonality::FreeBsd => FreeBsd::BIN_PREFIX,
    }
}

#[inline]
pub fn name(personality: OsPersonality) -> &'static str {
    match personality {
        OsPersonality::Linux => Linux::NAME,
        OsPersonality::FreeBsd => FreeBsd::NAME,
    }
}
