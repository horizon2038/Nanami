//! Real configuration transactions with only storage/device/clock I/O mocked.
#![allow(dead_code, unused_imports)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use std::println;
use std::{cell::RefCell, mem};
pub type Word = usize;
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub mod net {
    pub use nanami_uapi::net::*;
}
use net::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    Unsupported,
    InvalidArgument,
    Protocol,
    Transport,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
pub mod ipc {
    use super::*;
    #[derive(Default)]
    pub struct ServiceRequest {
        pub identifier: Word,
        pub arg0: Word,
        pub arg1: Word,
        pub arg2: Word,
        pub arg3: Word,
    }
}
pub mod timer {
    use super::*;
    pub fn timer_service_monotonic_ticks(_: Word) -> Result<(Word, Word), RequestError> {
        Ok((0, 1000))
    }
}
#[derive(Default)]
struct Io {
    settings: Option<(Ipv4Settings, Word)>,
    fail: bool,
    connect_fail: bool,
    resets: usize,
    notifications: usize,
    port_changed: bool,
}
thread_local! { static IO: RefCell<Io> = RefCell::new(Io::default()); }
mod settings_store {
    use super::*;
    pub struct Store;
    impl Store {
        pub fn connect() -> Result<Self, RequestError> {
            if IO.with(|io| io.borrow().connect_fail) {
                return Err(RequestError::Transport);
            }
            Ok(Self)
        }
        pub fn load(&self) -> Result<(Ipv4Settings, Word), RequestError> {
            IO.with(|io| {
                let io = io.borrow();
                if io.fail {
                    return Err(RequestError::Transport);
                }
                Ok(io.settings.unwrap_or((Ipv4Settings::DEFAULT, 0)))
            })
        }
        pub fn save(&self, settings: Ipv4Settings, revision: Word) -> Result<(), RequestError> {
            IO.with(|io| {
                let mut io = io.borrow_mut();
                if io.fail {
                    return Err(RequestError::Transport);
                }
                io.settings = Some((settings, revision));
                Ok(())
            })
        }
    }
}
mod dhcp {
    use super::*;
    pub struct Acquisition(bool);
    impl Acquisition {
        pub const OFF: Self = Self(false);
        pub fn active(&self) -> bool {
            self.0
        }
        pub fn start(r: &mut NetRuntime, _: Word) -> Self {
            r.dhcp_waiting = true;
            Self(true)
        }
        pub fn poll(&mut self, _: &mut NetRuntime, _: Word) -> Word {
            IPV4_STATE_ACQUIRING
        }
    }
}
#[derive(PartialEq, Debug)]
struct ArpCache(bool);
impl ArpCache {
    const EMPTY: Self = Self(false);
}
macro_rules! queue {
    ($name:ident) => {
        struct $name;
        impl $name {
            fn new() -> Self {
                Self
            }
        }
    };
}
queue!(UdpRxQueue);
queue!(IcmpRxQueue);
queue!(RawRxQueue);
const TCP_MAX_CONNECTIONS: usize = 4;
mod tcp {
    use super::*;
    pub fn tcp_reset(_: &mut NetRuntime, _: usize) {
        IO.with(|io| io.borrow_mut().resets += 1);
    }
}
mod arp {
    use super::*;
    pub fn emit_gratuitous_arp_reply(_: &NetRuntime) -> Result<(), RequestError> {
        Ok(())
    }
}
fn refresh_dhcp_port(_: &mut NetRuntime, _: bool) -> bool {
    IO.with(|io| mem::take(&mut io.borrow_mut().port_changed))
}
fn notify_client_sessions(_: &NetRuntime) {
    IO.with(|io| io.borrow_mut().notifications += 1);
}
fn map_request_error_to_status(_: RequestError) -> Word {
    5
}
struct NetRuntime {
    configuration: configuration::Configuration,
    timer_port: Option<Word>,
    ip: [u8; 4],
    subnet_mask: [u8; 4],
    gateway_ip: [u8; 4],
    dns_ip: [u8; 4],
    arp: ArpCache,
    udp_rx: UdpRxQueue,
    icmp_rx: IcmpRxQueue,
    raw_rx: RawRxQueue,
    dns_waiting: bool,
    dns_answer_valid: bool,
    dhcp_waiting: bool,
}
#[path = "../../nanami/servers/apps/net-server/src/app/configuration.rs"]
mod configuration;
fn settings() -> Ipv4Settings {
    Ipv4Settings {
        mode: IPV4_MODE_MANUAL,
        address: [10, 0, 2, 15],
        netmask: [255, 255, 255, 0],
        gateway: [10, 0, 2, 2],
        dns: [10, 0, 2, 3],
    }
}
fn setup() -> NetRuntime {
    IO.with(|io| *io.borrow_mut() = Io::default());
    NetRuntime {
        configuration: configuration::Configuration::new(),
        timer_port: Some(1),
        ip: [10, 1, 0, 5],
        subnet_mask: [255, 255, 0, 0],
        gateway_ip: [10, 1, 0, 1],
        dns_ip: [10, 1, 0, 1],
        arp: ArpCache(true),
        udp_rx: UdpRxQueue,
        icmp_rx: IcmpRxQueue,
        raw_rx: RawRxQueue,
        dns_waiting: true,
        dns_answer_valid: true,
        dhcp_waiting: false,
    }
}
fn save(r: &mut NetRuntime, s: Ipv4Settings, revision: Word) -> (Word, Word, Word) {
    configuration::save(
        r,
        ipc::ServiceRequest {
            identifier: 10,
            arg0: s.mode,
            arg1: pack_ipv4(s.address) as Word | ((pack_ipv4(s.netmask) as Word) << 32),
            arg2: pack_ipv4(s.gateway) as Word | ((pack_ipv4(s.dns) as Word) << 32),
            arg3: revision,
        },
    )
}
#[test]
fn unavailable_storage_is_not_treated_as_a_missing_settings_file() {
    let _ = setup();
    IO.with(|io| io.borrow_mut().connect_fail = true);
    let configuration = configuration::Configuration::new();
    assert_eq!(configuration.state, IPV4_STATE_FAILED);
}

