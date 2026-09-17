use core::ptr::{read_volatile, write_volatile};

use libnanami::{RequestError, Word};

#[path = "hpet_clock.rs"]
mod hpet_clock;
use hpet_clock::Clock;

pub const TICK_HZ: u64 = 1_000_000_000;
pub const MODE: &str = "hpet-one-shot";

const SLOT_DEVICE_MANAGER: Word = 17;
const HPET_MMIO_BYTES: Word = 0x1000;

const REG_CAPABILITIES: usize = 0x000;
const REG_CONFIGURATION: usize = 0x010;
const REG_INTERRUPT_STATUS: usize = 0x020;
const REG_MAIN_COUNTER: usize = 0x0f0;
const REG_TIMER0_CONFIGURATION: usize = 0x100;
const REG_TIMER0_COMPARATOR: usize = 0x108;

const GENERAL_ENABLE: u64 = 1 << 0;
const GENERAL_LEGACY_REPLACEMENT: u64 = 1 << 1;
const CAP_LEGACY_REPLACEMENT: u64 = 1 << 15;
const CAP_COUNTER_64: u64 = 1 << 13;
const TIMER_LEVEL: u64 = 1 << 1;
const TIMER_INTERRUPT_ENABLE: u64 = 1 << 2;
const TIMER_PERIODIC: u64 = 1 << 3;
const TIMER_COUNTER_64: u64 = 1 << 5;
const TIMER_SET_VALUE: u64 = 1 << 6;
const TIMER_32BIT: u64 = 1 << 8;
const TIMER_FSB_ENABLE: u64 = 1 << 14;
const TIMER_ROUTE_SHIFT: u64 = 9;
const TIMER_ROUTE_MASK: u64 = 0x1f << TIMER_ROUTE_SHIFT;

pub struct PreparedTimer {
    pub resource: Word,
    pub irq_number: Word,
    clock: Clock,
    legacy: bool,
    comparator_wide: bool,
    configuration: u64,
    armed: bool,
    deadline: Option<u64>,
    comparator: u64,
}

unsafe fn read64(base: Word, offset: usize) -> u64 {
    unsafe { read_volatile((base as usize + offset) as *const u64) }
}

unsafe fn write64(base: Word, offset: usize, value: u64) {
    unsafe { write_volatile((base as usize + offset) as *mut u64, value) };
}

fn select_interrupt(base: Word) -> Result<(Word, bool), RequestError> {
    let capabilities = unsafe { read64(base, REG_CAPABILITIES) };
    if (capabilities & CAP_LEGACY_REPLACEMENT) != 0 {
        return Ok((0, true));
    }

    let timer_capabilities = unsafe { read64(base, REG_TIMER0_CONFIGURATION) };
    let routes = (timer_capabilities >> 32) as u32;
    let mut irq = 16u32;
    while irq < 32 {
        if (routes & (1u32 << irq)) != 0 {
            return Ok((irq as Word, false));
        }
        irq += 1;
    }
    let mut irq = 2u32;
    while irq < 16 {
        if irq != 8 && irq != 12 && irq != 13 && (routes & (1u32 << irq)) != 0 {
            return Ok((irq as Word, false));
        }
        irq += 1;
    }
    Err(RequestError::Unsupported)
}

pub fn prepare(_timer_resource_slot: Word) -> Result<PreparedTimer, RequestError> {
    libnanami::connect_service_by_name(
        nanami_services::device::DEVICE_MANAGER_SERVICE,
        SLOT_DEVICE_MANAGER,
    )?;
    let manager = libnanami::ipc::process_slot_descriptor(SLOT_DEVICE_MANAGER);
    let physical_base = nanami_services::device::hpet_mmio_base(manager)?;
    let page_base = physical_base & !0xfff;
    let page_offset = physical_base - page_base;
    let (_, mapped_base) = libnanami::request_mmio(page_base, HPET_MMIO_BYTES + page_offset)?;
    let virtual_base = mapped_base + page_offset;

    let capabilities = unsafe { read64(virtual_base, REG_CAPABILITIES) };
    let period_fs = capabilities >> 32;
    let timer_count = ((capabilities >> 8) & 0x1f) + 1;
    if period_fs == 0 || period_fs > 100_000_000 || timer_count == 0 {
        return Err(RequestError::Protocol);
    }
    let timer_capabilities = unsafe { read64(virtual_base, REG_TIMER0_CONFIGURATION) };
    let (irq_number, legacy) = select_interrupt(virtual_base)?;
    libnanami::println!(
        "[hpet-server] mmio={:#x} period-fs={} irq={} legacy={}",
        physical_base,
        period_fs,
        irq_number,
        legacy as usize
    );
    Ok(PreparedTimer {
        resource: virtual_base,
        irq_number,
        clock: Clock::new(period_fs, capabilities & CAP_COUNTER_64 != 0),
        legacy,
        comparator_wide: capabilities & CAP_COUNTER_64 != 0
            && timer_capabilities & TIMER_COUNTER_64 != 0,
        configuration: 0,
        armed: false,
        deadline: None,
        comparator: 0,
    })
}

