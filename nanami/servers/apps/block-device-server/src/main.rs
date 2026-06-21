#![no_std]
#![no_main]

use core::cmp::min;
use core::ptr;
use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

const SLOT_SERVICE_PORT: Word = 20;
const BLOCK_SIZE: usize = 1024;
const BLOCK_COUNT: usize = 64;
const RAMDISK_BYTES: usize = BLOCK_SIZE * BLOCK_COUNT;
const INODE_SIZE: usize = 128;
const INODE_COUNT: usize = 32;
const INODE_TABLE_BLOCK: usize = 5;
const ROOT_DIR_BLOCK: usize = 9;
const DOCS_DIR_BLOCK: usize = 10;
const HELLO_BLOCK: usize = 11;
const README_BLOCK: usize = 12;
const ROOT_INODE: u32 = 2;
const DOCS_INODE: u32 = 11;
const HELLO_INODE: u32 = 12;
const README_INODE: u32 = 13;
const HELLO_TEXT: &[u8] = b"Hello from Nanami ext2 ramdisk.\n";
const README_TEXT: &[u8] = b"This file lives in /docs/readme.txt.\n";

static mut RAMDISK: [u8; RAMDISK_BYTES] = [0; RAMDISK_BYTES];

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    pid: Word,
    shm_local: Word,
    shm_size: Word,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shm_local: 0,
        shm_size: 0,
    };
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[block-device] panic\n");
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    unsafe { init_ramdisk() };
    nanami_services::registry::register_block_device().map_err(log_reg_error)?;
    libnanami::print!("[block-device] service registered: block-device\n");

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut session = ClientSession::EMPTY;
    let mut pending = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;

    loop {
        let event = if has_reply {
            has_reply = false;
            match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending.0,
                pending.1,
                pending.2,
            ) {
                Ok(event) => event,
                Err(e) => {
                    log_request_error("[block-device] reply_receive failed: ", e);
                    continue;
                }
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(event) => event,
                Err(e) => return Err(log_error("[block-device] receive failed: ", e)),
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                pending = handle_request(request, &mut session);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::print!("[block-device] fault id={}", identifier);
                libnanami::print!(" reason={:#x}\n", reason);
            }
        }
    }
}

