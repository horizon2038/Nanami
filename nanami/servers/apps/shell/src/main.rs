#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::{RequestError, Word};

#[path = "app/font.rs"]
mod font;
mod file;

use font::TextRenderer;

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_NETWORK_SERVICE: Word = 24;
const SLOT_VFS_SERVICE: Word = 25;
const WINDOW_X: Word = 90;
const WINDOW_Y: Word = 78;
const CONTENT_WIDTH: usize = 712;
const CONTENT_HEIGHT: usize = 396;
const COLS: usize = CONTENT_WIDTH / FONT_W;
const ROWS: usize = CONTENT_HEIGHT / FONT_H;
const FONT_W: usize = 8;
const FONT_H: usize = 12;
const MAX_LINE: usize = 96;
const MAX_ROWS: usize = 32;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[shell] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    let (used, remaining, total) = libnanami::heap::heap_stats();
    libnanami::println!(
        "[shell] allocation failed size={:#x} align={:#x} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
        layout.size(),
        layout.align(),
        used,
        remaining,
        total
    );
    let _ = libnanami::request_exit();
    loop {
        core::hint::spin_loop();
    }
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[shell] start\n");
    libnanami::ipc::init_ipc_tls().map_err(|e| log_error("[shell] ipc tls failed: ", e))?;
    let _ = libnanami::heap::init_heap(9 * 1024 * 1024)
        .map_err(|e| log_error("[shell] heap init failed: ", e))?;
    let text = TextRenderer::new();
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);
    libnanami::ipc::bind_current_thread_notification(notification)
        .map_err(|e| log_error("[shell] bind notification failed: ", e))?;
    let (honoka_port, honoka_pid) = connect_honoka_service();
    let window_id = nanami_services::gfx::honoka::honoka_create_window_with_title(
        honoka_port,
        WINDOW_X,
        WINDOW_Y,
        CONTENT_WIDTH as Word,
        CONTENT_HEIGHT as Word,
        b"Shell",
    )
    .map_err(|e| log_error("[shell] create window failed: ", e))?;
    let present_notification = attach_honoka_present_notification(honoka_pid, window_id)
        .map_err(|e| log_error("[shell] present notification failed: ", e))?;
    let (shared_base, size_bytes) =
        nanami_services::gfx::honoka::honoka_attach_logical_framebuffer(honoka_port, window_id)
            .map_err(|e| log_error("[shell] attach framebuffer failed: ", e))?;
    let framebuffer =
        shared_base.saturating_add(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let _pixel_bytes =
        size_bytes.saturating_sub(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let (input_base, _input_bytes) =
        nanami_services::gfx::honoka::honoka_attach_input_queue(honoka_port, window_id)
            .map_err(|e| log_error("[shell] attach input queue failed: ", e))?;
    nanami_services::gfx::honoka::honoka_attach_input_notification(honoka_port, window_id)
        .map_err(|e| log_error("[shell] attach input notification failed: ", e))?;

    let mut shell = Shell::new(
        honoka_port,
        window_id,
        shared_base,
        framebuffer,
        present_notification,
        text,
    );
    shell.boot();
    shell.repaint_all();
    shell.present_full();

    let mut input_queue = nanami_services::input::InputEventQueue::new(input_base);
    loop {
        drain_input(&mut input_queue, &mut shell);
        let waited = libnanami::ipc::notification_wait(notification)
            .map_err(|e| log_error("[shell] notification wait failed: ", e))?;
        if (waited & nanami_services::gfx::honoka::HONOKA_NOTIFICATION_INPUT) != 0 {
            drain_input(&mut input_queue, &mut shell);
        }
    }
}

struct Shell {
    honoka_port: Word,
    window_id: Word,
    damage_queue: Word,
    framebuffer: Word,
    present_notification: Word,
    text: TextRenderer,
    rows: [[u8; COLS]; MAX_ROWS],
    row_count: usize,
    input: [u8; MAX_LINE],
    input_len: usize,
    shift_down: bool,
    files: file::FileShell,
}

impl Shell {
    fn new(
        honoka_port: Word,
        window_id: Word,
        damage_queue: Word,
        framebuffer: Word,
        present_notification: Word,
        text: TextRenderer,
    ) -> Self {
        Self {
            honoka_port,
            window_id,
            damage_queue,
            framebuffer,
            present_notification,
            text,
            rows: [[0; COLS]; MAX_ROWS],
            row_count: 0,
            input: [0; MAX_LINE],
            input_len: 0,
            shift_down: false,
            files: file::FileShell::new(),
        }
    }

    fn boot(&mut self) {
        self.push_line_bytes(b"Nun shell on Honoka");
        self.push_line_bytes(b"type 'help' for commands");
        self.push_prompt();
    }

    fn repaint_all(&mut self) {
        fill_rect(
            self.framebuffer,
            CONTENT_WIDTH,
            CONTENT_HEIGHT,
            0,
            0,
            CONTENT_WIDTH,
            CONTENT_HEIGHT,
            0x0010_1418,
        );
        let mut row = 0usize;
        while row < self.row_count && row < ROWS {
            self.text.draw_text(
                self.framebuffer,
                CONTENT_WIDTH,
                row * FONT_H,
                &self.rows[row],
            );
            row += 1;
        }
    }

    fn present_full(&self) {
        push_damage_rect(self.damage_queue, 0, 0, CONTENT_WIDTH, CONTENT_HEIGHT);
        let _ = libnanami::ipc::notification_notify(self.present_notification);
        let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
            self.honoka_port,
            self.window_id,
            0,
            0,
            CONTENT_WIDTH as Word,
            CONTENT_HEIGHT as Word,
        );
    }

    fn repaint_row(&mut self, row: usize) {
        if row >= self.row_count || row >= ROWS {
            return;
        }
        let y = row * FONT_H;
        fill_rect(
            self.framebuffer,
            CONTENT_WIDTH,
            CONTENT_HEIGHT,
            0,
            y,
            CONTENT_WIDTH,
            FONT_H,
            0x0010_1418,
        );
        self.text
            .draw_text(self.framebuffer, CONTENT_WIDTH, y, &self.rows[row]);
    }

    fn present_row(&self, row: usize) {
        if row >= ROWS {
            return;
        }
        push_damage_rect(self.damage_queue, 0, row * FONT_H, CONTENT_WIDTH, FONT_H);
        let _ = libnanami::ipc::notification_notify(self.present_notification);
        let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
            self.honoka_port,
            self.window_id,
            0,
            (row * FONT_H) as Word,
            CONTENT_WIDTH as Word,
            FONT_H as Word,
        );
    }

    fn on_key(&mut self, code: Word, pressed: bool) {
        match code {
            0x2a | 0x36 => {
                self.shift_down = pressed;
                return;
            }
            _ => {}
        }
        if !pressed {
            return;
        }
        match code {
            0x1c => self.submit(),
            0x0e => self.backspace(),
            _ => {
                if let Some(ch) = scancode_to_ascii(code, self.shift_down) {
                    self.type_char(ch);
                }
            }
        }
    }

    fn type_char(&mut self, ch: u8) {
        if self.input_len >= MAX_LINE {
            return;
        }
        self.input[self.input_len] = ch;
        self.input_len += 1;
        self.refresh_prompt_line();
    }

    fn backspace(&mut self) {
        if self.input_len == 0 {
            return;
        }
        self.input_len -= 1;
        self.refresh_prompt_line();
    }

    fn submit(&mut self) {
        self.finish_current_line();
        self.execute_command();
        self.input_len = 0;
        self.push_prompt();
        self.repaint_all();
        self.present_full();
    }

    fn execute_command(&mut self) {
        if self.input_len == 0 {
            return;
        }
        if bytes_eq(&self.input[..self.input_len], b"help") {
            self.push_line_bytes(b"commands: help, services, netinfo, fstest");
            self.push_line_bytes(b"          ls, cat, rm, mkdir, cd");
            self.push_line_bytes(b"          clear, echo, about");
        } else if bytes_eq(&self.input[..self.input_len], b"services") {
            self.show_services();
        } else if bytes_eq(&self.input[..self.input_len], b"netinfo") {
            self.show_netinfo();
        } else if bytes_eq(&self.input[..self.input_len], b"fstest") {
            self.run_fs_test();
            self.files.invalidate_vfs_session();
        } else if let Some(output) = self.files.execute(&self.input[..self.input_len]) {
            let mut i = 0usize;
            while i < output.len() {
                self.push_line(output.line(i));
                i += 1;
            }
        } else if bytes_eq(&self.input[..self.input_len], b"clear") {
            self.row_count = 0;
        } else if bytes_eq(&self.input[..self.input_len], b"about") {
            self.push_line_bytes(b"Honoka shell: shared-memory UI client");
        } else if starts_with(&self.input[..self.input_len], b"echo ") {
            let mut line = [0u8; COLS];
            copy_bytes(&mut line, &self.input[5..self.input_len]);
            self.push_line(line);
        } else if starts_with(&self.input[..self.input_len], b"window ") {
            let mut window_name = [0u8; 32];
            if self.input_len <= 7 {
                self.push_line_bytes(b"usage: window <title>");
                return;
            }

            copy_bytes(&mut window_name, &self.input[7..self.input_len]);

            match nanami_services::gfx::honoka::honoka_create_window_with_title(
                self.honoka_port,
                WINDOW_X,
                WINDOW_Y,
                CONTENT_WIDTH as Word,
                CONTENT_HEIGHT as Word,
                &window_name,
            ) {
                Ok(_) => {
                    self.push_line_bytes(b"created window");
                }
                Err(_) => self.push_line_bytes(b"create window failed"),
            }
        } else {
            self.push_line_bytes(b"unknown command");
        }
    }

    fn show_services(&mut self) {
        self.push_line_bytes(b"services:");
        let mut ordinal = 0usize;
        while ordinal < 64 {
            match libnanami::service_info_by_ordinal(ordinal as Word) {
                Ok((pid, service_kind)) => {
                    let mut line = [0u8; COLS];
                    let mut pos = 0usize;
                    pos = append_bytes(&mut line, pos, b"  pid=");
                    pos = append_decimal(&mut line, pos, pid);
                    pos = append_bytes(&mut line, pos, b"  ");
                    let _ = append_bytes(&mut line, pos, service_name(service_kind));
                    self.push_line(line);
                }
                Err(_) => break,
            }
            ordinal += 1;
        }
    }

    fn show_netinfo(&mut self) {
        let _ = nanami_services::registry::connect_network_service(SLOT_NETWORK_SERVICE);
        let net_port = libnanami::ipc::process_slot_descriptor(SLOT_NETWORK_SERVICE);
        let (ip, gateway, dns) = match nanami_services::net::net_service_ipv4_config(net_port) {
            Ok(v) => v,
            Err(_) => {
                self.push_line_bytes(b"netinfo: network-service unavailable");
                return;
            }
        };
        let mac = match nanami_services::net::net_service_mac_address(net_port) {
            Ok(v) => v,
            Err(_) => {
                self.push_line_bytes(b"netinfo: mac query failed");
                return;
            }
        };

        self.push_line_bytes(b"network:");
        self.push_line(format_ipv4_line(b"  ip      ", ip));
        self.push_line(format_ipv4_line(b"  gateway ", gateway));
        self.push_line(format_ipv4_line(b"  dns     ", dns));
        self.push_line(format_mac_line(b"  mac     ", mac));
    }

    fn run_fs_test(&mut self) {
        self.push_line_bytes(b"fstest: connect vfs-service");
        let _ = nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE);
        let vfs_port = libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE);
        let (shm, shm_size) =
            match nanami_services::vfs::vfs_attach_shared_memory(vfs_port, 0x4000) {
                Ok(v) => v,
                Err(_) => {
                    self.push_line_bytes(b"fstest: vfs-service unavailable");
                    return;
                }
            };
        if shm_size < 0x1000 {
            self.push_line_bytes(b"fstest: shm too small");
            return;
        }

        if !self.fs_ls_root(vfs_port, shm) {
            return;
        }
        if !self.fs_create_write_read(vfs_port, shm) {
            return;
        }
        self.push_line_bytes(b"fstest: ok");
    }

    fn fs_stat(&mut self, vfs_port: Word, shm: Word, path: &[u8], label: &[u8]) -> bool {
        write_shm_bytes(shm, 0, path);
        match nanami_services::vfs::vfs_stat(vfs_port, 0, path.len() as Word) {
            Ok((inode, size, kind)) => {
                let mut line = [0u8; COLS];
                let mut pos = 0usize;
                pos = append_bytes(&mut line, pos, label);
                pos = append_bytes(&mut line, pos, b": inode=");
                pos = append_decimal(&mut line, pos, inode);
                pos = append_bytes(&mut line, pos, b" size=");
                pos = append_decimal(&mut line, pos, size);
                pos = append_bytes(&mut line, pos, b" type=");
                let _ = append_decimal(&mut line, pos, kind);
                self.push_line(line);
                true
            }
            Err(_) => {
                self.push_line_bytes(b"fstest: stat failed");
                false
            }
        }
    }

    fn fs_cat(&mut self, vfs_port: Word, shm: Word, path: &[u8]) -> bool {
        write_shm_bytes(shm, 0, path);
        let handle = match nanami_services::vfs::vfs_open(vfs_port, 0, path.len() as Word) {
            Ok(h) => h,
            Err(_) => {
                self.push_line_bytes(b"fstest: open /hello.txt failed");
                return false;
            }
        };
        let bytes = match nanami_services::vfs::vfs_read(vfs_port, handle, 0, 64, 512) {
            Ok(n) => n as usize,
            Err(_) => {
                let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
                self.push_line_bytes(b"fstest: read /hello.txt failed");
                return false;
            }
        };
        let mut line = [0u8; COLS];
        let mut pos = append_bytes(&mut line, 0, b"cat /hello.txt: ");
        pos = append_shm_text(&mut line, pos, shm, 512, bytes.min(48));
        let _ = pos;
        self.push_line(line);
        let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
        true
    }

    fn fs_ls_root(&mut self, vfs_port: Word, shm: Word) -> bool {
        write_shm_bytes(shm, 0, b"/");
        let handle = match nanami_services::vfs::vfs_open(vfs_port, 0, 1) {
            Ok(h) => h,
            Err(_) => {
                self.push_line_bytes(b"fstest: open / failed");
                return false;
            }
        };
        let (entries, _) = match nanami_services::vfs::vfs_read_dir(vfs_port, handle, 0, 4, 512) {
            Ok(v) => v,
            Err(_) => {
                let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
                self.push_line_bytes(b"fstest: readdir / failed");
                return false;
            }
        };
        self.push_line_bytes(b"ls /:");
        let mut i = 0usize;
        while i < entries as usize && i < 4 {
            self.push_line(format_dirent_line(
                shm,
                512 + i * nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES,
            ));
            i += 1;
        }
        let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
        true
    }

    fn fs_create_write_read(&mut self, vfs_port: Word, shm: Word) -> bool {
        let path = b"/fstest.txt";
        let renamed = b"/fstest-renamed.txt";
        let body = b"Nanami ext2 write path ok";

        write_shm_bytes(shm, 0, path);
        let _ = nanami_services::vfs::vfs_remove(vfs_port, 0, path.len() as Word);
        write_shm_bytes(shm, 0, renamed);
        let _ = nanami_services::vfs::vfs_remove(vfs_port, 0, renamed.len() as Word);

        write_shm_bytes(shm, 0, path);
        if nanami_services::vfs::vfs_create(vfs_port, 0, path.len() as Word).is_err() {
            self.push_line_bytes(b"fstest: create failed");
            return false;
        }
        let handle = match nanami_services::vfs::vfs_open(vfs_port, 0, path.len() as Word) {
            Ok(h) => h,
            Err(_) => {
                self.push_line_bytes(b"fstest: open new file failed");
                return false;
            }
        };
        write_shm_bytes(shm, 512, body);
        if nanami_services::vfs::vfs_write(vfs_port, handle, 0, body.len() as Word, 512).is_err() {
            let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
            self.push_line_bytes(b"fstest: write failed");
            return false;
        }
        if nanami_services::vfs::vfs_read(vfs_port, handle, 0, body.len() as Word, 768).is_err() {
            let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
            self.push_line_bytes(b"fstest: readback failed");
            return false;
        }
        let _ = nanami_services::vfs::vfs_close(vfs_port, handle);
        if !shm_bytes_eq(shm, 768, body) {
            self.push_line_bytes(b"fstest: readback mismatch");
            return false;
        }

        write_shm_bytes(shm, 0, path);
        write_shm_bytes(shm, 256, renamed);
        if nanami_services::vfs::vfs_rename(
            vfs_port,
            0,
            path.len() as Word,
            256,
            renamed.len() as Word,
        )
        .is_err()
        {
            self.push_line_bytes(b"fstest: rename failed");
            return false;
        }
        write_shm_bytes(shm, 0, renamed);
        if nanami_services::vfs::vfs_remove(vfs_port, 0, renamed.len() as Word).is_err() {
            self.push_line_bytes(b"fstest: remove failed");
            return false;
        }
        self.push_line_bytes(b"fstest: create/write/read/rename/remove ok");
        true
    }

    fn push_prompt(&mut self) {
        let mut line = [0u8; COLS];
        line[0] = b'>';
        line[1] = b' ';
        self.push_line(line);
        self.refresh_prompt_line();
    }

    fn finish_current_line(&mut self) {
        self.refresh_prompt_line();
    }

    fn refresh_prompt_line(&mut self) {
        if self.row_count == 0 {
            return;
        }
        let row = self.row_count - 1;
        let mut line = [0u8; COLS];
        line[0] = b'>';
        line[1] = b' ';
        let max = self.input_len.min(COLS.saturating_sub(3));
        let mut i = 0usize;
        while i < max {
            line[2 + i] = self.input[i];
            i += 1;
        }
        if 2 + max < COLS {
            line[2 + max] = b'_';
        }
        self.rows[row] = line;
        self.repaint_row(row);
        self.present_row(row);
    }

    fn push_line_bytes(&mut self, bytes: &[u8]) {
        let mut line = [0u8; COLS];
        copy_bytes(&mut line, bytes);
        self.push_line(line);
    }

    fn push_line(&mut self, line: [u8; COLS]) {
        if self.row_count >= MAX_ROWS || self.row_count >= ROWS {
            let limit = self.row_count.min(MAX_ROWS).min(ROWS);
            let mut i = 1usize;
            while i < limit {
                self.rows[i - 1] = self.rows[i];
                i += 1;
            }
            self.row_count = limit.saturating_sub(1);
        }
        self.rows[self.row_count] = line;
        self.row_count += 1;
    }
}

