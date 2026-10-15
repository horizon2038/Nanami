//! Event-ring ownership and the xHCI interrupt handshake.
use super::{
    registers::*,
    ring::{Consumer, Trb},
};
use core::sync::atomic::{fence, Ordering};

pub(super) struct Events {
    ring: Consumer,
    processed: u64,
    published: u64,
}

impl Events {
    pub unsafe fn new(physical: u64, virtual_address: usize) -> Self {
        Self {
            ring: unsafe { Consumer::new(physical, virtual_address) },
            processed: physical,
            published: physical,
        }
    }

    pub fn begin(&self, op: usize, interrupter: usize) {
        unsafe {
            write(op, USBSTS, 1 << 3);
            // Preserve IE: enumeration also drains events before IRQs are enabled.
            write(interrupter, 0, read(interrupter, 0) | 1);
            let _ = read(interrupter, 0); // Flush the posted IP acknowledgement.
        }
    }

    pub fn pop(&mut self, interrupter: usize) -> Option<Trb> {
        // ERDP acknowledges the last examined entry, NOT the next unread TRB.
        // A Cycle mismatch permits publishing the empty position as well.
        self.processed = self.ring.next_physical();
        let event = self.ring.pop();
        if self.processed != self.published {
            fence(Ordering::SeqCst);
            unsafe { write64(interrupter, 0x18, self.processed) };
            self.published = self.processed;
        }
        event
    }

    pub fn ready(&self) -> bool {
        self.ring.ready()
    }

    pub fn dequeue(&self) -> u64 {
        self.ring.next_physical()
    }

    pub fn diagnostic_position(&self) -> (u64, u32, u32) {
        self.ring.diagnostic_position()
    }

    pub fn finish(&mut self, interrupter: usize) {
        // Also clear EHB for an empty drain (e.g. an IRQ whose events were
        // already consumed by a synchronous transfer). Otherwise IRQs stay gated.
        fence(Ordering::SeqCst);
        unsafe {
            write64(interrupter, 0x18, self.processed | 8);
            let _ = read(interrupter, 0x18); // Complete before unmasking the IRQ.
        }
        self.published = self.processed;
    }
}
