use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::{
    BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY, BLOCK_DEVICE_CONTROL_GET_INFO,
    BLOCK_DEVICE_REQUEST_CONTROL, BLOCK_DEVICE_REQUEST_READ, BLOCK_DEVICE_REQUEST_WRITE,
};

pub fn block_device_attach_shared_memory(
    service_port: CapabilityDescriptor,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, local_vaddr, mapped_size) = call_port(
        service_port,
        BLOCK_DEVICE_REQUEST_CONTROL,
        BLOCK_DEVICE_CONTROL_ATTACH_SHARED_MEMORY,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((local_vaddr, mapped_size))
}

pub fn block_device_info(service_port: CapabilityDescriptor) -> Result<(Word, Word), RequestError> {
    let (status, block_size, block_count) = call_port(
        service_port,
        BLOCK_DEVICE_REQUEST_CONTROL,
        BLOCK_DEVICE_CONTROL_GET_INFO,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((block_size, block_count))
}

pub fn block_device_read(
    service_port: CapabilityDescriptor,
    block_index: Word,
    block_count: Word,
    shm_offset: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        BLOCK_DEVICE_REQUEST_READ,
        block_index,
        block_count,
        shm_offset,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn block_device_write(
    service_port: CapabilityDescriptor,
    block_index: Word,
    block_count: Word,
    shm_offset: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        BLOCK_DEVICE_REQUEST_WRITE,
        block_index,
        block_count,
        shm_offset,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}