fn drain_input(input_queue: &mut nanami_services::input::InputEventQueue, shell: &mut Shell) {
    let mut drained = 0usize;
    while drained < 256 {
        let Some(packed) = input_queue.pop() else {
            break;
        };
        let (kind, code, value0, _, _) = nanami_services::input::unpack_input_event(packed);
        if kind == nanami_services::input::INPUT_EVENT_KIND_KEY {
            shell.on_key(code, value0 != 0);
        }
        drained += 1;
    }
}

fn connect_honoka_service() -> (Word, Word) {
    loop {
        match nanami_services::registry::connect_honoka_service_with_pid(SLOT_HONOKA_SERVICE) {
            Ok(pid) => {
                return (
                    libnanami::ipc::process_slot_descriptor(SLOT_HONOKA_SERVICE),
                    pid,
                )
            }
            Err(e) => {
                log_request_error("[shell] waiting honoka-service: ", e);
                busy_delay();
            }
        }
    }
}

fn attach_honoka_present_notification(
    honoka_pid: Word,
    window_id: Word,
) -> Result<Word, RequestError> {
    libnanami::request_notification_port_copy(
        honoka_pid,
        libnanami::PROCESS_SLOT_NOTIFICATION,
        SLOT_HONOKA_PRESENT_NOTIFICATION,
        nanami_services::gfx::honoka::HONOKA_NOTIFICATION_PRESENT | (window_id & 0xffff_ffff),
    )?;
    Ok(libnanami::ipc::process_slot_descriptor(
        SLOT_HONOKA_PRESENT_NOTIFICATION,
    ))
}

