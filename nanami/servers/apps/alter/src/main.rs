#![no_std]
#![no_main]

use libnanami::Word;

const SLOT_ALTER_SERVICE: Word = 28;
const SLOT_TERMINAL_SERVICE: Word = 29;
const SLOT_TIMER_SERVICE: Word = 30;

const ALTER_REQUEST_CONTROL: Word = 0xb101;
const ALTER_REQUEST_STATUS: Word = 0xb103;
const ALTER_REQUEST_SPAWN_LINUX: Word = 0xb105;
const ALTER_REQUEST_KILL: Word = 0xb106;
const ALTER_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;
const ALTER_LAUNCH_FLAG_STRACE: Word = 1 << 0;
const ALTER_LAUNCH_FLAG_DIAGNOSTICS: Word = 1 << 1;
const ALTER_LAUNCH_FLAG_GRAPHICS: Word = 1 << 2;
const ALTER_SHM_BYTES: Word = 0x4000;
const ALTER_LAUNCH_MAX_ARGS: usize = 8;
const ALTER_LAUNCH_MAX_ENVS: usize = 8;
const ALTER_ENV: [&[u8]; 4] = [
    b"PATH=/bin:/usr/bin",
    b"HOME=/",
    b"USER=root",
    b"TERM=nanami",
];

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::println!("[alter] panic");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()?;

    let mut terminal = TerminalClient::new();
    let terminal_id = parse_terminal_id().unwrap_or(0);
    if terminal_id != 0 {
        let _ = terminal.connect(terminal_id);
    }

    let launch = match parse_cli() {
        Ok(launch) => launch,
        Err(()) => {
            terminal.write_line(
                b"usage: alter [-t] [-d] [-g|--graphics] [-os linux|freebsd] <binary> [args]",
            );
            return Ok(());
        }
    };

    let mut alter = match AlterClient::connect(launch.os.service_name()) {
        Ok(client) => client,
        Err(()) => {
            terminal.write_line(launch.os.unavailable_message());
            return Ok(());
        }
    };

    let (offset, len) = match alter.write_launch_block(&launch) {
        Some(value) => value,
        None => {
            terminal.write_line(b"alter: argument block too large");
            return Ok(());
        }
    };

    let mut flags = 0;
    if launch.trace {
        flags |= ALTER_LAUNCH_FLAG_STRACE;
    }
    if launch.diagnostics {
        flags |= ALTER_LAUNCH_FLAG_DIAGNOSTICS;
    }
    if launch.graphics {
        flags |= ALTER_LAUNCH_FLAG_GRAPHICS;
    }
    let (status, pid, _) = match libnanami::call_service_port(
        alter.port,
        ALTER_REQUEST_SPAWN_LINUX,
        offset as Word,
        len as Word,
        terminal_id,
        flags,
        5,
    ) {
        Ok(reply) => reply,
        Err(_) => {
            terminal.write_line(b"alter: ipc failed");
            return Ok(());
        }
    };
    if status != libnanami::OS_RESPONSE_OK {
        terminal.write_status_line(b"alter: spawn failed ", status);
        return Ok(());
    }

    wait_child(pid, alter.port, &mut terminal);
    Ok(())
}

struct Launch {
    trace: bool,
    diagnostics: bool,
    graphics: bool,
    os: AlterOs,
    first_arg: usize,
    argc: usize,
}

#[derive(Clone, Copy)]
enum AlterOs {
    Linux,
    FreeBsd,
}

impl AlterOs {
    fn service_name(self) -> &'static str {
        match self {
            Self::Linux => "alter-linux",
            Self::FreeBsd => "alter-freebsd",
        }
    }

    fn unavailable_message(self) -> &'static [u8] {
        match self {
            Self::Linux => b"alter: alter-linux unavailable",
            Self::FreeBsd => b"alter: alter-freebsd unavailable",
        }
    }
}

