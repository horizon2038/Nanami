#![no_std]
#![no_main]

use libnanami::Word;

const SLOT_ALTER_LINUX: Word = 26;
const SLOT_TERMINAL_SERVICE: Word = 29;
const SLOT_TIMER_SERVICE: Word = 30;
const SLOT_TEST_NOTIFICATION: Word = 31;
const ALTER_REQUEST_CONTROL: Word = 0xb101;
const ALTER_REQUEST_STATUS: Word = 0xb103;
const ALTER_REQUEST_SPAWN_LINUX: Word = 0xb105;
const ALTER_REQUEST_KILL: Word = 0xb106;
const ALTER_REQUEST_KILL_TERMINAL: Word = 0xb107;
const ALTER_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
const ALTER_LAUNCH_FLAG_GRAPHICS: Word = 1 << 2;
const TEST_TIMEOUT_MS: Word = 300_000;
const DIRECT_ARGV: [&[u8]; 2] = [b"/alter/linux/bin/iwasm", b"/bin/hello-world.wasm"];
const BASH_ARGV: [&[u8]; 1] = [b"/alter/linux/bin/bash"];
const TEST_ENV: [&[u8]; 4] = [b"PATH=/bin:/usr/bin", b"PWD=/", b"HOME=/", b"TERM=nanami"];
const IWASM_COMMAND: &[u8] =
    b"iwasm /bin/hello-world.wasm; printf '\\036IWASM-STATUS:%s\\037\\n' \"$?\"\n";
const NETWORK_COMMAND: &[u8] = b"busybox ip a; printf '\\036IP-STATUS:%s\\037\\n' \"$?\"; busybox ping -c 1 -W 3 10.0.2.2; printf '\\036PING-STATUS:%s\\037\\n' \"$?\"; busybox nslookup example.com 10.0.2.3 >/tmp/nslookup.out 2>&1; printf '\\036UDP-STATUS:%s\\037\\n' \"$?\"; printf 'alter-tcp-ok' | busybox nc -w 3 10.0.2.2 18080; printf '\\036TCP-STATUS:%s\\037\\n' \"$?\"\n";
const VIRTUAL_DEV_COMMAND: &[u8] =
    b"busybox ls /dev/input; printf '\\036DEV-STATUS:%s\\037\\n' \"$?\"\n";
const VIRTUAL_SYS_INPUT_COMMAND: &[u8] =
    b"busybox ls /sys/class/input; printf '\\036SYS-INPUT-STATUS:%s\\037\\n' \"$?\"\n";
const VIRTUAL_FB_DISABLED_COMMAND: &[u8] =
    b"busybox ls /dev; printf '\\036FB-DISABLED-STATUS:%s\\037\\n' \"$?\"\n";
const VIRTUAL_PROC_COMMAND: &[u8] =
    b"busybox cat /proc/version; printf '\\036PROC-STATUS:%s\\037\\n' \"$?\"\n";
const VIRTUAL_TEMP_WRITE_COMMAND: &[u8] =
    b"printf 'alter-vfs-ok' >/temp/alter-vfs; printf '\\036TEMP-WRITE:%s\\037\\n' \"$?\"\n";
const VIRTUAL_TEMP_READ_COMMAND: &[u8] =
    b"busybox cat /tmp/alter-vfs; printf '\\036TEMP-READ:%s\\037\\n' \"$?\"\n";
const GRAPHICS_COMMAND: &[u8] =
    b"busybox ls /dev; printf X >/dev/fb0; printf '\\036GRAPHICS-STATUS:%s\\037\\n' \"$?\"\n";