fn handle_request(request: ServiceRequest, session: &mut ClientSession) -> (Word, Word, Word) {
    match request.code {
        nanami_services::block::BLOCK_DEVICE_REQUEST_CONTROL => handle_control(request, session),
        nanami_services::block::BLOCK_DEVICE_REQUEST_READ => handle_read(request, session),
        nanami_services::block::BLOCK_DEVICE_REQUEST_WRITE => handle_write(request, session),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_control(request: ServiceRequest, session: &mut ClientSession) -> (Word, Word, Word) {
    match request.arg0 {
        nanami_services::block::BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                nanami_services::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => {
                    *session = ClientSession {
                        active: true,
                        pid: request.identifier,
                        shm_local: local,
                        shm_size: size,
                    };
                    libnanami::print!("[block-device] shm attached pid={}", request.identifier);
                    libnanami::print!(" local={:#x} peer={:#x}\n", local, peer);
                    (libnanami::OS_RESPONSE_OK, peer, size)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::block::BLOCK_DEVICE_CONTROL_GET_INFO => (
            libnanami::OS_RESPONSE_OK,
            BLOCK_SIZE as Word,
            BLOCK_COUNT as Word,
        ),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_read(request: ServiceRequest, session: &ClientSession) -> (Word, Word, Word) {
    if !session.active || session.pid != request.identifier {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let block = request.arg0 as usize;
    let count = request.arg1 as usize;
    let offset = request.arg2 as usize;
    if count == 0 || block >= BLOCK_COUNT || block + count > BLOCK_COUNT {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let bytes = count * BLOCK_SIZE;
    if offset + bytes > session.shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            ramdisk_ptr().add(block * BLOCK_SIZE),
            (session.shm_local as usize + offset) as *mut u8,
            bytes,
        );
    }
    (libnanami::OS_RESPONSE_OK, bytes as Word, 0)
}

fn handle_write(request: ServiceRequest, session: &ClientSession) -> (Word, Word, Word) {
    if !session.active || session.pid != request.identifier {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let block = request.arg0 as usize;
    let count = request.arg1 as usize;
    let offset = request.arg2 as usize;
    if count == 0 || block >= BLOCK_COUNT || block + count > BLOCK_COUNT {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let bytes = count * BLOCK_SIZE;
    if offset + bytes > session.shm_size as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    unsafe {
        ptr::copy_nonoverlapping(
            (session.shm_local as usize + offset) as *const u8,
            ramdisk_mut_ptr().add(block * BLOCK_SIZE),
            bytes,
        );
    }
    (libnanami::OS_RESPONSE_OK, bytes as Word, 0)
}

unsafe fn init_ramdisk() {
    ptr::write_bytes(ramdisk_mut_ptr(), 0, RAMDISK_BYTES);
    init_superblock();
    init_group_descriptor();
    init_bitmaps();
    init_inodes();
    init_root_dir();
    init_docs_dir();
    write_bytes(HELLO_BLOCK * BLOCK_SIZE, HELLO_TEXT);
    write_bytes(README_BLOCK * BLOCK_SIZE, README_TEXT);
}

unsafe fn init_superblock() {
    let base = BLOCK_SIZE;
    w32(base + 0, INODE_COUNT as u32);
    w32(base + 4, BLOCK_COUNT as u32);
    w32(base + 12, (BLOCK_COUNT - 13) as u32);
    w32(base + 16, (INODE_COUNT - README_INODE as usize) as u32);
    w32(base + 20, 1); // first data block for 1KiB ext2
    w32(base + 24, 0); // 1024-byte block
    w32(base + 32, BLOCK_COUNT as u32);
    w32(base + 40, INODE_COUNT as u32);
    w16(base + 56, 0xef53);
    w16(base + 58, 1);
    w32(base + 76, 1);
    w32(base + 84, 11);
    w16(base + 88, INODE_SIZE as u16);
}

unsafe fn init_group_descriptor() {
    let base = 2 * BLOCK_SIZE;
    w32(base + 0, 3);
    w32(base + 4, 4);
    w32(base + 8, INODE_TABLE_BLOCK as u32);
    w16(base + 12, (BLOCK_COUNT - 13) as u16);
    w16(base + 14, (INODE_COUNT - README_INODE as usize) as u16);
    w16(base + 16, 2);
}

unsafe fn init_bitmaps() {
    for block in 0..=README_BLOCK {
        set_bitmap_bit(3 * BLOCK_SIZE, block);
    }
    for inode in 1..=README_INODE as usize {
        set_bitmap_bit(4 * BLOCK_SIZE, inode - 1);
    }
}

unsafe fn init_inodes() {
    write_inode(ROOT_INODE, 0x41ed, BLOCK_SIZE as u32, ROOT_DIR_BLOCK as u32);
    write_inode(DOCS_INODE, 0x41ed, BLOCK_SIZE as u32, DOCS_DIR_BLOCK as u32);
    write_inode(
        HELLO_INODE,
        0x81a4,
        HELLO_TEXT.len() as u32,
        HELLO_BLOCK as u32,
    );
    write_inode(
        README_INODE,
        0x81a4,
        README_TEXT.len() as u32,
        README_BLOCK as u32,
    );
}

unsafe fn init_root_dir() {
    let base = ROOT_DIR_BLOCK * BLOCK_SIZE;
    write_dirent(base, ROOT_INODE, 12, 1, 2, b".");
    write_dirent(base + 12, ROOT_INODE, 12, 2, 2, b"..");
    write_dirent(base + 24, DOCS_INODE, 12, 4, 2, b"docs");
    write_dirent(
        base + 36,
        HELLO_INODE,
        (BLOCK_SIZE - 36) as u16,
        9,
        1,
        b"hello.txt",
    );
}

unsafe fn init_docs_dir() {
    let base = DOCS_DIR_BLOCK * BLOCK_SIZE;
    write_dirent(base, DOCS_INODE, 12, 1, 2, b".");
    write_dirent(base + 12, ROOT_INODE, 12, 2, 2, b"..");
    write_dirent(
        base + 24,
        README_INODE,
        (BLOCK_SIZE - 24) as u16,
        10,
        1,
        b"readme.txt",
    );
}

unsafe fn write_inode(inode: u32, mode: u16, size: u32, block0: u32) {
    let base = INODE_TABLE_BLOCK * BLOCK_SIZE + ((inode as usize - 1) * INODE_SIZE);
    w16(base + 0, mode);
    w16(base + 2, 1000);
    w32(base + 4, size);
    w32(base + 24, 2); // sectors
    w32(base + 40, block0);
}

unsafe fn write_dirent(
    offset: usize,
    inode: u32,
    rec_len: u16,
    name_len: u8,
    file_type: u8,
    name: &[u8],
) {
    w32(offset + 0, inode);
    w16(offset + 4, rec_len);
    RAMDISK[offset + 6] = name_len;
    RAMDISK[offset + 7] = file_type;
    write_bytes(offset + 8, name);
}

unsafe fn set_bitmap_bit(base: usize, bit: usize) {
    RAMDISK[base + bit / 8] |= 1 << (bit % 8);
}

unsafe fn write_bytes(offset: usize, data: &[u8]) {
    let n = min(data.len(), RAMDISK_BYTES - offset);
    ptr::copy_nonoverlapping(data.as_ptr(), ramdisk_mut_ptr().add(offset), n);
}

unsafe fn ramdisk_ptr() -> *const u8 {
    core::ptr::addr_of!(RAMDISK) as *const u8
}

unsafe fn ramdisk_mut_ptr() -> *mut u8 {
    core::ptr::addr_of_mut!(RAMDISK) as *mut u8
}

unsafe fn w16(offset: usize, value: u16) {
    RAMDISK[offset] = value as u8;
    RAMDISK[offset + 1] = (value >> 8) as u8;
}

unsafe fn w32(offset: usize, value: u32) {
    RAMDISK[offset] = value as u8;
    RAMDISK[offset + 1] = (value >> 8) as u8;
    RAMDISK[offset + 2] = (value >> 16) as u8;
    RAMDISK[offset + 3] = (value >> 24) as u8;
}

fn log_reg_error(e: RequestError) -> libnanami::NanamiError {
    log_request_error("[block-device] register failed: ", e);
    e.into()
}

fn log_error(prefix: &str, e: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, e);
    e.into()
}

fn log_request_error(prefix: &str, e: RequestError) {
    libnanami::print!("{}{}\n", prefix, e);
}

fn map_request_error_to_status(e: RequestError) -> Word {
    match e {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}

libnanami::nanami_entry!(nanami_main);
