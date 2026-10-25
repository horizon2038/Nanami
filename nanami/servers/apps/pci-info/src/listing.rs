use core::fmt::{self, Write};
use nanami_client::pci::Device;

pub const USAGE: &str = "usage: pci-info [-n|-nn] [-v]\n  -n   numeric IDs only\n  -nn  names and numeric IDs (default)\n  -v   include programming interface and command/status\n";

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Options {
    pub numeric: bool,
    pub verbose: bool,
    pub help: bool,
}
impl Options {
    pub fn parse<'a>(args: impl Iterator<Item = &'a [u8]>) -> Result<Self, &'static str> {
        let mut options = Self::default();
        for arg in args {
            match arg {
                b"-n" => options.numeric = true,
                b"-nn" => options.numeric = false,
                b"-v" => options.verbose = true,
                b"-h" | b"--help" => options.help = true,
                _ => return Err("pci-info: unknown option\n"),
            }
        }
        Ok(options)
    }
}

pub struct Names<'a> {
    pub class: &'a str,
    pub vendor: &'a str,
    pub device: &'a str,
}

pub fn write_device(
    out: &mut impl Write,
    device: Device,
    revision: u8,
    names: Names<'_>,
    options: &Options,
) -> fmt::Result {
    let address = device.address;
    write!(
        out,
        "{:04x}:{:02x}:{:02x}.{} ",
        address.segment, address.bus, address.device, address.function
    )?;
    if options.numeric {
        write!(
            out,
            "{:04x}: {:04x}:{:04x}",
            device.class >> 8,
            device.vendor_id,
            device.device_id
        )?;
    } else {
        let class = if names.class.is_empty() {
            "Unknown class"
        } else {
            names.class
        };
        let vendor = if names.vendor.is_empty() {
            "Unknown vendor"
        } else {
            names.vendor
        };
        let product = if names.device.is_empty() {
            "Unknown device"
        } else {
            names.device
        };
        write!(
            out,
            "{} [{:04x}]: {} {} [{:04x}:{:04x}]",
            class,
            device.class >> 8,
            vendor,
            product,
            device.vendor_id,
            device.device_id
        )?;
    }
    writeln!(out, " (rev {:02x})", revision)
}

pub fn write_details(
    out: &mut impl Write,
    device: Device,
    interface: &str,
    command_status: u32,
) -> fmt::Result {
    write!(
        out,
        "  class={:06x} prog-if={:02x}",
        device.class, device.class as u8
    )?;
    if !interface.is_empty() {
        write!(out, " ({})", interface)?;
    }
    writeln!(out, " header={:02x}", device.header)?;
    writeln!(
        out,
        "  command={:04x} status={:04x}",
        command_status as u16,
        command_status >> 16
    )
}
