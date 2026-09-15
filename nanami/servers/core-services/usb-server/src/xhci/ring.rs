//! DMA ring ownership and cycle-bit publication. A producer has at most one
//! outstanding TD; completion (not timeout) returns its storage to software.
use core::ptr;
use core::sync::atomic::{fence, Ordering};

pub const ENTRIES: usize = 256;
pub const CYCLE: u32 = 1;
pub const IOC: u32 = 1 << 5;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub fn kind(self) -> u32 {
        (self.control >> 10) & 0x3f
    }
    pub fn code(self) -> u8 {
        (self.status >> 24) as u8
    }
    pub fn slot(self) -> u8 {
        (self.control >> 24) as u8
    }
    pub fn endpoint(self) -> u8 {
        ((self.control >> 16) & 0x1f) as u8
    }
}

pub struct Producer {
    pub physical: u64,
    virtual_address: usize,
    enqueue: usize,
    cycle: bool,
    pending: bool,
}

impl Producer {
    /// The caller owns a zeroed, page-aligned DMA page for this ring's lifetime.
    pub unsafe fn new(physical: u64, virtual_address: usize) -> Self {
        Self {
            physical,
            virtual_address,
            enqueue: 0,
            cycle: true,
            pending: false,
        }
    }

    pub fn submit(&mut self, trbs: &[Trb]) -> Option<u64> {
        if self.pending || trbs.is_empty() || trbs.len() >= ENTRIES - 1 {
            return None;
        }
        let mut first = core::ptr::null_mut::<Trb>();
        let mut first_control = 0;
        let mut last = 0;
        for trb in trbs {
            unsafe {
                if self.enqueue == ENTRIES - 1 {
                    let link = (self.virtual_address as *mut Trb).add(self.enqueue);
                    ptr::write_volatile(&mut (*link).parameter, self.physical);
                    ptr::write_volatile(&mut (*link).status, 0);
                    fence(Ordering::Release);
                    ptr::write_volatile(&mut (*link).control, (6 << 10) | 2 | self.cycle as u32);
                    self.enqueue = 0;
                    self.cycle = !self.cycle;
                }
                let dst = (self.virtual_address as *mut Trb).add(self.enqueue);
                let control = (trb.control & !CYCLE) | self.cycle as u32;
                ptr::write_volatile(&mut (*dst).parameter, trb.parameter);
                ptr::write_volatile(&mut (*dst).status, trb.status);
                if first.is_null() {
                    first = dst;
                    first_control = control;
                    ptr::write_volatile(&mut (*dst).control, control ^ CYCLE);
                } else {
                    ptr::write_volatile(&mut (*dst).control, control);
                }
                last = self.physical + (self.enqueue * 16) as u64;
                self.enqueue += 1;
            }
        }
        // Publish the first TRB only after the entire TD (including links) exists.
        fence(Ordering::Release);
        unsafe { ptr::write_volatile(&mut (*first).control, first_control) };
        self.pending = true;
        Some(last)
    }

    pub fn complete(&mut self) {
        self.pending = false;
    }
}

pub struct Consumer {
    pub physical: u64,
    virtual_address: usize,
    dequeue: usize,
    cycle: bool,
}

impl Consumer {
    pub unsafe fn new(physical: u64, virtual_address: usize) -> Self {
        Self {
            physical,
            virtual_address,
            dequeue: 0,
            cycle: true,
        }
    }
    pub fn pop(&mut self) -> Option<Trb> {
        unsafe {
            let src = (self.virtual_address as *const Trb).add(self.dequeue);
            let control = ptr::read_volatile(&(*src).control);
            if control & CYCLE != self.cycle as u32 {
                return None;
            }
            fence(Ordering::Acquire);
            let event = Trb {
                parameter: ptr::read_volatile(&(*src).parameter),
                status: ptr::read_volatile(&(*src).status),
                control,
            };
            self.dequeue += 1;
            if self.dequeue == ENTRIES {
                self.dequeue = 0;
                self.cycle = !self.cycle;
            }
            Some(event)
        }
    }
    pub fn next_physical(&self) -> u64 {
        self.physical + (self.dequeue * 16) as u64
    }
}
