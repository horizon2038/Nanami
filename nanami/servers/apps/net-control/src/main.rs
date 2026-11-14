#![no_std]
#![no_main]
use core::fmt::{self, Write};
use nanami_client::net::*;
use nanami_runtime::{RequestError, Word};
nanami_runtime::nanami_entry!(main);
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[net-control] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}
const HELP: &str = "net-control [show]\nnet-control dhcp\nnet-control manual ADDRESS NETMASK GATEWAY [DNS]\nnet-control mode dhcp|manual\nnet-control address|netmask|gateway|dns VALUE\nnet-control reload\nField/mode edits are saved; reload applies them. dhcp/manual save and apply.\nReload terminates existing connections. Use 0.0.0.0 for no gateway/DNS.\n";
fn main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    let mut output = Output::new();
    let result = run(&mut output);
    if let Err(error) = &result {
        let _ = writeln!(output, "net-control: {}", error);
    }
    output.flush();
    result.map_err(Into::into)
}
fn argument(index: usize) -> Option<&'static str> {
    core::str::from_utf8(nanami_runtime::process_arg(index)?).ok()
}
fn run(output: &mut Output) -> Result<(), RequestError> {
    if matches!(argument(1), Some("--help" | "help")) {
        let _ = output.write_str(HELP);
        return Ok(());
    }
    nanami_client::registry::connect_network_service(23)?;
    let port = nanami_runtime::ipc::process_slot_descriptor(23);
    let command = argument(1).unwrap_or("show");
    if command == "reload" && argument(2).is_none() {
        net_service_reload(port)?;
        let _ = writeln!(
            output,
            "Configuration reloaded; existing connections closed."
        );
        return show(port, output);
    }
    if command == "show" && argument(2).is_none() {
        return show(port, output);
    }
    let (mut settings, revision) = net_service_settings(port)?;
    let mut apply = false;
    let valid = match command {
        "dhcp" if argument(2).is_none() => {
            settings.mode = IPV4_MODE_DHCP;
            apply = true;
            true
        }
        "manual" => {
            settings.mode = IPV4_MODE_MANUAL;
            apply = true;
            match (
                argument(2).and_then(parse_ipv4),
                argument(3).and_then(parse_ipv4),
                argument(4).and_then(parse_ipv4),
            ) {
                (Some(address), Some(netmask), Some(gateway)) => {
                    settings.address = address;
                    settings.netmask = netmask;
                    settings.gateway = gateway;
                    settings.dns = match argument(5) {
                        Some(value) => parse_ipv4(value).ok_or(RequestError::InvalidArgument)?,
                        None => [0; 4],
                    };
                    argument(6).is_none()
                }
                _ => false,
            }
        }
        "mode" if argument(3).is_none() => match argument(2) {
            Some("dhcp") => {
                settings.mode = IPV4_MODE_DHCP;
                true
            }
            Some("manual") => {
                settings.mode = IPV4_MODE_MANUAL;
                true
            }
            _ => false,
        },
        "address" | "netmask" | "gateway" | "dns" if argument(3).is_none() => {
            if let Some(value) = argument(2).and_then(parse_ipv4) {
                match command {
                    "address" => settings.address = value,
                    "netmask" => settings.netmask = value,
                    "gateway" => settings.gateway = value,
                    _ => settings.dns = value,
                }
                true
            } else {
                false
            }
        }
        _ => false,
    };
    if !valid {
        let _ = output.write_str(HELP);
        return Err(RequestError::InvalidArgument);
    }
    let validation = if apply {
        settings.validate_apply()
    } else {
        settings.validate_draft()
    };
    if let Err(message) = validation {
        let _ = writeln!(output, "{}", message);
        return Err(RequestError::InvalidArgument);
    }
    net_service_save_settings(port, settings, revision)?;
    if apply {
        net_service_reload(port)?;
    }
    let _ = writeln!(
        output,
        "Settings saved{}.",
        if apply {
            " and applied"
        } else {
            "; run net-control reload to apply"
        }
    );
    show(port, output)
}
fn address(output: &mut Output, label: &str, ip: [u8; 4]) {
    let _ = writeln!(output, "{}: {}.{}.{}.{}", label, ip[0], ip[1], ip[2], ip[3]);
}
fn show(port: Word, output: &mut Output) -> Result<(), RequestError> {
    let (settings, revision) = net_service_settings(port)?;
    let (state, active_mode) = net_service_config_state(port)?;
    let _ = writeln!(
        output,
        "Saved mode: {} (revision {})",
        if settings.mode == IPV4_MODE_DHCP {
            "DHCP"
        } else {
            "manual"
        },
        revision
    );
    address(output, "Saved address", settings.address);
    address(output, "Saved netmask", settings.netmask);
    address(output, "Saved gateway", settings.gateway);
    address(output, "Saved DNS", settings.dns);
    let _ = writeln!(
        output,
        "Active mode: {} / {}",
        if active_mode == IPV4_MODE_DHCP {
            "DHCP"
        } else {
            "manual"
        },
        match state {
            IPV4_STATE_READY => "ready",
            IPV4_STATE_ACQUIRING => "acquiring",
            IPV4_STATE_FAILED => "failed",
            _ => "unconfigured",
        }
    );
    let (ip, gateway, dns) = net_service_ipv4_config(port)?;
    address(output, "Active address", ip);
    address(output, "Active netmask", net_service_ipv4_netmask(port)?);
    address(output, "Active gateway", gateway);
    address(output, "Active DNS", dns);
    Ok(())
}
struct Output {
    bytes: [u8; 2048],
    len: usize,
}
impl Output {
    fn new() -> Self {
        Self {
            bytes: [0; 2048],
            len: 0,
        }
    }
    fn flush(&self) {
        let terminal = nanami_runtime::process_env_value(b"NANAMI_TERMINAL_ID")
            .and_then(|b| core::str::from_utf8(b).ok())
            .and_then(|s| s.parse::<Word>().ok());
        let sent = (|| -> Result<(), RequestError> {
            let id = terminal.ok_or(RequestError::InvalidArgument)?;
            nanami_client::registry::connect_terminal_service(22)?;
            let port = nanami_runtime::ipc::process_slot_descriptor(22);
            let (base, bytes) = nanami_client::terminal::terminal_attach_shared_memory(port, 4096)?;
            if base == 0 || bytes < self.len {
                return Err(RequestError::Protocol);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(self.bytes.as_ptr(), base as *mut u8, self.len);
            }
            let written = nanami_client::terminal::terminal_write_output(port, id, 0, self.len)?;
            if written != self.len {
                return Err(RequestError::Transport);
            }
            Ok(())
        })();
        if sent.is_err() {
            for byte in &self.bytes[..self.len] {
                nanami_runtime::debug::write_char(*byte as char);
            }
        }
    }
}
impl Write for Output {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}
