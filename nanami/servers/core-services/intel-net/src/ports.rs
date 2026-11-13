//! Physical ports retain independent DMA rings, including after deselection.
use crate::{registers::*, ring::Rings, service::Service};
use nanami_client::net::*;
use nanami_runtime::{RequestError, Word};

pub struct Port<R> {
    pub regs: R,
    pub rings: Rings,
    pub physical: u64,
    pub mac: [u8; 6],
    pub bdf: Word,
}

impl<R: Registers> Port<R> {
    pub fn carrier(&self) -> bool {
        self.regs.read(STATUS) & LINK_UP != 0
    }
}

pub fn select<R: Registers>(
    ports: &mut [Port<R>],
    active: &mut usize,
    service: &mut Service,
    next: usize,
) -> Result<(), RequestError> {
    if next >= ports.len() {
        return Err(RequestError::InvalidArgument);
    }
    if next == *active {
        return Ok(());
    }
    let old = &mut ports[*active];
    old.regs.write(RCTL, 0);
    old.regs.write(TCTL, 0);
    old.regs.flush();
    // Never reuse/free the old port's descriptors: already submitted DMA may
    // still complete. Each port has its own permanently pinned allocation.
    *active = next;
    let port = &mut ports[next];
    service.select(port.physical, port.mac);
    port.regs
        .write(RCTL, if service.enabled() { RX_CONTROL } else { 0 });
    port.regs
        .write(TCTL, if service.enabled() { TX_CONTROL } else { 0 });
    port.regs.flush();
    Ok(())
}

pub fn control<R: Registers>(
    ports: &mut [Port<R>],
    active: &mut usize,
    service: &mut Service,
    code: Word,
    index: Word,
) -> Result<(Word, Word), RequestError> {
    match code {
        NET_DEVICE_CONTROL_GET_FEATURES => {
            Ok((NET_DEVICE_FEATURE_RECV_BATCH | NET_DEVICE_FEATURE_PORTS, 0))
        }
        NET_DEVICE_CONTROL_PORT_COUNT => Ok((ports.len(), *active)),
        NET_DEVICE_CONTROL_PORT_INFO => {
            let port = ports.get(index).ok_or(RequestError::InvalidArgument)?;
            Ok((
                port.bdf,
                if port.carrier() {
                    NET_DEVICE_PORT_CARRIER
                } else {
                    0
                },
            ))
        }
        NET_DEVICE_CONTROL_SELECT_PORT => {
            select(ports, active, service, index)?;
            let mac = ports[*active].mac;
            let value = mac
                .iter()
                .enumerate()
                .fold(0, |value, (i, byte)| value | ((*byte as Word) << (8 * i)));
            Ok((value, 0))
        }
        _ => Err(RequestError::Unsupported),
    }
}
