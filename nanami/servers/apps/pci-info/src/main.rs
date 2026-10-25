#![no_std]
#![no_main]
mod listing;
mod output;
use core::fmt::Write;
use nanami_client::pci;
use nanami_runtime::{RequestError, Word};

const PCI_SLOT: Word = 16;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[pci-info] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {}
}

fn list(out: &mut output::Output, options: &listing::Options) -> Result<(), RequestError> {
    // This command also builds on platforms without a PCI service: report that
    // immediately rather than treating an optional provider as a boot dependency.
    if let Err(error) = nanami_client::registry::connect_service(pci::PCI_SERVICE, PCI_SLOT) {
        let _ = writeln!(out, "pci-info: PCI service unavailable: {}", error);
        return Err(error);
    }
    let port = nanami_runtime::ipc::process_slot_descriptor(PCI_SLOT);
    let mut index = 0;
    let mut class = [0; pci::PCI_NAME_BUFFER_BYTES];
    let mut vendor = [0; pci::PCI_NAME_BUFFER_BYTES];
    let mut product = [0; pci::PCI_NAME_BUFFER_BYTES];
    while let Some(device) = pci::enumerate(port, index)? {
        let names = if options.numeric {
            listing::Names {
                class: "",
                vendor: "",
                device: "",
            }
        } else {
            let mut class_name = pci::name(port, index, pci::PCI_NAME_SUBCLASS, &mut class)?;
            if class_name.is_empty() {
                class_name = pci::name(port, index, pci::PCI_NAME_CLASS, &mut class)?;
            }
            listing::Names {
                class: class_name,
                vendor: pci::name(port, index, pci::PCI_NAME_VENDOR, &mut vendor)?,
                device: pci::name(port, index, pci::PCI_NAME_DEVICE, &mut product)?,
            }
        };
        let revision = pci::read(port, device.address, 8, 1)? as u8;
        listing::write_device(out, device, revision, names, options)
            .map_err(|_| RequestError::Transport)?;
        if options.verbose {
            let interface = if options.numeric {
                ""
            } else {
                pci::name(port, index, pci::PCI_NAME_INTERFACE, &mut class)?
            };
            let command_status = pci::read(port, device.address, 4, 4)?;
            listing::write_details(out, device, interface, command_status)
                .map_err(|_| RequestError::Transport)?;
        }
        out.flush()?;
        index += 1;
    }
    if index == 0 {
        out.write_str("pci-info: no PCI devices\n")
            .map_err(|_| RequestError::Transport)?;
    }
    Ok(())
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    let mut out = output::Output::new()?;
    let mut index = 1;
    let args = core::iter::from_fn(|| {
        let arg = nanami_runtime::process_arg(index);
        index += 1;
        arg
    });
    let result = match listing::Options::parse(args) {
        Ok(options) if options.help => out
            .write_str(listing::USAGE)
            .map_err(|_| RequestError::Transport),
        Ok(options) => list(&mut out, &options),
        Err(message) => {
            let _ = out.write_str(message);
            let _ = out.write_str(listing::USAGE);
            Err(RequestError::InvalidArgument)
        }
    };
    if let Err(error) = result {
        let _ = writeln!(out, "pci-info: {}", error);
    }
    out.flush()?;
    result.map_err(Into::into)
}
nanami_runtime::nanami_entry!(nanami_main);