fn scancode_to_ascii(code: Word, shift: bool) -> Option<u8> {
    let ch = match code {
        0x02 => {
            if shift {
                b'!'
            } else {
                b'1'
            }
        }
        0x03 => {
            if shift {
                b'@'
            } else {
                b'2'
            }
        }
        0x04 => {
            if shift {
                b'#'
            } else {
                b'3'
            }
        }
        0x05 => {
            if shift {
                b'$'
            } else {
                b'4'
            }
        }
        0x06 => {
            if shift {
                b'%'
            } else {
                b'5'
            }
        }
        0x07 => {
            if shift {
                b'^'
            } else {
                b'6'
            }
        }
        0x08 => {
            if shift {
                b'&'
            } else {
                b'7'
            }
        }
        0x09 => {
            if shift {
                b'*'
            } else {
                b'8'
            }
        }
        0x0a => {
            if shift {
                b'('
            } else {
                b'9'
            }
        }
        0x0b => {
            if shift {
                b')'
            } else {
                b'0'
            }
        }
        0x0c => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        0x0d => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        0x10 => letter(b'q', shift),
        0x11 => letter(b'w', shift),
        0x12 => letter(b'e', shift),
        0x13 => letter(b'r', shift),
        0x14 => letter(b't', shift),
        0x15 => letter(b'y', shift),
        0x16 => letter(b'u', shift),
        0x17 => letter(b'i', shift),
        0x18 => letter(b'o', shift),
        0x19 => letter(b'p', shift),
        0x1a => {
            if shift {
                b'{'
            } else {
                b'['
            }
        }
        0x1b => {
            if shift {
                b'}'
            } else {
                b']'
            }
        }
        0x1e => letter(b'a', shift),
        0x1f => letter(b's', shift),
        0x20 => letter(b'd', shift),
        0x21 => letter(b'f', shift),
        0x22 => letter(b'g', shift),
        0x23 => letter(b'h', shift),
        0x24 => letter(b'j', shift),
        0x25 => letter(b'k', shift),
        0x26 => letter(b'l', shift),
        0x27 => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        0x28 => {
            if shift {
                b'"'
            } else {
                b'\''
            }
        }
        0x29 => {
            if shift {
                b'~'
            } else {
                b'`'
            }
        }
        0x2b => {
            if shift {
                b'|'
            } else {
                b'\\'
            }
        }
        0x2c => letter(b'z', shift),
        0x2d => letter(b'x', shift),
        0x2e => letter(b'c', shift),
        0x2f => letter(b'v', shift),
        0x30 => letter(b'b', shift),
        0x31 => letter(b'n', shift),
        0x32 => letter(b'm', shift),
        0x33 => {
            if shift {
                b'<'
            } else {
                b','
            }
        }
        0x34 => {
            if shift {
                b'>'
            } else {
                b'.'
            }
        }
        0x35 => {
            if shift {
                b'?'
            } else {
                b'/'
            }
        }
        0x39 => b' ',
        _ => return None,
    };
    Some(ch)
}