#[test]
fn dhcp_without_a_clock_fails_instead_of_waiting_forever() {
    let mut r = setup();
    r.timer_port = None;
    configuration::apply(&mut r, Ipv4Settings::DEFAULT);
    assert_eq!(r.ip, [0; 4]);
    assert_eq!(r.configuration.state, IPV4_STATE_FAILED);
    assert!(!r.configuration.acquisition.active());
}

#[test]
fn failed_save_and_stale_revision_never_replace_the_saved_or_active_settings() {
    let mut r = setup();
    let active = r.ip;
    IO.with(|io| io.borrow_mut().fail = true);
    assert_eq!(save(&mut r, settings(), 0).0, 5);
    assert_eq!(r.configuration.revision, 0);
    assert_eq!(r.configuration.draft, Ipv4Settings::DEFAULT);
    IO.with(|io| io.borrow_mut().fail = false);
    assert_eq!(save(&mut r, settings(), 0), (0, 1, 0));
    assert_eq!(r.ip, active); // save alone cannot change an active flow
    assert_eq!(
        save(&mut r, Ipv4Settings::DEFAULT, 0).0,
        NET_SERVICE_RESPONSE_CONFIG_CONFLICT
    );
    assert_eq!(r.configuration.draft, settings());
}
#[test]
fn invalid_reload_preserves_active_state_and_valid_reload_resets_flows() {
    let mut r = setup();
    let active = r.ip;
    let invalid = Ipv4Settings {
        mode: IPV4_MODE_MANUAL,
        ..Ipv4Settings::DEFAULT
    };
    assert_eq!(save(&mut r, invalid, 0).0, 0);
    assert_eq!(
        configuration::reload(&mut r),
        Err(RequestError::InvalidArgument)
    );
    assert_eq!(r.ip, active);
    assert_eq!(r.arp, ArpCache(true));
    IO.with(|io| assert_eq!(io.borrow().resets, 0));
    assert_eq!(save(&mut r, settings(), 1).0, 0);
    configuration::reload(&mut r).unwrap();
    assert_eq!(r.ip, settings().address);
    assert_eq!(r.configuration.state, IPV4_STATE_READY);
    assert_eq!(r.arp, ArpCache::EMPTY);
    assert!(!r.dns_waiting && !r.dns_answer_valid);
    IO.with(|io| assert_eq!(io.borrow().resets, TCP_MAX_CONNECTIONS));
    // Saving an incomplete draft must not leak it into late-carrier recovery.
    assert_eq!(save(&mut r, invalid, 2).0, 0);
    IO.with(|io| io.borrow_mut().port_changed = true);
    configuration::maintenance(&mut r);
    assert_eq!(r.ip, settings().address);
}
#[test]
fn switching_to_dhcp_clears_old_routes_without_blocking_and_manual_cancels_acquisition() {
    let mut r = setup();
    configuration::apply(&mut r, Ipv4Settings::DEFAULT);
    assert_eq!(r.ip, [0; 4]);
    assert_eq!(r.gateway_ip, [0; 4]);
    assert_eq!(r.configuration.state, IPV4_STATE_ACQUIRING);
    assert!(r.configuration.acquisition.active() && r.dhcp_waiting);
    configuration::apply(&mut r, settings());
    assert!(!r.configuration.acquisition.active());
    assert!(!r.dhcp_waiting);
    assert_eq!(r.ip, settings().address);
}
