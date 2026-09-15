use super::*;

pub(super) const TCP_HDR_LEN: usize = 20;
pub(super) const TCP_PAYLOAD_MAX: usize = 1460;
pub(super) const TCP_RX_META_LEN: usize = 12;
pub(super) const TCP_MAX_CONNECTIONS: usize = 1024;
pub(super) const TCP_STATE_CLOSED: u8 = 0;
pub(super) const TCP_STATE_SYN_RECEIVED: u8 = 1;
pub(super) const TCP_STATE_ESTABLISHED: u8 = 2;
pub(super) const TCP_STATE_FIN_WAIT1: u8 = 3;
pub(super) const TCP_STATE_FIN_WAIT2: u8 = 4;
pub(super) const TCP_STATE_CLOSE_WAIT: u8 = 5;
pub(super) const TCP_STATE_LAST_ACK: u8 = 6;
pub(super) const TCP_STATE_SYN_SENT: u8 = 7;
pub(super) const TCP_FLAG_FIN: u8 = 0x01;
pub(super) const TCP_FLAG_SYN: u8 = 0x02;
pub(super) const TCP_FLAG_RST: u8 = 0x04;
pub(super) const TCP_FLAG_ACK: u8 = 0x10;

#[derive(Clone, Copy)]
pub(super) struct TcpConnection {
    pub(super) active: bool,
    pub(super) owner_id: Word,
    pub(super) connection_id: Word,
    pub(super) accepted: bool,
    pub(super) eof_pending: bool,
    pub(super) state: u8,
    pub(super) peer_ip: [u8; 4],
    pub(super) peer_port: u16,
    pub(super) local_port: u16,
    pub(super) snd_iss: u32,
    pub(super) snd_nxt: u32,
    pub(super) snd_una: u32,
    pub(super) rcv_nxt: u32,
}

impl TcpConnection {
    pub(super) const EMPTY: Self = Self {
        active: false,
        owner_id: 0,
        connection_id: 0,
        accepted: false,
        eof_pending: false,
        state: TCP_STATE_CLOSED,
        peer_ip: [0; 4],
        peer_port: 0,
        local_port: 0,
        snd_iss: 0,
        snd_nxt: 0,
        snd_una: 0,
        rcv_nxt: 0,
    };
}
