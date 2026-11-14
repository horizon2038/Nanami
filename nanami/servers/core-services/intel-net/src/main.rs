#![no_std]
#![no_main]
extern crate alloc;
#[cfg(not(target_arch = "x86_64"))]
compile_error!("intel-net needs a coherent PCI DMA backend");

mod chipset;
mod ports;
mod registers;
mod ring;
mod routing;
mod service;

use alloc::vec::Vec;
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

fn discover(port: Word, timer: Word) -> Result<Vec<Candidate>, RequestError> {
    let mut candidates = Vec::new();
    let mut index = 0;
    while let Some(device) = pci::enumerate(port, index)? {
        index += 1;
        let Some(kind) = family(device.vendor_id, device.device_id, device.class) else {
            continue;
        };
        if candidates.len() == 8 {
            break;
        }
        match prepare(port, device, kind, timer) {
            Ok(candidate) => {
                nanami_runtime::println!(
                    "[intel-net] port={} PCI {:06x} {} status={:#x} carrier={}",
                    candidates.len(),
                    device.address.packed(),
                    kind.driver(),
                    candidate.regs.read(STATUS),
                    candidate.regs.read(STATUS) & LINK_UP != 0
                );
                candidates.push(candidate);
            }
            Err(error) => nanami_runtime::println!(
                "[intel-net] PCI {:06x} {} initialization failed: {}",
                device.address.packed(),
                kind.driver(),
                error
            ),
        }
    }
    if candidates.is_empty() {
        return Err(RequestError::Unsupported);
    }
    Ok(candidates)
}

fn interrupt_route(port: Word, acpi: Option<Word>, address: pci::Address) -> Option<Word> {
    let interrupt = pci::read(port, address, 0x3c, 4).ok()?;
    let pin = (interrupt >> 8) as u8;
    if !(1..=4).contains(&pin) {
        return None;
    }
    if let Some(acpi) = acpi {
        let mut bridges = Vec::new();
        let mut index = 0;
        while let Ok(Some(device)) = pci::enumerate(port, index) {
            index += 1;
            if device.class >> 8 == 0x0604 {
                let buses = pci::read(port, device.address, 0x18, 4).ok()?;
                bridges.push(routing::Bridge {
                    address: device.address,
                    primary: buses as u8,
                    secondary: (buses >> 8) as u8,
                    subordinate: (buses >> 16) as u8,
                });
            }
        }
        if let Some(irq) = routing::resolve(
            address,
            pin,
            |index| bridges.get(index).copied(),
            |address, pin| nanami_client::power::pci_intx_route(acpi, address.packed(), pin).ok(),
        ) {
            return Some(irq);
        }
    }
    let line = (interrupt & 0xff) as Word;
    // Firmware-programmed line only; no PIIX-specific guessed PIRQ registers.
    (line != 0 && line < 24).then_some(line)
}

