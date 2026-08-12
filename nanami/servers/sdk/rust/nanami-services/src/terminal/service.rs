use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::*;

pub fn terminal_attach_shared_memory(
    service_port: CapabilityDescriptor,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, local_vaddr, mapped_size) = call_port(
        service_port,
        TERMINAL_REQUEST_CONTROL,
        TERMINAL_CONTROL_ATTACH_SHARED_MEMORY,
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

pub fn terminal_create(
    service_port: CapabilityDescriptor,
    columns: Word,
    rows: Word,
) -> Result<Word, RequestError> {
    let (status, terminal_id, _) = call_port(
        service_port,
        TERMINAL_REQUEST_CREATE,
        columns,
        rows,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(terminal_id)
}

pub fn terminal_write_input(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    terminal_write(
        service_port,
        TERMINAL_REQUEST_WRITE_INPUT,
        terminal_id,
        offset,
        len,
    )
}

pub fn terminal_read_input(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    terminal_read(
        service_port,
        TERMINAL_REQUEST_READ_INPUT,
        terminal_id,
        offset,
        max_len,
    )
}

pub fn terminal_write_output(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    terminal_write(
        service_port,
        TERMINAL_REQUEST_WRITE_OUTPUT,
        terminal_id,
        offset,
        len,
    )
}

pub fn terminal_read_output(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    terminal_read(
        service_port,
        TERMINAL_REQUEST_READ_OUTPUT,
        terminal_id,
        offset,
        max_len,
    )
}

pub fn terminal_get_size(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, columns, rows) = call_port(
        service_port,
        TERMINAL_REQUEST_GET_SIZE,
        terminal_id,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((columns, rows))
}

pub fn terminal_attach_output_notification(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    source_notification_slot: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        TERMINAL_REQUEST_ATTACH_OUTPUT_NOTIFICATION,
        terminal_id,
        source_notification_slot,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn terminal_attach_input_notification(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    source_notification_slot: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        TERMINAL_REQUEST_ATTACH_INPUT_NOTIFICATION,
        terminal_id,
        source_notification_slot,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn terminal_clear(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    flags: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        TERMINAL_REQUEST_CLEAR,
        terminal_id,
        flags,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn terminal_set_echo(
    service_port: CapabilityDescriptor,
    terminal_id: Word,
    enabled: bool,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        TERMINAL_REQUEST_SET_ECHO,
        terminal_id,
        enabled as Word,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

fn terminal_write(
    service_port: CapabilityDescriptor,
    code: Word,
    terminal_id: Word,
    offset: Word,
    len: Word,
) -> Result<Word, RequestError> {
    let (status, written, _) = call_port(service_port, code, terminal_id, offset, len, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(written)
}

fn terminal_read(
    service_port: CapabilityDescriptor,
    code: Word,
    terminal_id: Word,
    offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    let (status, bytes, _) = call_port(service_port, code, terminal_id, offset, max_len, 0, 4)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(bytes)
}
