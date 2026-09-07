use libnanami::Word;

pub const ELF_MACHINE: u16 = 0x3e;
pub const PLATFORM: &[u8] = b"x86_64";
pub const PLATFORM_NUL: &[u8] = b"x86_64\0";
pub const UNAME_MACHINE: &[u8] = b"x86_64";
pub const PROC_VERSION: &[u8] = b"Linux version 6.1.0-alter (Nanami/A9N) x86_64\n";
pub const CPU_INFO: &[u8] =
    b"processor\t: 0\nvendor_id\t: A9N Project\nmodel name\t: Alter virtual x86_64 processor\n";
pub const STAT_SIZE: usize = 144;

pub const fn clone_child_tid(args: [Word; 6]) -> Word {
    args[3]
}

pub const fn clone_tls(args: [Word; 6]) -> Word {
    args[4]
}

pub unsafe fn write_stat_buffer(base: Word, inode: Word, size: Word, mode: Word, rdev: Word) {
    unsafe {
        core::ptr::write_bytes(base as *mut u8, 0, STAT_SIZE);
        write_u64(base, 0);
        write_u64(base + 8, inode);
        write_u64(base + 16, 1);
        write_u32(base + 24, mode as u32);
        write_u32(base + 28, 0);
        write_u32(base + 32, 0);
        write_u64(base + 40, rdev);
        write_u64(base + 48, size);
        write_u64(base + 56, 4096);
        write_u64(base + 64, align_up(size, 512) / 512);
    }
}

const fn align_up(value: Word, align: Word) -> Word {
    value.saturating_add(align - 1) & !(align - 1)
}

unsafe fn write_u32(address: Word, value: u32) {
    unsafe { core::ptr::write_unaligned(address as *mut u32, value) };
}

unsafe fn write_u64(address: Word, value: Word) {
    unsafe { core::ptr::write_unaligned(address as *mut Word, value) };
}
