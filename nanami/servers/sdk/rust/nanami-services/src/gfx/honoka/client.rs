use super::*;
use core::sync::atomic::{AtomicUsize, Ordering};

// A coalescing mailbox after the ordinary input ring, within its 16-KiB
// allocation. A full mouse/key queue must never discard the final resize.
const RESIZE_WORD: usize =
    crate::input::INPUT_EVENT_QUEUE_HEADER_WORDS + crate::input::INPUT_EVENT_QUEUE_CAPACITY;

pub fn publish_resize(base: Word, width: Word, height: Word) {
    if base == 0 {
        return;
    }
    let event = crate::input::pack_input_event(
        crate::input::INPUT_EVENT_KIND_WINDOW_RESIZE,
        0,
        width as i16,
        height as i16,
        0,
    );
    unsafe {
        (&*((base as *const AtomicUsize).add(RESIZE_WORD))).store(event, Ordering::Release);
    }
}

pub struct WindowEventQueue {
    base: Word,
    input: crate::input::InputEventQueue,
}

impl WindowEventQueue {
    pub fn new(base: Word) -> Self {
        Self {
            base,
            input: crate::input::InputEventQueue::new(base),
        }
    }
    pub fn pop(&mut self) -> Option<Word> {
        if self.base != 0 && self.input.is_valid() {
            let mailbox = unsafe { &*((self.base as *const AtomicUsize).add(RESIZE_WORD)) };
            if mailbox.load(Ordering::Acquire) != 0 {
                let event = mailbox.swap(0, Ordering::AcqRel);
                if event != 0 {
                    return Some(event);
                }
            }
        }
        self.input.pop()
    }
    pub fn is_empty(&self) -> bool {
        (self.base == 0
            || unsafe {
                (&*((self.base as *const AtomicUsize).add(RESIZE_WORD))).load(Ordering::Acquire)
                    == 0
            })
            && self.input.is_empty()
    }
}

/// Client-owned surface. Resize is synchronous: callers must stop drawing
/// before the call and use only the returned mapping/stride afterward.
pub struct WindowSurface {
    pub base: Word,
    pub bytes: Word,
    pub width: usize,
    pub height: usize,
    retired: Option<(Word, Word)>,
}

impl WindowSurface {
    pub fn attach(port: Word, window: Word) -> Result<Self, RequestError> {
        let (width, height) = honoka_get_window_content_size(port, window)?;
        let (base, bytes) = honoka_attach_logical_framebuffer(port, window)?;
        Ok(Self {
            base,
            bytes,
            width,
            height,
            retired: None,
        })
    }
    pub fn pixels(&self) -> Word {
        self.base + HONOKA_DAMAGE_QUEUE_BYTES
    }
    pub fn pixel_bytes(&self) -> Word {
        self.bytes - HONOKA_DAMAGE_QUEUE_BYTES
    }

    pub fn resize(
        &mut self,
        port: Word,
        window: Word,
        width: usize,
        height: usize,
    ) -> Result<bool, RequestError> {
        self.resize_with(port, window, width, height, |_, _| {})
    }

    pub fn resize_with(
        &mut self,
        port: Word,
        window: Word,
        width: usize,
        height: usize,
        paint: impl FnOnce(&Self, &Self),
    ) -> Result<bool, RequestError> {
        if width == self.width && height == self.height {
            return Ok(false);
        }
        if let Some((base, bytes)) = self.retired {
            libnanami::request_mapping_release(base, bytes)?;
            self.retired = None;
        }
        let (status, base, bytes) = call_port(
            port,
            HONOKA_REQUEST_RESIZE_LOGICAL_FRAMEBUFFER,
            window,
            width,
            height,
            0,
            4,
        )?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        // The server commits exactly the requested dimensions, or rejects.
        let old = (self.base, self.bytes);
        let next = Self {
            base,
            bytes,
            width,
            height,
            retired: None,
        };
        paint(self, &next);
        *self = next;
        if libnanami::request_mapping_release(old.0, old.1).is_err() {
            // Retain cleanup ownership, but never expose the old drawing pointer
            // after the server has committed the new surface.
            self.retired = Some(old);
        }
        Ok(true)
    }
}
