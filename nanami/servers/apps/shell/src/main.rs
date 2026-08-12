#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::{RequestError, Word};

mod ansi_escape;
mod exec;
mod file;
#[path = "app/font.rs"]
mod font;
mod foreground;

use font::{TextRenderer, DEFAULT_TEXT_COLOR};

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_NETWORK_SERVICE: Word = 24;
const SLOT_VFS_SERVICE: Word = 25;
const SLOT_POSIX_SERVICE: Word = 26;
const SLOT_POSIX_TEST_TIMER_SERVICE: Word = 27;
const SLOT_EXEC_SERVICE: Word = 31;
const SLOT_SHELL_TIMER_SERVICE: Word = 30;
const WINDOW_X: Word = 90;
const WINDOW_Y: Word = 78;
const WINDOW_OPACITY: u8 = 224;
const CONTENT_WIDTH: usize = 712;
const CONTENT_HEIGHT: usize = 396;
const COLS: usize = CONTENT_WIDTH / FONT_W;
const ROWS: usize = CONTENT_HEIGHT / FONT_H;
const FONT_W: usize = 8;
const FONT_H: usize = 12;
const MAX_LINE: usize = 96;
const MAX_ROWS: usize = 128;
const HISTORY_MAX: usize = 16;
const MODIFIER_LEFT_SHIFT: u8 = 1 << 0;
const MODIFIER_RIGHT_SHIFT: u8 = 1 << 1;
const FOREGROUND_DRAIN_BATCHES: usize = 64;

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
    nanami_services::gfx::honoka::honoka_set_window_opacity(honoka_port, window_id, WINDOW_OPACITY)
        .map_err(|e| log_error("[shell] set window opacity failed: ", e))?;
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
    start_shell_timer();

    let mut input_queue = nanami_services::input::InputEventQueue::new(input_base);
    loop {
        drain_input(&mut input_queue, &mut shell);
        if shell.drain_foreground_output() {
            shell.repaint_all();
            shell.present_full();
            continue;
        }
        let waited = libnanami::ipc::notification_wait(notification)
            .map_err(|e| log_error("[shell] notification wait failed: ", e))?;
        if (waited & nanami_services::gfx::honoka::HONOKA_NOTIFICATION_INPUT) != 0 {
            drain_input(&mut input_queue, &mut shell);
        }
        if (waited & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT) != 0 {
            shell.on_timer();
        }
        if shell.drain_foreground_output() {
            shell.repaint_all();
            shell.present_full();
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
    row_colors: [[u32; COLS]; MAX_ROWS],
    row_count: usize,
    scroll_offset: usize,
    input: [u8; MAX_LINE],
    input_len: usize,
    history: [[u8; MAX_LINE]; HISTORY_MAX],
    history_lens: [usize; HISTORY_MAX],
    history_count: usize,
    history_cursor: usize,
    cursor_visible: bool,
    cursor_ticks: usize,
    modifier_state: u8,
    files: file::FileShell,
    exec: exec::ExecShell,
    foreground: foreground::ForegroundApp,
    foreground_partial_row: bool,
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
            row_colors: [[DEFAULT_TEXT_COLOR; COLS]; MAX_ROWS],
            row_count: 0,
            scroll_offset: 0,
            input: [0; MAX_LINE],
            input_len: 0,
            history: [[0; MAX_LINE]; HISTORY_MAX],
            history_lens: [0; HISTORY_MAX],
            history_count: 0,
            history_cursor: 0,
            cursor_visible: true,
            cursor_ticks: 0,
            modifier_state: 0,
            files: file::FileShell::new(),
            exec: exec::ExecShell::new(),
            foreground: foreground::ForegroundApp::new(),
            foreground_partial_row: false,
        }
    }

    fn boot(&mut self) {
        self.push_line_bytes(b"Nanami Shell");
        self.push_line_bytes(b"* type 'help' for commands");
        self.push_prompt();
    }

    fn repaint_all(&mut self) {
        fill_rect(
            self.framebuffer,
            (CONTENT_WIDTH, CONTENT_HEIGHT),
            (0, 0, CONTENT_WIDTH, CONTENT_HEIGHT),
            0x0010_1418,
        );
        let start = self.visible_start();
        let mut row = 0usize;
        while row < ROWS {
            let source = start + row;
            if source >= self.row_count {
                break;
            }
            self.text.draw_text_colored(
                self.framebuffer,
                CONTENT_WIDTH,
                row * FONT_H,
                &self.rows[source],
                &self.row_colors[source],
            );
            self.draw_block_cursor(source, row);
            row += 1;
        }
    }

    fn present_full(&self) {
        if push_damage_rect(self.damage_queue, 0, 0, CONTENT_WIDTH, CONTENT_HEIGHT) {
            let _ = libnanami::ipc::notification_notify(self.present_notification);
        } else {
            let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
                self.honoka_port,
                self.window_id,
                0,
                0,
                CONTENT_WIDTH as Word,
                CONTENT_HEIGHT as Word,
            );
        }
    }

    fn repaint_row(&mut self, row: usize) {
        if row >= ROWS {
            return;
        }
        let source = self.visible_start() + row;
        if source >= self.row_count {
            return;
        }
        let y = row * FONT_H;
        fill_rect(
            self.framebuffer,
            (CONTENT_WIDTH, CONTENT_HEIGHT),
            (0, y, CONTENT_WIDTH, FONT_H),
            0x0010_1418,
        );
        self.text.draw_text_colored(
            self.framebuffer,
            CONTENT_WIDTH,
            y,
            &self.rows[source],
            &self.row_colors[source],
        );
        self.draw_block_cursor(source, row);
    }

    fn draw_block_cursor(&self, logical_row: usize, screen_row: usize) {
        if !self.cursor_visible
            || logical_row + 1 != self.row_count
            || self.rows[logical_row] != self.prompt_line()
        {
            return;
        }
        let visible_input = self.input_len.min(COLS.saturating_sub(3));
        let column = 2 + visible_input;
        if column >= COLS {
            return;
        }
        fill_rect(
            self.framebuffer,
            (CONTENT_WIDTH, CONTENT_HEIGHT),
            (column * FONT_W, screen_row * FONT_H, FONT_W, FONT_H),
            DEFAULT_TEXT_COLOR,
        );
    }

    fn present_row(&self, row: usize) {
        if row >= ROWS {
            return;
        }
        if push_damage_rect(self.damage_queue, 0, row * FONT_H, CONTENT_WIDTH, FONT_H) {
            let _ = libnanami::ipc::notification_notify(self.present_notification);
        } else {
            let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
                self.honoka_port,
                self.window_id,
                0,
                (row * FONT_H) as Word,
                CONTENT_WIDTH as Word,
                FONT_H as Word,
            );
        }
    }

    fn on_key(&mut self, code: Word, pressed: bool) {
        match code {
            0x2a => {
                self.set_modifier(MODIFIER_LEFT_SHIFT, pressed);
                return;
            }
            0x36 => {
                self.set_modifier(MODIFIER_RIGHT_SHIFT, pressed);
                return;
            }
            _ => {}
        }
        if !pressed {
            return;
        }
        if self.foreground.is_active() {
            self.on_foreground_key(code);
            return;
        }
        match code {
            0x1c => self.submit(),
            0x0e => self.backspace(),
            0x48 => self.history_prev(),
            0x50 => self.history_next(),
            0x49 => self.scroll_page_up(),
            0x51 => self.scroll_page_down(),
            _ => {
                if let Some(ch) = scancode_to_ascii(code, self.shift_active()) {
                    self.type_char(ch);
                }
            }
        }
    }

    fn on_foreground_key(&mut self, code: Word) {
        let mut input_ok = true;
        match code {
            0x58 => {
                let output = self.foreground.terminate_active();
                self.push_foreground_output(output);
                self.input_len = 0;
                self.push_prompt();
                self.repaint_all();
                self.present_full();
                return;
            }
            0x1c => {
                input_ok = self.foreground.send_input_byte(b'\n');
            }
            0x0e => {
                input_ok = self.foreground.send_input_byte(0x7f);
            }
            _ => {
                if let Some(ch) = scancode_to_ascii(code, self.shift_active()) {
                    input_ok = self.foreground.send_input_byte(ch);
                }
            }
        }
        if !input_ok {
            self.push_line_bytes(b"[foreground input failed]");
        }
        if self.drain_foreground_output() {
            self.repaint_all();
            self.present_full();
        }
    }

    fn type_char(&mut self, ch: u8) {
        if self.input_len >= MAX_LINE {
            return;
        }
        self.input[self.input_len] = ch;
        self.input_len += 1;
        self.history_cursor = self.history_count;
        self.cursor_visible = true;
        self.cursor_ticks = 0;
        self.refresh_prompt_line();
    }

    fn backspace(&mut self) {
        if self.input_len == 0 {
            return;
        }
        self.input_len -= 1;
        self.input[self.input_len] = 0;
        self.history_cursor = self.history_count;
        self.cursor_visible = true;
        self.cursor_ticks = 0;
        self.refresh_prompt_line();
    }

    fn submit(&mut self) {
        self.finish_current_line();
        self.trim_input();
        self.push_history();
        self.execute_command();
        let _ = self.drain_foreground_output();
        self.input = [0; MAX_LINE];
        self.input_len = 0;
        if !self.foreground.is_active() {
            self.push_prompt();
        }
        self.repaint_all();
        self.present_full();
    }

    fn execute_command(&mut self) {
        if self.input_len == 0 {
            return;
        }
        if bytes_eq(&self.input[..self.input_len], b"help") {
            self.push_line_bytes(b"commands: help, services, netinfo, fstest, posixtest");
            self.push_line_bytes(b"          ls, cat, rm, mkdir, cd");
            self.push_line_bytes(b"          path [PATH], external apps via PATH");
            self.push_line_bytes(b"          nanami-info memory|process, performance-monitor");
            self.push_line_bytes(b"foreground app: F12 terminate");
            self.push_line_bytes(b"          nanami-control os.log enable|disable");
            self.push_line_bytes(b"          clear, echo, about");
        } else if bytes_eq(&self.input[..self.input_len], b"services") {
            self.show_services();
        } else if bytes_eq(&self.input[..self.input_len], b"netinfo") {
            self.show_netinfo();
        } else if bytes_eq(&self.input[..self.input_len], b"fstest") {
            self.run_fs_test();
            self.files.invalidate_vfs_session();
        } else if bytes_eq(&self.input[..self.input_len], b"posixtest") {
            self.run_posix_test();
        } else if starts_with(&self.input[..self.input_len], b"nanami-control ") {
            self.run_nanami_control();
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
        } else if bytes_eq(&self.input[..self.input_len], b"echo") {
            self.push_line([0; COLS]);
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
        } else if let Some(output) = self.exec.execute_builtin(&self.input[..self.input_len]) {
            let mut i = 0usize;
            while i < output.len() {
                self.push_line(output.line(i));
                i += 1;
            }
        } else {
            self.spawn_external_foreground();
        }
    }

    fn spawn_external_foreground(&mut self) {
        let mut terminal_output = foreground::CommandOutput::new();
        if !self.foreground.ensure_terminal(&mut terminal_output) {
            self.push_foreground_output(terminal_output);
            return;
        }
        self.foreground.prepare_start();
        let mut output = exec::CommandOutput::new();
        if let Some((pid, _path, _path_len)) = self.exec.spawn_with_terminal(
            &self.input[..self.input_len],
            self.foreground.terminal_id(),
            &mut output,
        ) {
            self.foreground.start(pid, self.exec.service_port());
            return;
        }
        let mut i = 0usize;
        while i < output.len() {
            self.push_line(output.line(i));
            i += 1;
        }
    }

    fn run_nanami_control(&mut self) {
        let input = &self.input[..self.input_len];
        let result = if bytes_eq(input, b"nanami-control os.log enable") {
            libnanami::request_nanami_control("os.log", "enable")
        } else if bytes_eq(input, b"nanami-control os.log disable") {
            libnanami::request_nanami_control("os.log", "disable")
        } else {
            self.push_line_bytes(b"usage: nanami-control os.log enable|disable");
            return;
        };

        match result {
            Ok(()) => self.push_line_bytes(b"nanami-control: ok"),
            Err(_) => self.push_line_bytes(b"nanami-control: failed"),
        }
    }

    fn drain_foreground_output(&mut self) -> bool {
        let mut changed = false;
        let mut batches = 0usize;
        while batches < FOREGROUND_DRAIN_BATCHES {
            let Some(output) = self.foreground.drain_output() else {
                break;
            };
            let had_prompt = self.remove_prompt_row_if_present();
            let should_restore_prompt = had_prompt || !self.foreground.is_active();
            self.push_foreground_output(output);
            if should_restore_prompt {
                self.push_colored_line(self.prompt_line(), [DEFAULT_TEXT_COLOR; COLS]);
            }
            changed = true;
            batches += 1;
        }
        changed
    }

    fn push_foreground_output(&mut self, output: foreground::CommandOutput) -> bool {
        let mut scrolled = false;
        if output.clear_screen() {
            self.row_count = 0;
            self.scroll_offset = 0;
            self.foreground_partial_row = false;
            scrolled = true;
        }
        let mut i = 0usize;
        while i < output.len() {
            scrolled |=
                self.push_foreground_line(output.line(i), output.colors(i), output.is_partial(i));
            i += 1;
        }
        scrolled
    }

    fn push_foreground_line(
        &mut self,
        line: [u8; COLS],
        colors: [u32; COLS],
        partial: bool,
    ) -> bool {
        if self.foreground_partial_row && self.row_count != 0 {
            let row = self.row_count - 1;
            self.rows[row] = line;
            self.row_colors[row] = colors;
            self.foreground_partial_row = partial;
            return false;
        }
        let scrolled = self.push_colored_line(line, colors);
        self.foreground_partial_row = partial;
        scrolled
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
        let (shm, shm_size) = match nanami_services::vfs::vfs_attach_shared_memory(vfs_port, 0x4000)
        {
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
        while i < entries && i < 4 {
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

    fn run_posix_test(&mut self) {
        self.push_line_bytes(b"posixtest: connect posix-service");
        let _ = nanami_services::registry::connect_posix_service(SLOT_POSIX_SERVICE);
        let posix_port = libnanami::ipc::process_slot_descriptor(SLOT_POSIX_SERVICE);
        let (shm, shm_size) =
            match nanami_services::posix::posix_attach_shared_memory(posix_port, 0x4000) {
                Ok(v) => v,
                Err(_) => {
                    self.push_line_bytes(b"posixtest: posix-service unavailable");
                    return;
                }
            };
        if shm_size < 0x1000 {
            self.push_line_bytes(b"posixtest: shm too small");
            return;
        }

        match nanami_services::posix::posix_getpid(posix_port) {
            Ok(pid) => {
                let mut line = [0u8; COLS];
                let pos = append_bytes(&mut line, 0, b"posix pid=");
                let _ = append_decimal(&mut line, pos, pid);
                self.push_line(line);
            }
            Err(_) => {
                self.push_line_bytes(b"posixtest: getpid failed");
                return;
            }
        }

        if !self.posix_process_memory_test(posix_port) {
            return;
        }
        if !self.posix_environment_test(posix_port, shm) {
            return;
        }
        if !self.posix_process_lifecycle_test(posix_port) {
            return;
        }
        if !self.posix_spawn_test(posix_port, shm) {
            return;
        }
        if !self.posix_dev_zero_test(posix_port, shm) {
            return;
        }
        if !self.posix_dir_test(posix_port, shm) {
            return;
        }
        if !self.posix_file_test(posix_port, shm) {
            return;
        }
        if !self.posix_stat_test(posix_port, shm) {
            return;
        }
        self.push_line_bytes(b"posixtest: ok");
    }

    fn posix_process_memory_test(&mut self, posix_port: Word) -> bool {
        let ppid = match nanami_services::posix::posix_getppid(posix_port) {
            Ok(ppid) => ppid,
            Err(_) => {
                self.push_line_bytes(b"posixtest: getppid failed");
                return false;
            }
        };
        if ppid != nanami_services::posix::POSIX_PROCESS_ROOT_PID {
            self.push_line_bytes(b"posixtest: ppid mismatch");
            return false;
        }
        if nanami_services::posix::posix_get_native_pid(posix_port).is_err() {
            self.push_line_bytes(b"posixtest: native pid failed");
            return false;
        }
        if nanami_services::posix::posix_getuid(posix_port).ok()
            != Some(nanami_services::posix::POSIX_ROOT_UID)
            || nanami_services::posix::posix_geteuid(posix_port).ok()
                != Some(nanami_services::posix::POSIX_ROOT_UID)
            || nanami_services::posix::posix_getgid(posix_port).ok()
                != Some(nanami_services::posix::POSIX_ROOT_GID)
            || nanami_services::posix::posix_getegid(posix_port).ok()
                != Some(nanami_services::posix::POSIX_ROOT_GID)
        {
            self.push_line_bytes(b"posixtest: credential mismatch");
            return false;
        }
        let pid = match nanami_services::posix::posix_getpid(posix_port) {
            Ok(pid) => pid,
            Err(_) => {
                self.push_line_bytes(b"posixtest: getpid recheck failed");
                return false;
            }
        };
        if nanami_services::posix::posix_getpgid(posix_port, 0).ok() != Some(pid)
            || nanami_services::posix::posix_getsid(posix_port, 0).ok()
                != Some(nanami_services::posix::POSIX_PROCESS_ROOT_PID)
        {
            self.push_line_bytes(b"posixtest: process group mismatch");
            return false;
        }
        if nanami_services::posix::posix_setpgid(posix_port, 0, 0).is_err()
            || nanami_services::posix::posix_getpgid(posix_port, 0).ok() != Some(pid)
        {
            self.push_line_bytes(b"posixtest: setpgid failed");
            return false;
        }
        if nanami_services::posix::posix_getpagesize() != 4096 {
            self.push_line_bytes(b"posixtest: pagesize mismatch");
            return false;
        }
        let (base, mapped) = match nanami_services::posix::posix_mmap_anonymous(4096) {
            Ok(v) => v,
            Err(_) => {
                self.push_line_bytes(b"posixtest: mmap anon failed");
                return false;
            }
        };
        if base == 0 || mapped < 4096 {
            self.push_line_bytes(b"posixtest: mmap anon invalid");
            return false;
        }
        unsafe {
            core::ptr::write_volatile(base as *mut u64, 0x706f_7369_782d_6d6d);
            if core::ptr::read_volatile(base as *const u64) != 0x706f_7369_782d_6d6d {
                self.push_line_bytes(b"posixtest: mmap anon rw failed");
                return false;
            }
        }
        let (base2, mapped2) = match nanami_services::posix::posix_mmap(
            4096,
            nanami_services::posix::POSIX_PROT_READ | nanami_services::posix::POSIX_PROT_WRITE,
            nanami_services::posix::POSIX_MAP_PRIVATE | nanami_services::posix::POSIX_MAP_ANONYMOUS,
        ) {
            Ok(v) => v,
            Err(_) => {
                self.push_line_bytes(b"posixtest: mmap flags failed");
                return false;
            }
        };
        if !is_unsupported(nanami_services::posix::posix_mprotect(
            base,
            4096,
            nanami_services::posix::POSIX_PROT_READ,
        )) {
            self.push_line_bytes(b"posixtest: mprotect should be unsupported");
            return false;
        }
        if let Err(e) = nanami_services::posix::posix_munmap(base, 4096) {
            self.push_posix_error(b"posixtest: munmap failed ", e);
            return false;
        }
        if let Err(e) = nanami_services::posix::posix_munmap(base2, mapped2) {
            self.push_posix_error(b"posixtest: munmap second failed ", e);
            return false;
        }
        self.push_line_bytes(b"posixtest: process/memory ok");
        true
    }

    fn posix_environment_test(&mut self, posix_port: Word, shm: Word) -> bool {
        write_shm_bytes(shm, 0, b"PATH");
        match nanami_services::posix::posix_getenv(posix_port, 0, 4, 512, 64) {
            Ok(len) if len > 0 => {}
            _ => {
                self.push_line_bytes(b"posixtest: getenv PATH failed");
                return false;
            }
        }

        write_shm_bytes(shm, 0, b"NANAMI_ENV");
        write_shm_bytes(shm, 128, b"working");
        if nanami_services::posix::posix_setenv(posix_port, 0, 10, 128, 7).is_err() {
            self.push_line_bytes(b"posixtest: setenv failed");
            return false;
        }
        match nanami_services::posix::posix_getenv(posix_port, 0, 10, 512, 32) {
            Ok(7) if shm_bytes_eq(shm, 512, b"working") => {}
            _ => {
                self.push_line_bytes(b"posixtest: getenv mismatch");
                return false;
            }
        }

        let count = match nanami_services::posix::posix_env_count(posix_port) {
            Ok(count) if count >= 4 => count,
            _ => {
                self.push_line_bytes(b"posixtest: env count failed");
                return false;
            }
        };
        let mut found = false;
        let mut index = 0;
        while index < count {
            match nanami_services::posix::posix_env_at(posix_port, index, 768, 64) {
                Ok((10, 7)) if shm_bytes_eq(shm, 768, b"NANAMI_ENV=working") => {
                    found = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => {
                    self.push_line_bytes(b"posixtest: env_at failed");
                    return false;
                }
            }
            index += 1;
        }
        if !found {
            self.push_line_bytes(b"posixtest: env_at missing variable");
            return false;
        }

        write_shm_bytes(shm, 0, b"NANAMI_ENV");
        if nanami_services::posix::posix_unsetenv(posix_port, 0, 10).is_err() {
            self.push_line_bytes(b"posixtest: unsetenv failed");
            return false;
        }
        if !matches!(
            nanami_services::posix::posix_getenv(posix_port, 0, 10, 512, 32),
            Err(libnanami::RequestError::Status(
                libnanami::OS_RESPONSE_INVALID_DESCRIPTOR
            ))
        ) {
            self.push_line_bytes(b"posixtest: unsetenv still visible");
            return false;
        }
        self.push_line_bytes(b"posixtest: environment ok");
        true
    }

    fn posix_process_lifecycle_test(&mut self, posix_port: Word) -> bool {
        let self_pid = match nanami_services::posix::posix_getpid(posix_port) {
            Ok(pid) => pid,
            Err(_) => {
                self.push_line_bytes(b"posixtest: getpid for kill failed");
                return false;
            }
        };
        if nanami_services::posix::posix_kill(posix_port, self_pid, 0).is_err() {
            self.push_line_bytes(b"posixtest: kill probe failed");
            return false;
        }
        let _ = nanami_services::posix::posix_fork(posix_port);
        self.push_line_bytes(b"posixtest: lifecycle ok");
        true
    }

    fn posix_spawn_test(&mut self, posix_port: Word, shm: Word) -> bool {
        let path = b"/posix-inherit.txt";
        let body = b"POSIX inherited fd works";
        write_shm_bytes(shm, 0, path);
        let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
        let fd = match nanami_services::posix::posix_open(
            posix_port,
            0,
            path.len() as Word,
            nanami_services::posix::POSIX_O_CREAT | nanami_services::posix::POSIX_O_TRUNC,
        ) {
            Ok(fd) => fd,
            Err(_) => {
                self.push_line_bytes(b"posixtest: inherit file open failed");
                return false;
            }
        };
        if fd != 3 {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: inherit fd was not 3");
            return false;
        }
        write_shm_bytes(shm, 512, body);
        if nanami_services::posix::posix_write(posix_port, fd, 512, body.len() as Word).is_err()
            || nanami_services::posix::posix_seek(
                posix_port,
                fd,
                0,
                nanami_services::posix::POSIX_SEEK_SET,
            )
            .is_err()
        {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: inherit file setup failed");
            return false;
        }

        let image = b"posix-child.elf";
        write_shm_bytes(shm, 0, image);
        let pid = match nanami_services::posix::posix_spawn(posix_port, 0, image.len() as Word) {
            Ok(pid) => pid,
            Err(e) => {
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                self.push_posix_error(b"posixtest: spawn failed ", e);
                return false;
            }
        };
        if pid == 0 {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: spawn invalid pid");
            return false;
        }
        let status = match self.wait_posix_child(posix_port, pid) {
            Some(status) => status,
            None => {
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                self.push_line_bytes(b"posixtest: child did not exit");
                return false;
            }
        };
        if status != 0 {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: inherited fd child failed");
            return false;
        }

        let cloexec_fd = match nanami_services::posix::posix_dup(posix_port, fd) {
            Ok(fd) => fd,
            Err(_) => {
                write_shm_bytes(shm, 0, path);
                let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
                self.push_line_bytes(b"posixtest: cloexec dup failed");
                return false;
            }
        };
        if nanami_services::posix::posix_fcntl_setfd(
            posix_port,
            cloexec_fd,
            nanami_services::posix::POSIX_FD_CLOEXEC,
        )
        .is_err()
        {
            let _ = nanami_services::posix::posix_close(posix_port, cloexec_fd);
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            write_shm_bytes(shm, 0, path);
            let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
            self.push_line_bytes(b"posixtest: cloexec setfd failed");
            return false;
        }
        match nanami_services::posix::posix_fcntl_getfd(posix_port, cloexec_fd) {
            Ok(flags) if (flags & nanami_services::posix::POSIX_FD_CLOEXEC) != 0 => {}
            _ => {
                let _ = nanami_services::posix::posix_close(posix_port, cloexec_fd);
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                write_shm_bytes(shm, 0, path);
                let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
                self.push_line_bytes(b"posixtest: cloexec flag mismatch");
                return false;
            }
        }
        let dup_fd = match nanami_services::posix::posix_dup(posix_port, cloexec_fd) {
            Ok(dup_fd) => dup_fd,
            Err(_) => {
                let _ = nanami_services::posix::posix_close(posix_port, cloexec_fd);
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                write_shm_bytes(shm, 0, path);
                let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
                self.push_line_bytes(b"posixtest: cloexec dup clear failed");
                return false;
            }
        };
        if nanami_services::posix::posix_fcntl_getfd(posix_port, dup_fd).ok() != Some(0) {
            let _ = nanami_services::posix::posix_close(posix_port, dup_fd);
            let _ = nanami_services::posix::posix_close(posix_port, cloexec_fd);
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            write_shm_bytes(shm, 0, path);
            let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
            self.push_line_bytes(b"posixtest: dup inherited cloexec flag");
            return false;
        }
        let _ = nanami_services::posix::posix_close(posix_port, dup_fd);
        let _ = nanami_services::posix::posix_close(posix_port, cloexec_fd);
        let _ = nanami_services::posix::posix_close(posix_port, fd);
        write_shm_bytes(shm, 0, path);
        let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
        self.push_line_bytes(b"posixtest: spawn/wait/fd inherit/fcntl ok");
        true
    }

    fn wait_posix_child(&mut self, posix_port: Word, pid: Word) -> Option<Word> {
        let _ = nanami_services::registry::connect_timer_service(SLOT_POSIX_TEST_TIMER_SERVICE);
        let timer_port = libnanami::ipc::process_slot_descriptor(SLOT_POSIX_TEST_TIMER_SERVICE);
        let mut retry = 0usize;
        while retry < 64 {
            match nanami_services::posix::posix_waitpid(
                posix_port,
                pid,
                nanami_services::posix::POSIX_WAIT_NOHANG,
            ) {
                Ok((0, _)) => {}
                Ok((waited_pid, status)) if waited_pid == pid => {
                    return Some(status);
                }
                Ok(_) => {
                    return None;
                }
                Err(_) => {
                    return None;
                }
            }
            let _ = nanami_services::timer::timer_service_sleep_blocking_server_milliseconds(
                timer_port, 10,
            );
            retry += 1;
        }
        None
    }

    fn push_posix_error(&mut self, prefix: &[u8], error: libnanami::RequestError) {
        let mut line = [0u8; COLS];
        let mut pos = append_bytes(&mut line, 0, prefix);
        match error {
            libnanami::RequestError::Status(status) => {
                pos = append_bytes(&mut line, pos, b"status=");
                let _ = append_decimal(&mut line, pos, status);
            }
            libnanami::RequestError::InvalidArgument => {
                let _ = append_bytes(&mut line, pos, b"invalid-argument");
            }
            libnanami::RequestError::Unsupported => {
                let _ = append_bytes(&mut line, pos, b"unsupported");
            }
            libnanami::RequestError::Transport => {
                let _ = append_bytes(&mut line, pos, b"transport");
            }
            libnanami::RequestError::Protocol => {
                let _ = append_bytes(&mut line, pos, b"protocol");
            }
        }
        self.push_line(line);
    }

    fn posix_dev_zero_test(&mut self, posix_port: Word, shm: Word) -> bool {
        write_shm_bytes(shm, 0, b"/dev/zero");
        let fd = match nanami_services::posix::posix_open(posix_port, 0, 9, 0) {
            Ok(fd) => fd,
            Err(_) => {
                self.push_line_bytes(b"posixtest: open /dev/zero failed");
                return false;
            }
        };
        let bytes = match nanami_services::posix::posix_read(posix_port, fd, 512, 16) {
            Ok(bytes) => bytes,
            Err(_) => {
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                self.push_line_bytes(b"posixtest: read /dev/zero failed");
                return false;
            }
        };
        let _ = nanami_services::posix::posix_close(posix_port, fd);
        if bytes != 16 || !shm_zeroes(shm, 512, 16) {
            self.push_line_bytes(b"posixtest: /dev/zero mismatch");
            return false;
        }
        self.push_line_bytes(b"posixtest: /dev/zero ok");
        true
    }

    fn posix_dir_test(&mut self, posix_port: Word, shm: Word) -> bool {
        let path = b"/posixdir";
        write_shm_bytes(shm, 0, path);
        let _ = nanami_services::posix::posix_rmdir(posix_port, 0, path.len() as Word);
        if nanami_services::posix::posix_mkdir(posix_port, 0, path.len() as Word).is_err() {
            self.push_line_bytes(b"posixtest: mkdir failed");
            return false;
        }
        if nanami_services::posix::posix_chdir(posix_port, 0, path.len() as Word).is_err() {
            self.push_line_bytes(b"posixtest: chdir failed");
            return false;
        }
        match nanami_services::posix::posix_getcwd(posix_port, 512, 64) {
            Ok(len) if len == path.len() as Word && shm_bytes_eq(shm, 512, path) => {}
            _ => {
                self.push_line_bytes(b"posixtest: getcwd mismatch");
                return false;
            }
        }

        write_shm_bytes(shm, 0, b".");
        let fd = match nanami_services::posix::posix_open(
            posix_port,
            0,
            1,
            nanami_services::posix::POSIX_O_DIRECTORY,
        ) {
            Ok(fd) => fd,
            Err(_) => {
                self.push_line_bytes(b"posixtest: open directory failed");
                return false;
            }
        };
        if nanami_services::posix::posix_read_dir(posix_port, fd, 4, 768).is_err() {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: readdir failed");
            return false;
        }
        let _ = nanami_services::posix::posix_close(posix_port, fd);

        write_shm_bytes(shm, 0, b"/");
        if nanami_services::posix::posix_chdir(posix_port, 0, 1).is_err() {
            self.push_line_bytes(b"posixtest: chdir / failed");
            return false;
        }
        write_shm_bytes(shm, 0, path);
        if nanami_services::posix::posix_rmdir(posix_port, 0, path.len() as Word).is_err() {
            self.push_line_bytes(b"posixtest: rmdir failed");
            return false;
        }
        self.push_line_bytes(b"posixtest: cwd/readdir ok");
        true
    }

    fn posix_file_test(&mut self, posix_port: Word, shm: Word) -> bool {
        let path = b"/posixtest.txt";
        let renamed = b"/posixtest-renamed.txt";
        let body = b"POSIX facade write path ok";

        write_shm_bytes(shm, 0, path);
        let _ = nanami_services::posix::posix_unlink(posix_port, 0, path.len() as Word);
        write_shm_bytes(shm, 0, renamed);
        let _ = nanami_services::posix::posix_unlink(posix_port, 0, renamed.len() as Word);

        write_shm_bytes(shm, 0, path);
        let fd = match nanami_services::posix::posix_open(
            posix_port,
            0,
            path.len() as Word,
            nanami_services::posix::POSIX_O_CREAT | nanami_services::posix::POSIX_O_TRUNC,
        ) {
            Ok(fd) => fd,
            Err(_) => {
                self.push_line_bytes(b"posixtest: create/open failed");
                return false;
            }
        };
        write_shm_bytes(shm, 512, body);
        if nanami_services::posix::posix_write(posix_port, fd, 512, body.len() as Word).is_err() {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: write failed");
            return false;
        }
        match nanami_services::posix::posix_fstat(posix_port, fd) {
            Ok((_, size, kind, _, _))
                if size == body.len() as Word
                    && kind == nanami_services::posix::POSIX_FILE_TYPE_REGULAR => {}
            _ => {
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                self.push_line_bytes(b"posixtest: fstat mismatch");
                return false;
            }
        }
        if nanami_services::posix::posix_seek(
            posix_port,
            fd,
            0,
            nanami_services::posix::POSIX_SEEK_SET,
        )
        .is_err()
        {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: seek failed");
            return false;
        }
        if nanami_services::posix::posix_read(posix_port, fd, 768, body.len() as Word).is_err() {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: read after seek failed");
            return false;
        }
        if !shm_bytes_eq(shm, 768, body) {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: seek read mismatch");
            return false;
        }
        if nanami_services::posix::posix_seek(
            posix_port,
            fd,
            0,
            nanami_services::posix::POSIX_SEEK_SET,
        )
        .is_err()
        {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: dup seek failed");
            return false;
        }
        let dup_fd = match nanami_services::posix::posix_dup(posix_port, fd) {
            Ok(dup_fd) => dup_fd,
            Err(_) => {
                let _ = nanami_services::posix::posix_close(posix_port, fd);
                self.push_line_bytes(b"posixtest: dup failed");
                return false;
            }
        };
        if nanami_services::posix::posix_dup2(posix_port, dup_fd, 7).ok() != Some(7) {
            let _ = nanami_services::posix::posix_close(posix_port, dup_fd);
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: dup2 failed");
            return false;
        }
        if nanami_services::posix::posix_read(posix_port, 7, 768, body.len() as Word).is_err()
            || !shm_bytes_eq(shm, 768, body)
        {
            let _ = nanami_services::posix::posix_close(posix_port, 7);
            let _ = nanami_services::posix::posix_close(posix_port, dup_fd);
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: dup2 read mismatch");
            return false;
        }
        let eof = nanami_services::posix::posix_read(posix_port, fd, 768, 1).unwrap_or(1);
        if eof != 0 {
            let _ = nanami_services::posix::posix_close(posix_port, 7);
            let _ = nanami_services::posix::posix_close(posix_port, dup_fd);
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: dup offset mismatch");
            return false;
        }
        let _ = nanami_services::posix::posix_close(posix_port, 7);
        let _ = nanami_services::posix::posix_close(posix_port, dup_fd);
        let _ = nanami_services::posix::posix_close(posix_port, fd);

        write_shm_bytes(shm, 0, path);
        let fd = match nanami_services::posix::posix_open(posix_port, 0, path.len() as Word, 0) {
            Ok(fd) => fd,
            Err(_) => {
                self.push_line_bytes(b"posixtest: reopen failed");
                return false;
            }
        };
        if nanami_services::posix::posix_read(posix_port, fd, 768, body.len() as Word).is_err() {
            let _ = nanami_services::posix::posix_close(posix_port, fd);
            self.push_line_bytes(b"posixtest: readback failed");
            return false;
        }
        let _ = nanami_services::posix::posix_close(posix_port, fd);
        if !shm_bytes_eq(shm, 768, body) {
            self.push_line_bytes(b"posixtest: readback mismatch");
            return false;
        }

        write_shm_bytes(shm, 0, path);
        write_shm_bytes(shm, 256, renamed);
        if nanami_services::posix::posix_rename(
            posix_port,
            0,
            path.len() as Word,
            256,
            renamed.len() as Word,
        )
        .is_err()
        {
            self.push_line_bytes(b"posixtest: rename failed");
            return false;
        }
        write_shm_bytes(shm, 0, renamed);
        if nanami_services::posix::posix_unlink(posix_port, 0, renamed.len() as Word).is_err() {
            self.push_line_bytes(b"posixtest: unlink failed");
            return false;
        }
        self.push_line_bytes(b"posixtest: file io ok");
        true
    }

    fn posix_stat_test(&mut self, posix_port: Word, shm: Word) -> bool {
        write_shm_bytes(shm, 0, b"/dev/null");
        let (_, _, kind, major, minor) = match nanami_services::posix::posix_stat(posix_port, 0, 9)
        {
            Ok(v) => v,
            Err(_) => {
                self.push_line_bytes(b"posixtest: stat /dev/null failed");
                return false;
            }
        };
        if kind != nanami_services::posix::POSIX_FILE_TYPE_CHAR_DEVICE
            || major != nanami_services::posix::POSIX_DEV_NULL_MAJOR
            || minor != nanami_services::posix::POSIX_DEV_NULL_MINOR
        {
            self.push_line_bytes(b"posixtest: device stat mismatch");
            return false;
        }
        self.push_line_bytes(b"posixtest: stat ok");
        true
    }

    fn push_prompt(&mut self) {
        self.push_line(self.prompt_line());
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
        let line = self.prompt_line();
        self.rows[row] = line;
        self.row_colors[row] = [DEFAULT_TEXT_COLOR; COLS];
        self.scroll_to_bottom();
        self.repaint_present_logical_row(row);
    }

    fn prompt_line(&self) -> [u8; COLS] {
        let mut line = [0u8; COLS];
        line[0] = b'>';
        line[1] = b' ';
        let max = self.input_len.min(COLS.saturating_sub(3));
        let input_start = self.input_len.saturating_sub(max);
        let mut i = 0usize;
        while i < max {
            line[2 + i] = self.input[input_start + i];
            i += 1;
        }
        line
    }

    fn set_modifier(&mut self, modifier: u8, pressed: bool) {
        if pressed {
            self.modifier_state |= modifier;
        } else {
            self.modifier_state &= !modifier;
        }
    }

    fn shift_active(&self) -> bool {
        self.modifier_state != 0
    }

    fn trim_input(&mut self) {
        let mut start = 0usize;
        while start < self.input_len && self.input[start] == b' ' {
            start += 1;
        }
        let mut end = self.input_len;
        while end > start && self.input[end - 1] == b' ' {
            end -= 1;
        }
        let len = end - start;
        if start != 0 && len != 0 {
            self.input.copy_within(start..end, 0);
        }
        self.input[len..self.input_len].fill(0);
        self.input_len = len;
    }

    fn on_timer(&mut self) {
        if self.foreground.is_active() {
            if let Some(output) = self.foreground.poll_status() {
                let had_prompt = self.remove_prompt_row_if_present();
                let should_restore_prompt = had_prompt || !self.foreground.is_active();
                self.push_foreground_output(output);
                if should_restore_prompt {
                    self.push_colored_line(self.prompt_line(), [DEFAULT_TEXT_COLOR; COLS]);
                }
                self.repaint_all();
                self.present_full();
            }
            return;
        }
        self.cursor_ticks = self.cursor_ticks.wrapping_add(1);
        if !self.cursor_ticks.is_multiple_of(5) {
            return;
        }
        self.cursor_visible = !self.cursor_visible;
        self.refresh_prompt_line();
    }

    fn repaint_present_logical_row(&mut self, logical_row: usize) {
        let start = self.visible_start();
        if logical_row < start {
            return;
        }
        let screen_row = logical_row - start;
        if screen_row >= ROWS {
            return;
        }
        self.repaint_row(screen_row);
        self.present_row(screen_row);
    }

    fn visible_start(&self) -> usize {
        if self.row_count <= ROWS {
            0
        } else {
            self.scroll_offset.min(self.row_count - ROWS)
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.row_count.saturating_sub(ROWS);
    }

    fn scroll_page_up(&mut self) {
        let old = self.scroll_offset;
        self.scroll_offset = self.scroll_offset.saturating_sub(ROWS.saturating_sub(1));
        if self.scroll_offset == old {
            return;
        }
        self.repaint_all();
        self.present_full();
    }

    fn scroll_page_down(&mut self) {
        let old = self.scroll_offset;
        self.scroll_offset =
            (self.scroll_offset + ROWS.saturating_sub(1)).min(self.row_count.saturating_sub(ROWS));
        if self.scroll_offset == old {
            return;
        }
        self.repaint_all();
        self.present_full();
    }

    fn scroll_lines(&mut self, delta: i16) {
        if self.row_count <= ROWS || delta == 0 {
            return;
        }
        let old = self.scroll_offset;
        if delta > 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(delta as usize);
        } else {
            self.scroll_offset =
                (self.scroll_offset + (-delta) as usize).min(self.row_count.saturating_sub(ROWS));
        }
        if self.scroll_offset == old {
            return;
        }
        self.repaint_all();
        self.present_full();
    }

    fn push_history(&mut self) {
        if self.input_len == 0 {
            self.history_cursor = self.history_count;
            return;
        }
        if self.history_count != 0 {
            let last = (self.history_count - 1) % HISTORY_MAX;
            if self.history_lens[last] == self.input_len
                && self.history[last][..self.input_len] == self.input[..self.input_len]
            {
                self.history_cursor = self.history_count;
                return;
            }
        }
        let slot = self.history_count % HISTORY_MAX;
        self.history[slot] = [0; MAX_LINE];
        self.history[slot][..self.input_len].copy_from_slice(&self.input[..self.input_len]);
        self.history_lens[slot] = self.input_len;
        self.history_count = self.history_count.saturating_add(1);
        self.history_cursor = self.history_count;
    }

    fn history_prev(&mut self) {
        let available = self.history_count.min(HISTORY_MAX);
        if available == 0 {
            return;
        }
        let oldest = self.history_count - available;
        if self.history_cursor > oldest {
            self.history_cursor -= 1;
        }
        self.load_history_cursor();
    }

    fn history_next(&mut self) {
        if self.history_cursor < self.history_count {
            self.history_cursor += 1;
        }
        if self.history_cursor == self.history_count {
            self.input_len = 0;
        } else {
            self.load_history_cursor();
        }
        self.cursor_visible = true;
        self.cursor_ticks = 0;
        self.refresh_prompt_line();
    }

    fn load_history_cursor(&mut self) {
        if self.history_cursor >= self.history_count {
            return;
        }
        let slot = self.history_cursor % HISTORY_MAX;
        let len = self.history_lens[slot];
        self.input = [0; MAX_LINE];
        self.input[..len].copy_from_slice(&self.history[slot][..len]);
        self.input_len = len;
        self.cursor_visible = true;
        self.cursor_ticks = 0;
        self.refresh_prompt_line();
    }

    fn remove_prompt_row_if_present(&mut self) -> bool {
        if self.row_count == 0 {
            return false;
        }
        let row = self.row_count - 1;
        if self.rows[row] != self.prompt_line() {
            return false;
        }
        self.row_count -= 1;
        true
    }

    fn push_line_bytes(&mut self, bytes: &[u8]) {
        let mut line = [0u8; COLS];
        copy_bytes(&mut line, bytes);
        self.push_line(line);
    }

    fn push_line(&mut self, line: [u8; COLS]) {
        self.foreground_partial_row = false;
        let _ = self.push_colored_line(line, [DEFAULT_TEXT_COLOR; COLS]);
    }

    fn push_colored_line(&mut self, line: [u8; COLS], colors: [u32; COLS]) -> bool {
        let following_bottom =
            self.row_count <= ROWS || self.scroll_offset >= self.row_count.saturating_sub(ROWS);
        let mut scrolled = false;
        if self.row_count >= MAX_ROWS {
            scrolled = true;
            let limit = self.row_count.min(MAX_ROWS);
            let mut i = 1usize;
            while i < limit {
                self.rows[i - 1] = self.rows[i];
                self.row_colors[i - 1] = self.row_colors[i];
                i += 1;
            }
            self.row_count = limit.saturating_sub(1);
            self.scroll_offset = self.scroll_offset.saturating_sub(1);
        }
        self.rows[self.row_count] = line;
        self.row_colors[self.row_count] = colors;
        self.row_count += 1;
        if following_bottom {
            self.scroll_to_bottom();
        }
        scrolled
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
        } else if kind == nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL {
            shell.scroll_lines(value0.saturating_mul(3));
        } else if kind == nanami_services::input::INPUT_EVENT_KIND_WINDOW_CLOSE {
            shell.foreground.shutdown();
            let _ = nanami_services::gfx::honoka::honoka_destroy_window(
                shell.honoka_port,
                shell.window_id,
            );
            let _ = libnanami::request_exit();
            loop {
                core::hint::spin_loop();
            }
        }
        drained += 1;
    }
}

fn start_shell_timer() {
    if nanami_services::registry::connect_timer_service(SLOT_SHELL_TIMER_SERVICE).is_err() {
        libnanami::println!("[shell] timer-service unavailable; periodic updates disabled");
        return;
    }
    let timer_port = libnanami::ipc::process_slot_descriptor(SLOT_SHELL_TIMER_SERVICE);
    if let Err(error) = nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        timer_port,
        100,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    ) {
        log_request_error("[shell] timer interval failed: ", error);
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
                libnanami::yield_now();
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
    framebuffer_size: (usize, usize),
    rect: (usize, usize, usize, usize),
    color: u32,
) {
    let (fb_width, fb_height) = framebuffer_size;
    let (x, y, width, height) = rect;
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

fn push_damage_rect(base: Word, x: usize, y: usize, width: usize, height: usize) -> bool {
    if read_word(base, 0) != nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_MAGIC {
        return false;
    }
    let capacity = read_word(base, 1) as usize;
    if capacity == 0 || capacity > nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_CAPACITY {
        return false;
    }
    let head = (read_word(base, 2) as usize) % capacity;
    let tail = (read_word(base, 3) as usize) % capacity;
    let next = (tail + 1) % capacity;
    if next == head {
        return false;
    }
    let entry = nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS
        + tail * nanami_services::gfx::honoka::HONOKA_DAMAGE_ENTRY_WORDS;
    write_word(base, entry, x as Word);
    write_word(base, entry + 1, y as Word);
    write_word(base, entry + 2, width as Word);
    write_word(base, entry + 3, height as Word);
    write_word(base, 3, next as Word);
    true
}

fn read_word(base: Word, index: usize) -> Word {
    unsafe {
        let ptr = (base + word_offset(index)) as *const AtomicUsize;
        (*ptr).load(Ordering::SeqCst) as Word
    }
}

fn write_word(base: Word, index: usize, value: Word) {
    unsafe {
        let ptr = (base + word_offset(index)) as *const AtomicUsize;
        (*ptr).store(value, Ordering::SeqCst);
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
    unsafe { core::ptr::read_unaligned((base + offset) as *const Word) }
}

fn read_shm_byte(base: Word, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u8) }
}

fn write_shm_bytes(base: Word, offset: usize, bytes: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), (base + offset) as *mut u8, bytes.len());
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

fn shm_zeroes(base: Word, offset: usize, len: usize) -> bool {
    let mut i = 0usize;
    while i < len {
        if read_shm_byte(base, offset + i) != 0 {
            return false;
        }
        i += 1;
    }
    true
}

fn is_unsupported<T>(result: Result<T, RequestError>) -> bool {
    match result {
        Err(RequestError::Unsupported) => true,
        Err(RequestError::Status(status)) if status == libnanami::OS_RESPONSE_ILLEGAL_OPERATION => {
            true
        }
        _ => false,
    }
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
    let inode = read_shm_word(
        base,
        offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_INODE_OFFSET,
    );
    let kind = read_shm_word(
        base,
        offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET,
    );
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

fn log_error(prefix: &str, err: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

libnanami::nanami_entry!(nanami_main);
