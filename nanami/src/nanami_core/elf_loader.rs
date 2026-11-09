use nun::CapabilityError;

#[path = "elf_loader/staging.rs"]
pub mod staging;
#[path = "elf_loader/page.rs"]
pub mod page;

/// Reads only the bytes needed by the parser into request-local storage.
pub trait ElfReader {
    fn len(&self) -> usize;
    fn read(&mut self, offset: usize, length: usize) -> Result<&[u8], CapabilityError>;
}

impl ElfReader for &[u8] {
    fn len(&self) -> usize { <[u8]>::len(self) }
    fn read(&mut self, offset: usize, length: usize) -> Result<&[u8], CapabilityError> {
        let end = offset.checked_add(length).ok_or(CapabilityError::InvalidArgument)?;
        self.get(offset..end).ok_or(CapabilityError::InvalidArgument)
    }
}

const ELF64_HEADER_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;
const ELF64_SHDR_SIZE: usize = 64;
const ELF64_SYM_SIZE: usize = 24;
const ET_DYN: u16 = 3;
#[cfg(target_arch = "x86_64")]
const ELF_MACHINE: u16 = 0x3e;
#[cfg(target_arch = "aarch64")]
const ELF_MACHINE: u16 = 0xb7;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_DYNSYM: u32 = 11;
const MAX_LOAD_SEGMENTS: usize = 16;
const IPC_BUFFER_SYMBOL: &[u8] = b"__ipc_buffer_start";
const DYNAMIC_IMAGE_LOAD_BIAS: usize = 0x400000;

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
    pub segment_permissions: [u8; MAX_LOAD_SEGMENTS],
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
            segment_permissions: [7; MAX_LOAD_SEGMENTS],
            ipc_buffer_start: None,
        }
    }

    /// Only preinstall rights for pages belonging to one LOAD. Shared boundary
    /// pages keep the legacy mapping until the supervisor applies LOADs in order.
    pub fn spawn_page_permissions(&self, page: usize, page_size: usize) -> usize {
        let Some(end) = page.checked_add(page_size) else { return 7; };
        let mut permissions = None;
        for (index, segment) in self.segments[..self.segment_count].iter().enumerate() {
            let Some(segment_end) = segment.virtual_address.checked_add(segment.memory_size)
                else { return 7; };
            if segment.memory_size == 0 || page >= segment_end || end <= segment.virtual_address {
                continue;
            }
            if permissions.is_some() { return 7; }
            permissions = Some(self.segment_permissions[index] as usize);
        }
        // A zero-rights LOAD and gaps retain legacy behavior until track().
        permissions.filter(|rights| *rights != 0).unwrap_or(7)
    }

    pub fn exec_page_permissions(&self, sealed: bool, page: usize, page_size: usize) -> usize {
        if sealed && self.ipc_buffer_start.is_none() {
            self.spawn_page_permissions(page, page_size)
        } else {
            7
        }
    }

    /// Share only a complete file page supplied by exactly one LOAD. The
    /// ordinary fill path owns padding, BSS, overlapping LOADs and native TLS.
    pub fn snapshot_page(&self, page: usize, page_size: usize) -> Option<(usize, usize)> {
        if self.ipc_buffer_start.is_some() || page_size == 0 { return None; }
        let end = page.checked_add(page_size)?;
        let mut candidate = None;
        for (index, segment) in self.segments[..self.segment_count].iter().enumerate() {
            let memory_end = segment.virtual_address.checked_add(segment.memory_size)?;
            if segment.memory_size == 0 || page >= memory_end || end <= segment.virtual_address {
                continue;
            }
            if candidate.is_some() { return None; }
            let rights = self.segment_permissions[index] as usize;
            if !matches!(rights, 1 | 5) || page < segment.virtual_address
                || end > segment.virtual_address.checked_add(segment.file_size)?
            { return None; }
            let offset = segment.offset.checked_add(page - segment.virtual_address)?;
            if offset % page_size != 0 { return None; }
            candidate = Some((offset, rights));
        }
        candidate
    }
}

pub fn parse_elf64(image: &[u8]) -> Result<ElfImage, CapabilityError> {
    parse_elf64_reader(&mut &*image)
}

pub fn parse_elf64_header(image: &[u8]) -> Result<ElfImage, CapabilityError> {
    parse_elf64_impl(&mut &*image, false)
}

/// Resolve a linker-defined boot symbol using the same load bias as PT_LOAD.
pub fn symbol_value(image: &[u8], name: &[u8]) -> Result<Option<usize>, CapabilityError> {
    symbol_value_reader(&mut &*image, name)
}

fn symbol_value_reader(image: &mut impl ElfReader, name: &[u8]) -> Result<Option<usize>, CapabilityError> {
    if image.len() < ELF64_HEADER_SIZE {
        return Err(CapabilityError::InvalidArgument);
    }
    let bias = if read_u16(image, 16)? == ET_DYN {
        DYNAMIC_IMAGE_LOAD_BIAS
    } else {
        0
    };
    let offset = read_u64(image, 40)? as usize;
    let stride = read_u16(image, 58)? as usize;
    let count = read_u16(image, 60)? as usize;
    if stride < ELF64_SHDR_SIZE || count == 0 {
        return Ok(None);
    }
    find_symbol_value(image, offset, stride, count, name)?
        .map(|value| {
            value
                .checked_add(bias)
                .ok_or(CapabilityError::InvalidArgument)
        })
        .transpose()
}

