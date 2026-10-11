#![no_std]
#![no_main]

extern crate alloc;

#[cfg(not(target_arch = "x86_64"))]
compile_error!("acpi-server currently supports x86_64 only");

mod failure;
mod firmware;
mod host;
mod power;
mod routing;

use nanami_runtime::{ipc::ServiceEvent, RequestError, Word};
use nanami_client::{power::ACPI_SERVICE, registry, vfs};

const SERVICE_SLOT: Word = 20;
const VFS_SLOT: Word = 23;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[acpi-server] panic: {}", info);
    failure::stop()
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    registry::connect_service(nanami_client::device::DEVICE_MANAGER_SERVICE, 24)?;
    let rsdp = nanami_client::device::acpi_rsdp(nanami_runtime::ipc::process_slot_descriptor(24))?;
    // Register before loading AML. Routing runs on notification without holding
    // a driver-manager RPC; power methods remain deferred until shutdown.
    registry::register_service(ACPI_SERVICE)?;
    nanami_runtime::ipc::bind_current_thread_notification(nanami_runtime::ipc::process_slot_descriptor(
        nanami_runtime::PROCESS_SLOT_NOTIFICATION,
    ))?;
    nanami_runtime::println!("[acpi-server] ready rsdp={:#x} (S5 on demand)", rsdp);
    let port = nanami_runtime::ipc::process_slot_descriptor(SERVICE_SLOT);
    let mut reply = None;
    let mut state = State {
        host: None,
        firmware: None,
        power: None,
        vfs_port: None,
    };
    loop {
        let event = match reply.take() {
            Some(status) => nanami_runtime::ipc::service_reply_receive_event(port, status, 0, 0)?,
            None => nanami_runtime::ipc::service_receive_event(port)?,
        };
        if let ServiceEvent::Notification { identifier, .. } = event {
            if identifier & nanami_client::device::ACPI_ROUTE_NOTIFICATION != 0 {
                state.route_usb(rsdp);
            }
            continue;
        }
        if let ServiceEvent::Request(request) = event {
            failure::request_active(true);
            reply = Some(
                if request.code != nanami_client::power::POWER_REQUEST_SHUTDOWN {
                    nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT
                } else {
                    match state.shutdown(rsdp) {
                        Err(error) => {
                            nanami_runtime::println!("[acpi-server] shutdown aborted: {}", error);
                            nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION
                        }
                        Ok(()) => nanami_runtime::OS_RESPONSE_FATAL,
                    }
                },
            );
            failure::request_active(false);
        }
    }
}

struct State {
    host: Option<host::Host>,
    firmware: Option<firmware::Firmware>,
    power: Option<power::Power<host::Host>>,
    vfs_port: Option<Word>,
}

impl State {
    fn firmware(&mut self, rsdp: Word) -> Result<&firmware::Firmware, RequestError> {
        if self.host.is_none() {
            self.host = Some(host::Host::new()?);
        }
        if self.firmware.is_none() {
            self.firmware = Some(
                firmware::load(self.host.as_ref().unwrap().clone(), rsdp).map_err(|error| {
                    nanami_runtime::println!("[acpi-server] firmware: {:?}", error);
                    RequestError::Unsupported
                })?,
            );
        }
        Ok(self.firmware.as_ref().unwrap())
    }

    fn route_usb(&mut self, rsdp: Word) {
        let manager = nanami_runtime::ipc::process_slot_descriptor(24);
        let router = self.firmware(rsdp).and_then(|firmware| {
            routing::Routing::new(firmware.interpreter.clone()).map_err(|error| {
                nanami_runtime::println!("[acpi-server] PCI routing init: {}", error);
                RequestError::Unsupported
            })
        });
        for index in 0..4 {
            let Ok(Some((bdf, pin))) = nanami_client::device::usb_pci_route(manager, index)
            else {
                continue;
            };
            let irq = router.as_ref().ok().and_then(|router| {
                match router.route(bdf, pin).and_then(|route| {
                    route.supported_irq().map_err(|error| {
                        nanami_runtime::println!(
                            "[acpi-server] USB PCI {:06x} unsupported route {:?}",
                            bdf,
                            route
                        );
                        error
                    })
                }) {
                    Ok(irq) => {
                        nanami_runtime::println!(
                            "[acpi-server] USB PCI {:06x} pin={} -> GSI={} level/low",
                            bdf,
                            pin,
                            irq
                        );
                        Some(irq)
                    }
                    Err(error) => {
                        nanami_runtime::println!(
                            "[acpi-server] USB PCI {:06x} pin={} route unavailable: {}",
                            bdf,
                            pin,
                            error
                        );
                        None
                    }
                }
            });
            if let Err(error) = nanami_client::device::report_usb_irq(manager, index, irq) {
                nanami_runtime::println!("[acpi-server] USB IRQ report failed: {}", error);
            }
        }
    }

    fn shutdown(&mut self, rsdp: Word) -> Result<(), RequestError> {
        if self.power.is_none() {
            self.power = Some(self.firmware(rsdp)?.power().map_err(|error| {
                nanami_runtime::println!("[acpi-server] S5 initialization: {:?}", error);
                RequestError::Unsupported
            })?);
        }
        let Self {
            power, vfs_port, ..
        } = self;
        let power = power.as_ref().unwrap();
        nanami_runtime::println!("[acpi-server] S5 types={:?}", power.sleep_types());
        if vfs_port.is_none() {
            registry::connect_vfs_service(VFS_SLOT)?;
            *vfs_port = Some(nanami_runtime::ipc::process_slot_descriptor(VFS_SLOT));
        }
        let vfs_port = vfs_port.unwrap();
        // Prepare firmware only after all storage writes are durable. Mutations
        // stay frozen until S5, or are re-enabled if the transition fails.
        vfs::vfs_freeze(vfs_port)?;
        failure::storage_frozen(true);
        let result = power.prepare().and_then(|()| {
            nanami_runtime::println!("[acpi-server] entering S5");
            power.enter_s5()
        });
        if let Err(error) = power.restore_mode() {
            nanami_runtime::println!(
                "[acpi-server] restoring firmware power mode failed: {:?}",
                error
            );
        }
        vfs::vfs_thaw(vfs_port)?;
        failure::storage_frozen(false);
        result.map_err(|error| {
            nanami_runtime::println!("[acpi-server] S5 failed: {:?}", error);
            RequestError::Unsupported
        })
    }
}

nanami_runtime::nanami_entry!(nanami_main);
