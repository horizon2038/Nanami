use crate::Word;

pub const NET_DEVICE_REQUEST_SEND: Word = 0x2001;
pub const NET_DEVICE_REQUEST_RECV: Word = 0x2002;
pub const NET_DEVICE_REQUEST_CONTROL: Word = 0x2003;
pub const NET_DEVICE_REQUEST_RECV_BATCH: Word = 0x2004;

pub const NET_DEVICE_CONTROL_LINK_UP: Word = 1;
pub const NET_DEVICE_CONTROL_LINK_DOWN: Word = 2;
pub const NET_DEVICE_CONTROL_ATTACH_SHARED_MEMORY: Word = 16;
pub const NET_DEVICE_CONTROL_GET_MAC: Word = 17;
pub const NET_DEVICE_CONTROL_ATTACH_RX_NOTIFICATION: Word = 18;
pub const NET_DEVICE_CONTROL_GET_FEATURES: Word = 19;
pub const NET_DEVICE_FEATURE_RECV_BATCH: Word = 1;

/// RECV_BATCH(offset, slots) writes up to `slots` fixed-stride records and
/// returns their count. Each record is a little-endian u32 length followed by
/// that many frame bytes. Unused bytes/records are unspecified. Bounds are
/// validated before dequeuing; the legacy single-frame API remains available.
pub const NET_DEVICE_RX_BATCH_MAX: usize = 32;
pub const NET_DEVICE_RX_FRAME_MAX: usize = 1536;
pub const NET_DEVICE_RX_SLOT_STRIDE: usize = 4 + NET_DEVICE_RX_FRAME_MAX;

pub const NET_SERVICE_REQUEST_SEND: Word = 0x3001;
pub const NET_SERVICE_REQUEST_RECV: Word = 0x3002;
pub const NET_SERVICE_REQUEST_CONTROL: Word = 0x3003;
pub const NET_SERVICE_REQUEST_STATS: Word = 0x3004;
pub const NET_SERVICE_REQUEST_UDP_SEND: Word = 0x3010;
pub const NET_SERVICE_REQUEST_UDP_RECV: Word = 0x3011;
pub const NET_SERVICE_REQUEST_TCP_SEND: Word = 0x3020;
pub const NET_SERVICE_REQUEST_TCP_RECV: Word = 0x3021;
pub const NET_SERVICE_REQUEST_TCP_ACCEPT: Word = 0x3022;
pub const NET_SERVICE_REQUEST_TCP_CONNECT: Word = 0x3023;
pub const NET_SERVICE_REQUEST_DNS_QUERY: Word = 0x3030;
pub const NET_SERVICE_REQUEST_ICMP_SEND: Word = 0x3040;
pub const NET_SERVICE_REQUEST_ICMP_RECV: Word = 0x3041;

pub const NET_SERVICE_CONTROL_LINK_UP: Word = 1;
pub const NET_SERVICE_CONTROL_LINK_DOWN: Word = 2;
pub const NET_SERVICE_CONTROL_POLL: Word = 3;
pub const NET_SERVICE_CONTROL_ATTACH_SHARED_MEMORY: Word = 16;
pub const NET_SERVICE_CONTROL_GET_IPV4_CONFIG: Word = 17;
pub const NET_SERVICE_CONTROL_GET_MAC: Word = 18;
pub const NET_SERVICE_CONTROL_UDP_BIND: Word = 32;
pub const NET_SERVICE_CONTROL_TCP_BIND: Word = 33;
pub const NET_SERVICE_CONTROL_UDP_UNBIND: Word = 34;
pub const NET_SERVICE_CONTROL_TCP_UNBIND: Word = 35;
pub const NET_SERVICE_CONTROL_ATTACH_RX_NOTIFICATION: Word = 36;

pub const NET_NOTIFICATION_RX: Word = 1 << 20;

/// TCP_SEND could not resolve the next-hop MAC. No data or FIN was consumed;
/// retain the response and retry after receiving packets or a retry timer.
pub const NET_SERVICE_RESPONSE_WOULD_BLOCK: Word = 0x3000_0001;

pub const TCP_RX_META_LEN: Word = 12;
pub const TCP_ACCEPT_META_LEN: Word = 12;
pub const ICMP_RX_META_LEN: Word = 8;
