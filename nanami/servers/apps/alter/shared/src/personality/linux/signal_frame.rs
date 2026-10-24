//! Linux UAPI signal frames, deliberately without FPU/SIMD state.
use crate::common::signals::AltStack;

#[cfg(target_arch = "x86_64")]
#[path = "arch/x86_64_signal.rs"]
mod implementation;
#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64_signal.rs"]
mod implementation;
pub use implementation::*;

pub fn put(frame: &mut [u8], offset: usize, value: usize) {
    frame[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
}

pub fn get(frame: &[u8], offset: usize) -> usize {
    u64::from_le_bytes(frame[offset..offset + 8].try_into().unwrap()) as usize
}

pub fn put_stack(frame: &mut [u8], offset: usize, stack: AltStack) {
    put(frame, offset, stack.sp);
    put(frame, offset + 8, stack.flags);
    put(frame, offset + 16, stack.size);
}

pub fn get_stack(frame: &[u8], offset: usize) -> AltStack {
    AltStack {
        sp: get(frame, offset),
        flags: get(frame, offset + 8) as u32 as usize,
        size: get(frame, offset + 16),
    }
}

pub fn user_address(address: usize) -> bool {
    address != 0 && address < 0x0000_8000_0000_0000
}
