#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::{cell::RefCell, fmt::Write};
pub type Word = usize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Transport,
}
#[path = "../../nanami/servers/apps/pci-info/src/listing.rs"]
mod listing;
#[path = "../../nanami/servers/apps/pci-info/src/output.rs"]
mod output;
#[path = "../../nanami/servers/uapi/src/pci.rs"]
pub mod pci;

struct Terminal {
    environment: Option<&'static [u8]>,
    shared: Box<[u8; 4096]>,
    received: Vec<u8>,
    limit: usize,
    blocked: usize,
    bad_count: bool,
}
impl Default for Terminal {
    fn default() -> Self {
        Self {
            environment: Some(b"17"),
            shared: Box::new([0; 4096]),
            received: Vec::new(),
            limit: 7,
            blocked: 2,
            bad_count: false,
        }
    }
}
thread_local! { static TERMINAL: RefCell<Terminal> = RefCell::new(Terminal::default()); }
pub fn process_env_value(_: &[u8]) -> Option<&'static [u8]> {
    TERMINAL.with(|terminal| terminal.borrow().environment)
}
pub fn yield_now() {}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub mod registry {
    pub fn connect_terminal_service(slot: usize) -> Result<(), crate::RequestError> {
        assert_eq!(slot, 22);
        Ok(())
    }
}
pub mod debug {
    pub fn write_char(byte: char) {
        super::TERMINAL.with(|terminal| terminal.borrow_mut().received.push(byte as u8));
    }
}
pub mod terminal {
    use super::*;
    pub fn terminal_attach_shared_memory(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
        TERMINAL.with(|terminal| Ok((terminal.borrow_mut().shared.as_mut_ptr() as Word, 4096)))
    }
    pub fn terminal_write_output(
        port: Word,
        id: Word,
        offset: Word,
        count: Word,
    ) -> Result<Word, RequestError> {
        assert_eq!((port, id, offset), (22, 17, 0));
        TERMINAL.with(|terminal| {
            let mut terminal = terminal.borrow_mut();
            if terminal.bad_count {
                return Ok(count + 1);
            }
            if terminal.blocked != 0 {
                terminal.blocked -= 1;
                return Ok(0);
            }
            let written = count.min(terminal.limit);
            let data = terminal.shared[..written].to_vec();
            terminal.received.extend_from_slice(&data);
            Ok(written)
        })
    }
}

fn device() -> pci::Device {
    pci::Device {
        address: pci::Address::new(0, 2, 0),
        vendor_id: 0x8086,
        device_id: 0x3e92,
        class: 0x030000,
        header: 0,
    }
}

#[test]
fn names_and_numeric_listing_always_preserve_the_hardware_ids() {
    let mut text = String::new();
    listing::write_device(
        &mut text,
        device(),
        2,
        listing::Names {
            class: "VGA compatible controller",
            vendor: "Intel Corporation",
            device: "UHD Graphics 630",
        },
        &listing::Options::default(),
    )
    .unwrap();
    assert_eq!(text, "0000:00:02.0 VGA compatible controller [0300]: Intel Corporation UHD Graphics 630 [8086:3e92] (rev 02)\n");
    text.clear();
    listing::write_device(
        &mut text,
        device(),
        2,
        listing::Names {
            class: "",
            vendor: "",
            device: "",
        },
        &listing::Options {
            numeric: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(text, "0000:00:02.0 0300: 8086:3e92 (rev 02)\n");
}

#[test]
fn unknown_entries_have_readable_fallbacks_and_verbose_output_is_non_destructive() {
    let mut text = String::new();
    listing::write_device(
        &mut text,
        device(),
        0,
        listing::Names {
            class: "",
            vendor: "",
            device: "",
        },
        &Default::default(),
    )
    .unwrap();
    assert!(text.contains("Unknown class [0300]: Unknown vendor Unknown device [8086:3e92]"));
    listing::write_details(&mut text, device(), "VGA controller", 0xf9000007).unwrap();
    assert!(text.contains("class=030000 prog-if=00 (VGA controller) header=00"));
    assert!(text.contains("command=0007 status=f900"));
}

#[test]
fn cli_options_are_explicit_and_unknown_options_are_not_ignored() {
    let options = listing::Options::parse([b"-n".as_slice(), b"-v"].into_iter()).unwrap();
    assert!(options.numeric && options.verbose && !options.help);
    let options =
        listing::Options::parse([b"-n".as_slice(), b"-nn", b"--help"].into_iter()).unwrap();
    assert!(!options.numeric && options.help);
    assert!(listing::Options::parse([b"--probe-bars".as_slice()].into_iter()).is_err());
}

#[test]
fn buffered_terminal_output_survives_partial_writes_and_temporary_backpressure() {
    let mut output = output::Output::new().unwrap();
    let text = "PCI Ω\n".repeat(300);
    output.write_str(&text).unwrap();
    output.flush().unwrap();
    TERMINAL.with(|terminal| assert_eq!(terminal.borrow().received, text.as_bytes()));
}

#[test]
fn invalid_terminal_metadata_and_oversized_write_results_are_errors() {
    TERMINAL.with(|terminal| terminal.borrow_mut().environment = Some(b"17x"));
    assert!(matches!(
        output::Output::new(),
        Err(RequestError::InvalidArgument)
    ));
    TERMINAL.with(|terminal| {
        let mut terminal = terminal.borrow_mut();
        terminal.environment = Some(b"17");
        terminal.bad_count = true;
    });
    let mut output = output::Output::new().unwrap();
    output.write_str("pci\n").unwrap();
    assert_eq!(output.flush(), Err(RequestError::Protocol));
}

#[test]
fn nonterminal_launches_write_to_the_serial_console() {
    TERMINAL.with(|terminal| terminal.borrow_mut().environment = None);
    let mut output = output::Output::new().unwrap();
    output.write_str("PCI test\n").unwrap();
    output.flush().unwrap();
    TERMINAL.with(|terminal| assert_eq!(terminal.borrow().received, b"PCI test\n"));
}
