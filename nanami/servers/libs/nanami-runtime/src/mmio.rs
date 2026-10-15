use crate::{call_os_port, Attribute, RequestError, Word, OS_REQUEST_MMIO_REQUEST, OS_RESPONSE_OK};

pub fn request_mmio(
    physical_address: Word,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    request_mmio_with_mapping(physical_address, size_bytes, Attribute::UNCACHED)
}

pub fn request_mmio_with_mapping(
    physical_address: Word,
    size_bytes: Word,
    mapping: Attribute,
) -> Result<(Word, Word), RequestError> {
    if physical_address == 0 || size_bytes == 0 {
        return Err(RequestError::InvalidArgument);
    }
    if mapping != Attribute::NORMAL
        && mapping != Attribute::BUFFERED
        && mapping != Attribute::UNCACHED
    {
        return Err(RequestError::InvalidArgument);
    }
    let (status, paddr, vaddr) = call_os_port(
        OS_REQUEST_MMIO_REQUEST,
        physical_address,
        size_bytes,
        mapping.bits(),
        0,
        4, // code + paddr + bytes + mapping (arg2), all in IPC registers.
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((paddr, vaddr))
}