const KEYBOARD_ARGV: [&[u8]; 3] = [b"/alter/linux/bin/busybox", b"cat", b"/dev/input/event0"];
const MOUSE_ARGV: [&[u8]; 3] = [b"/alter/linux/bin/busybox", b"cat", b"/dev/input/event1"];
const PROMPT: &[u8] = b"# ";
const WASM_OUTPUT: &[u8] = b"Hello, Alter/Linux + WAMR!";
const STATUS_OUTPUT: &[u8] = b"\x1eIWASM-STATUS:0\x1f";
const IP_LINK_OUTPUT: &[u8] = b"eth0";
const IP_STATUS_OUTPUT: &[u8] = b"\x1eIP-STATUS:0\x1f";
const PING_STATUS_OUTPUT: &[u8] = b"\x1ePING-STATUS:0\x1f";
const UDP_STATUS_OUTPUT: &[u8] = b"\x1eUDP-STATUS:0\x1f";
const TCP_STATUS_OUTPUT: &[u8] = b"\x1eTCP-STATUS:0\x1f";
const EVENT0_OUTPUT: &[u8] = b"event0";
const EVENT1_OUTPUT: &[u8] = b"event1";
const PROC_VERSION_OUTPUT: &[u8] = b"Linux version 6.1.0-alter";
const VIRTUAL_FS_OUTPUT: &[u8] = b"alter-vfs-ok";
const DEV_STATUS_OUTPUT: &[u8] = b"\x1eDEV-STATUS:0\x1f";
const SYS_INPUT_STATUS_OUTPUT: &[u8] = b"\x1eSYS-INPUT-STATUS:0\x1f";
const FB_DISABLED_STATUS_OUTPUT: &[u8] = b"\x1eFB-DISABLED-STATUS:0\x1f";
const PROC_STATUS_OUTPUT: &[u8] = b"\x1ePROC-STATUS:0\x1f";
const TEMP_WRITE_STATUS_OUTPUT: &[u8] = b"\x1eTEMP-WRITE:0\x1f";
const TEMP_READ_STATUS_OUTPUT: &[u8] = b"\x1eTEMP-READ:0\x1f";
const FB0_OUTPUT: &[u8] = b"fb0";
const GRAPHICS_STATUS_OUTPUT: &[u8] = b"\x1eGRAPHICS-STATUS:0\x1f";
const KEYBOARD_EVENT_OUTPUT: &[u8] = b"\x01\x00\x1e\x00\x01\x00\x00\x00";
const MOUSE_EVENT_OUTPUT: &[u8] = b"\x02\x00\x00\x00\x05\x00\x00\x00";

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::println!("[alter-test] panic");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::println!("[alter-test] iwasm regression start");
    libnanami::ipc::init_ipc_tls()?;

    let alter_port = connect_service("alter-linux", SLOT_ALTER_LINUX)?;
    let terminal_port = connect_terminal_service()?;
    let timer_port = connect_timer_service()?;
    let (alter_shm, alter_shm_size) = attach_alter_memory(alter_port)?;
    if alter_shm_size < 0x1000 {
        return fail("alter shared memory too small", alter_port, 0);
    }

    let (terminal_shm, terminal_shm_size) =
        nanami_services::terminal::terminal_attach_shared_memory(terminal_port, 0x4000)?;
    if terminal_shm_size < 0x1000 {
        return fail("terminal shared memory too small", alter_port, 0);
    }

    libnanami::request_notification_port_create(SLOT_TEST_NOTIFICATION, 0)?;
    let terminal_id = nanami_services::terminal::terminal_create(terminal_port, 80, 24)?;
    nanami_services::terminal::terminal_attach_output_notification(
        terminal_port,
        terminal_id,
        SLOT_TEST_NOTIFICATION,
    )?;
    nanami_services::timer::timer_service_sleep_async_on_notification_milliseconds(
        timer_port,
        TEST_TIMEOUT_MS,
        SLOT_TEST_NOTIFICATION,
    )?;

    let launch_len = write_launch_block(alter_shm, &DIRECT_ARGV, &TEST_ENV);
    let (status, direct_pid, _) = libnanami::call_service_port(
        alter_port,
        ALTER_REQUEST_SPAWN_LINUX,
        0,
        launch_len as Word,
        terminal_id,
        0,
        5,
    )?;
    if status != libnanami::OS_RESPONSE_OK {
        return fail("direct iwasm spawn failed", alter_port, terminal_id);
    }

    let notification = libnanami::ipc::process_slot_descriptor(SLOT_TEST_NOTIFICATION);
    let mut direct_patterns = [(WASM_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut direct_patterns,
    )? {
        return fail("direct iwasm output timeout", alter_port, terminal_id);
    }
    if !wait_for_exit(alter_port, timer_port, direct_pid)? {
        return fail("direct iwasm status failed", alter_port, terminal_id);
    }
    let _ = libnanami::call_service_port(alter_port, ALTER_REQUEST_KILL, direct_pid, 1, 0, 0, 3);
    libnanami::println!(
        "[alter-test] PASS direct iwasm pid={} output-and-status-ok",
        direct_pid
    );

    let launch_len = write_launch_block(alter_shm, &BASH_ARGV, &TEST_ENV);
    let (status, bash_pid, _) = libnanami::call_service_port(
        alter_port,
        ALTER_REQUEST_SPAWN_LINUX,
        0,
        launch_len as Word,
        terminal_id,
        0,
        5,
    )?;
    if status != libnanami::OS_RESPONSE_OK {
        return fail("bash spawn failed", alter_port, terminal_id);
    }

    let mut prompt_patterns = [(PROMPT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut prompt_patterns,
    )? {
        return fail("bash prompt timeout", alter_port, terminal_id);
    }

    nanami_services::terminal::terminal_set_echo(terminal_port, terminal_id, false)?;
    write_terminal_input(terminal_port, terminal_id, terminal_shm, IWASM_COMMAND)?;

    let mut result_patterns = [(WASM_OUTPUT, 0usize), (STATUS_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut result_patterns,
    )? {
        return fail("iwasm output timeout", alter_port, terminal_id);
    }

    libnanami::println!(
        "[alter-test] PASS interactive iwasm pid={} output-and-status-ok",
        bash_pid
    );

    write_terminal_input(
        terminal_port,
        terminal_id,
        terminal_shm,
        VIRTUAL_DEV_COMMAND,
    )?;
    let mut virtual_dev_patterns = [
        (EVENT0_OUTPUT, 0usize),
        (EVENT1_OUTPUT, 0usize),
        (DEV_STATUS_OUTPUT, 0usize),
    ];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut virtual_dev_patterns,
    )? {
        return fail("virtual dev output timeout", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS virtual /dev");

    write_terminal_input(
        terminal_port,
        terminal_id,
        terminal_shm,
        VIRTUAL_SYS_INPUT_COMMAND,
    )?;
    let mut virtual_sys_patterns = [
        (EVENT0_OUTPUT, 0usize),
        (EVENT1_OUTPUT, 0usize),
        (SYS_INPUT_STATUS_OUTPUT, 0usize),
    ];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut virtual_sys_patterns,
    )? {
        return fail("virtual sys input timeout", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS virtual /sys/class/input");

    write_terminal_input(
        terminal_port,
        terminal_id,
        terminal_shm,
        VIRTUAL_FB_DISABLED_COMMAND,
    )?;
    if !wait_for_output_without_pattern(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        FB_DISABLED_STATUS_OUTPUT,
        FB0_OUTPUT,
    )? {
        return fail("disabled framebuffer visible", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS graphics option disabled");

    write_terminal_input(
        terminal_port,
        terminal_id,
        terminal_shm,
        VIRTUAL_PROC_COMMAND,
    )?;
    let mut virtual_proc_patterns = [(PROC_VERSION_OUTPUT, 0usize), (PROC_STATUS_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut virtual_proc_patterns,
    )? {
        return fail("virtual proc output timeout", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS virtual /proc");

    write_terminal_input(
        terminal_port,
        terminal_id,
        terminal_shm,
        VIRTUAL_TEMP_WRITE_COMMAND,
    )?;
    let mut virtual_temp_write_patterns = [(TEMP_WRITE_STATUS_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut virtual_temp_write_patterns,
    )? {
        return fail("virtual temp write timeout", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS virtual /temp write");

    write_terminal_input(
        terminal_port,
        terminal_id,
        terminal_shm,
        VIRTUAL_TEMP_READ_COMMAND,
    )?;
    let mut virtual_temp_read_patterns = [
        (VIRTUAL_FS_OUTPUT, 0usize),
        (TEMP_READ_STATUS_OUTPUT, 0usize),
    ];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut virtual_temp_read_patterns,
    )? {
        return fail("virtual temp read timeout", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS virtual fs pid={}", bash_pid);

    let _ = libnanami::call_service_port(alter_port, ALTER_REQUEST_KILL, bash_pid, 1, 0, 0, 3);
    let launch_len = write_launch_block(alter_shm, &BASH_ARGV, &TEST_ENV);
    let (status, graphics_pid, _) = libnanami::call_service_port(
        alter_port,
        ALTER_REQUEST_SPAWN_LINUX,
        0,
        launch_len as Word,
        terminal_id,
        ALTER_LAUNCH_FLAG_GRAPHICS,
        5,
    )?;
    if status != libnanami::OS_RESPONSE_OK {
        return fail("graphics bash spawn failed", alter_port, terminal_id);
    }
    let mut graphics_prompt = [(PROMPT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut graphics_prompt,
    )? {
        return fail("graphics bash prompt timeout", alter_port, terminal_id);
    }
    write_terminal_input(terminal_port, terminal_id, terminal_shm, GRAPHICS_COMMAND)?;
    let mut graphics_patterns = [(FB0_OUTPUT, 0usize), (GRAPHICS_STATUS_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut graphics_patterns,
    )? {
        return fail("graphics framebuffer timeout", alter_port, terminal_id);
    }
    libnanami::println!(
        "[alter-test] PASS graphics framebuffer pid={}",
        graphics_pid
    );

    let launch_len = write_launch_block(alter_shm, &KEYBOARD_ARGV, &TEST_ENV);
    let (status, keyboard_pid, _) = libnanami::call_service_port(
        alter_port,
        ALTER_REQUEST_SPAWN_LINUX,
        0,
        launch_len as Word,
        terminal_id,
        ALTER_LAUNCH_FLAG_GRAPHICS,
        5,
    )?;
    if status != libnanami::OS_RESPONSE_OK {
        return fail("keyboard evdev timeout", alter_port, terminal_id);
    }
    let mut keyboard_patterns = [(KEYBOARD_EVENT_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut keyboard_patterns,
    )? {
        return fail("keyboard evdev timeout", alter_port, terminal_id);
    }
    let _ = libnanami::call_service_port(alter_port, ALTER_REQUEST_KILL, keyboard_pid, 1, 0, 0, 3);
    libnanami::println!("[alter-test] PASS evdev keyboard");

    let launch_len = write_launch_block(alter_shm, &MOUSE_ARGV, &TEST_ENV);
    let (status, mouse_pid, _) = libnanami::call_service_port(
        alter_port,
        ALTER_REQUEST_SPAWN_LINUX,
        0,
        launch_len as Word,
        terminal_id,
        ALTER_LAUNCH_FLAG_GRAPHICS,
        5,
    )?;
    if status != libnanami::OS_RESPONSE_OK {
        return fail("mouse evdev timeout", alter_port, terminal_id);
    }
    let mut mouse_patterns = [(MOUSE_EVENT_OUTPUT, 0usize)];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut mouse_patterns,
    )? {
        return fail("mouse evdev timeout", alter_port, terminal_id);
    }
    let _ = libnanami::call_service_port(alter_port, ALTER_REQUEST_KILL, mouse_pid, 1, 0, 0, 3);
    libnanami::println!("[alter-test] PASS evdev mouse");

    write_terminal_input(terminal_port, terminal_id, terminal_shm, NETWORK_COMMAND)?;
    let mut network_patterns = [
        (IP_LINK_OUTPUT, 0usize),
        (IP_STATUS_OUTPUT, 0usize),
        (PING_STATUS_OUTPUT, 0usize),
        (UDP_STATUS_OUTPUT, 0usize),
        (TCP_STATUS_OUTPUT, 0usize),
    ];
    if !wait_for_output(
        terminal_port,
        terminal_id,
        terminal_shm,
        notification,
        &mut network_patterns,
    )? {
        return fail("socket output timeout", alter_port, terminal_id);
    }
    libnanami::println!("[alter-test] PASS interactive sockets pid={}", graphics_pid);
    cleanup(alter_port, terminal_id);
    Ok(())
}

fn connect_service(name: &str, slot: Word) -> Result<Word, libnanami::RequestError> {
    loop {
        match libnanami::connect_service_by_name(name, slot) {
            Ok(()) => return Ok(libnanami::ipc::process_slot_descriptor(slot)),
            Err(_) => libnanami::yield_now(),
        }
    }
}

fn connect_terminal_service() -> Result<Word, libnanami::RequestError> {
    loop {
        match nanami_services::registry::connect_terminal_service(SLOT_TERMINAL_SERVICE) {
            Ok(()) => {
                return Ok(libnanami::ipc::process_slot_descriptor(
                    SLOT_TERMINAL_SERVICE,
                ));
            }
            Err(_) => libnanami::yield_now(),
        }
    }
}

fn connect_timer_service() -> Result<Word, libnanami::RequestError> {
    loop {
        match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
            Ok(()) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE)),
            Err(_) => libnanami::yield_now(),
        }
    }
}

fn attach_alter_memory(alter_port: Word) -> Result<(Word, Word), libnanami::RequestError> {
    let (status, shm, size) = libnanami::call_service_port(
        alter_port,
        ALTER_REQUEST_CONTROL,
        ALTER_CONTROL_ATTACH_SHARED_MEMORY,
        0x4000,
        0,
        0,
        3,
    )?;
    if status != libnanami::OS_RESPONSE_OK {
        return Err(libnanami::RequestError::Status(status));
    }
    Ok((shm, size))
}

fn write_launch_block(shm: Word, argv: &[&[u8]], env: &[&[u8]]) -> usize {
    unsafe {
        core::ptr::write_unaligned(shm as *mut Word, argv.len() as Word);
        core::ptr::write_unaligned((shm + 8) as *mut Word, env.len() as Word);
    }
    let mut offset = 16usize;
    for value in argv.iter().chain(env.iter()) {
        unsafe {
            core::ptr::copy_nonoverlapping(
                value.as_ptr(),
                (shm as usize + offset) as *mut u8,
                value.len(),
            );
            core::ptr::write((shm as usize + offset + value.len()) as *mut u8, 0);
        }
        offset += value.len() + 1;
    }
    offset
}

fn wait_for_exit(
    alter_port: Word,
    timer_port: Word,
    pid: Word,
) -> Result<bool, libnanami::RequestError> {
    let mut attempts = 0usize;
    while attempts < 1000 {
        let (status, exited, exit_status) =
            libnanami::call_service_port(alter_port, ALTER_REQUEST_STATUS, pid, 0, 0, 0, 3)?;
        if status != libnanami::OS_RESPONSE_OK {
            return Ok(false);
        }
        if exited != 0 {
            return Ok(exit_status == 0);
        }
        nanami_services::timer::timer_service_sleep_milliseconds(timer_port, 10)?;
        attempts += 1;
    }
    Ok(false)
}

fn write_terminal_input(
    terminal_port: Word,
    terminal_id: Word,
    terminal_shm: Word,
    input: &[u8],
) -> Result<(), libnanami::RequestError> {
    unsafe {
        core::ptr::copy_nonoverlapping(input.as_ptr(), terminal_shm as *mut u8, input.len());
    }
    let written = nanami_services::terminal::terminal_write_input(
        terminal_port,
        terminal_id,
        0,
        input.len() as Word,
    )?;
    if written != input.len() as Word {
        return Err(libnanami::RequestError::InvalidArgument);
    }
    Ok(())
}

fn wait_for_output(
    terminal_port: Word,
    terminal_id: Word,
    terminal_shm: Word,
    notification: Word,
    patterns: &mut [(&[u8], usize)],
) -> Result<bool, libnanami::RequestError> {
    loop {
        loop {
            let bytes = nanami_services::terminal::terminal_read_output(
                terminal_port,
                terminal_id,
                0,
                0x1000,
            )? as usize;
            if bytes == 0 {
                break;
            }
            let chunk = unsafe { core::slice::from_raw_parts(terminal_shm as *const u8, bytes) };
            for byte in chunk {
                for (pattern, matched) in patterns.iter_mut() {
                    advance_match(pattern, matched, *byte);
                }
            }
            if patterns
                .iter()
                .all(|(pattern, matched)| *matched == pattern.len())
            {
                return Ok(true);
            }
        }

        let identifier = libnanami::ipc::notification_wait(notification)?;
        if identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT != 0 {
            return Ok(false);
        }
    }
}

fn wait_for_output_without_pattern(
    terminal_port: Word,
    terminal_id: Word,
    terminal_shm: Word,
    notification: Word,
    required: &[u8],
    forbidden: &[u8],
) -> Result<bool, libnanami::RequestError> {
    let mut required_matched = 0usize;
    let mut forbidden_matched = 0usize;
    loop {
        loop {
            let bytes = nanami_services::terminal::terminal_read_output(
                terminal_port,
                terminal_id,
                0,
                0x1000,
            )? as usize;
            if bytes == 0 {
                break;
            }
            let chunk = unsafe { core::slice::from_raw_parts(terminal_shm as *const u8, bytes) };
            for byte in chunk {
                advance_match(required, &mut required_matched, *byte);
                advance_match(forbidden, &mut forbidden_matched, *byte);
                if forbidden_matched == forbidden.len() {
                    return Ok(false);
                }
                if required_matched == required.len() {
                    return Ok(true);
                }
            }
        }

        let identifier = libnanami::ipc::notification_wait(notification)?;
        if identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT != 0 {
            return Ok(false);
        }
    }
}

fn advance_match(pattern: &[u8], matched: &mut usize, byte: u8) {
    if *matched == pattern.len() {
        return;
    }
    if byte == pattern[*matched] {
        *matched += 1;
    } else {
        *matched = usize::from(byte == pattern[0]);
    }
}

fn fail(reason: &str, alter_port: Word, terminal_id: Word) -> libnanami::NanamiResult {
    libnanami::println!("[alter-test] FAIL {}", reason);
    cleanup(alter_port, terminal_id);
    Ok(())
}

fn cleanup(alter_port: Word, terminal_id: Word) {
    if terminal_id != 0 {
        let _ = libnanami::call_service_port(
            alter_port,
            ALTER_REQUEST_KILL_TERMINAL,
            terminal_id,
            1,
            0,
            0,
            3,
        );
    }
}

libnanami::nanami_entry!(nanami_main);
