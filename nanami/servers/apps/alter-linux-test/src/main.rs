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
const TEST_TIMEOUT_MS: Word = 300_000;
const DIRECT_ARGV: [&[u8]; 2] = [b"/alter/linux/bin/iwasm", b"/bin/hello-world.wasm"];
const BASH_ARGV: [&[u8]; 1] = [b"/alter/linux/bin/bash"];
const TEST_ENV: [&[u8]; 4] = [b"PATH=/bin:/usr/bin", b"PWD=/", b"HOME=/", b"TERM=nanami"];
const IWASM_COMMAND: &[u8] =
    b"iwasm /bin/hello-world.wasm; printf '\\036IWASM-STATUS:%s\\037\\n' \"$?\"\n";
const NETWORK_COMMAND: &[u8] = b"busybox ip a; printf '\\036IP-STATUS:%s\\037\\n' \"$?\"; busybox ping -c 1 -W 3 10.0.2.2; printf '\\036PING-STATUS:%s\\037\\n' \"$?\"; busybox nslookup example.com 10.0.2.3 >/tmp/nslookup.out 2>&1; printf '\\036UDP-STATUS:%s\\037\\n' \"$?\"; printf 'alter-tcp-ok' | busybox nc -w 3 10.0.2.2 18080; printf '\\036TCP-STATUS:%s\\037\\n' \"$?\"\n";
const PROMPT: &[u8] = b"# ";
const WASM_OUTPUT: &[u8] = b"Hello, Alter/Linux + WAMR!";
const STATUS_OUTPUT: &[u8] = b"\x1eIWASM-STATUS:0\x1f";
const IP_LINK_OUTPUT: &[u8] = b"eth0";
const IP_STATUS_OUTPUT: &[u8] = b"\x1eIP-STATUS:0\x1f";
const PING_STATUS_OUTPUT: &[u8] = b"\x1ePING-STATUS:0\x1f";
const UDP_STATUS_OUTPUT: &[u8] = b"\x1eUDP-STATUS:0\x1f";
const TCP_STATUS_OUTPUT: &[u8] = b"\x1eTCP-STATUS:0\x1f";

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
    libnanami::println!("[alter-test] PASS interactive sockets pid={}", bash_pid);
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
                ))
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
