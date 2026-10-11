use super::constants::*;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
use a9n_abi::CapabilityDescriptor;

/// Positioned I/O never changes the shared open-file-description offset.
pub fn posix_pread(
    port: CapabilityDescriptor,
    fd: Word,
    out: Word,
    len: Word,
    offset: Word,
) -> Result<Word, RequestError> {
    positioned_io(port, POSIX_REQUEST_PREAD, fd, out, len, offset)
}

pub fn posix_pread_direct(
    port: CapabilityDescriptor,
    fd: Word,
    out: Word,
    len: Word,
    offset: Word,
) -> Result<Word, RequestError> {
    positioned_io(port, POSIX_REQUEST_PREAD_DIRECT, fd, out, len, offset)
}

pub fn posix_pwrite(
    port: CapabilityDescriptor,
    fd: Word,
    input: Word,
    len: Word,
    offset: Word,
) -> Result<Word, RequestError> {
    positioned_io(port, POSIX_REQUEST_PWRITE, fd, input, len, offset)
}

pub fn posix_pwrite_direct(
    port: CapabilityDescriptor,
    fd: Word,
    input: Word,
    len: Word,
    offset: Word,
) -> Result<Word, RequestError> {
    positioned_io(port, POSIX_REQUEST_PWRITE_DIRECT, fd, input, len, offset)
}

fn positioned_io(
    port: CapabilityDescriptor,
    code: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    offset: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(port, code, fd, buffer, len, offset, 5)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn posix_read(
    service_port: CapabilityDescriptor,
    fd: Word,
    out_offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) =
        call_port(service_port, POSIX_REQUEST_READ, fd, out_offset, len, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn posix_read_direct(
    service_port: CapabilityDescriptor,
    fd: Word,
    out_offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        POSIX_REQUEST_READ_DIRECT,
        fd,
        out_offset,
        len,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn posix_write(
    service_port: CapabilityDescriptor,
    fd: Word,
    input_offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        POSIX_REQUEST_WRITE,
        fd,
        input_offset,
        len,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}

pub fn posix_write_direct(
    service_port: CapabilityDescriptor,
    fd: Word,
    input_offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(
        service_port,
        POSIX_REQUEST_WRITE_DIRECT,
        fd,
        input_offset,
        len,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}
