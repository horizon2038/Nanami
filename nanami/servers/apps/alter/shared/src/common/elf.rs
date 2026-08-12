use libnanami::Word;

const ELF64_HEADER_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;

#[derive(Clone, Copy)]
pub struct LoadSegment {
    pub offset: Word,
    pub virtual_address: Word,
    pub file_size: Word,
    pub memory_size: Word,
    pub flags: Word,
}

#[derive(Clone, Copy)]
pub struct ElfMetadata {
    pub elf_type: Word,
    pub entry_point: Word,
    pub program_header_offset: Word,
    pub program_header_entry_size: Word,
    pub program_header_count: Word,
    pub load_segment_count: Word,
    pub has_interpreter: bool,
    pub first_load: LoadSegment,
    pub program_header_vaddr: Word,
    pub tls_vaddr: Word,
    pub tls_file_size: Word,
    pub tls_memory_size: Word,
    pub tls_align: Word,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    Invalid,
    Unsupported,
}

pub fn parse_elf64_header(image: &[u8]) -> Result<ElfMetadata, ElfError> {
    if image.len() < ELF64_HEADER_SIZE {
        return Err(ElfError::Invalid);
    }
    if image[0] != 0x7f || image[1] != b'E' || image[2] != b'L' || image[3] != b'F' {
        return Err(ElfError::Invalid);
    }
    if image[4] != 2 || image[5] != 1 {
        return Err(ElfError::Unsupported);
    }
    let elf_type = read_u16(image, 16)?;
    if elf_type != 2 && elf_type != 3 {
        return Err(ElfError::Unsupported);
    }
    if read_u16(image, 18)? != 0x3e {
        return Err(ElfError::Unsupported);
    }

    let entry_point = read_u64(image, 24)? as Word;
    let phoff = read_u64(image, 32)? as usize;
    let phentsize = read_u16(image, 54)? as usize;
    let phnum = read_u16(image, 56)? as usize;
    if phentsize < ELF64_PHDR_SIZE {
        return Err(ElfError::Invalid);
    }
    let ph_end = phoff
        .checked_add(phentsize.saturating_mul(phnum))
        .ok_or(ElfError::Invalid)?;
    if ph_end > image.len() {
        return Err(ElfError::Invalid);
    }

    let mut load_count = 0usize;
    let mut has_interpreter = false;
    let mut first_load = LoadSegment {
        offset: 0,
        virtual_address: 0,
        file_size: 0,
        memory_size: 0,
        flags: 0,
    };
    let mut program_header_vaddr = 0;
    let mut tls_vaddr = 0;
    let mut tls_file_size = 0;
    let mut tls_memory_size = 0;
    let mut tls_align = 0;

    let mut i = 0usize;
    while i < phnum {
        let base = phoff + i * phentsize;
        let p_type = read_u32(image, base)?;
        if p_type == PT_LOAD {
            let segment = LoadSegment {
                flags: read_u32(image, base + 4)? as Word,
                offset: read_u64(image, base + 8)? as Word,
                virtual_address: read_u64(image, base + 16)? as Word,
                file_size: read_u64(image, base + 32)? as Word,
                memory_size: read_u64(image, base + 40)? as Word,
            };
            if segment.memory_size < segment.file_size {
                return Err(ElfError::Invalid);
            }
            if load_count == 0 {
                first_load = segment;
            }
            if phoff as Word >= segment.offset
                && (phoff as Word) < segment.offset.saturating_add(segment.file_size)
            {
                program_header_vaddr =
                    segment.virtual_address + ((phoff as Word).saturating_sub(segment.offset));
            }
            load_count += 1;
        } else if p_type == PT_INTERP {
            has_interpreter = true;
        } else if p_type == PT_PHDR {
            program_header_vaddr = read_u64(image, base + 16)? as Word;
        } else if p_type == PT_TLS {
            tls_vaddr = read_u64(image, base + 16)? as Word;
            tls_file_size = read_u64(image, base + 32)? as Word;
            tls_memory_size = read_u64(image, base + 40)? as Word;
            tls_align = read_u64(image, base + 48)? as Word;
        }
        i += 1;
    }

    if load_count == 0 {
        return Err(ElfError::Invalid);
    }

    Ok(ElfMetadata {
        elf_type: elf_type as Word,
        entry_point,
        program_header_offset: phoff as Word,
        program_header_entry_size: phentsize as Word,
        program_header_count: phnum as Word,
        load_segment_count: load_count as Word,
        has_interpreter,
        first_load,
        program_header_vaddr,
        tls_vaddr,
        tls_file_size,
        tls_memory_size,
        tls_align,
    })
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, ElfError> {
    if offset + 2 > data.len() {
        return Err(ElfError::Invalid);
    }
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, ElfError> {
    if offset + 4 > data.len() {
        return Err(ElfError::Invalid);
    }
    Ok(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, ElfError> {
    if offset + 8 > data.len() {
        return Err(ElfError::Invalid);
    }
    Ok(u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ]))
}
