use core::ptr::{read_volatile, write_volatile};

use libnanami::{RequestError, Word};

pub const TICK_HZ: u64 = 100;

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
const TIMER_INTERRUPT_ENABLE: u64 = 1 << 2;
const TIMER_PERIODIC: u64 = 1 << 3;
const TIMER_PERIODIC_CAPABLE: u64 = 1 << 4;
const TIMER_SET_VALUE: u64 = 1 << 6;
const TIMER_ROUTE_SHIFT: u64 = 9;
const TIMER_ROUTE_MASK: u64 = 0x1f << TIMER_ROUTE_SHIFT;

pub struct PreparedTimer {
    pub resource: Word,
    pub irq_number: Word,
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
    if (timer_capabilities & TIMER_PERIODIC_CAPABLE) == 0 {
        return Err(RequestError::Unsupported);
    }
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
    })
}

pub fn start(base: Word) -> Result<(), RequestError> {
    let capabilities = unsafe { read64(base, REG_CAPABILITIES) };
    let period_fs = capabilities >> 32;
    if period_fs == 0 {
        return Err(RequestError::Protocol);
    }
    let interval = 1_000_000_000_000_000u64
        .checked_div(period_fs.saturating_mul(TICK_HZ))
        .ok_or(RequestError::Protocol)?;
    if interval == 0 {
        return Err(RequestError::Protocol);
    }
    let (irq_number, legacy) = select_interrupt(base)?;

    unsafe {
        let mut general = read64(base, REG_CONFIGURATION);
        general &= !(GENERAL_ENABLE | GENERAL_LEGACY_REPLACEMENT);
        write64(base, REG_CONFIGURATION, general);
        write64(base, REG_INTERRUPT_STATUS, u64::MAX);
        write64(base, REG_MAIN_COUNTER, 0);

        let current = read64(base, REG_TIMER0_CONFIGURATION);
        let mut config = current
            & !(TIMER_INTERRUPT_ENABLE | TIMER_PERIODIC | TIMER_SET_VALUE | TIMER_ROUTE_MASK);
        config |= TIMER_INTERRUPT_ENABLE | TIMER_PERIODIC | TIMER_SET_VALUE;
        if !legacy {
            config |= (irq_number as u64) << TIMER_ROUTE_SHIFT;
        }
        write64(base, REG_TIMER0_CONFIGURATION, config);
        // Periodic mode requires an accumulator write followed by the reload value.
        write64(base, REG_TIMER0_COMPARATOR, interval);
        write64(base, REG_TIMER0_COMPARATOR, interval);

        general |= GENERAL_ENABLE;
        if legacy {
            general |= GENERAL_LEGACY_REPLACEMENT;
        }
        write64(base, REG_CONFIGURATION, general);
    }
    Ok(())
}

pub fn rearm() -> Result<(), RequestError> {
    Ok(())
}
