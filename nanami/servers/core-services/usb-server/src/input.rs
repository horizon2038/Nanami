use crate::usb::hid::Event;
use libnanami::{RequestError, Word};
use nanami_services::input::*;

pub struct Input {
    queue: Option<InputEventQueue>,
    notification: Word,
    sequence: Word,
    keys: [u8; 512],
    buttons: [u8; 3],
    pending: bool,
}
impl Input {
    pub fn new() -> Self {
        Self {
            queue: None,
            notification: 0,
            sequence: 0,
            keys: [0; 512],
            buttons: [0; 3],
            pending: false,
        }
    }
    pub fn connected(&self) -> bool {
        self.queue.is_some()
    }
    pub fn connect(&mut self) -> Result<(), RequestError> {
        let pid = nanami_services::registry::connect_input_service_with_pid(22)?;
        let port = libnanami::ipc::process_slot_descriptor(22);
        input_service_attach_driver(port, INPUT_DRIVER_KEYBOARD)?;
        input_service_attach_driver(port, INPUT_DRIVER_MOUSE)?;
        let (base, bytes) = input_service_attach_driver_shared(port, INPUT_DRIVER_KEYBOARD)?;
        let queue = InputEventQueue::new(base);
        if bytes < INPUT_EVENT_QUEUE_BYTES || !queue.is_valid() {
            return Err(RequestError::Protocol);
        }
        libnanami::request_notification_port_copy(
            pid,
            libnanami::PROCESS_SLOT_NOTIFICATION,
            23,
            INPUT_DRIVER_NOTIFICATION_IDENTIFIER,
        )?;
        self.notification = libnanami::ipc::process_slot_descriptor(23);
        self.queue = Some(queue);
        // Reports may have arrived while input-server was still starting.
        for code in 0..self.keys.len() {
            if self.keys[code] != 0 {
                self.publish(Event {
                    kind: 1,
                    code,
                    x: 1,
                    y: 0,
                });
            }
        }
        for bit in 0..3 {
            if self.buttons[bit] != 0 {
                self.publish(Event {
                    kind: 2,
                    code: bit + 1,
                    x: 1,
                    y: 0,
                });
            }
        }
        libnanami::print!("[usb-server] input attached\n");
        Ok(())
    }
    pub fn emit(&mut self, event: Event) {
        let count = match event.kind {
            1 => self.keys.get_mut(event.code),
            2 if event.code != 0 => self.buttons.get_mut(event.code - 1),
            _ => None,
        };
        if let Some(count) = count {
            let before = *count != 0;
            *count = if event.x != 0 {
                count.saturating_add(1)
            } else {
                count.saturating_sub(1)
            };
            if before == (*count != 0) {
                return;
            }
        }
        self.publish(event);
    }
    fn publish(&mut self, event: Event) {
        if let Some(queue) = self.queue.as_mut() {
            queue.push_with_event_kind(
                event.kind,
                pack_input_event(event.kind, event.code, event.x, event.y, self.sequence),
            );
            self.sequence = self.sequence.wrapping_add(1);
            self.pending = true;
        }
    }
    pub fn flush(&mut self) {
        if self.pending {
            if libnanami::ipc::notification_notify(self.notification).is_ok() {
                self.pending = false;
            }
        }
    }
}
