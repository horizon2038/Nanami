use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
pub use nanami_uapi::gpu::*;

pub struct Surface {
    pub port: Word,
    pub id: Word,
    pub address: Word,
    pub layout: Layout,
    pub accelerated: Word,
}
fn request(
    port: Word,
    code: Word,
    args: [Word; 4],
    words: u8,
) -> Result<(Word, Word), RequestError> {
    let (status, a, b) = call_port(port, code, args[0], args[1], args[2], args[3], words)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((a, b))
}
pub fn info(port: Word) -> Result<Info, RequestError> {
    let (a, b) = request(port, GET_INFO, [0; 4], 1)?;
    Info::decode(a, b).ok_or(RequestError::Protocol)
}
pub fn create(port: Word, layout: Layout, accelerated: Word) -> Result<Surface, RequestError> {
    if Layout::new(layout.width, layout.height, layout.stride) != Some(layout) {
        return Err(RequestError::InvalidArgument);
    }
    let (id, address) = request(
        port,
        CREATE,
        [
            layout.width | (layout.height << 32),
            layout.stride,
            FORMAT_XRGB8888,
            0,
        ],
        4,
    )?;
    if id == 0 || address == 0 || address & 4095 != 0 {
        return Err(RequestError::Protocol);
    }
    Ok(Surface {
        port,
        id,
        address,
        layout,
        accelerated,
    })
}
pub fn destroy(surface: &Surface) -> Result<(), RequestError> {
    request(surface.port, DESTROY, [surface.id, 0, 0, 0], 2)?;
    nanami_runtime::request_mapping_release(surface.address, surface.layout.bytes)
}
pub fn fill(surface: &Surface, rect: Rect, color: u32) -> Result<(), RequestError> {
    if !rect.inside(surface.layout) {
        return Err(RequestError::InvalidArgument);
    }
    request(
        surface.port,
        FILL,
        [surface.id, rect.position(), rect.size(), color as Word],
        5,
    )?;
    Ok(())
}
pub fn copy(
    source: &Surface,
    destination: &Surface,
    from: Rect,
    x: Word,
    y: Word,
) -> Result<(), RequestError> {
    let to = Rect {
        x,
        y,
        width: from.width,
        height: from.height,
    };
    if source.port != destination.port
        || !from.inside(source.layout)
        || !to.inside(destination.layout)
        || (source.id == destination.id && from.overlaps(to))
    {
        return Err(RequestError::InvalidArgument);
    }
    let coordinates = from.x | (from.y << 16) | (x << 32) | (y << 48);
    request(
        source.port,
        COPY,
        [source.id, destination.id, coordinates, from.size()],
        5,
    )?;
    Ok(())
}
pub fn present(surface: &Surface, rect: Rect) -> Result<(), RequestError> {
    if !rect.inside(surface.layout) {
        return Err(RequestError::InvalidArgument);
    }
    request(
        surface.port,
        PRESENT,
        [surface.id, rect.position(), rect.size(), 0],
        4,
    )?;
    Ok(())
}
