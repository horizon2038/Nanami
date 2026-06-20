use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::{
    NET_SERVICE_REQUEST_CONTROL, NET_SERVICE_REQUEST_DNS_QUERY, NET_SERVICE_REQUEST_RECV,
    NET_SERVICE_REQUEST_SEND, NET_SERVICE_REQUEST_STATS, NET_SERVICE_REQUEST_TCP_RECV,
    NET_SERVICE_REQUEST_TCP_SEND, NET_SERVICE_REQUEST_UDP_RECV, NET_SERVICE_REQUEST_UDP_SEND,
};

pub fn net_service_send(
    service_port: CapabilityDescriptor,
    buffer_addr: Word,
    buffer_len: Word,
) -> Result<Word, RequestError> {
    let (status, detail0, _) = call_port(
        service_port,
        NET_SERVICE_REQUEST_SEND,
        buffer_addr,
        buffer_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(detail0)
}

pub fn net_service_recv(
    service_port: CapabilityDescriptor,
    buffer_addr: Word,
    buffer_len: Word,
) -> Result<Word, RequestError> {
    let (status, detail0, _) = call_port(
        service_port,
        NET_SERVICE_REQUEST_RECV,
        buffer_addr,
        buffer_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(detail0)
}

pub fn net_service_control(
    service_port: CapabilityDescriptor,
    control_code: Word,
    arg0: Word,
    arg1: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = net_service_control_ex(service_port, control_code, arg0, arg1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn net_service_control_ex(
    service_port: CapabilityDescriptor,
    control_code: Word,
    arg0: Word,
    arg1: Word,
) -> Result<(Word, Word, Word), RequestError> {
    let (status, detail0, detail1) = call_port(
        service_port,
        NET_SERVICE_REQUEST_CONTROL,
        control_code,
        arg0,
        arg1,
        0,
        4,
    )?;
    Ok((status, detail0, detail1))
}

pub fn net_service_stats(service_port: CapabilityDescriptor) -> Result<(Word, Word), RequestError> {
    let (status, detail0, detail1) =
        call_port(service_port, NET_SERVICE_REQUEST_STATS, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((detail0, detail1))
}

pub fn net_service_ipv4_config(
    service_port: CapabilityDescriptor,
) -> Result<([u8; 4], [u8; 4], [u8; 4]), RequestError> {
    let (status, ip_gateway, dns) = net_service_control_ex(
        service_port,
        super::constants::NET_SERVICE_CONTROL_GET_IPV4_CONFIG,
        0,
        0,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((
        unpack_ipv4((ip_gateway >> 32) as u32),
        unpack_ipv4((ip_gateway & 0xffff_ffff) as u32),
        unpack_ipv4(dns as u32),
    ))
}

pub fn net_service_mac_address(
    service_port: CapabilityDescriptor,
) -> Result<[u8; 6], RequestError> {
    let (status, mac, _) = net_service_control_ex(
        service_port,
        super::constants::NET_SERVICE_CONTROL_GET_MAC,
        0,
        0,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok([
        ((mac >> 40) & 0xff) as u8,
        ((mac >> 32) & 0xff) as u8,
        ((mac >> 24) & 0xff) as u8,
        ((mac >> 16) & 0xff) as u8,
        ((mac >> 8) & 0xff) as u8,
        (mac & 0xff) as u8,
    ])
}

fn unpack_ipv4(value: u32) -> [u8; 4] {
    [
        ((value >> 24) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ]
}

pub fn net_service_udp_send(
    service_port: CapabilityDescriptor,
    payload_offset: Word,
    payload_len: Word,
    src_port: u16,
    dst_port: u16,
    dst_ip_be: u32,
) -> Result<Word, RequestError> {
    let ports = ((src_port as Word) << 16) | (dst_port as Word);
    let (status, sent, _) = call_port(
        service_port,
        NET_SERVICE_REQUEST_UDP_SEND,
        payload_offset,
        payload_len,
        ports,
        dst_ip_be as Word,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(sent)
}

pub fn net_service_udp_recv(
    service_port: CapabilityDescriptor,
    meta_offset: Word,
    payload_offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    let (status, received, _) = call_port(
        service_port,
        NET_SERVICE_REQUEST_UDP_RECV,
        meta_offset,
        payload_offset,
        max_len,
        0,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(received)
}

pub fn net_service_tcp_send(
    service_port: CapabilityDescriptor,
    payload_offset: Word,
    payload_len: Word,
    flags: Word,
) -> Result<Word, RequestError> {
    net_service_tcp_send_on_connection(service_port, 0, payload_offset, payload_len, flags)
}

pub fn net_service_tcp_send_on_connection(
    service_port: CapabilityDescriptor,
    connection_id: Word,
    payload_offset: Word,
    payload_len: Word,
    flags: Word,
) -> Result<Word, RequestError> {
    let (status, sent, _) = call_port(
        service_port,
        NET_SERVICE_REQUEST_TCP_SEND,
        payload_offset,
        payload_len,
        flags,
        connection_id,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(sent)
}

pub fn net_service_tcp_recv(
    service_port: CapabilityDescriptor,
    meta_offset: Word,
    payload_offset: Word,
    max_len: Word,
) -> Result<Word, RequestError> {
    let (received, _) =
        net_service_tcp_recv_ex(service_port, meta_offset, payload_offset, max_len)?;
    Ok(received)
}

pub fn net_service_tcp_recv_ex(
    service_port: CapabilityDescriptor,
    meta_offset: Word,
    payload_offset: Word,
    max_len: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, received, connection_id) = call_port(
        service_port,
        NET_SERVICE_REQUEST_TCP_RECV,
        meta_offset,
        payload_offset,
        max_len,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((received, connection_id))
}

pub fn net_service_dns_query(
    service_port: CapabilityDescriptor,
    name_offset: Word,
    name_len: Word,
    out_ip_offset: Word,
    timeout_ms: Word,
) -> Result<Word, RequestError> {
    let (status, ip_be, _) = call_port(
        service_port,
        NET_SERVICE_REQUEST_DNS_QUERY,
        name_offset,
        name_len,
        out_ip_offset,
        timeout_ms,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(ip_be)
}
