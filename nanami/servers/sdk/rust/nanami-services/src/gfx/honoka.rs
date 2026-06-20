use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub const HONOKA_REQUEST_CREATE_WINDOW: Word = 0x7001;
pub const HONOKA_REQUEST_ATTACH_LOGICAL_FRAMEBUFFER: Word = 0x7002;
pub const HONOKA_REQUEST_MOVE_WINDOW: Word = 0x7003;
pub const HONOKA_REQUEST_SET_WINDOW_VISIBLE: Word = 0x7004;
pub const HONOKA_REQUEST_ATTACH_INPUT_QUEUE: Word = 0x7005;
pub const HONOKA_REQUEST_ATTACH_INPUT_NOTIFICATION: Word = 0x7006;
pub const HONOKA_REQUEST_SET_WINDOW_TITLE: Word = 0x7007;
pub const HONOKA_REQUEST_CREATE_WINDOW_WITH_TITLE: Word = 0x7008;
pub const HONOKA_REQUEST_GET_WINDOW_CONTENT_SIZE: Word = 0x7009;
pub const HONOKA_REQUEST_INVALIDATE_LOGICAL_FRAMEBUFFER: Word = 0x7010;
pub const HONOKA_NOTIFICATION_PRESENT: Word = 1 << 48;
pub const HONOKA_NOTIFICATION_INPUT: Word = 1 << 49;
pub const HONOKA_INPUT_FLAG_ABSOLUTE: Word = 1;
pub const HONOKA_DAMAGE_QUEUE_MAGIC: Word = 0x484f_4e4f_4b41_4451;
pub const HONOKA_DAMAGE_QUEUE_BYTES: Word = 0x1000;
pub const HONOKA_DAMAGE_QUEUE_HEADER_WORDS: usize = 5;
pub const HONOKA_DAMAGE_QUEUE_CAPACITY: usize = 64;
pub const HONOKA_DAMAGE_ENTRY_WORDS: usize = 4;
pub const HONOKA_WINDOW_TITLE_BYTES: usize = 24;
pub const HONOKA_CREATE_WINDOW_TITLE_BYTES: usize = 16;

pub fn honoka_create_window(
    honoka_port: CapabilityDescriptor,
    x: Word,
    y: Word,
    width: Word,
    height: Word,
) -> Result<Word, RequestError> {
    let (status, window_id, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_CREATE_WINDOW,
        x,
        y,
        width,
        height,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(window_id)
}

pub fn honoka_create_window_with_title(
    honoka_port: CapabilityDescriptor,
    x: Word,
    y: Word,
    width: Word,
    height: Word,
    title: &[u8],
) -> Result<Word, RequestError> {
    let geom0 = (x & 0xffff_ffff) | ((y & 0xffff_ffff) << 32);
    let geom1 = (width & 0xffff_ffff) | ((height & 0xffff_ffff) << 32);
    let mut chunks = [0usize; 2];
    let limit = title.len().min(HONOKA_CREATE_WINDOW_TITLE_BYTES);
    let mut i = 0usize;
    while i < limit {
        let chunk = i / 8;
        let shift = (i % 8) * 8;
        chunks[chunk] |= (title[i] as Word) << shift;
        i += 1;
    }
    let (status, window_id, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_CREATE_WINDOW_WITH_TITLE,
        geom0,
        geom1,
        chunks[0],
        chunks[1],
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(window_id)
}

pub fn honoka_attach_logical_framebuffer(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, client_vaddr, size_bytes) = call_port(
        honoka_port,
        HONOKA_REQUEST_ATTACH_LOGICAL_FRAMEBUFFER,
        window_id,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((client_vaddr, size_bytes))
}

pub fn honoka_get_window_content_size(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, packed_size, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_GET_WINDOW_CONTENT_SIZE,
        window_id,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((packed_size & 0xffff_ffff, (packed_size >> 32) & 0xffff_ffff))
}

pub fn honoka_move_window(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
    x: Word,
    y: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_MOVE_WINDOW,
        window_id,
        x,
        y,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn honoka_set_window_visible(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
    visible: bool,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_SET_WINDOW_VISIBLE,
        window_id,
        if visible { 1 } else { 0 },
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn honoka_attach_input_queue(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, queue_vaddr, queue_bytes) = call_port(
        honoka_port,
        HONOKA_REQUEST_ATTACH_INPUT_QUEUE,
        window_id,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((queue_vaddr, queue_bytes))
}

pub fn honoka_attach_input_notification(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_ATTACH_INPUT_NOTIFICATION,
        window_id,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn honoka_set_window_title(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
    title: &[u8],
) -> Result<(), RequestError> {
    let mut chunks = [0usize; 3];
    let limit = title.len().min(HONOKA_WINDOW_TITLE_BYTES);
    let mut i = 0usize;
    while i < limit {
        let chunk = i / 8;
        let shift = (i % 8) * 8;
        chunks[chunk] |= (title[i] as Word) << shift;
        i += 1;
    }
    let (status, _, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_SET_WINDOW_TITLE,
        window_id,
        chunks[0],
        chunks[1],
        chunks[2],
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn honoka_invalidate_logical_framebuffer(
    honoka_port: CapabilityDescriptor,
    window_id: Word,
    x: Word,
    y: Word,
    width: Word,
    height: Word,
) -> Result<(), RequestError> {
    let packed_xy = (x & 0xffff_ffff) | ((y & 0xffff_ffff) << 32);
    let packed_wh = (width & 0xffff_ffff) | ((height & 0xffff_ffff) << 32);
    let (status, _, _) = call_port(
        honoka_port,
        HONOKA_REQUEST_INVALIDATE_LOGICAL_FRAMEBUFFER,
        window_id,
        packed_xy,
        packed_wh,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
