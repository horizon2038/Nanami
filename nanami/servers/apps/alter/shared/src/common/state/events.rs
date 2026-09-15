use super::*;

impl Runtime {
    pub fn push_keyboard_event(&mut self, event: Word) {
        let next = (self.keyboard_tail + 1) % ALTER_EVDEV_QUEUE_CAPACITY;
        if next == self.keyboard_head {
            self.keyboard_head = (self.keyboard_head + 1) % ALTER_EVDEV_QUEUE_CAPACITY;
        }
        self.keyboard_events[self.keyboard_tail] = event;
        self.keyboard_tail = next;
    }

    pub fn pop_keyboard_event(&mut self) -> Option<Word> {
        if self.keyboard_head == self.keyboard_tail {
            return None;
        }
        let event = self.keyboard_events[self.keyboard_head];
        self.keyboard_head = (self.keyboard_head + 1) % ALTER_EVDEV_QUEUE_CAPACITY;
        Some(event)
    }

    pub fn keyboard_event_ready(&self) -> bool {
        self.keyboard_head != self.keyboard_tail
    }

    pub fn push_mouse_event(&mut self, event: Word) {
        let next = (self.mouse_tail + 1) % ALTER_EVDEV_QUEUE_CAPACITY;
        if next == self.mouse_head {
            self.mouse_head = (self.mouse_head + 1) % ALTER_EVDEV_QUEUE_CAPACITY;
        }
        self.mouse_events[self.mouse_tail] = event;
        self.mouse_tail = next;
    }

    pub fn pop_mouse_event(&mut self) -> Option<Word> {
        if self.mouse_head == self.mouse_tail {
            return None;
        }
        let event = self.mouse_events[self.mouse_head];
        self.mouse_head = (self.mouse_head + 1) % ALTER_EVDEV_QUEUE_CAPACITY;
        Some(event)
    }

    pub fn mouse_event_ready(&self) -> bool {
        self.mouse_head != self.mouse_tail
    }
}