pub fn parse_elf64_reader(image: &mut impl ElfReader) -> Result<ElfImage, CapabilityError> {
    parse_elf64_impl(image, true)
}

fn parse_elf64_impl(image: &mut impl ElfReader, require_full_image: bool) -> Result<ElfImage, CapabilityError> {
    if image.len() < ELF64_HEADER_SIZE {
        return Err(CapabilityError::InvalidArgument);
    }
    let magic = image.read(0, 6)?;
    if magic[0] != 0x7f || magic[1] != b'E' || magic[2] != b'L' || magic[3] != b'F' {
        return Err(CapabilityError::InvalidArgument);
    }
    if magic[4] != 2 || magic[5] != 1 {
        return Err(CapabilityError::InvalidArgument);
    }
    if read_u16(image, 18)? != ELF_MACHINE {
        return Err(CapabilityError::InvalidArgument);
    }

    let elf_type = read_u16(image, 16)?;
    let load_bias = if elf_type == ET_DYN {
        DYNAMIC_IMAGE_LOAD_BIAS
    } else {
        0
    };
    let entry = (read_u64(image, 24)? as usize)
        .checked_add(load_bias)
        .ok_or(CapabilityError::InvalidArgument)?;
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
    checked_table(phoff, phnum, phentsize, image.len())?;

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
            let vaddr = (read_u64(image, base + 16)? as usize)
                .checked_add(load_bias)
                .ok_or(CapabilityError::InvalidArgument)?;
            let filesz = read_u64(image, base + 32)? as usize;
            let memsz = read_u64(image, base + 40)? as usize;
            if memsz < filesz {
                return Err(CapabilityError::InvalidArgument);
            }
            vaddr
                .checked_add(memsz)
                .ok_or(CapabilityError::InvalidArgument)?;
            let segment_file_end = offset
                .checked_add(filesz)
                .ok_or(CapabilityError::InvalidArgument)?;
            if require_full_image && segment_file_end > image.len() {
                return Err(CapabilityError::InvalidArgument);
            }

            out.segments[out.segment_count] = LoadSegment {
                offset,
                virtual_address: vaddr,
                file_size: filesz,
                memory_size: memsz,
            };
            let flags = read_u32(image, base + 4)?;
            out.segment_permissions[out.segment_count] =
                (((flags & 4) >> 2) | (flags & 2) | ((flags & 1) << 2)) as u8;
            out.segment_count += 1;
        }
        i += 1;
    }

    if out.segment_count == 0 {
        return Err(CapabilityError::InvalidArgument);
    }

    if require_full_image && shoff < image.len() && shentsize >= ELF64_SHDR_SIZE && shnum > 0 {
        out.ipc_buffer_start =
            find_symbol_value(image, shoff, shentsize, shnum, IPC_BUFFER_SYMBOL)?
                .map(|value| {
                    value
                        .checked_add(load_bias)
                        .ok_or(CapabilityError::InvalidArgument)
                })
                .transpose()?;
    }

    Ok(out)
}

fn find_symbol_value(
    image: &mut impl ElfReader,
    shoff: usize,
    shentsize: usize,
    shnum: usize,
    target_name: &[u8],
) -> Result<Option<usize>, CapabilityError> {
    checked_table(shoff, shnum, shentsize, image.len())?;
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
            || sym_offset
                .checked_add(sym_size)
                .is_none_or(|end| end > image.len())
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
        if str_offset >= image.len()
            || str_offset
                .checked_add(str_size)
                .is_none_or(|end| end > image.len())
        {
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

            let length = (str_size - name_offset).min(target_name.len().saturating_add(1));
            if symbol_name_matches(image.read(str_offset + name_offset, length)?, target_name) {
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

fn symbol_name_matches(data: &[u8], target: &[u8]) -> bool {
    // One byte beyond the target distinguishes an exact name from a longer
    // prefix match. A name ending at the string table's end remains accepted
    // without a NUL, exactly as read_cstr did for the complete remaining table.
    let limit = data.len().min(target.len().saturating_add(1));
    read_cstr(&data[..limit]) == target
}

#[cfg(test)]
pub(crate) fn symbol_name_matches_for_test(data: &[u8], target: &[u8]) -> bool {
    symbol_name_matches(data, target)
}

fn checked_table(
    offset: usize,
    count: usize,
    stride: usize,
    length: usize,
) -> Result<(), CapabilityError> {
    let end = count
        .checked_mul(stride)
        .and_then(|bytes| offset.checked_add(bytes))
        .ok_or(CapabilityError::InvalidArgument)?;
    if end > length {
        return Err(CapabilityError::InvalidArgument);
    }
    Ok(())
}

fn read_u16(data: &mut impl ElfReader, offset: usize) -> Result<u16, CapabilityError> {
    let bytes = data.read(offset, 2)?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u32(data: &mut impl ElfReader, offset: usize) -> Result<u32, CapabilityError> {
    let bytes = data.read(offset, 4)?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u64(data: &mut impl ElfReader, offset: usize) -> Result<u64, CapabilityError> {
    let bytes = data.read(offset, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}
