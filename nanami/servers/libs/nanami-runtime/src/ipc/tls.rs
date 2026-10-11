use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, Ordering};

use a9n_abi::{CapabilityDescriptor, IpcBuffer};

use crate::{map_capability_error, RequestError, Word};

const SELF_PCB_DESCRIPTOR: CapabilityDescriptor = 0x0801_0000_0000_0000;
static IPC_TLS_READY: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    static mut __ipc_buffer_start: u8;
}

pub const fn process_slot_descriptor(slot: Word) -> CapabilityDescriptor {
    0x0800_0000_0000_0000usize | ((slot & 0xff) << 48)
}

pub fn init_ipc_tls() -> Result<(), RequestError> {
    if IPC_TLS_READY.load(Ordering::Acquire) {
        return Ok(());
    }

    let current_ptr = unsafe { a9n_abi::arch::ipc_buffer::unsafe_get_ipc_buffer() };
    if !current_ptr.is_null() {
        IPC_TLS_READY.store(true, Ordering::Release);
        return Ok(());
    }

    let ipc_buffer_ptr = addr_of_mut!(__ipc_buffer_start) as *mut IpcBuffer;
    let ipc_buffer = unsafe { &mut *ipc_buffer_ptr };
    a9n_abi::arch::ipc_buffer::early_configure_to_tls(SELF_PCB_DESCRIPTOR, ipc_buffer)
        .map_err(map_capability_error)?;

    IPC_TLS_READY.store(true, Ordering::Release);
    Ok(())
}
