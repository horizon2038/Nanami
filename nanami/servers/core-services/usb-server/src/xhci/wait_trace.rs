//! Read-only snapshots buffered during a long wait, printed only AFTER it ends.
use super::{events::Events, registers::*};

#[derive(Clone, Copy)]
pub(super) struct Target {
    pub phase: &'static str,
    pub slot: u8,
    pub dci: u8,
    pub td: u64,
    pub bytes: usize,
    pub context: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Snapshot {
    command: u32,
    status: u32,
    iman: u32,
    erdp: u64,
    next: u64,
    control: u32,
    cycle: u32,
    mfindex: u32,
    ep_state: u32,
    ep_dequeue: u64,
}

impl Snapshot {
    pub fn capture(
        op: usize,
        interrupter: usize,
        runtime: usize,
        events: &Events,
        target: Option<Target>,
    ) -> Self {
        let (next, control, cycle) = events.diagnostic_position();
        let context = target.map_or(0, |t| t.context) as *const u32;
        let (ep_state, ep_dequeue) = if context.is_null() {
            (u32::MAX, 0)
        } else {
            unsafe {
                (
                    core::ptr::read_volatile(context) & 7,
                    u64::from(core::ptr::read_volatile(context.add(2)))
                        | u64::from(core::ptr::read_volatile(context.add(3))) << 32,
                )
            }
        };
        unsafe {
            Self {
                command: read(op, USBCMD),
                status: read(op, USBSTS),
                iman: read(interrupter, 0),
                erdp: u64::from(read(interrupter, 0x18)) | u64::from(read(interrupter, 0x1c)) << 32,
                next,
                control,
                cycle,
                mfindex: read(runtime, MFINDEX),
                ep_state,
                ep_dequeue,
            }
        }
    }

    pub fn report(&self, point: &str) {
        nanami_runtime::println!("[pipe.td.state] point={} USBCMD={:#x} USBSTS={:#x} IMAN={:#x} ERDP={:#x} next={:#x} next-control={:#x} expected-cycle={} MFINDEX={} ep-state={} ep-dequeue={:#x}",
            point, self.command, self.status, self.iman, self.erdp, self.next, self.control,
            self.cycle, self.mfindex, self.ep_state, self.ep_dequeue);
    }
}