fn letter(ch: u8, shift: bool) -> u8 {
    if shift {
        ch - 32
    } else {
        ch
    }
}

fn fill_rect(
    vaddr: Word,
    fb_width: usize,
    fb_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: u32,
) {
    let y_end = y.saturating_add(height).min(fb_height);
    let x_end = x.saturating_add(width).min(fb_width);
    let mut yy = y;
    while yy < y_end {
        let mut xx = x;
        while xx < x_end {
            put_pixel(vaddr, fb_width, xx, yy, color);
            xx += 1;
        }
        yy += 1;
    }
}

fn put_pixel(vaddr: Word, fb_width: usize, x: usize, y: usize, color: u32) {
    let index = y.saturating_mul(fb_width).saturating_add(x);
    unsafe {
        core::ptr::write_volatile((vaddr + (index * 4) as Word) as *mut u32, color);
    }
}

fn push_damage_rect(base: Word, x: usize, y: usize, width: usize, height: usize) {
    write_word(
        base,
        nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS,
        x as Word,
    );
    write_word(
        base,
        nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS + 1,
        y as Word,
    );
    write_word(
        base,
        nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS + 2,
        width as Word,
    );
    write_word(
        base,
        nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS + 3,
        height as Word,
    );
    write_word(base, 4, read_word(base, 4).wrapping_add(1).max(1));
}

