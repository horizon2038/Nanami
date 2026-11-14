use super::*;
use nanami_client::net::*;
pub struct Configuration {
    pub draft: Ipv4Settings,
    pub revision: Word,
    pub active_mode: Word,
    pub state: Word,
    pub acquisition: dhcp::Acquisition,
    store: Option<settings_store::Store>,
}
impl Configuration {
    pub fn new() -> Self {
        let (store, loaded) = match settings_store::Store::connect() {
            Ok(store) => {
                let loaded = store.load();
                (Some(store), loaded)
            }
            Err(error) => (None, Err(error)),
        };
        let (draft, revision, state) = match loaded {
            Ok((draft, revision)) => (draft, revision, IPV4_STATE_UNCONFIGURED),
            Err(error) => {
                nanami_runtime::println!("[net-server] configuration load failed: {}", error);
                (Ipv4Settings::DEFAULT, 0, IPV4_STATE_FAILED)
            }
        };
        Self {
            draft,
            revision,
            active_mode: draft.mode,
            state,
            acquisition: dhcp::Acquisition::OFF,
            store,
        }
    }
}
pub fn save(
    runtime: &mut NetRuntime,
    request: nanami_runtime::ipc::ServiceRequest,
) -> (Word, Word, Word) {
    if request.identifier == 0 {
        return (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    if request.arg3 != runtime.configuration.revision {
        return (
            NET_SERVICE_RESPONSE_CONFIG_CONFLICT,
            runtime.configuration.revision,
            0,
        );
    }
    let settings = Ipv4Settings {
        mode: request.arg0,
        address: unpack_ipv4(request.arg1),
        netmask: unpack_ipv4(request.arg1 >> 32),
        gateway: unpack_ipv4(request.arg2),
        dns: unpack_ipv4(request.arg2 >> 32),
    };
    if settings.validate_draft().is_err() {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(revision) = runtime.configuration.revision.checked_add(1) else {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    let Some(store) = runtime.configuration.store.as_ref() else {
        return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    if let Err(error) = store.save(settings, revision) {
        return (map_request_error_to_status(error), 0, 0);
    }
    runtime.configuration.draft = settings;
    runtime.configuration.revision = revision;
    nanami_runtime::println!(
        "[net-server] settings saved revision={} mode={}",
        revision,
        settings.mode
    );
    (nanami_runtime::OS_RESPONSE_OK, revision, 0)
}
pub fn reload(runtime: &mut NetRuntime) -> Result<(), RequestError> {
    let store = runtime
        .configuration
        .store
        .as_ref()
        .ok_or(RequestError::Unsupported)?;
    let (settings, revision) = store.load()?;
    settings
        .validate_apply()
        .map_err(|_| RequestError::InvalidArgument)?;
    if settings.mode == IPV4_MODE_DHCP && runtime.timer_port.is_none() {
        return Err(RequestError::Unsupported);
    }
    runtime.configuration.draft = settings;
    runtime.configuration.revision = revision;
    apply(runtime, settings);
    Ok(())
}
pub fn apply(runtime: &mut NetRuntime, settings: Ipv4Settings) {
    // Preserve listening sockets/SHM ownership, but never reuse a flow/ARP
    // entry learned under the old IP or route. IDs retain their generations.
    for index in 0..TCP_MAX_CONNECTIONS {
        tcp::tcp_reset(runtime, index);
    }
    runtime.arp = ArpCache::EMPTY;
    runtime.udp_rx = UdpRxQueue::new();
    runtime.icmp_rx = IcmpRxQueue::new();
    runtime.raw_rx = RawRxQueue::new();
    runtime.dns_waiting = false;
    runtime.dns_answer_valid = false;
    runtime.dhcp_waiting = false;
    runtime.configuration.acquisition = dhcp::Acquisition::OFF;
    runtime.configuration.active_mode = settings.mode;
    if settings.mode == IPV4_MODE_MANUAL {
        super::refresh_dhcp_port(runtime, false);
        runtime.ip = settings.address;
        runtime.subnet_mask = settings.netmask;
        runtime.gateway_ip = settings.gateway;
        runtime.dns_ip = settings.dns;
        runtime.configuration.state = IPV4_STATE_READY;
        let _ = arp::emit_gratuitous_arp_reply(runtime);
    } else {
        runtime.ip = [0; 4];
        runtime.subnet_mask = [0; 4];
        runtime.gateway_ip = [0; 4];
        runtime.dns_ip = [0; 4];
        let now = now_ms(runtime).unwrap_or(0);
        if runtime.timer_port.is_some() {
            runtime.configuration.acquisition = dhcp::Acquisition::start(runtime, now);
            runtime.configuration.state = IPV4_STATE_ACQUIRING;
        } else {
            runtime.configuration.state = IPV4_STATE_FAILED;
            nanami_runtime::println!("[net-server] DHCP requires a timer service");
        }
    }
    notify_client_sessions(runtime);
    nanami_runtime::println!("[net-server] configuration applied mode={} address={}.{}.{}.{} mask={}.{}.{}.{} gateway={}.{}.{}.{}",
        settings.mode, runtime.ip[0], runtime.ip[1], runtime.ip[2], runtime.ip[3],
        runtime.subnet_mask[0], runtime.subnet_mask[1], runtime.subnet_mask[2], runtime.subnet_mask[3],
        runtime.gateway_ip[0], runtime.gateway_ip[1], runtime.gateway_ip[2], runtime.gateway_ip[3]);
}
pub fn tick(runtime: &mut NetRuntime) {
    if !runtime.configuration.acquisition.active() {
        return;
    }
    let Some(now) = now_ms(runtime) else {
        return;
    };
    let mut acquisition = core::mem::replace(
        &mut runtime.configuration.acquisition,
        dhcp::Acquisition::OFF,
    );
    runtime.configuration.state = acquisition.poll(runtime, now);
    runtime.configuration.acquisition = acquisition;
}
pub fn maintenance(runtime: &mut NetRuntime) {
    if runtime.configuration.active_mode == IPV4_MODE_MANUAL
        && runtime.configuration.state == IPV4_STATE_READY
        && super::refresh_dhcp_port(runtime, false)
    {
        // Late carrier must also select the right physical LAN in manual
        // mode. Never apply an uncommitted draft while doing link maintenance.
        let active = Ipv4Settings {
            mode: IPV4_MODE_MANUAL,
            address: runtime.ip,
            netmask: runtime.subnet_mask,
            gateway: runtime.gateway_ip,
            dns: runtime.dns_ip,
        };
        apply(runtime, active);
    }
}
fn now_ms(runtime: &NetRuntime) -> Option<Word> {
    let (ticks, hz) =
        nanami_client::timer::timer_service_monotonic_ticks(runtime.timer_port?).ok()?;
    if hz == 0 {
        return None;
    }
    Some(((ticks as u128 * 1000) / hz as u128).min(usize::MAX as u128) as usize)
}
