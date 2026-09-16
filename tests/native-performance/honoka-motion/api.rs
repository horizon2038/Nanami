use super::*;

pub fn call_port(
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: Word,
    _: u8,
) -> Result<(Word, Word, Word), RequestError> {
    panic!("unexpected IPC")
}
pub fn request_shared_memory(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
    panic!("unexpected allocation")
}
pub fn request_mapping_release(_: Word, _: Word) -> Result<(), RequestError> {
    Ok(())
}
pub fn request_process_alive(_: Word) -> Result<bool, RequestError> {
    Ok(true)
}
pub fn request_notification_port_copy(
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<(), RequestError> {
    Ok(())
}
pub mod exec {
    use super::*;
    pub fn exec_spawn_path(_: Word, _: Word, _: Word, _: Word) -> Result<Word, RequestError> {
        panic!("unexpected spawn")
    }
}
pub mod ipc {
    use super::*;
    pub fn notification_notify(_: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
}
pub mod gfx {
    use super::*;
    pub use crate::honoka_api as honoka;
    pub fn display_service_present(
        _: Word,
        x: Word,
        y: Word,
        width: Word,
        height: Word,
    ) -> Result<(), RequestError> {
        PRESENTED.with(|rects| {
            rects.borrow_mut().push(framebuffer::Rect::new(
                x as i32,
                y as i32,
                width as i32,
                height as i32,
            ))
        });
        Ok(())
    }
}
pub mod font {
    use super::*;
    pub struct TextRenderer;
    impl TextRenderer {
        pub fn draw_title(
            &self,
            _: &framebuffer::Framebuffer,
            _: framebuffer::Rect,
            _: i32,
            _: i32,
            _: &[u8],
            _: u32,
            _: u32,
            _: u8,
        ) {
        }
    }
}
