#![no_std]
#![no_main]

use libnanami::Word;

const SLOT_POSIX_SERVICE: Word = 22;
const EXPECTED: &[u8] = b"POSIX inherited fd works";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_services::posix::posix_exit(125)
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()?;
    nanami_services::registry::connect_posix_service(SLOT_POSIX_SERVICE)?;
    let posix_port = libnanami::ipc::process_slot_descriptor(SLOT_POSIX_SERVICE);
    let (shm, shm_size) = nanami_services::posix::posix_attach_shared_memory(posix_port, 0x1000)?;
    if shm == 0 || shm_size < 0x1000 {
        nanami_services::posix::posix_exit(2);
    }

    let bytes = match nanami_services::posix::posix_read(posix_port, 3, 0, EXPECTED.len() as Word) {
        Ok(bytes) => bytes,
        Err(_) => nanami_services::posix::posix_exit(3),
    };
    if bytes != EXPECTED.len() as Word {
        nanami_services::posix::posix_exit(4);
    }
    if !shm_matches(shm, EXPECTED) {
        nanami_services::posix::posix_exit(5);
    }
    nanami_services::posix::posix_exit(0)
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

libnanami::nanami_entry!(nanami_main);