fn parse_cli() -> Result<Launch, ()> {
    let argc = libnanami::process_argc();
    if argc < 2 {
        return Err(());
    }

    let mut index = 1usize;
    let mut trace = false;
    let mut diagnostics = false;
    let mut graphics = false;
    let mut os = AlterOs::Linux;
    while index < argc {
        let arg = libnanami::process_arg(index).ok_or(())?;
        if bytes_eq(arg, b"-t") || bytes_eq(arg, b"--strace") {
            trace = true;
            index += 1;
        } else if bytes_eq(arg, b"-d") || bytes_eq(arg, b"--diagnostics") {
            diagnostics = true;
            index += 1;
        } else if bytes_eq(arg, b"-g") || bytes_eq(arg, b"--graphics") {
            graphics = true;
            index += 1;
        } else if bytes_eq(arg, b"-os") {
            let os_name = libnanami::process_arg(index + 1).ok_or(())?;
            if bytes_eq(os_name, b"linux") {
                os = AlterOs::Linux;
            } else if bytes_eq(os_name, b"freebsd") {
                os = AlterOs::FreeBsd;
            } else {
                return Err(());
            }
            index += 2;
        } else {
            break;
        }
    }
    if index + 1 < argc {
        let arg = libnanami::process_arg(index).ok_or(())?;
        if bytes_eq(arg, b"linux") {
            os = AlterOs::Linux;
            index += 1;
        } else if bytes_eq(arg, b"freebsd") {
            os = AlterOs::FreeBsd;
            index += 1;
        }
    }
    if index >= argc || argc - index > ALTER_LAUNCH_MAX_ARGS {
        return Err(());
    }
    if diagnostics {
        log_cli(argc, index, os);
    }
    Ok(Launch {
        trace,
        diagnostics,
        graphics,
        os,
        first_arg: index,
        argc: argc - index,
    })
}

fn log_cli(argc: usize, first_arg: usize, os: AlterOs) {
    libnanami::println!(
        "[alter] cli argc={} first_arg={} os={}",
        argc,
        first_arg,
        os.service_name()
    );
    let mut index = 0usize;
    while index < argc {
        if let Some(arg) = libnanami::process_arg(index) {
            if let Ok(text) = core::str::from_utf8(arg) {
                libnanami::println!("[alter] argv[{}]={}", index, text);
            }
        }
        index += 1;
    }
}

struct AlterClient {
    port: Word,
    shm: Word,
    shm_size: Word,
}

impl AlterClient {
    fn connect(service_name: &str) -> Result<Self, ()> {
        libnanami::connect_service_by_name(service_name, SLOT_ALTER_SERVICE).map_err(|_| ())?;
        let port = libnanami::ipc::process_slot_descriptor(SLOT_ALTER_SERVICE);
        let (status, shm, size) = libnanami::call_service_port(
            port,
            ALTER_REQUEST_CONTROL,
            ALTER_CONTROL_ATTACH_SHARED_MEMORY,
            ALTER_SHM_BYTES,
            0,
            0,
            3,
        )
        .map_err(|_| ())?;
        if status != libnanami::OS_RESPONSE_OK || shm == 0 || size == 0 {
            return Err(());
        }
        Ok(Self {
            port,
            shm,
            shm_size: size,
        })
    }

    fn write_launch_block(&mut self, launch: &Launch) -> Option<(usize, usize)> {
        if ALTER_ENV.len() > ALTER_LAUNCH_MAX_ENVS {
            return None;
        }
        let mut cursor = 16usize;
        write_shm_word(self.shm, 0, launch.argc as Word);
        write_shm_word(self.shm, 8, ALTER_ENV.len() as Word);
        let mut i = 0usize;
        while i < launch.argc {
            let arg = libnanami::process_arg(launch.first_arg + i)?;
            if i == 0 && launch.diagnostics {
                if let Ok(text) = core::str::from_utf8(arg) {
                    libnanami::println!("[alter] launch image={}", text);
                }
            }
            cursor = write_shm_string(self.shm, self.shm_size, cursor, arg)?;
            i += 1;
        }
        i = 0;
        while i < ALTER_ENV.len() {
            cursor = write_shm_string(self.shm, self.shm_size, cursor, ALTER_ENV[i])?;
            i += 1;
        }
        Some((0, cursor))
    }
}

struct TerminalClient {
    connected: bool,
    port: Word,
    shm: Word,
    terminal_id: Word,
}

impl TerminalClient {
    const fn new() -> Self {
        Self {
            connected: false,
            port: 0,
            shm: 0,
            terminal_id: 0,
        }
    }

    fn connect(&mut self, terminal_id: Word) -> Result<(), ()> {
        nanami_services::registry::connect_terminal_service(SLOT_TERMINAL_SERVICE)
            .map_err(|_| ())?;
        self.port = libnanami::ipc::process_slot_descriptor(SLOT_TERMINAL_SERVICE);
        let (shm, size) = nanami_services::terminal::terminal_attach_shared_memory(
            self.port,
            nanami_services::terminal::TERMINAL_DEFAULT_SHM_BYTES,
        )
        .map_err(|_| ())?;
        if shm == 0 || size == 0 {
            return Err(());
        }
        self.shm = shm;
        self.terminal_id = terminal_id;
        self.connected = true;
        Ok(())
    }

    fn write_line(&mut self, text: &[u8]) {
        self.write(text);
        self.write(b"\n");
    }

