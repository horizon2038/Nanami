#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::{
    linux_clock_gettime, linux_close, linux_exit_group, linux_getpid, linux_nanosleep, linux_open,
    linux_readv, linux_write, LinuxIoVec, LinuxTimespec,
};
