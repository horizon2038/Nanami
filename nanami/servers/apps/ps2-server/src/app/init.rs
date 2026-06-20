use libnanami::{RequestError, Word};

use crate::constants::{
    PS2_ACK_TIMEOUT, PS2_COMMAND_PORT, PS2_CONTROLLER_ENABLE_AUX, PS2_CONTROLLER_READ_CONFIG,
    PS2_CONTROLLER_WRITE_CONFIG, PS2_CONTROLLER_WRITE_TO_MOUSE, PS2_DATA_PORT, PS2_MOUSE_ACK,
    PS2_MOUSE_CMD_DISABLE_DATA_REPORTING, PS2_MOUSE_CMD_ENABLE_DATA_REPORTING,
    PS2_MOUSE_CMD_SET_DEFAULTS, PS2_MOUSE_CMD_SET_RESOLUTION, PS2_MOUSE_CMD_SET_SAMPLE_RATE,
    PS2_MOUSE_CMD_SET_SCALING_1_TO_1, PS2_MOUSE_RESEND, PS2_MOUSE_RESOLUTION,
    PS2_MOUSE_SAMPLE_RATE, PS2_STATUS_PORT,
};
use crate::controller::drain_controller;
use crate::state::Ps2Server;

pub fn initialize_mouse(server: &mut Ps2Server) -> Result<(), RequestError> {
    let _ = drain_controller(server);

    wait_input_buffer_clear(server.io_desc)?;
    libnanami::io::io_write(
        server.io_desc,
        PS2_COMMAND_PORT,
        1,
        PS2_CONTROLLER_ENABLE_AUX,
    )?;

    wait_input_buffer_clear(server.io_desc)?;
    libnanami::io::io_write(
        server.io_desc,
        PS2_COMMAND_PORT,
        1,
        PS2_CONTROLLER_READ_CONFIG,
    )?;

    let mut config = read_controller_config(server)?;
    let config_before = config;
    config |= 1 << 0;
    config |= 1 << 1;
    config &= !(1 << 4);
    config &= !(1 << 5);

    wait_input_buffer_clear(server.io_desc)?;
    libnanami::io::io_write(
        server.io_desc,
        PS2_COMMAND_PORT,
        1,
        PS2_CONTROLLER_WRITE_CONFIG,
    )?;
    wait_input_buffer_clear(server.io_desc)?;
    libnanami::io::io_write(server.io_desc, PS2_DATA_PORT, 1, config)?;

    libnanami::print!("[ps2-server] ctrl cfg ");
    libnanami::print!("{:#x}", config_before);
    libnanami::print!(" -> ");
    libnanami::print!("{:#x}", config);
    libnanami::print!("\n");

    write_mouse_command(server, PS2_MOUSE_CMD_DISABLE_DATA_REPORTING)?;
    write_mouse_command(server, PS2_MOUSE_CMD_SET_DEFAULTS)?;
    write_mouse_command(server, PS2_MOUSE_CMD_SET_SCALING_1_TO_1)?;
    write_mouse_command_with_data(server, PS2_MOUSE_CMD_SET_RESOLUTION, PS2_MOUSE_RESOLUTION)?;
    write_mouse_command_with_data(server, PS2_MOUSE_CMD_SET_SAMPLE_RATE, PS2_MOUSE_SAMPLE_RATE)?;
    write_mouse_command(server, PS2_MOUSE_CMD_ENABLE_DATA_REPORTING)?;

    libnanami::println!(
        "[ps2-server] mouse rate={}Hz resolution={} scaling=1:1",
        PS2_MOUSE_SAMPLE_RATE,
        PS2_MOUSE_RESOLUTION
    );

    Ok(())
}

fn read_controller_config(server: &mut Ps2Server) -> Result<Word, RequestError> {
    let mut i = 0usize;
    while i < PS2_ACK_TIMEOUT {
        let status = libnanami::io::io_read(server.io_desc, PS2_STATUS_PORT, 1)?;
        if (status & 0x01) == 0 {
            i += 1;
            continue;
        }

        let data = libnanami::io::io_read(server.io_desc, PS2_DATA_PORT, 1)? as u8;
        if (status & 0x20) != 0 {
            server.mouse.push_byte(
                data,
                &mut server.mouse_batch,
                &mut server.mouse_packet_count,
            );
            i += 1;
            continue;
        }

        return Ok(data as Word);
    }
    Err(RequestError::Transport)
}

fn write_mouse_command(server: &mut Ps2Server, command: Word) -> Result<(), RequestError> {
    wait_input_buffer_clear(server.io_desc)?;
    libnanami::io::io_write(
        server.io_desc,
        PS2_COMMAND_PORT,
        1,
        PS2_CONTROLLER_WRITE_TO_MOUSE,
    )?;
    wait_input_buffer_clear(server.io_desc)?;
    libnanami::io::io_write(server.io_desc, PS2_DATA_PORT, 1, command)?;

    wait_mouse_ack(server)
}

fn write_mouse_command_with_data(
    server: &mut Ps2Server,
    command: Word,
    data: Word,
) -> Result<(), RequestError> {
    write_mouse_command(server, command)?;
    write_mouse_command(server, data)
}

fn wait_mouse_ack(server: &mut Ps2Server) -> Result<(), RequestError> {
    let mut i = 0usize;
    while i < PS2_ACK_TIMEOUT {
        let status = libnanami::io::io_read(server.io_desc, PS2_STATUS_PORT, 1)?;

        if (status & 0x01) == 0 {
            i += 1;
            continue;
        }

        let data = libnanami::io::io_read(server.io_desc, PS2_DATA_PORT, 1)? as u8;

        if (status & 0x20) == 0 {
            server
                .keyboard
                .push_byte(data, &mut server.key_events, &mut server.key_event_count);
            i += 1;
            continue;
        }

        if data == PS2_MOUSE_ACK {
            return Ok(());
        }

        if data == PS2_MOUSE_RESEND {
            libnanami::print!("[ps2-server] mouse resend\n");
            return Err(RequestError::Protocol);
        }

        server.mouse.push_byte(
            data,
            &mut server.mouse_batch,
            &mut server.mouse_packet_count,
        );
        i += 1;
    }

    libnanami::print!("[ps2-server] mouse ack timeout\n");
    Err(RequestError::Transport)
}

fn wait_input_buffer_clear(io_desc: Word) -> Result<(), RequestError> {
    let mut i = 0usize;
    while i < 200_000 {
        let status = libnanami::io::io_read(io_desc, PS2_STATUS_PORT, 1)?;
        if (status & 0x02) == 0 {
            return Ok(());
        }
        i += 1;
    }
    Err(RequestError::Transport)
}
