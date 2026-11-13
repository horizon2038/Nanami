#![no_std]
#![no_main]
#[cfg(not(target_arch = "x86_64"))]
compile_error!("intel-net needs a coherent PCI DMA backend");

mod chipset;
mod registers;
mod ring;
mod service;

use core::ptr;
use nanami_client::{
    pci, registry, timer,
    uapi::intel_net::{family, Family},
};
use nanami_runtime::{
    ipc::{ServiceEvent, ServiceRequest},
    RequestError, Word,
};
use registers::*;

const PCI_SLOT: Word = 16;
const TIMER_SLOT: Word = 17;
const IRQ_NOTIFICATION: Word = 18;
const IRQ_SLOT: Word = 19;
const SERVICE_SLOT: Word = 20;
const CLIENT_NOTIFICATION: Word = 24;
const ACPI_SLOT: Word = 25;
const MMIO_BYTES: usize = 128 * 1024;

fn retain_dma() -> ! {
    // Never let process exit recycle memory still reachable by bus mastering.
    loop {
        let _ = nanami_runtime::ipc::notification_wait(
            nanami_runtime::ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION),
        );
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[intel-net] panic: {}; retaining DMA", info);
    retain_dma()
}

struct Mmio {
    address: usize,
    timer: Word,
}
impl Registers for Mmio {
    fn read(&self, offset: usize) -> u32 {
        assert!(offset & 3 == 0 && offset < MMIO_BYTES);
        unsafe { ptr::read_volatile((self.address + offset) as *const u32) }
    }
    fn write(&mut self, offset: usize, value: u32) {
        assert!(offset & 3 == 0 && offset < MMIO_BYTES);
        unsafe {
            ptr::write_volatile((self.address + offset) as *mut u32, value);
        }
    }
    fn delay_ms(&mut self, ms: usize) -> Result<(), RequestError> {
        timer::timer_service_sleep_milliseconds(self.timer, ms)
    }
}

struct Candidate {
    regs: Mmio,
    device: pci::Device,
    family: Family,
    mac: [u8; 6],
}

fn prepare(
    port: Word,
    device: pci::Device,
    kind: Family,
    timer: Word,
) -> Result<Candidate, RequestError> {
    let bar = pci::probe_bar(port, device.address, 0)?;
    if bar.io || bar.bytes < MMIO_BYTES {
        return Err(RequestError::Unsupported);
    }
    // D0 and memory decode first; keep INTx and bus mastering disabled until
    // reset has stopped any firmware queues. Never write PCI status RW1C bits.
    if pci::read(port, device.address, 6, 2)? & (1 << 4) != 0 {
        let mut cap = pci::read(port, device.address, 0x34, 1)? as usize & !3;
        let mut seen = [false; 64];
        for _ in 0..48 {
            if cap == 0 {
                break;
            }
            if cap < 0x40 || seen[cap / 4] {
                return Err(RequestError::Protocol);
            }
            seen[cap / 4] = true;
            let header = pci::read(port, device.address, cap, 4)?;
            if header as u8 == 1 {
                let pm = pci::read(port, device.address, cap + 4, 2)?;
                pci::write(port, device.address, cap + 4, 2, pm & !0x8003)?;
                timer::timer_service_sleep_milliseconds(timer, 10)?;
            }
            cap = ((header >> 8) as u8 & !3) as usize;
        }
    }
    pci::disable_message_interrupts(port, device.address)?;
    let command = pci::read(port, device.address, 4, 2)?;
    pci::write(port, device.address, 4, 2, (command | 2 | (1 << 10)) & !4)?;
    let (_, address) = nanami_runtime::request_mmio(bar.physical, MMIO_BYTES)?;
    let mut regs = Mmio { address, timer };
    let mac = chipset::reset(&mut regs, kind)?;
    Ok(Candidate {
        regs,
        device,
        family: kind,
        mac,
    })
}

fn select(port: Word, timer: Word) -> Result<Candidate, RequestError> {
    let mut candidates: [Option<Candidate>; 8] = core::array::from_fn(|_| None);
    let mut count = 0;
    let mut index = 0;
    while let Some(device) = pci::enumerate(port, index)? {
        index += 1;
        let Some(kind) = family(device.vendor_id, device.device_id, device.class) else {
            continue;
        };
        if count == candidates.len() {
            break;
        }
        match prepare(port, device, kind, timer) {
            Ok(candidate) => {
                candidates[count] = Some(candidate);
                count += 1;
            }
            Err(error) => nanami_runtime::println!(
                "[intel-net] PCI {:06x} {} initialization failed: {}",
                device.address.packed(),
                kind.driver(),
                error
            ),
        }
    }
    if count == 0 {
        return Err(RequestError::Unsupported);
    }
    // Give every PHY the same bounded opportunity to negotiate. Unselected
    // devices remain DMA/interrupt-disabled; they never publish net-device.
    for attempt in 0..=150 {
        for candidate in &mut candidates[..count] {
            if candidate.as_ref().unwrap().regs.read(STATUS) & LINK_UP != 0 {
                return Ok(candidate.take().unwrap());
            }
        }
        if attempt != 150 {
            timer::timer_service_sleep_milliseconds(timer, 20)?;
        }
    }
    nanami_runtime::println!("[intel-net] no carrier; selecting first supported PCI function");
    Ok(candidates[0].take().unwrap())
}

