use crate::constants::{MAX_PS2_BYTES_PER_DRAIN, PS2_DATA_PORT, PS2_STATUS_PORT};
use crate::logging::log_request_error;
use crate::state::{DrainState, Ps2Server};

pub fn drain_controller(server: &mut Ps2Server) -> DrainState {
    let mut count = 0usize;

    while count < MAX_PS2_BYTES_PER_DRAIN {
        count += 1;

        let status = match libnanami::io::io_read(server.io_desc, PS2_STATUS_PORT, 1) {
            Ok(v) => v,
            Err(e) => {
                log_request_error("[ps2-server] status read failed: ", e);
                return DrainState::Empty;
            }
        };

        if (status & 0x01) == 0 {
            return DrainState::Empty;
        }

        let data = match libnanami::io::io_read(server.io_desc, PS2_DATA_PORT, 1) {
            Ok(v) => v as u8,
            Err(e) => {
                log_request_error("[ps2-server] data read failed: ", e);
                return DrainState::Empty;
            }
        };

        if (status & 0x20) != 0 {
            server.mouse.push_byte(
                data,
                &mut server.mouse_batch,
                &mut server.mouse_packet_count,
            );
        } else {
            let prev = server.key_event_count;
            server
                .keyboard
                .push_byte(data, &mut server.key_events, &mut server.key_event_count);
            if server.key_event_count > prev {
                server.key_count = server.key_count.wrapping_add(1);
            }
        }
    }

    DrainState::ReachedBudget
}