fn serve(candidates: Vec<Candidate>, port: Word) -> Result<(), RequestError> {
    let timer_port = candidates[0].regs.timer;
    let mut ports = Vec::new();
    let mut devices = Vec::new();
    for mut selected in candidates {
        let (physical, memory) = nanami_runtime::request_dma(ring::DMA_BYTES)?;
        let advanced = matches!(selected.family, Family::Igb | Family::I210);
        let rings = unsafe { ring::Rings::new(memory, physical as u64, advanced) };
        // I210/I211 master-disable resets/holds the DMA queues. Leave that
        // state BEFORE programming/enabling them, while global RX/TX is off.
        // The new descriptors already reference our pinned DMA allocation.
        if let Err(error) = chipset::start_queues(
            &mut selected.regs,
            selected.family,
            physical as u64,
            (physical + ring::TX_OFFSET) as u64,
            ring::RING_BYTES,
            (ring::COUNT - 1) as u32,
            || {
                let command = pci::read(port, selected.device.address, 4, 2)?;
                pci::write(port, selected.device.address, 4, 2, command | 6 | (1 << 10))
            },
        ) {
            selected.regs.write(IMC, u32::MAX);
            selected.regs.write(RCTL, 0);
            selected.regs.write(TCTL, 0);
            selected.regs.flush();
            nanami_runtime::println!(
                "[intel-net] PCI {:06x} queue setup failed: {}; RXDCTL={:#x} TXDCTL={:#x}; retaining DMA, trying other ports",
                selected.device.address.packed(), error,
                selected.regs.read(RXDCTL), selected.regs.read(TXDCTL)
            );
            // request_dma mappings have no Rust destructor. Keep these pages
            // pinned until process destruction; never reuse them for another NIC.
            continue;
        }
        devices.push(selected.device);
        ports.push(ports::Port {
            regs: selected.regs,
            rings,
            physical: physical as u64,
            mac: selected.mac,
            bdf: selected.device.address.packed(),
        });
    }
    if ports.is_empty() {
        return Err(RequestError::Unsupported);
    }
    let mut active = ports.iter().position(|port| port.carrier()).unwrap_or(0);
    // Multiple IRQs can identify the same stable notification object. Keep
    // each interrupt capability separate, including when PCI NICs share a GSI.
    let notification = nanami_runtime::PROCESS_SLOT_NOTIFICATION;
    nanami_runtime::ipc::bind_current_thread_notification(
        nanami_runtime::ipc::process_slot_descriptor(notification),
    )?;
    let mut irq_ports = Vec::new();
    let mut interrupts: Vec<(Word, Word)> = Vec::new();
    let acpi = registry::connect_service(nanami_client::power::ACPI_SERVICE, ACPI_SLOT)
        .ok()
        .map(|_| nanami_runtime::ipc::process_slot_descriptor(ACPI_SLOT));
    for (index, device) in devices.iter().enumerate() {
        let irq = if let Some(number) = interrupt_route(port, acpi, device.address) {
            if interrupts.iter().any(|(irq, _)| *irq == number) {
                true
            } else {
                let slot = 64 + index;
                match nanami_runtime::request_irq(number, notification, slot) {
                    Ok(()) => {
                        interrupts.push((number, slot));
                        true
                    }
                    Err(error) => {
                        nanami_runtime::println!(
                            "[intel-net] PCI {:06x} INTx {} unavailable: {}; timer fallback",
                            device.address.packed(),
                            number,
                            error
                        );
                        false
                    }
                }
            }
        } else {
            false
        };
        irq_ports.push(irq);
        if irq {
            let command = pci::read(port, device.address, 4, 2)?;
            pci::write(port, device.address, 4, 2, command & !(1 << 10))?;
            let regs = &mut ports[index].regs;
            let _ = regs.read(ICR);
            // Advanced I211/igb use EITR[0], not the legacy ITR address.
            let advanced = matches!(
                family(device.vendor_id, device.device_id, device.class),
                Some(Family::Igb | Family::I210)
            );
            regs.write(if advanced { 0x1680 } else { ITR }, 128); // 32.768 us, bounded rate
            regs.write(IMS, RX_INTERRUPTS);
            nanami_runtime::println!(
                "[intel-net] PCI {:06x} interrupt RX enabled",
                device.address.packed()
            );
        }
    }
    for (_, slot) in &interrupts {
        nanami_runtime::ipc::interrupt_ack(nanami_runtime::ipc::process_slot_descriptor(*slot))?;
    }
    // No busy polling. Unsupported/shared INTx routes use notification-driven
    // 2 ms polling; IRQ mode has only a slow lost-wakeup/link maintenance tick.
    timer::timer_service_interval_on_notification_milliseconds(
        timer_port,
        if irq_ports[active] { 1000 } else { 2 },
        notification,
    )?;
    let mut service = service::Service::new(ports[active].physical, ports[active].mac);
    ports[active].regs.write(RCTL, RX_CONTROL);
    ports[active].regs.write(TCTL, TX_CONTROL);
    ports[active].regs.flush();
    registry::register_net_device()?;
    nanami_runtime::println!(
        "[intel-net] ready ports={} active={} PCI {:06x} carrier={} rx={}",
        ports.len(),
        active,
        ports[active].bdf,
        ports[active].carrier(),
        if irq_ports[active] {
            "interrupt"
        } else {
            "timer"
        }
    );
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
                use nanami_client::net::*;
                let inventory = request.code == NET_DEVICE_REQUEST_CONTROL
                    && matches!(
                        request.arg0,
                        NET_DEVICE_CONTROL_GET_FEATURES
                            | NET_DEVICE_CONTROL_PORT_COUNT
                            | NET_DEVICE_CONTROL_PORT_INFO
                            | NET_DEVICE_CONTROL_SELECT_PORT
                    );
                if inventory && service.authorized(request.identifier) {
                    let previous = active;
                    reply = Some(
                        match ports::control(
                            &mut ports,
                            &mut active,
                            &mut service,
                            request.arg0,
                            request.arg1,
                        ) {
                            Ok((a, b)) => (nanami_runtime::OS_RESPONSE_OK, a, b),
                            Err(_) => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
                        },
                    );
                    if previous != active {
                        timer::timer_service_interval_on_notification_milliseconds(
                            timer_port,
                            if irq_ports[active] { 1000 } else { 2 },
                            notification,
                        )?;
                        nanami_runtime::println!(
                            "[intel-net] selected port={} PCI {:06x} carrier={} MAC-Mbps={}",
                            active,
                            ports[active].bdf,
                            ports[active].carrier(),
                            link_mbps(ports[active].regs.read(STATUS))
                        );
                    }
                } else {
                    let selected = &mut ports[active];
                    reply = Some(service.request(request, &mut selected.regs, &mut selected.rings));
                }
            }
            ServiceEvent::Notification { .. } => {
                // Deassert all NICs sharing an owned GSI before rearming it.
                for port in &mut ports {
                    let _ = port.regs.read(ICR);
                }
                let selected = &mut ports[active];
                selected.rings.reclaim_tx();
                service.notify_rx(&selected.rings);
                for (_, slot) in &interrupts {
                    nanami_runtime::ipc::interrupt_ack(
                        nanami_runtime::ipc::process_slot_descriptor(*slot),
                    )?;
                    service.notify_rx(&selected.rings); // cover the masked-interrupt window
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
    let candidates = discover(port, timer)?;
    // From this point onward, errors must retain all pinned DMA allocations.
    if let Err(error) = serve(candidates, port) {
        nanami_runtime::println!("[intel-net] stopped: {}; retaining DMA", error);
    }
    retain_dma()
}

nanami_runtime::nanami_entry!(nanami_main);
