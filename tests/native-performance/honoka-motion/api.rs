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
thread_local! {
    static MAPPINGS: RefCell<Vec<Box<[u64]>>> = const { RefCell::new(Vec::new()) };
    pub static RELEASED: RefCell<Vec<(Word, Word)>> = const { RefCell::new(Vec::new()) };
    pub static FAIL_ALLOCATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub fn request_shared_memory(_: Word, size: Word) -> Result<(Word, Word), RequestError> {
    if FAIL_ALLOCATION.with(|fail| fail.replace(false)) {
        return Err(RequestError::Transport);
    }
    let mut memory = vec![0u64; size.div_ceil(8)].into_boxed_slice();
    let base = memory.as_mut_ptr() as usize;
    MAPPINGS.with(|mappings| mappings.borrow_mut().push(memory));
    Ok((base, base))
}
pub fn request_mapping_release(base: Word, size: Word) -> Result<(), RequestError> {
    // Retain allocations until the test thread exits, so tests can verify the
    // other participant's mapping remains accessible after local release.
    RELEASED.with(|released| released.borrow_mut().push((base, size)));
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
        pub fn text_width(&self, text: &[u8]) -> i32 {
            text.len() as i32 * 7
        }
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
