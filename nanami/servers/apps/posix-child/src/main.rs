#![no_std]
#![no_main]

use nanami_runtime::Word;

const SLOT_POSIX_SERVICE: Word = 22;
const EXPECTED: &[u8] = b"POSIX inherited fd works";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_client::posix::posix_exit(125)
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    nanami_client::registry::connect_posix_service(SLOT_POSIX_SERVICE)?;
    let posix_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_POSIX_SERVICE);
    let (shm, shm_size) = nanami_client::posix::posix_attach_shared_memory(posix_port, 0x1000)?;
    if shm == 0 || shm_size < 0x1000 {
        nanami_client::posix::posix_exit(2);
    }
    let bytes = match nanami_client::posix::posix_read(posix_port, 3, 0, EXPECTED.len() as Word) {
        Ok(bytes) => bytes,
        Err(_) => nanami_client::posix::posix_exit(3),
    };
    if bytes != EXPECTED.len() as Word {
        nanami_client::posix::posix_exit(4);
    }
    if !shm_matches(shm, EXPECTED) {
        nanami_client::posix::posix_exit(5);
    }
    nanami_client::posix::posix_exit(0)
}

fn shm_matches(base: Word, expected: &[u8]) -> bool {
    let mut i = 0usize;
    while i < expected.len() {
        let byte = unsafe { core::ptr::read_volatile((base as usize + i) as *const u8) };
        if byte != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}

nanami_runtime::nanami_entry!(nanami_main);