impl PreparedTimer {
    pub fn start(&mut self) -> Result<(), RequestError> {
        let base = self.resource;
        unsafe {
            write64(base, REG_CONFIGURATION, 0);
            let count = ((read64(base, REG_CAPABILITIES) >> 8) & 0x1f) + 1;
            for index in 0..count as usize {
                let offset = REG_TIMER0_CONFIGURATION + index * 0x20;
                let config = read64(base, offset) & !(TIMER_INTERRUPT_ENABLE | TIMER_FSB_ENABLE);
                write64(base, offset, config);
            }
            write64(base, REG_INTERRUPT_STATUS, (1u64 << count) - 1);
            write64(base, REG_MAIN_COUNTER, 0);
            self.configuration = read64(base, REG_TIMER0_CONFIGURATION)
                & !(TIMER_LEVEL
                    | TIMER_INTERRUPT_ENABLE
                    | TIMER_PERIODIC
                    | TIMER_SET_VALUE
                    | TIMER_32BIT
                    | TIMER_FSB_ENABLE
                    | TIMER_ROUTE_MASK);
            if !self.comparator_wide {
                self.configuration |= TIMER_32BIT;
            }
            if !self.legacy {
                self.configuration |= (self.irq_number as u64) << TIMER_ROUTE_SHIFT;
            }
            write64(base, REG_TIMER0_CONFIGURATION, self.configuration);
            write64(
                base,
                REG_CONFIGURATION,
                GENERAL_ENABLE
                    | if self.legacy {
                        GENERAL_LEGACY_REPLACEMENT
                    } else {
                        0
                    },
            );
        }
        Ok(())
    }

    fn counter(&mut self) -> u64 {
        let counter = (self.resource as usize + REG_MAIN_COUNTER) as *const u32;
        let raw = unsafe {
            if self.clock.wide {
                // A chipset can split even an aligned 64-bit MMIO read.
                loop {
                    let high = read_volatile(counter.add(1));
                    let low = read_volatile(counter);
                    if high == read_volatile(counter.add(1)) {
                        break (u64::from(high) << 32) | u64::from(low);
                    }
                }
            } else {
                u64::from(read_volatile(counter))
            }
        };
        self.clock.observe(raw)
    }

    pub fn now(&mut self) -> u64 {
        let cycles = self.counter();
        self.clock.nanoseconds(cycles)
    }

    pub fn on_interrupt(&mut self) {
        unsafe {
            write64(self.resource, REG_INTERRUPT_STATUS, 1);
        }
    }

    pub fn arm(&mut self, deadline: Option<u64>) -> Result<(), RequestError> {
        if deadline.is_none() && self.clock.wide {
            if self.armed {
                unsafe {
                    write64(self.resource, REG_TIMER0_CONFIGURATION, self.configuration);
                }
                self.armed = false;
            }
            return Ok(());
        }
        let now = self.counter();
        if self.armed && self.deadline == deadline && now < self.comparator {
            return Ok(());
        }
        // A 32-bit counter needs wrap maintenance even when clients are idle;
        // a 32-bit comparator also needs bounded steps for distant deadlines.
        let limit = if self.comparator_wide {
            u64::MAX
        } else {
            now.saturating_add(1 << 30)
        };
        let target = deadline.map_or(limit, |ns| self.clock.cycles_ceil(ns).min(limit));
        let mut lead = self.clock.cycles_ceil(10_000).max(2); // 10 us programming guard
        let max_lead = if self.comparator_wide {
            u64::MAX
        } else {
            1 << 29
        };
        lead = lead.min(max_lead);
        for _ in 0..16 {
            let now = self.counter();
            let comparator = target.max(now.saturating_add(lead));
            unsafe {
                write64(self.resource, REG_TIMER0_CONFIGURATION, self.configuration);
                // HPET compares for equality, so check for a missed write below.
                write64(
                    self.resource,
                    REG_TIMER0_COMPARATOR,
                    if self.comparator_wide {
                        comparator
                    } else {
                        comparator as u32 as u64
                    },
                );
                write64(
                    self.resource,
                    REG_TIMER0_CONFIGURATION,
                    self.configuration | TIMER_INTERRUPT_ENABLE,
                );
            }
            let after = self.counter();
            if comparator.saturating_sub(after) >= lead / 2 {
                self.armed = true;
                self.deadline = deadline;
                self.comparator = comparator;
                return Ok(());
            }
            // Account for preemption/SMI between reading and programming HPET.
            lead = lead.saturating_mul(2).min(max_lead);
        }
        Err(RequestError::Transport)
    }
}