    fn write_status_line(&mut self, prefix: &[u8], status: Word) {
        let mut line = [0u8; 80];
        let mut pos = append_bytes(&mut line, 0, prefix);
        pos = append_decimal(&mut line, pos, status);
        let _ = pos;
        self.write_line(&line[..string_len(&line)]);
    }

    fn write(&mut self, bytes: &[u8]) {
        if !self.connected {
            let mut i = 0usize;
            while i < bytes.len() {
                libnanami::debug::write_char(bytes[i] as char);
                i += 1;
            }
            return;
        }
        let mut offset = 0usize;
        while offset < bytes.len() {
            let chunk = (bytes.len() - offset).min(256);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes[offset..].as_ptr(),
                    self.shm as *mut u8,
                    chunk,
                );
            }
            let _ = nanami_services::terminal::terminal_write_output(
                self.port,
                self.terminal_id,
                0,
                chunk as Word,
            );
            offset += chunk;
        }
    }
}

fn wait_child(pid: Word, alter_port: Word, terminal: &mut TerminalClient) {
    let timer_port = connect_timer_service();
    let mut status_failures = 0usize;
    loop {
        match libnanami::call_service_port(alter_port, ALTER_REQUEST_STATUS, pid, 0, 0, 0, 2) {
            Ok((libnanami::OS_RESPONSE_OK, 1, status)) => {
                let _ =
                    libnanami::call_service_port(alter_port, ALTER_REQUEST_KILL, pid, 0, 0, 0, 3);
                if status != 0 {
                    terminal.write_status_line(b"alter: exited status=", status);
                }
                break;
            }
            Ok((libnanami::OS_RESPONSE_OK, 0, _)) => {
                status_failures = 0;
            }
            Ok((status, _, _)) => {
                if status_failures == 0 {
                    terminal.write_status_line(b"alter: status failed ", status);
                }
                status_failures = status_failures.saturating_add(1);
            }
            Err(_) => {
                if status_failures == 0 {
                    terminal.write_line(b"alter: status ipc failed");
                }
                status_failures = status_failures.saturating_add(1);
            }
        }
        if let Some(port) = timer_port {
            let _ = nanami_services::timer::timer_service_sleep_milliseconds(port, 50);
        } else {
            let mut spin = 0usize;
            while spin < 500_000 {
                core::hint::spin_loop();
                spin += 1;
            }
        }
    }
}

fn connect_timer_service() -> Option<Word> {
    match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
        Ok(()) => Some(libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE)),
        Err(_) => None,
    }
}

fn parse_terminal_id() -> Option<Word> {
    let value = libnanami::process_env_value(b"NANAMI_TERMINAL_ID")?;
    parse_decimal(value)
}

fn parse_decimal(bytes: &[u8]) -> Option<Word> {
    let mut value: Word = 0;
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            return None;
        }
        value = value
            .saturating_mul(10)
            .saturating_add((bytes[i] - b'0') as Word);
        i += 1;
    }
    Some(value)
}

fn write_shm_string(base: Word, size: Word, offset: usize, bytes: &[u8]) -> Option<usize> {
    let end = offset.checked_add(bytes.len())?.checked_add(1)?;
    if end > size as usize {
        return None;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (base as usize + offset) as *mut u8,
            bytes.len(),
        );
        core::ptr::write((base as usize + offset + bytes.len()) as *mut u8, 0);
    }
    Some(end)
}

fn write_shm_word(base: Word, offset: usize, value: Word) {
    unsafe {
        core::ptr::write_unaligned((base as usize + offset) as *mut Word, value);
    }
}

fn bytes_eq(left: &[u8], right: &[u8]) -> bool {
    left == right
}

fn append_bytes(dst: &mut [u8], mut pos: usize, src: &[u8]) -> usize {
    let mut i = 0usize;
    while i < src.len() && pos < dst.len() {
        dst[pos] = src[i];
        pos += 1;
        i += 1;
    }
    pos
}

fn append_decimal(dst: &mut [u8], pos: usize, mut value: Word) -> usize {
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        digits[0] = b'0';
        len = 1;
    } else {
        while value != 0 && len < digits.len() {
            digits[len] = b'0' + (value % 10) as u8;
            value /= 10;
            len += 1;
        }
    }
    let mut out = pos;
    while len != 0 {
        len -= 1;
        if out < dst.len() {
            dst[out] = digits[len];
            out += 1;
        }
    }
    out
}

fn string_len(bytes: &[u8]) -> usize {
    let mut len = 0usize;
    while len < bytes.len() && bytes[len] != 0 {
        len += 1;
    }
    len
}

libnanami::nanami_entry!(nanami_main);
