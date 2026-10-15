use crate::Canvas;
use core::sync::atomic::{AtomicUsize, Ordering};
use nanami_client::{gfx::honoka::*, input::*};
use nanami_runtime::{ipc, RequestError, Word};

pub enum Event {
    Key(usize, bool),
    Move(i32, i32),
    Button(usize, bool),
    Wheel(i16),
    Resize(usize, usize),
    Close,
}
pub struct Window {
    pub port: Word,
    pub id: Word,
    pub surface: WindowSurface,
    queue: WindowEventQueue,
    present: Word,
    notification: Word,
}
impl Window {
    pub fn new(
        title: &str,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) -> Result<Self, RequestError> {
        let notification = ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
        ipc::bind_current_thread_notification(notification)?;
        let pid = nanami_client::registry::connect_honoka_service_with_pid(22)?;
        let port = ipc::process_slot_descriptor(22);
        let id = honoka_create_window_with_title(port, x, y, width, height, title.as_bytes())?;
        let attached = (|| {
            nanami_runtime::request_notification_port_copy(
                pid,
                nanami_runtime::PROCESS_SLOT_NOTIFICATION,
                23,
                HONOKA_NOTIFICATION_PRESENT | id,
            )?;
            let surface = WindowSurface::attach(port, id)?;
            let (base, _) = honoka_attach_input_queue(port, id)?;
            honoka_attach_input_notification(port, id)?;
            Ok(Self {
                port,
                id,
                surface,
                queue: WindowEventQueue::new(base),
                present: ipc::process_slot_descriptor(23),
                notification,
            })
        })();
        if attached.is_err() {
            let _ = honoka_destroy_window(port, id);
        }
        attached
    }
    pub fn size(&self) -> (usize, usize) {
        (self.surface.width, self.surface.height)
    }
    pub fn title(&self, title: &str) -> Result<(), RequestError> {
        let mut end = title.len().min(HONOKA_WINDOW_TITLE_BYTES);
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        honoka_set_window_title(self.port, self.id, &title.as_bytes()[..end])
    }
    pub fn event(&mut self) -> Result<Option<Event>, RequestError> {
        while let Some(packed) = self.queue.pop() {
            let (kind, code, a, b, _) = unpack_input_event(packed);
            let event = match kind {
                INPUT_EVENT_KIND_KEY => Event::Key(code, a != 0),
                INPUT_EVENT_KIND_MOUSE_MOVE => Event::Move(a as i32, b as i32),
                INPUT_EVENT_KIND_MOUSE_BUTTON => Event::Button(code, a != 0),
                INPUT_EVENT_KIND_MOUSE_WHEEL => Event::Wheel(a),
                INPUT_EVENT_KIND_WINDOW_CLOSE => Event::Close,
                INPUT_EVENT_KIND_WINDOW_RESIZE => {
                    self.surface.resize(
                        self.port,
                        self.id,
                        a as u16 as usize,
                        b as u16 as usize,
                    )?;
                    Event::Resize(self.surface.width, self.surface.height)
                }
                _ => continue,
            };
            return Ok(Some(event));
        }
        Ok(None)
    }
    pub fn wait(&self) -> Result<usize, RequestError> {
        if !self.queue.is_empty() {
            // Drain pending timer badges even under a sustained input backlog.
            // The idle path below still blocks rather than polling.
            return ipc::notification_poll(self.notification);
        }
        ipc::notification_wait(self.notification)
    }
    pub fn paint(&mut self, draw: impl FnOnce(&mut Canvas<'_>)) -> Result<(), RequestError> {
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(
                self.surface.pixels() as *mut u32,
                self.surface.pixel_bytes() / 4,
            )
        };
        draw(&mut Canvas::new(
            pixels,
            self.surface.width,
            self.surface.height,
        ));
        if self.push_damage() {
            ipc::notification_notify(self.present)
        } else {
            honoka_invalidate_logical_framebuffer(
                self.port,
                self.id,
                0,
                0,
                self.surface.width,
                self.surface.height,
            )
        }
    }
    fn push_damage(&self) -> bool {
        let word = |i: usize| unsafe { &*((self.surface.base as *const AtomicUsize).add(i)) };
        if word(0).load(Ordering::Acquire) != HONOKA_DAMAGE_QUEUE_MAGIC {
            return false;
        }
        let capacity = word(1).load(Ordering::Acquire);
        if capacity == 0 || capacity > HONOKA_DAMAGE_QUEUE_CAPACITY {
            return false;
        }
        let head = word(2).load(Ordering::Acquire) % capacity;
        let tail = word(3).load(Ordering::Relaxed) % capacity;
        let next = (tail + 1) % capacity;
        if next == head {
            return false;
        }
        let entry = HONOKA_DAMAGE_QUEUE_HEADER_WORDS + tail * HONOKA_DAMAGE_ENTRY_WORDS;
        for (i, value) in [0, 0, self.surface.width, self.surface.height]
            .iter()
            .enumerate()
        {
            word(entry + i).store(*value, Ordering::Relaxed);
        }
        word(3).store(next, Ordering::Release);
        true
    }
    pub fn close(&self) {
        let _ = honoka_destroy_window(self.port, self.id);
    }
}
