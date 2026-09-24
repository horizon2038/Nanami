use super::*;

// Each live connection owns the capacity it advertises. A slow reader must
// not consume another connection's receive credit (or stop the NIC/ARP pump).
pub(super) struct TcpRxBuffer {
    data: [u8; TCP_PAYLOAD_MAX],
    head: usize,
    len: usize,
    window: u16,
}

impl TcpRxBuffer {
    pub(super) const EMPTY: Self = Self {
        data: [0; TCP_PAYLOAD_MAX],
        head: 0,
        len: 0,
        window: 0,
    };
}

pub(super) struct TcpRxQueue {
    buffers: &'static mut [TcpRxBuffer; TCP_MAX_CONNECTIONS],
    ready: [usize; TCP_MAX_CONNECTIONS],
    head: usize,
    pub(super) count: usize,
}

impl TcpRxQueue {
    pub(super) fn new(buffers: &'static mut [TcpRxBuffer; TCP_MAX_CONNECTIONS]) -> Self {
        for buffer in buffers.iter_mut() {
            buffer.window = TCP_PAYLOAD_MAX as u16;
        }
        Self {
            buffers,
            ready: [0; TCP_MAX_CONNECTIONS],
            head: 0,
            count: 0,
        }
    }

    pub(super) fn window(&self, index: usize) -> u16 {
        self.buffers[index].window
    }

    pub(super) fn readable(&self, index: usize) -> bool {
        self.buffers[index].len != 0
    }

    pub(super) fn push(&mut self, index: usize, payload: &[u8]) -> usize {
        let buffer = &mut self.buffers[index];
        let len = min(payload.len(), buffer.window as usize);
        if len == 0 {
            return 0;
        }
        if buffer.len == 0 {
            self.ready[(self.head + self.count) % TCP_MAX_CONNECTIONS] = index;
            self.count += 1;
        }
        let tail = (buffer.head + buffer.len) % TCP_PAYLOAD_MAX;
        let first = min(len, TCP_PAYLOAD_MAX - tail);
        buffer.data[tail..tail + first].copy_from_slice(&payload[..first]);
        buffer.data[..len - first].copy_from_slice(&payload[first..len]);
        buffer.len += len;
        buffer.window -= len as u16;
        len
    }

    // Rotate indices, not packet-sized entries. Keep unread stream bytes and
    // return whether enough space opened to advertise a new window (SWS avoidance).
    pub(super) fn read_for(
        &mut self,
        connections: &[TcpConnection],
        owner_id: Word,
        connection_id: Word,
        dst: &mut [u8],
    ) -> Option<(usize, usize, bool)> {
        if dst.is_empty() {
            return None;
        }
        for _ in 0..self.count {
            let index = self.ready[self.head];
            self.head = (self.head + 1) % TCP_MAX_CONNECTIONS;
            self.count -= 1;
            let conn = &connections[index];
            let matches = conn.owner_id == owner_id
                && (connection_id == 0 || conn.connection_id == connection_id);
            let buffer = &mut self.buffers[index];
            let mut len = 0;
            let mut window_updated = false;
            if matches {
                len = min(dst.len(), buffer.len);
                let first = min(len, TCP_PAYLOAD_MAX - buffer.head);
                dst[..first].copy_from_slice(&buffer.data[buffer.head..buffer.head + first]);
                dst[first..len].copy_from_slice(&buffer.data[..len - first]);
                buffer.head = (buffer.head + len) % TCP_PAYLOAD_MAX;
                buffer.len -= len;
                let free = TCP_PAYLOAD_MAX - buffer.len;
                if free - buffer.window as usize >= TCP_PAYLOAD_MAX / 2 {
                    buffer.window = free as u16;
                    window_updated = true;
                }
            }
            if buffer.len != 0 {
                self.ready[(self.head + self.count) % TCP_MAX_CONNECTIONS] = index;
                self.count += 1;
            }
            if matches {
                return Some((index, len, window_updated));
            }
        }
        None
    }

    pub(super) fn reset(&mut self, index: usize) {
        let buffer = &mut self.buffers[index];
        if buffer.len != 0 {
            for _ in 0..self.count {
                let queued = self.ready[self.head];
                self.head = (self.head + 1) % TCP_MAX_CONNECTIONS;
                self.count -= 1;
                if queued != index {
                    self.ready[(self.head + self.count) % TCP_MAX_CONNECTIONS] = queued;
                    self.count += 1;
                }
            }
        }
        // Bytes outside len are never exposed, so no packet-sized clearing.
        buffer.head = 0;
        buffer.len = 0;
        buffer.window = TCP_PAYLOAD_MAX as u16;
    }
}

pub(super) fn handle_tcp_recv_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
) -> (Word, Word, Word) {
    let Some(session) = session_for(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    };
    let meta_offset = request.arg0;
    let payload_offset = request.arg1;
    let max_len = min(request.arg2, TCP_PAYLOAD_MAX);
    if meta_offset > session.shm_size
        || TCP_RX_META_LEN > session.shm_size - meta_offset
        || payload_offset > session.shm_size
        || max_len > session.shm_size - payload_offset
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if max_len == 0 {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    }
    let payload = unsafe {
        core::slice::from_raw_parts_mut((session.shm_local + payload_offset) as *mut u8, max_len)
    };
    if let Some((index, len, window_updated)) = runtime.tcp_rx.read_for(
        &runtime.tcp_connections,
        request.identifier,
        request.arg3,
        payload,
    ) {
        let conn = runtime.tcp_connections[index];
        unsafe {
            let meta = core::slice::from_raw_parts_mut(
                (session.shm_local + meta_offset) as *mut u8,
                TCP_RX_META_LEN,
            );
            meta[..4].copy_from_slice(&conn.peer_ip);
            write_u16_be(&mut meta[4..6], conn.peer_port);
            write_u16_be(&mut meta[6..8], len as u16);
            write_u32_be(&mut meta[8..12], conn.connection_id as u32);
        }
        if window_updated {
            if let Some(mac) = arp_lookup(runtime, next_hop_ip(runtime, conn.peer_ip)) {
                tcp::emit_ack(runtime, stats, index, mac);
            }
            // If ARP or this ACK is lost, the peer's zero-window probe receives
            // the current window as well; it is not dependent on new app TX.
        }
        return (libnanami::OS_RESPONSE_OK, len, conn.connection_id);
    }
    for conn in &mut runtime.tcp_connections {
        if conn.active
            && conn.owner_id == request.identifier
            && (conn.eof_pending || (request.arg3 != 0 && conn.state == TCP_STATE_CLOSE_WAIT))
            && (request.arg3 == 0 || conn.connection_id == request.arg3)
        {
            // A per-connection read keeps returning EOF. The multiplexed API
            // reports it once, so one half-closed peer cannot starve all others.
            if request.arg3 == 0 {
                conn.eof_pending = false;
            }
            return (libnanami::OS_RESPONSE_OK, 0, conn.connection_id);
        }
    }
    (libnanami::OS_RESPONSE_OK, 0, 0)
}
