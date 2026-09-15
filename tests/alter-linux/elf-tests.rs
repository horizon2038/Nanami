//! Host-side tests of the ELF parser actually used by Alter.
extern crate self as libnanami;
pub type Word = usize;
mod arch {
    pub const ELF_MACHINE: u16 = 62;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/elf.rs"]
mod elf;

fn put16(image: &mut [u8], offset: usize, value: u16) {
    image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(image: &mut [u8], offset: usize, value: u64) {
    image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn executable() -> Vec<u8> {
    let mut image = vec![0; 512];
    image[..6].copy_from_slice(b"\x7fELF\x02\x01");
    put16(&mut image, 16, 3);
    put16(&mut image, 18, 62);
    put64(&mut image, 24, 256);
    put64(&mut image, 32, 64);
    put16(&mut image, 54, 56);
    put16(&mut image, 56, 2);
    put32(&mut image, 64, 1);
    put32(&mut image, 68, 5);
    put64(&mut image, 96, 512);
    put64(&mut image, 104, 4096);
    put32(&mut image, 120, 3);
    put64(&mut image, 128, 192);
    put64(&mut image, 152, 28);
    image[192..220].copy_from_slice(b"/lib64/ld-linux-x86-64.so.2\0");
    image
}

#[test]
fn records_main_headers_and_interpreter() {
    let metadata = elf::parse_elf64_header(&executable()).unwrap();
    assert!(metadata.has_interpreter);
    assert_eq!(metadata.interpreter_offset, 192);
    assert_eq!(metadata.interpreter_size, 28);
    assert_eq!(metadata.program_header_vaddr, 64);
    assert_eq!(metadata.load_segment_count, 1);
    assert_eq!(metadata.segments[0].memory_size, 4096);
}
#[test]
fn static_elf_has_no_interpreter() {
    let mut image = executable();
    put32(&mut image, 120, 0);
    assert!(!elf::parse_elf64_header(&image).unwrap().has_interpreter);
}
#[test]
fn rejects_duplicate_interpreter() {
    let mut image = executable();
    put16(&mut image, 56, 3);
    image.copy_within(120..176, 176);
    assert!(elf::parse_elf64_header(&image).is_err());
}
#[test]
fn rejects_short_or_oversized_interpreter() {
    for size in [0, 1, 257, u64::MAX] {
        let mut image = executable();
        put64(&mut image, 152, size);
        assert!(elf::parse_elf64_header(&image).is_err());
    }
}
#[test]
fn rejects_segment_overflow_and_wrong_architecture() {
    let mut image = executable();
    put64(&mut image, 80, u64::MAX - 4095);
    assert!(elf::parse_elf64_header(&image).is_err());
    let mut image = executable();
    put16(&mut image, 18, 183);
    assert_eq!(
        elf::parse_elf64_header(&image).err(),
        Some(elf::ElfError::Unsupported)
    );
}
#[test]
fn rejects_truncated_program_headers_and_filesz_exceeding_memsz() {
    assert!(elf::parse_elf64_header(&executable()[..175]).is_err());
    let mut image = executable();
    put64(&mut image, 104, 1);
    assert!(elf::parse_elf64_header(&image).is_err());
}

#[test]
fn validates_complete_image_before_loading() {
    assert!(elf::parse_elf64_image(&executable()).is_ok());
    assert!(elf::parse_elf64_image(&executable()[..256]).is_err());
    let mut image = executable();
    image[219] = b'x';
    assert!(elf::parse_elf64_image(&image).is_err());
    let mut image = executable();
    image[195] = 0;
    assert!(elf::parse_elf64_image(&image).is_err());
    let mut image = executable();
    put64(&mut image, 128, 500);
    assert!(elf::parse_elf64_image(&image).is_err());
}

#[test]
fn covers_relro_coalesced_across_adjacent_load_segments() {
    let ranges = [(0x4000, 0x2000), (0x2000, 0x2000), (0x5000, 0x2000)];
    assert!(elf::ranges_cover(&ranges, 0x3000, 0x4000));
    assert!(!elf::ranges_cover(&ranges, 0x1000, 0x4000));
    assert!(!elf::ranges_cover(&ranges, 0x6000, 0x2000));
    assert!(!elf::ranges_cover(
        &[(0x2000, 0x1000), (0x4000, 0x1000)],
        0x2000,
        0x3000
    ));
    assert!(!elf::ranges_cover(&ranges, usize::MAX, 2));
}