fn read_word(base: Word, index: usize) -> Word {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr).load(Ordering::SeqCst) as Word
    }
}

fn write_word(base: Word, index: usize, value: Word) {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr).store(value as usize, Ordering::SeqCst);
    }
}

const fn word_offset(index: usize) -> Word {
    (index * core::mem::size_of::<Word>()) as Word
}

fn copy_bytes(dst: &mut [u8], src: &[u8]) {
    let mut i = 0usize;
    while i < dst.len() && i < src.len() {
        dst[i] = src[i];
        i += 1;
    }
}

fn append_bytes(dst: &mut [u8], mut pos: usize, src: &[u8]) -> usize {
    let mut i = 0usize;
    while pos < dst.len() && i < src.len() {
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
        return append_bytes(dst, pos, b"0");
    }
    while value != 0 && len < digits.len() {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    let mut out = pos;
    while len != 0 {
        len -= 1;
        if out >= dst.len() {
            break;
        }
        dst[out] = digits[len];
        out += 1;
    }
    out
}

fn read_shm_word(base: Word, offset: usize) -> Word {
    unsafe { core::ptr::read_unaligned((base as usize + offset) as *const Word) }
}

fn read_shm_byte(base: Word, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u8) }
}

fn write_shm_bytes(base: Word, offset: usize, bytes: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), (base as usize + offset) as *mut u8, bytes.len());
    }
}

