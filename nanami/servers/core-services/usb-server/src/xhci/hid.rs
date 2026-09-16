//! Select mouse Report Protocol only with a validated layout. Keyboard remains
//! in Boot Protocol. No interrupt transfers are submitted until this completes.
use super::*;
use crate::usb::hid::MouseReport;

impl Controller {
    pub(super) fn configure_hid(
        &mut self,
        slot: u8,
        device: &mut Device,
        input: &mut Input,
    ) -> Result<(), RequestError> {
        for index in 0..device.endpoints.len() {
            let interface = device.endpoints[index].interface;
            if interface.protocol == 2 {
                if let Some(layout) = self.mouse_report(slot, device, interface, input)? {
                    match self.control(slot, device, 0x21, 11, 1, interface.number as u16, 0, input)
                    {
                        Ok(_) => {
                            device.endpoints[index].mouse.set_report_protocol(layout);
                            libnanami::println!(
                                "[usb-server] mouse interface={} protocol=report wheel=true",
                                interface.number
                            );
                            continue;
                        }
                        Err(RequestError::Unsupported) => {} // STALL recovered by control().
                        Err(error) => return Err(error),
                    }
                }
                libnanami::println!(
                    "[usb-server] mouse interface={} protocol=boot wheel=false (report unavailable)",
                    interface.number
                );
            }
            self.control(slot, device, 0x21, 11, 0, interface.number as u16, 0, input)?;
            if interface.protocol == 1 {
                self.control(slot, device, 0x21, 10, 0, interface.number as u16, 0, input)?;
            }
        }
        Ok(())
    }

    fn mouse_report(
        &mut self,
        slot: u8,
        device: &mut Device,
        interface: BootInterface,
        input: &mut Input,
    ) -> Result<Option<MouseReport>, RequestError> {
        let length = interface.report_length as usize;
        if !(1..=4096).contains(&length) {
            return Ok(None);
        }
        match self.control(
            slot,
            device,
            0x81,
            6,
            0x2200,
            interface.number as u16,
            length,
            input,
        ) {
            Ok(actual) if actual == length => {
                let bytes = unsafe {
                    core::slice::from_raw_parts(device.dma.virtual_at(0x3000) as *const u8, length)
                };
                Ok(MouseReport::parse(bytes, interface.packet_size as usize)
                    .ok()
                    .filter(MouseReport::has_wheel))
            }
            Ok(_) | Err(RequestError::Unsupported) => Ok(None),
            // Do not reuse EP0/DMA after a transport failure to attempt a fallback.
            Err(error) => Err(error),
        }
    }
}
