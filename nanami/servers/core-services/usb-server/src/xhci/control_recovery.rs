//! A failed control pipe is not a failed controller. Establish ownership with
//! Stop/Reset Endpoint before the caller disables a slot or retains its DMA.
use super::*;

impl Controller {
    pub(super) fn stop_control_after_timeout(
        &mut self,
        slot: u8,
        port: u8,
        input: &mut Input,
    ) -> Result<(), RequestError> {
        // Output context is HC-owned. The dequeue pointer may not be current
        // until Stop Endpoint completes, so do not use this snapshot for reuse.
        let offset = SLOT_BASE + (slot as usize - 1) * SLOT_BYTES + 0x1000 + self.context_bytes;
        let context = self.dma.virtual_at(offset) as *const u32;
        let (state, dequeue, status) = unsafe {
            (
                core::ptr::read_volatile(context) & 7,
                u64::from(core::ptr::read_volatile(context.add(2)))
                    | u64::from(core::ptr::read_volatile(context.add(3))) << 32,
                read(self.op, PORTSC + (port as usize - 1) * 16),
            )
        };
        nanami_runtime::println!(
            "[usb-server] control timeout port={} slot={} PORTSC={:#x} ep-state={} ep-dequeue={:#x}",
            port, slot, status, state, dequeue
        );
        match state {
            1 | 2 => {
                self.command(
                    Trb {
                        // Running -> Stop; Halted -> Reset. Wait for completion
                        // BEFORE the caller can Disable Slot or release ownership.
                        control: (if state == 1 { 15 } else { 14 }) << 10
                            | (1 << 16)
                            | (slot as u32) << 24,
                        ..Trb::default()
                    },
                    input,
                )?;
            }
            0 | 3 => {}
            _ => return Err(RequestError::Protocol),
        }
        // Stop-generated transfer events precede command completion. Drain any
        // remaining events before an enumeration caller issues Disable Slot.
        for _ in 0..ring::ENTRIES {
            let Some(event) = self.event() else { break };
            self.dispatch(event, input);
        }
        self.events.finish(self.interrupter);
        if !self.running {
            return Err(RequestError::Transport);
        }
        nanami_runtime::println!(
            "[usb-server] control pipe quiesced slot={}; other devices remain online",
            slot
        );
        Ok(())
    }
}
