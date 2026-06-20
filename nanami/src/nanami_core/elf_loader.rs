use nun::CapabilityError;

const ELF64_HEADER_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;
const ELF64_SHDR_SIZE: usize = 64;
const ELF64_SYM_SIZE: usize = 24;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_DYNSYM: u32 = 11;
const MAX_LOAD_SEGMENTS: usize = 16;
const IPC_BUFFER_SYMBOL: &[u8] = b"__ipc_buffer_start";

#[derive(Clone, Copy)]
pub struct LoadSegment {
    pub offset: usize,
    pub virtual_address: usize,
    pub file_size: usize,
    pub memory_size: usize,
}

#[derive(Clone, Copy)]
pub struct ElfImage {
    pub entry_point: usize,
    pub segments: [LoadSegment; MAX_LOAD_SEGMENTS],
    pub segment_count: usize,
    pub ipc_buffer_start: Option<usize>,
}

impl ElfImage {
    pub const fn empty() -> Self {
        Self {
            entry_point: 0,
            segments: [LoadSegment {
                offset: 0,
                virtual_address: 0,
                file_size: 0,
                memory_size: 0,
            }; MAX_LOAD_SEGMENTS],
            segment_count: 0,
            ipc_buffer_start: None,
        }
    }
}

pub fn parse_elf64(image: &[u8]) -> Result<ElfImage, CapabilityError> {
    if image.len() < ELF64_HEADER_SIZE {
        return Err(CapabilityError::InvalidArgument);
    }
    if image[0] != 0x7f || image[1] != b'E' || image[2] != b'L' || image[3] != b'F' {
        return Err(CapabilityError::InvalidArgument);
    }
    if image[4] != 2 || image[5] != 1 {
        return Err(CapabilityError::InvalidArgument);
    }

    let entry = read_u64(image, 24)? as usize;
    let phoff = read_u64(image, 32)? as usize;
    let shoff = read_u64(image, 40)? as usize;
    let phentsize = read_u16(image, 54)? as usize;
    let phnum = read_u16(image, 56)? as usize;
    let shentsize = read_u16(image, 58)? as usize;
    let shnum = read_u16(image, 60)? as usize;

    if phentsize < ELF64_PHDR_SIZE {
        return Err(CapabilityError::InvalidArgument);
    }
    if phoff >= image.len() {
        return Err(CapabilityError::InvalidArgument);
    }

    let mut out = ElfImage::empty();
    out.entry_point = entry;

    let mut i = 0usize;
    while i < phnum {
        let base = phoff + i * phentsize;
        if base + ELF64_PHDR_SIZE > image.len() {
            return Err(CapabilityError::InvalidArgument);
        }

        let p_type = read_u32(image, base)?;
        if p_type == PT_LOAD {
            if out.segment_count >= MAX_LOAD_SEGMENTS {
                return Err(CapabilityError::InvalidArgument);
            }

            let offset = read_u64(image, base + 8)? as usize;
            let vaddr = read_u64(image, base + 16)? as usize;
            let filesz = read_u64(image, base + 32)? as usize;
            let memsz = read_u64(image, base + 40)? as usize;
            if memsz < filesz {
                return Err(CapabilityError::InvalidArgument);
            }
            if offset
                .checked_add(filesz)
                .filter(|end| *end <= image.len())
                .is_none()
            {
                return Err(CapabilityError::InvalidArgument);
            }

            out.segments[out.segment_count] = LoadSegment {
                offset,
                virtual_address: vaddr,
                file_size: filesz,
                memory_size: memsz,
            };
            out.segment_count += 1;
        }
        i += 1;
    }

    if out.segment_count == 0 {
        return Err(CapabilityError::InvalidArgument);
    }

    if shoff < image.len() && shentsize >= ELF64_SHDR_SIZE && shnum > 0 {
        out.ipc_buffer_start =
            find_symbol_value(image, shoff, shentsize, shnum, IPC_BUFFER_SYMBOL)?;
    }

    Ok(out)
}

fn find_symbol_value(
    image: &[u8],
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    target_name: &[u8],
) -> Result<Option<usize>, CapabilityError> {
    let mut section = 0usize;
    while section < shnum {
        let sh_base = shoff + section * shentsize;
        if sh_base + ELF64_SHDR_SIZE > image.len() {
            return Err(CapabilityError::InvalidArgument);
        }

        let sh_type = read_u32(image, sh_base + 4)?;
        if sh_type != SHT_SYMTAB && sh_type != SHT_DYNSYM {
            section += 1;
            continue;
        }

        let sym_offset = read_u64(image, sh_base + 24)? as usize;
        let sym_size = read_u64(image, sh_base + 32)? as usize;
        let sym_entsize = read_u64(image, sh_base + 56)? as usize;
        let linked_strtab_index = read_u32(image, sh_base + 40)? as usize;
        if sym_offset >= image.len()
            || sym_offset + sym_size > image.len()
            || sym_entsize < ELF64_SYM_SIZE
            || linked_strtab_index >= shnum
        {
            return Err(CapabilityError::InvalidArgument);
        }

        let str_sh_base = shoff + linked_strtab_index * shentsize;
        if str_sh_base + ELF64_SHDR_SIZE > image.len() {
            return Err(CapabilityError::InvalidArgument);
        }

        let str_offset = read_u64(image, str_sh_base + 24)? as usize;
        let str_size = read_u64(image, str_sh_base + 32)? as usize;
        if str_offset >= image.len() || str_offset + str_size > image.len() {
            return Err(CapabilityError::InvalidArgument);
        }

        let sym_count = sym_size / sym_entsize;
        let mut i = 0usize;
        while i < sym_count {
            let sym_base = sym_offset + i * sym_entsize;
            if sym_base + ELF64_SYM_SIZE > image.len() {
                return Err(CapabilityError::InvalidArgument);
            }

            let name_offset = read_u32(image, sym_base)? as usize;
            if name_offset >= str_size {
                i += 1;
                continue;
            }

            let symbol_name = read_cstr(&image[str_offset + name_offset..str_offset + str_size]);
            if symbol_name == target_name {
                let value = read_u64(image, sym_base + 8)? as usize;
                return Ok(Some(value));
            }

            i += 1;
        }

        section += 1;
    }

    Ok(None)
}

fn read_cstr(data: &[u8]) -> &[u8] {
    let mut i = 0usize;
    while i < data.len() {
        if data[i] == 0 {
            return &data[..i];
        }
        i += 1;
    }
    data
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, CapabilityError> {
    if offset + 2 > data.len() {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, CapabilityError> {
    if offset + 4 > data.len() {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, CapabilityError> {
    if offset + 8 > data.len() {
        return Err(CapabilityError::InvalidArgument);
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