fn shm_bytes_eq(base: Word, offset: usize, expected: &[u8]) -> bool {
    let mut i = 0usize;
    while i < expected.len() {
        if read_shm_byte(base, offset + i) != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn append_shm_text(dst: &mut [u8], mut pos: usize, base: Word, offset: usize, len: usize) -> usize {
    let mut i = 0usize;
    while pos < dst.len() && i < len {
        let byte = read_shm_byte(base, offset + i);
        dst[pos] = match byte {
            b'\n' | b'\r' | b'\t' => b' ',
            0x20..=0x7e => byte,
            _ => b'.',
        };
        pos += 1;
        i += 1;
    }
    pos
}

fn format_dirent_line(base: Word, offset: usize) -> [u8; COLS] {
    let inode = read_shm_word(base, offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_INODE_OFFSET);
    let kind = read_shm_word(base, offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET);
    let name_len = read_shm_word(
        base,
        offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET,
    ) as usize;
    let name_len = name_len.min(nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES);
    let mut line = [0u8; COLS];
    let mut pos = append_bytes(&mut line, 0, b"  ");
    pos = append_shm_text(
        &mut line,
        pos,
        base,
        offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET,
        name_len,
    );
    pos = append_bytes(&mut line, pos, b" inode=");
    pos = append_decimal(&mut line, pos, inode);
    pos = append_bytes(&mut line, pos, b" type=");
    let _ = append_decimal(&mut line, pos, kind);
    line
}

fn append_hex_byte(dst: &mut [u8], pos: usize, value: u8) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = pos;
    if out < dst.len() {
        dst[out] = HEX[(value >> 4) as usize];
        out += 1;
    }
    if out < dst.len() {
        dst[out] = HEX[(value & 0x0f) as usize];
        out += 1;
    }
    out
}

fn format_ipv4_line(prefix: &[u8], ip: [u8; 4]) -> [u8; COLS] {
    let mut line = [0u8; COLS];
    let mut pos = append_bytes(&mut line, 0, prefix);
    pos = append_decimal(&mut line, pos, ip[0] as Word);
    pos = append_bytes(&mut line, pos, b".");
    pos = append_decimal(&mut line, pos, ip[1] as Word);
    pos = append_bytes(&mut line, pos, b".");
    pos = append_decimal(&mut line, pos, ip[2] as Word);
    pos = append_bytes(&mut line, pos, b".");
    let _ = append_decimal(&mut line, pos, ip[3] as Word);
    line
}

fn format_mac_line(prefix: &[u8], mac: [u8; 6]) -> [u8; COLS] {
    let mut line = [0u8; COLS];
    let mut pos = append_bytes(&mut line, 0, prefix);
    let mut i = 0usize;
    while i < mac.len() {
        if i != 0 {
            pos = append_bytes(&mut line, pos, b":");
        }
        pos = append_hex_byte(&mut line, pos, mac[i]);
        i += 1;
    }
    line
}

fn service_name(kind: Word) -> &'static [u8] {
    match kind {
        nanami_services::registry::SERVICE_KIND_NET_DEVICE => b"net-device",
        nanami_services::registry::SERVICE_KIND_NETWORK_SERVICE => b"network-service",
        nanami_services::registry::SERVICE_KIND_TIMER_SERVICE => b"timer-service",
        nanami_services::registry::SERVICE_KIND_DISPLAY_SERVICE => b"display_service",
        nanami_services::registry::SERVICE_KIND_INPUT_SERVICE => b"input-service",
        nanami_services::registry::SERVICE_KIND_HONOKA_SERVICE => b"honoka-service",
        nanami_services::registry::SERVICE_KIND_VFS_SERVICE => b"vfs-service",
        nanami_services::registry::SERVICE_KIND_BLOCK_DEVICE => b"block-device",
        _ => b"unknown-service",
    }
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn starts_with(a: &[u8], b: &[u8]) -> bool {
    if a.len() < b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < b.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn busy_delay() {
    let mut i = 0usize;
    while i < 400_000 {
        core::hint::spin_loop();
        i += 1;
    }
}

fn log_error(prefix: &str, err: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

libnanami::nanami_entry!(nanami_main);