fn interrupt_route(port: Word, device: pci::Device) -> Option<Word> {
    let interrupt = pci::read(port, device.address, 0x3c, 4).ok()?;
    let pin = (interrupt >> 8) as u8;
    if !(1..=4).contains(&pin) {
        return None;
    }
    if registry::connect_service(nanami_client::power::ACPI_SERVICE, ACPI_SLOT).is_ok() {
        let acpi = nanami_runtime::ipc::process_slot_descriptor(ACPI_SLOT);
        if let Ok(irq) = nanami_client::power::pci_intx_route(acpi, device.address.packed(), pin) {
            return Some(irq);
        }
    }
    let line = (interrupt & 0xff) as Word;
    // Firmware-programmed line only; no PIIX-specific guessed PIRQ registers.
    (line != 0 && line < 24).then_some(line)
}

fn serve(
    mut selected: Candidate,
    port: Word,
    physical: Word,
    memory: Word,
) -> Result<(), RequestError> {
    let advanced = matches!(selected.family, Family::Igb | Family::I210);
    let mut rings = unsafe { ring::Rings::new(memory, physical as u64, advanced) };
    let regs = &mut selected.regs;
    chipset::configure_queues(
        regs,
        selected.family,
        physical as u64,
        (physical + ring::TX_OFFSET) as u64,
        ring::RING_BYTES,
    )?;
    let mut irq = false;
    let mut notification = nanami_runtime::PROCESS_SLOT_NOTIFICATION;
    if let Some(number) = interrupt_route(port, selected.device) {
        match nanami_runtime::request_irq(number, IRQ_NOTIFICATION, IRQ_SLOT) {
            Ok(()) => {
                nanami_runtime::ipc::bind_current_thread_notification(
                    nanami_runtime::ipc::process_slot_descriptor(IRQ_NOTIFICATION),
                )?;
                notification = IRQ_NOTIFICATION;
                irq = true;
                nanami_runtime::println!("[intel-net] INTx GSI={}", number);
            }
            Err(error) => nanami_runtime::println!(
                "[intel-net] INTx {} unavailable: {}; timer RX fallback",
                number,
                error
            ),
        }
    }
    let command = pci::read(port, selected.device.address, 4, 2)?;
    let command = if irq {
        command & !(1 << 10)
    } else {
        command | (1 << 10)
    };
    pci::write(port, selected.device.address, 4, 2, command | 6)?;
    regs.write(RCTL, RX_CONTROL);
    regs.write(TCTL, TX_CONTROL);
    regs.write(RX_BASE + TAIL, (ring::COUNT - 1) as u32);
    regs.flush();
    if irq {
        let _ = regs.read(ICR);
        regs.write(ITR, 4000); // bounded interrupt rate; ~1 ms on 82574/PCH
        regs.write(IMS, RX_INTERRUPTS);
        nanami_runtime::ipc::interrupt_ack(nanami_runtime::ipc::process_slot_descriptor(IRQ_SLOT))?;
    }
    // No busy polling. Unsupported/shared INTx routes use notification-driven
    // 2 ms polling; IRQ mode has only a slow lost-wakeup/link maintenance tick.
    timer::timer_service_interval_on_notification_milliseconds(
        regs.timer,
        if irq { 1000 } else { 2 },
        notification,
    )?;
    let mut service = service::Service::new(physical as u64, selected.mac);
    registry::register_net_device()?;
    nanami_runtime::println!("[intel-net] ready {} PCI {:06x} device={:04x} MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rx={}",
        selected.family.driver(), selected.device.address.packed(), selected.device.device_id,
        selected.mac[0], selected.mac[1], selected.mac[2], selected.mac[3], selected.mac[4], selected.mac[5],
        if irq { "interrupt" } else { "timer" });
    let service_port = nanami_runtime::ipc::process_slot_descriptor(SERVICE_SLOT);
    let mut reply = None;
    loop {
        let event = match reply.take() {
            Some((s, a, b)) => {
                nanami_runtime::ipc::service_reply_receive_event(service_port, s, a, b)?
            }
            None => nanami_runtime::ipc::service_receive_event(service_port)?,
        };
        match event {
            ServiceEvent::Request(request) => {
                reply = Some(service.request(request, regs, &mut rings));
            }
            ServiceEvent::Notification { .. } => {
                let _ = regs.read(ICR); // clear device level before rearming IO-APIC
                rings.reclaim_tx();
                service.notify_rx(&rings);
                if irq {
                    nanami_runtime::ipc::interrupt_ack(
                        nanami_runtime::ipc::process_slot_descriptor(IRQ_SLOT),
                    )?;
                    service.notify_rx(&rings); // cover the masked-interrupt window
                }
            }
            ServiceEvent::Fault { .. } => {}
        }
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    registry::connect_boot_service("timer-service", TIMER_SLOT)?;
    let timer = nanami_runtime::ipc::process_slot_descriptor(TIMER_SLOT);
    let port = pci::connect(PCI_SLOT)?;
    let selected = select(port, timer)?;
    let (physical, memory) = nanami_runtime::request_dma(ring::DMA_BYTES)?;
    // From this point onward, errors must retain the pinned DMA allocation.
    if let Err(error) = serve(selected, port, physical, memory) {
        nanami_runtime::println!("[intel-net] stopped: {}; retaining DMA", error);
    }
    retain_dma()
}

nanami_runtime::nanami_entry!(nanami_main);
