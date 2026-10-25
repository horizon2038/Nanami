//! ACPI resolves firmware routing; the manager owns device assignment and policy.
use nanami_client::device::*;
use nanami_runtime::{ipc::ServiceRequest, RequestError, Word};

const ACPI_NOTIFICATION_SLOT: Word = 22;
pub const MAX_CONTROLLERS: usize = 4;

#[derive(Clone, Copy, Default)]
struct Route {
    address: Word,
    pin: u8,
    pending: bool,
}

#[derive(Default)]
pub struct Routes {
    devices: [Route; MAX_CONTROLLERS],
    notification_ready: bool,
    started: bool,
}

impl Routes {
    pub fn add(&mut self, index: usize, address: Word, pin: u8, irq: Option<Word>) {
        self.devices[index] = Route {
            address,
            pin,
            pending: (1..=4).contains(&pin) && irq.is_none(),
        };
    }

    pub fn query(&mut self, index: usize, acpi_pid: Word) -> Result<bool, RequestError> {
        let route = self
            .devices
            .get(index)
            .ok_or(RequestError::InvalidArgument)?;
        if !route.pending {
            return Ok(true);
        }
        if acpi_pid == 0 {
            self.devices[index].pending = false;
            return Ok(true);
        }
        if !self.started {
            // USB requests this only after timer-service is ready. Start AML
            // asynchronously: neither manager nor USB holds an RPC to ACPI.
            if !self.notification_ready {
                nanami_runtime::request_notification_port_copy(
                    acpi_pid,
                    nanami_runtime::PROCESS_SLOT_NOTIFICATION,
                    ACPI_NOTIFICATION_SLOT,
                    ACPI_ROUTE_NOTIFICATION,
                )?;
                self.notification_ready = true;
            }
            nanami_runtime::ipc::notification_notify(
                nanami_runtime::ipc::process_slot_descriptor(ACPI_NOTIFICATION_SLOT),
            )?;
            self.started = true;
        }
        Ok(false)
    }

    pub fn handle(
        &mut self,
        request: ServiceRequest,
        acpi_pid: Word,
        controllers: &mut [Option<UsbControllerResource>; MAX_CONTROLLERS],
    ) -> (Word, Word, Word) {
        if acpi_pid == 0 || request.identifier != acpi_pid {
            return (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
        }
        let Some(route) = self.devices.get_mut(request.arg0) else {
            return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
        };
        if request.code == DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE {
            return (
                nanami_runtime::OS_RESPONSE_OK,
                route.address,
                if route.pending { route.pin as Word } else { 0 },
            );
        }
        let irq = request.arg1;
        // Current A9N HAL explicitly configures GSI16..23 as level/active-low.
        // Do not silently feed an arbitrary AML GSI into its ISA-default path.
        if !route.pending || (irq != 0 && !(16..=23).contains(&irq)) {
            return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
        }
        let Some(controller) = controllers[request.arg0].as_mut() else {
            return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
        };
        controller.irq = (irq != 0).then_some(irq);
        route.pending = false;
        if irq == 0 {
            nanami_runtime::println!(
                "[driver-manager] USB PCI {:06x} no usable ACPI INTx route; polling fallback",
                route.address
            );
        } else {
            nanami_runtime::println!(
                "[driver-manager] USB PCI {:06x} ACPI INTx GSI={}",
                route.address,
                irq
            );
        }
        (nanami_runtime::OS_RESPONSE_OK, 0, 0)
    }
}
