use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::Word;

use crate::constants::{
    MAX_WINDOWS, MENU_BAR_HEIGHT, SLOT_WINDOW_INPUT_NOTIFICATION_BASE, TITLE_BAR_HEIGHT,
};
use crate::font::TextRenderer;
use crate::framebuffer::{clamp_i32, Framebuffer, Rect, ScreenInfo};
use crate::input::InputEvent;

const MAX_DIRTY_RECTS: usize = 256;
const MAX_INDIVIDUAL_DIRTY_RECTS: usize = 64;
const CLIENT_PADDING: i32 = 4;
const WINDOW_CORNER_RADIUS: i32 = 8;
const DRAG_OUTLINE_THICKNESS: i32 = 2;
const CLOSE_BUTTON_SIZE: i32 = 14;
const CLOSE_BUTTON_RIGHT_MARGIN: i32 = 10;
const TITLE_TEXT_Y_OFFSET: i32 = 10;
const CLOCK_TEXT_BYTES: usize = 8;
const SHELL_ICON_RECT: Rect = Rect::new(12, 4, 28, MENU_BAR_HEIGHT - 8);
const SHELL_PATH: &[u8] = b"/bin/shell";
const SHELL_PRIORITY: Word = 16;
const TITLE_TEXT_MAX: usize = nanami_services::gfx::honoka::HONOKA_WINDOW_TITLE_BYTES;
const DEFAULT_WALLPAPER_PNM: &[u8] = include_bytes!("../../assets/wallpapers/default.pnm");

#[derive(Clone, Copy)]
struct PnmImage<'a> {
    width: usize,
    height: usize,
    pixels: &'a [u8],
}

#[derive(Clone, Copy)]
struct Theme {
    background_top: u32,
    background_bottom: u32,
    menu_bar: u32,
    menu_edge: u32,
    window_body: u32,
    window_frame: u32,
    title_bar: u32,
    title_text: u32,
    accent: u32,
    cursor: u32,
    cursor_shadow: u32,
}

#[derive(Clone, Copy)]
struct Window {
    used: bool,
    owner_pid: Word,
    id: Word,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    visible: bool,
    opacity: u8,
    damage_queue: Word,
    local_fb: Word,
    fb_size: Word,
    input_queue: Word,
    input_notify: Word,
    input_notify_slot: Word,
    title: [u8; TITLE_TEXT_MAX],
    title_len: usize,
}

impl Window {
    const EMPTY: Self = Self {
        used: false,
        owner_pid: 0,
        id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        visible: false,
        opacity: nanami_services::gfx::honoka::HONOKA_WINDOW_OPACITY_OPAQUE,
        damage_queue: 0,
        local_fb: 0,
        fb_size: 0,
        input_queue: 0,
        input_notify: 0,
        input_notify_slot: 0,
        title: [0; TITLE_TEXT_MAX],
        title_len: 0,
    };

    fn rect(self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }

    fn content_rect(self) -> Rect {
        Rect::new(
            self.x + CLIENT_PADDING,
            self.y + TITLE_BAR_HEIGHT,
            self.width - CLIENT_PADDING * 2,
            self.height - TITLE_BAR_HEIGHT - CLIENT_PADDING,
        )
    }
}

pub struct Compositor {
    framebuffer: Framebuffer,
    windows: [Window; MAX_WINDOWS],
    next_window_id: Word,
    cursor_x: i32,
    cursor_y: i32,
    dragging_window: Option<usize>,
    drag_origin_x: i32,
    drag_origin_y: i32,
    drag_preview_x: i32,
    drag_preview_y: i32,
    drag_outline_visible: bool,
    theme: Theme,
    dirty_rects: [Rect; MAX_DIRTY_RECTS],
    dirty_count: usize,
    focused_window_id: Word,
    next_input_notification_slot: Word,
    clock_text: [u8; CLOCK_TEXT_BYTES],
    clock_len: usize,
    text: TextRenderer,
    exec_port: Word,
    exec_shm: Word,
    exec_shm_size: Word,
}

impl Compositor {
    pub fn new(
        framebuffer: Framebuffer,
        text: TextRenderer,
        exec_port: Word,
        exec_shm: Word,
        exec_shm_size: Word,
        theme_data: &[u8],
    ) -> Option<Self> {
        let screen = framebuffer.screen();
        let theme = parse_theme(&framebuffer, theme_data)?;
        let mut this = Self {
            framebuffer,
            windows: [Window::EMPTY; MAX_WINDOWS],
            next_window_id: 1,
            cursor_x: (screen.width / 2) as i32,
            cursor_y: (screen.height / 2) as i32,
            dragging_window: None,
            drag_origin_x: 0,
            drag_origin_y: 0,
            drag_preview_x: 0,
            drag_preview_y: 0,
            drag_outline_visible: false,
            theme,
            dirty_rects: [Rect::EMPTY; MAX_DIRTY_RECTS],
            dirty_count: 0,
            focused_window_id: 0,
            next_input_notification_slot: SLOT_WINDOW_INPUT_NOTIFICATION_BASE,
            clock_text: *b"--:--:--",
            clock_len: CLOCK_TEXT_BYTES,
            text,
            exec_port,
            exec_shm,
            exec_shm_size,
        };
        this.dirty_count = 0;
        this.mark_dirty(this.screen_rect());
        Some(this)
    }

    pub fn render_if_needed(&mut self) -> bool {
        if self.dirty_count == 0 {
            return false;
        }

        let count = self.dirty_count;
        self.dirty_count = 0;

        if count > MAX_INDIVIDUAL_DIRTY_RECTS {
            let mut rect = self.dirty_rects[0];
            let mut i = 1usize;
            while i < count {
                rect = union_rect(rect, self.dirty_rects[i]);
                i += 1;
            }
            self.render_and_present(rect);
            return false;
        }

        let mut i = 0usize;
        while i < count {
            self.render_and_present(self.dirty_rects[i]);
            i += 1;
        }

        false
    }

    pub fn process_input(&mut self, event: InputEvent) -> bool {
        match event {
            InputEvent::MouseMove { dx, dy } => self.move_cursor(dx, dy),
            InputEvent::MouseButton { code, pressed } => self.set_mouse_button(code, pressed),
            InputEvent::MouseWheel { delta } => self.scroll_front_window(delta),
            InputEvent::Key { code, pressed } => self.handle_key(code, pressed),
            InputEvent::Unknown => false,
        }
    }

    pub fn has_pending_render(&self) -> bool {
        self.dirty_count != 0
    }

    pub fn set_clock(&mut self, hour: u8, minute: u8, second: u8) {
        let mut next = [0u8; CLOCK_TEXT_BYTES];
        write_two_digits(&mut next[0..2], hour);
        next[2] = b':';
        write_two_digits(&mut next[3..5], minute);
        next[5] = b':';
        write_two_digits(&mut next[6..8], second);

        if self.clock_text != next {
            self.clock_text = next;
            self.clock_len = CLOCK_TEXT_BYTES;
            self.mark_dirty(self.clock_rect());
        }
    }

    pub fn invalidate_presented_logical_framebuffer(&mut self, window_id: Word) {
        if self.dragging_window.is_some() {
            return;
        }

        if window_id == 0 {
            self.drain_presented_logical_framebuffers();
            return;
        }

        if let Some(index) = self.find_window_by_id(window_id) {
            self.drain_window_damage(index);
        }
    }

    pub fn drain_presented_logical_framebuffers(&mut self) {
        if self.dragging_window.is_some() {
            return;
        }

        let mut i = 0usize;
        while i < MAX_WINDOWS {
            let window = self.windows[i];
            if window.used && window.visible && window.local_fb != 0 && window.damage_queue != 0 {
                self.drain_window_damage(i);
            }
            i += 1;
        }
    }

    pub fn create_window(
        &mut self,
        owner_pid: Word,
        x: i32,
        y: i32,
        content_width: i32,
        content_height: i32,
    ) -> Result<Word, libnanami::RequestError> {
        let index = self
            .find_free_window()
            .ok_or(libnanami::RequestError::Unsupported)?;
        let max_content_width =
            (self.framebuffer.screen().width as i32 - CLIENT_PADDING * 2).max(1);
        let max_content_height =
            (self.framebuffer.screen().height as i32 - TITLE_BAR_HEIGHT - CLIENT_PADDING).max(1);
        let content_width = clamp_i32(content_width, 72, max_content_width);
        let content_height = clamp_i32(content_height, 32, max_content_height);
        let width = content_width.saturating_add(CLIENT_PADDING * 2);
        let height = content_height
            .saturating_add(TITLE_BAR_HEIGHT)
            .saturating_add(CLIENT_PADDING);
        let width = clamp_i32(width, 80, self.framebuffer.screen().width as i32);
        let height = clamp_i32(
            height,
            TITLE_BAR_HEIGHT + 32,
            self.framebuffer.screen().height as i32,
        );
        let id = self.next_window_id;
        self.next_window_id = self.next_window_id.wrapping_add(1);
        let input_notify_slot = self.next_input_notification_slot;
        self.next_input_notification_slot = self.next_input_notification_slot.wrapping_add(1);
        self.windows[index] = Window {
            used: true,
            owner_pid,
            id,
            x,
            y,
            width,
            height,
            visible: true,
            opacity: nanami_services::gfx::honoka::HONOKA_WINDOW_OPACITY_OPAQUE,
            damage_queue: 0,
            local_fb: 0,
            fb_size: 0,
            input_queue: 0,
            input_notify: 0,
            input_notify_slot,
            title: make_default_title(id),
            title_len: default_title_len(id),
        };
        self.raise_window(index);
        self.focused_window_id = id;
        self.mark_dirty(self.windows[MAX_WINDOWS - 1].rect());
        Ok(id)
    }

    pub fn create_window_with_title(
        &mut self,
        owner_pid: Word,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        title0: Word,
        title1: Word,
    ) -> Result<Word, libnanami::RequestError> {
        let id = self.create_window(owner_pid, x, y, width, height)?;
        let index = self.find_owned_window(owner_pid, id)?;
        let (title, len) = decode_title_chunks(&[title0, title1], id);
        self.windows[index].title = title;
        self.windows[index].title_len = len;
        self.mark_dirty(self.windows[index].rect());
        Ok(id)
    }

    pub fn destroy_window(
        &mut self,
        owner_pid: Word,
        window_id: Word,
    ) -> Result<(), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        self.remove_window(index);
        Ok(())
    }

    pub fn reap_dead_windows(&mut self) {
        let mut checked_pids = [0; MAX_WINDOWS];
        let mut checked_alive = [true; MAX_WINDOWS];
        let mut checked_count = 0usize;
        let mut index = 0usize;

        while index < MAX_WINDOWS {
            let window = self.windows[index];
            if !window.used {
                index += 1;
                continue;
            }

            let cached = checked_pids[..checked_count]
                .iter()
                .position(|pid| *pid == window.owner_pid);
            let alive = if let Some(cached_index) = cached {
                checked_alive[cached_index]
            } else {
                let alive = libnanami::request_process_alive(window.owner_pid).unwrap_or(true);
                checked_pids[checked_count] = window.owner_pid;
                checked_alive[checked_count] = alive;
                checked_count += 1;
                alive
            };

            if !alive {
                self.remove_window(index);
            }
            index += 1;
        }
    }

    pub fn attach_logical_framebuffer(
        &mut self,
        owner_pid: Word,
        window_id: Word,
    ) -> Result<(Word, Word), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let content = self.windows[index].content_rect();
        let pixel_bytes = content
            .width
            .max(0)
            .saturating_mul(content.height.max(0))
            .saturating_mul(4) as Word;
        if pixel_bytes == 0 {
            return Err(libnanami::RequestError::InvalidArgument);
        }
        let size =
            nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES.saturating_add(pixel_bytes);
        let (local_vaddr, peer_vaddr) = libnanami::request_shared_memory(owner_pid, size)?;
        self.init_damage_queue(local_vaddr);
        self.windows[index].damage_queue = local_vaddr;
        self.windows[index].local_fb =
            local_vaddr.saturating_add(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
        self.windows[index].fb_size = pixel_bytes;
        self.clear_logical_framebuffer(self.windows[index].local_fb, pixel_bytes);
        self.mark_dirty(self.windows[index].rect());
        Ok((peer_vaddr, size))
    }

    pub fn window_content_size(
        &self,
        owner_pid: Word,
        window_id: Word,
    ) -> Result<(Word, Word), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let content = self.windows[index].content_rect();
        Ok((content.width.max(0) as Word, content.height.max(0) as Word))
    }

    pub fn move_window(
        &mut self,
        owner_pid: Word,
        window_id: Word,
        x: i32,
        y: i32,
    ) -> Result<(), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let old = self.windows[index].rect();
        self.windows[index].x = x;
        self.windows[index].y = y;
        self.mark_dirty(old);
        self.mark_dirty(self.windows[index].rect());
        Ok(())
    }

    pub fn set_window_title(
        &mut self,
        owner_pid: Word,
        window_id: Word,
        chunk0: Word,
        chunk1: Word,
        chunk2: Word,
    ) -> Result<(), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let (title, len) = decode_title_chunks(&[chunk0, chunk1, chunk2], window_id);
        self.windows[index].title = title;
        self.windows[index].title_len = len;
        self.mark_dirty(self.windows[index].rect());
        Ok(())
    }

    pub fn attach_input_queue(
        &mut self,
        owner_pid: Word,
        window_id: Word,
    ) -> Result<(Word, Word), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let size = nanami_services::input::INPUT_EVENT_QUEUE_BYTES;
        let (local_vaddr, peer_vaddr) = libnanami::request_shared_memory(owner_pid, size)?;
        nanami_services::input::InputEventQueue::new(local_vaddr).init();
        self.windows[index].input_queue = local_vaddr;
        Ok((peer_vaddr, size))
    }

    pub fn attach_input_notification(
        &mut self,
        owner_pid: Word,
        window_id: Word,
    ) -> Result<(), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let slot = self.windows[index].input_notify_slot;
        if slot == 0 {
            return Err(libnanami::RequestError::Unsupported);
        }
        libnanami::request_notification_port_copy(
            owner_pid,
            libnanami::PROCESS_SLOT_NOTIFICATION,
            slot,
            nanami_services::gfx::honoka::HONOKA_NOTIFICATION_INPUT | (window_id & 0xffff_ffff),
        )?;
        self.windows[index].input_notify = libnanami::ipc::process_slot_descriptor(slot);
        Ok(())
    }

    pub fn set_window_visible(
        &mut self,
        owner_pid: Word,
        window_id: Word,
        visible: bool,
    ) -> Result<(), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        if self.windows[index].visible != visible {
            self.windows[index].visible = visible;
            self.mark_dirty(self.windows[index].rect());
        }
        Ok(())
    }

    pub fn set_window_opacity(
        &mut self,
        owner_pid: Word,
        window_id: Word,
        opacity: Word,
    ) -> Result<(), libnanami::RequestError> {
        if opacity > u8::MAX as Word {
            return Err(libnanami::RequestError::InvalidArgument);
        }
        let index = self.find_owned_window(owner_pid, window_id)?;
        let opacity = opacity as u8;
        if self.windows[index].opacity != opacity {
            self.windows[index].opacity = opacity;
            self.mark_dirty(self.windows[index].rect());
        }
        Ok(())
    }

    pub fn invalidate_logical_framebuffer(
        &mut self,
        owner_pid: Word,
        window_id: Word,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<(), libnanami::RequestError> {
        let index = self.find_owned_window(owner_pid, window_id)?;
        let content = self.windows[index].content_rect();
        let dirty = Rect::new(content.x + x, content.y + y, width, height);
        self.mark_dirty(dirty);
        Ok(())
    }

    fn move_cursor(&mut self, dx: i32, dy: i32) -> bool {
        if dx == 0 && dy == 0 {
            return false;
        }

        let old_cursor = self.cursor_rect();
        let screen = self.framebuffer.screen();
        let max_x = if screen.width == 0 {
            0
        } else {
            (screen.width - 1) as i32
        };
        let max_y = if screen.height == 0 {
            0
        } else {
            (screen.height - 1) as i32
        };
        self.cursor_x = clamp_i32(self.cursor_x.saturating_add(dx), 0, max_x);
        self.cursor_y = clamp_i32(self.cursor_y.saturating_add(dy), 0, max_y);

        if let Some(index) = self.dragging_window {
            let new_cursor = self.cursor_rect();
            let old_preview = self.drag_preview_rect(index);
            self.drag_preview_x = self.cursor_x.saturating_sub(self.drag_origin_x);
            self.drag_preview_y = self.cursor_y.saturating_sub(self.drag_origin_y);
            let new_preview = self.drag_preview_rect(index);
            self.mark_dirty_outline(old_preview, DRAG_OUTLINE_THICKNESS + 1);
            self.mark_dirty_outline(new_preview, DRAG_OUTLINE_THICKNESS + 1);
            self.mark_dirty(old_cursor);
            self.mark_dirty(new_cursor);
        } else {
            self.mark_dirty(old_cursor);
            self.mark_dirty(self.cursor_rect());
            if let Some(index) = self.find_window_content_at(self.cursor_x, self.cursor_y) {
                self.deliver_client_mouse_position(index);
            }
        }

        true
    }

    fn set_mouse_button(&mut self, code: Word, pressed: bool) -> bool {
        let mut redraw_cursor = true;
        if pressed {
            let Some(index) = self.find_window_at(self.cursor_x, self.cursor_y) else {
                if code == 1 {
                    if self.point_in_shell_icon(self.cursor_x, self.cursor_y) {
                        match self.spawn_shell() {
                            Ok(pid) => {
                                libnanami::println!("[honoka] shell spawned pid={}", pid);
                            }
                            Err(e) => {
                                libnanami::println!("[honoka] shell spawn failed: {:?}", e);
                            }
                        }
                        self.mark_dirty(SHELL_ICON_RECT);
                        self.mark_dirty(self.cursor_rect());
                        return true;
                    }
                    if let Some(old_focus) = self.find_focused_window() {
                        let old_rect = self.windows[old_focus].rect();
                        self.focused_window_id = 0;
                        self.mark_dirty(old_rect);
                    }
                }
                return false;
            };

            if contains_rect(
                self.windows[index].content_rect(),
                self.cursor_x,
                self.cursor_y,
            ) {
                let window_id = self.windows[index].id;
                let old_focus = self.find_focused_window().map(|i| self.windows[i].rect());
                let old = self.windows[index].rect();
                self.raise_window(index);
                let raised = self.find_window_by_id(window_id).unwrap_or(MAX_WINDOWS - 1);
                self.focused_window_id = window_id;
                if let Some(rect) = old_focus {
                    self.mark_dirty(rect);
                }
                self.mark_dirty(old);
                self.mark_dirty(self.windows[raised].rect());
                self.deliver_client_mouse_position(raised);
                self.deliver_client_button(raised, code, true);
                self.mark_dirty(self.cursor_rect());
                return true;
            }

            if code != 1 || !self.point_in_title(index, self.cursor_x, self.cursor_y) {
                return false;
            }

            if self.point_in_close_button(index, self.cursor_x, self.cursor_y) {
                self.deliver_client_close(index);
                self.mark_dirty(self.windows[index].rect());
                self.mark_dirty(self.cursor_rect());
                return true;
            }

            {
                let old_focus = self.find_focused_window().map(|i| self.windows[i].rect());
                let window_id = self.windows[index].id;
                self.focused_window_id = window_id;
                self.drag_origin_x = self.cursor_x.saturating_sub(self.windows[index].x);
                self.drag_origin_y = self.cursor_y.saturating_sub(self.windows[index].y);
                self.drag_preview_x = self.windows[index].x;
                self.drag_preview_y = self.windows[index].y;
                let dirty = self.windows[index].rect();
                let drag_index = if index < MAX_WINDOWS - 1 {
                    self.raise_window(index);
                    self.mark_dirty(dirty);
                    self.mark_dirty(self.windows[MAX_WINDOWS - 1].rect());
                    MAX_WINDOWS - 1
                } else {
                    index
                };
                if let Some(rect) = old_focus {
                    self.mark_dirty(rect);
                }
                self.dragging_window = Some(drag_index);
                self.drag_outline_visible = true;
                self.mark_dirty_outline(
                    self.drag_preview_rect(drag_index),
                    DRAG_OUTLINE_THICKNESS + 1,
                );
                redraw_cursor = false;
            }
        } else {
            if let Some(index) = self.dragging_window {
                let old = self.windows[index].rect();
                let old_preview = self.drag_preview_rect(index);
                self.drag_outline_visible = false;
                self.windows[index].x = self.drag_preview_x;
                self.windows[index].y = self.drag_preview_y;
                let new = self.windows[index].rect();
                self.mark_dirty_outline(old_preview, DRAG_OUTLINE_THICKNESS + 1);
                self.mark_dirty(old);
                self.mark_dirty(new);
                self.dragging_window = None;
            } else if let Some(index) = self.find_focused_window() {
                self.deliver_client_mouse_position(index);
                self.deliver_client_button(index, code, false);
            }
            if code == 1 {}
        }
        if redraw_cursor {
            self.mark_dirty(self.cursor_rect());
        }
        true
    }

    fn scroll_front_window(&mut self, delta: i32) -> bool {
        if delta == 0 {
            return false;
        }
        if let Some(index) = self.find_window_content_at(self.cursor_x, self.cursor_y) {
            self.focused_window_id = self.windows[index].id;
            self.deliver_client_mouse_position(index);
            self.deliver_client_wheel(index, delta);
            return true;
        }
        false
    }

    fn handle_key(&mut self, code: Word, pressed: bool) -> bool {
        if self.dragging_window.is_some() {
            return false;
        }

        if let Some(index) = self.find_focused_window() {
            self.deliver_client_key(index, code, pressed);
            return true;
        }
        false
    }

    fn render_rect(&self, dirty: Rect) {
        if dirty.is_empty() {
            return;
        }
        let theme = self.theme;
        draw_background(&self.framebuffer, self.framebuffer.screen(), theme, dirty);
        draw_menu_bar(&self.framebuffer, self.framebuffer.screen(), theme, dirty);
        draw_clock(
            &self.framebuffer,
            &self.text,
            self.framebuffer.screen(),
            theme,
            dirty,
            &self.clock_text[..self.clock_len],
        );

        let mut i = 0usize;
        while i < MAX_WINDOWS {
            let window = self.windows[i];
            if window.used && window.visible && intersects(window.rect(), dirty) {
                draw_window(
                    &self.framebuffer,
                    &self.text,
                    window,
                    theme,
                    window.id == self.focused_window_id,
                    dirty,
                );
            }
            i += 1;
        }

        if self.drag_outline_visible {
            if let Some(index) = self.dragging_window {
                draw_drag_outline(
                    &self.framebuffer,
                    dirty,
                    self.drag_preview_rect(index),
                    theme.accent,
                );
            }
        }

        if intersects(self.cursor_rect(), dirty) {
            self.framebuffer.draw_cursor(
                self.cursor_x,
                self.cursor_y,
                theme.cursor,
                theme.cursor_shadow,
            );
        }
    }

    fn render_and_present(&self, dirty: Rect) {
        self.render_rect(dirty);
        if let Err(error) = self.framebuffer.present(dirty) {
            libnanami::println!("[honoka] display present failed: {}", error);
        }
    }

    fn mark_dirty(&mut self, rect: Rect) {
        let clipped = clip_to_screen(rect, self.framebuffer.screen());
        if clipped.is_empty() {
            return;
        }
        if self.dirty_count >= MAX_DIRTY_RECTS {
            self.dirty_count = 0;
            self.dirty_rects[0] = self.screen_rect();
            self.dirty_count = 1;
            return;
        }
        self.dirty_rects[self.dirty_count] = clipped;
        self.dirty_count += 1;
    }

    fn mark_dirty_outline(&mut self, rect: Rect, thickness: i32) {
        if rect.is_empty() || thickness <= 0 {
            return;
        }

        let t = thickness;
        let span_w = rect.width + t * 2;
        let span_h = rect.height + t * 2;
        let line = t * 2;
        self.mark_dirty(Rect::new(rect.x - t, rect.y - t, span_w, line));
        self.mark_dirty(Rect::new(
            rect.x - t,
            rect.y + rect.height - t,
            span_w,
            line,
        ));
        self.mark_dirty(Rect::new(rect.x - t, rect.y - t, line, span_h));
        self.mark_dirty(Rect::new(rect.x + rect.width - t, rect.y - t, line, span_h));
    }

    fn mark_dirty_coalesced(&mut self, rect: Rect) {
        if self.dirty_count != 0 {
            let mut i = 0usize;
            let mut merged = rect;
            while i < self.dirty_count {
                merged = union_rect(merged, self.dirty_rects[i]);
                i += 1;
            }
            self.dirty_count = 0;
            self.mark_dirty(merged);
            return;
        }
        self.mark_dirty(rect);
    }

    fn cursor_rect(&self) -> Rect {
        Rect::new(self.cursor_x, self.cursor_y, 18, 18).inflate(2)
    }

    fn clock_rect(&self) -> Rect {
        let screen = self.framebuffer.screen();
        Rect::new(screen.width as i32 - 98, 0, 92, MENU_BAR_HEIGHT)
    }

    fn screen_rect(&self) -> Rect {
        let screen = self.framebuffer.screen();
        Rect::new(0, 0, screen.width as i32, screen.height as i32)
    }

    fn drag_preview_rect(&self, index: usize) -> Rect {
        let window = self.windows[index];
        Rect::new(
            self.drag_preview_x,
            self.drag_preview_y,
            window.width,
            window.height,
        )
    }

    fn find_free_window(&self) -> Option<usize> {
        let mut i = 0usize;
        while i < MAX_WINDOWS {
            if !self.windows[i].used {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn find_owned_window(
        &self,
        owner_pid: Word,
        window_id: Word,
    ) -> Result<usize, libnanami::RequestError> {
        let mut i = 0usize;
        while i < MAX_WINDOWS {
            let window = self.windows[i];
            if window.used && window.id == window_id && window.owner_pid == owner_pid {
                return Ok(i);
            }
            i += 1;
        }
        Err(libnanami::RequestError::InvalidArgument)
    }

    fn find_window_at(&self, x: i32, y: i32) -> Option<usize> {
        let mut i = MAX_WINDOWS;
        while i > 0 {
            i -= 1;
            let w = self.windows[i];
            if w.used && w.visible && contains_rounded_rect(w.rect(), x, y) {
                return Some(i);
            }
        }
        None
    }

    fn point_in_title(&self, index: usize, x: i32, y: i32) -> bool {
        let w = self.windows[index];
        w.used
            && w.visible
            && contains_rounded_rect(w.rect(), x, y)
            && x >= w.x
            && x < w.x + w.width
            && y >= w.y
            && y < w.y + TITLE_BAR_HEIGHT
    }

    fn point_in_close_button(&self, index: usize, x: i32, y: i32) -> bool {
        let w = self.windows[index];
        w.used && w.visible && contains_rect(close_button_rect(w).inflate(2), x, y)
    }

    fn point_in_shell_icon(&self, x: i32, y: i32) -> bool {
        contains_rect(SHELL_ICON_RECT, x, y)
    }

    fn spawn_shell(&self) -> Result<Word, libnanami::RequestError> {
        if self.exec_shm == 0 || SHELL_PATH.len() > self.exec_shm_size as usize {
            return Err(libnanami::RequestError::InvalidArgument);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                SHELL_PATH.as_ptr(),
                self.exec_shm as *mut u8,
                SHELL_PATH.len(),
            );
        }
        nanami_services::exec::exec_spawn_path(
            self.exec_port,
            0,
            SHELL_PATH.len() as Word,
            SHELL_PRIORITY,
        )
    }

    fn find_window_content_at(&self, x: i32, y: i32) -> Option<usize> {
        let mut i = MAX_WINDOWS;
        while i > 0 {
            i -= 1;
            let w = self.windows[i];
            if w.used
                && w.visible
                && contains_rect(w.content_rect(), x, y)
                && contains_rounded_rect(w.rect(), x, y)
            {
                return Some(i);
            }
        }
        None
    }

    fn find_window_by_id(&self, window_id: Word) -> Option<usize> {
        let mut i = 0usize;
        while i < MAX_WINDOWS {
            let window = self.windows[i];
            if window.used && window.id == window_id {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn raise_window(&mut self, index: usize) {
        if index >= MAX_WINDOWS - 1 {
            return;
        }
        let selected = self.windows[index];
        let mut i = index;
        while i + 1 < MAX_WINDOWS {
            self.windows[i] = self.windows[i + 1];
            i += 1;
        }
        self.windows[MAX_WINDOWS - 1] = selected;
    }

    fn find_focused_window(&self) -> Option<usize> {
        if self.focused_window_id == 0 {
            return None;
        }
        self.find_window_by_id(self.focused_window_id)
    }

    fn deliver_client_mouse_position(&self, index: usize) {
        let window = self.windows[index];
        let content = window.content_rect();
        let local_x = clamp_i32(
            self.cursor_x - content.x,
            0,
            content.width.saturating_sub(1),
        );
        let local_y = clamp_i32(
            self.cursor_y - content.y,
            0,
            content.height.saturating_sub(1),
        );
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE,
            0,
            clamp_i16(local_x),
            clamp_i16(local_y),
            nanami_services::gfx::honoka::HONOKA_INPUT_FLAG_ABSOLUTE,
        );
        self.deliver_client_event(index, packed);
    }

    fn deliver_client_button(&self, index: usize, code: Word, pressed: bool) {
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON,
            code,
            if pressed { 1 } else { 0 },
            0,
            0,
        );
        self.deliver_client_event(index, packed);
    }

    fn deliver_client_wheel(&self, index: usize, delta: i32) {
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL,
            0,
            clamp_i16(delta),
            0,
            0,
        );
        self.deliver_client_event(index, packed);
    }

    fn deliver_client_key(&self, index: usize, code: Word, pressed: bool) {
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_KEY,
            code,
            if pressed { 1 } else { 0 },
            0,
            0,
        );
        self.deliver_client_event(index, packed);
    }

    fn deliver_client_close(&self, index: usize) {
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_WINDOW_CLOSE,
            0,
            0,
            0,
            0,
        );
        self.deliver_client_event(index, packed);
    }

    fn deliver_client_event(&self, index: usize, packed: Word) {
        let window = self.windows[index];
        if window.input_queue == 0 {
            return;
        }
        push_raw_input_event(window.input_queue, packed);
        if window.input_notify != 0 {
            let _ = libnanami::ipc::notification_notify(window.input_notify);
        }
    }

    fn clear_logical_framebuffer(&self, local_vaddr: Word, size: Word) {
        let theme = self.theme;
        let mut offset = 0usize;
        while offset + 4 <= size {
            unsafe {
                core::ptr::write_volatile((local_vaddr + offset) as *mut u32, theme.window_body);
            }
            offset += 4;
        }
    }

    fn remove_window(&mut self, index: usize) {
        let window = self.windows[index];
        if !window.used {
            return;
        }

        if self.focused_window_id == window.id {
            self.focused_window_id = 0;
        }
        if self.dragging_window == Some(index) {
            self.dragging_window = None;
            self.drag_outline_visible = false;
        }

        self.windows[index] = Window::EMPTY;
        if window.damage_queue != 0 {
            let size = nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES
                .saturating_add(window.fb_size);
            let _ = libnanami::request_mapping_release(window.damage_queue, size);
        }
        if window.input_queue != 0 {
            let _ = libnanami::request_mapping_release(
                window.input_queue,
                nanami_services::input::INPUT_EVENT_QUEUE_BYTES,
            );
        }
        self.mark_dirty(window.rect());
        self.mark_dirty(self.cursor_rect());
    }

    fn init_damage_queue(&self, base: Word) {
        write_word(
            base,
            0,
            nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_MAGIC,
        );
        write_word(
            base,
            1,
            nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_CAPACITY as Word,
        );
        write_word(base, 2, 0);
        write_word(base, 3, 0);
        write_word(base, 4, 0);
    }

    fn drain_window_damage(&mut self, index: usize) {
        let window = self.windows[index];
        if !window.visible || window.local_fb == 0 || window.damage_queue == 0 {
            return;
        }
        let content = window.content_rect();
        let mut merged = Rect::EMPTY;
        if read_word(window.damage_queue, 4) != 0 {
            let entry = nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS;
            let rect = Rect::new(
                read_word(window.damage_queue, entry) as i32,
                read_word(window.damage_queue, entry + 1) as i32,
                read_word(window.damage_queue, entry + 2) as i32,
                read_word(window.damage_queue, entry + 3) as i32,
            );
            write_word(window.damage_queue, 4, 0);
            if let Some(clipped) = intersect_rect(
                Rect::new(
                    content.x + rect.x,
                    content.y + rect.y,
                    rect.width,
                    rect.height,
                ),
                content,
            ) {
                merged = union_rect(merged, clipped);
            } else {
                merged = union_rect(merged, content);
            }
        }
        let mut drained = 0usize;
        while drained < nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_CAPACITY {
            let Some(rect) = pop_damage_rect(window.damage_queue) else {
                break;
            };
            if let Some(clipped) = intersect_rect(
                Rect::new(
                    content.x + rect.x,
                    content.y + rect.y,
                    rect.width,
                    rect.height,
                ),
                content,
            ) {
                merged = union_rect(merged, clipped);
            }
            drained += 1;
        }
        if !merged.is_empty() {
            self.mark_dirty_coalesced(merged);
        }
    }
}

fn pop_damage_rect(base: Word) -> Option<Rect> {
    if read_word(base, 0) != nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_MAGIC {
        return None;
    }
    let capacity = read_word(base, 1) as usize;
    if capacity == 0 || capacity > nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_CAPACITY {
        return None;
    }
    let head = (read_word(base, 2) as usize) % capacity;
    let tail = (read_word(base, 3) as usize) % capacity;
    if head == tail {
        return None;
    }
    let entry = nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS
        + head * nanami_services::gfx::honoka::HONOKA_DAMAGE_ENTRY_WORDS;
    let rect = Rect::new(
        read_word(base, entry) as i32,
        read_word(base, entry + 1) as i32,
        read_word(base, entry + 2) as i32,
        read_word(base, entry + 3) as i32,
    );
    write_word(base, 2, ((head + 1) % capacity) as Word);
    Some(rect)
}

fn push_raw_input_event(base: Word, packed: Word) {
    if read_word(base, 0) != nanami_services::input::INPUT_EVENT_QUEUE_MAGIC {
        return;
    }
    let capacity = read_word(base, 1) as usize;
    if capacity == 0 || capacity > nanami_services::input::INPUT_EVENT_QUEUE_CAPACITY {
        return;
    }
    let head = (read_word(base, 2) as usize) % capacity;
    let tail = (read_word(base, 3) as usize) % capacity;
    let next_tail = (tail + 1) % capacity;
    if next_tail == head {
        write_word(base, 4, read_word(base, 4).wrapping_add(1));
        return;
    }
    write_word(
        base,
        nanami_services::input::INPUT_EVENT_QUEUE_HEADER_WORDS + tail,
        packed,
    );
    write_word(base, 3, next_tail as Word);
}

fn contains_rect(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

fn contains_rounded_rect(rect: Rect, x: i32, y: i32) -> bool {
    rounded_row_span(rect, WINDOW_CORNER_RADIUS, y).is_some_and(|(x0, x1)| x >= x0 && x < x1)
}

fn rounded_row_span(rect: Rect, radius: i32, y: i32) -> Option<(i32, i32)> {
    if rect.is_empty() || y < rect.y || y >= rect.y.saturating_add(rect.height) {
        return None;
    }
    let radius = radius.max(0).min(rect.width / 2).min(rect.height / 2);
    if radius <= 1 {
        return Some((rect.x, rect.x.saturating_add(rect.width)));
    }

    let row = y - rect.y;
    let edge_distance = row.min(rect.height - 1 - row);
    if edge_distance >= radius {
        return Some((rect.x, rect.x.saturating_add(rect.width)));
    }

    let circle_radius = radius - 1;
    let dy = circle_radius - edge_distance;
    let mut inset = 0;
    while inset < radius {
        let dx = circle_radius - inset;
        if dx * dx + dy * dy <= circle_radius * circle_radius {
            break;
        }
        inset += 1;
    }
    Some((
        rect.x.saturating_add(inset),
        rect.x.saturating_add(rect.width).saturating_sub(inset),
    ))
}

fn clamp_i16(value: i32) -> i16 {
    clamp_i32(value, i16::MIN as i32, i16::MAX as i32) as i16
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

fn draw_background(framebuffer: &Framebuffer, screen: ScreenInfo, theme: Theme, dirty: Rect) {
    draw_wallpaper(framebuffer, screen, theme, dirty);
    let under_menu = Rect::new(0, 0, screen.width as i32, MENU_BAR_HEIGHT);
    if let Some(r) = intersect_rect(under_menu, dirty) {
        framebuffer.fill_rect_clip(r, theme.background_bottom);
    }
}

fn draw_wallpaper(framebuffer: &Framebuffer, screen: ScreenInfo, theme: Theme, dirty: Rect) {
    let desktop = Rect::new(
        0,
        MENU_BAR_HEIGHT,
        screen.width as i32,
        screen.height as i32 - MENU_BAR_HEIGHT,
    );
    let Some(area) = intersect_rect(desktop, dirty) else {
        return;
    };

    if let Some(image) = parse_pnm_p6(DEFAULT_WALLPAPER_PNM) {
        draw_scaled_pnm(framebuffer, desktop, area, image);
        return;
    }

    let height = (screen.height as i32 - MENU_BAR_HEIGHT).max(1);
    let mut y = area.y;
    while y < area.y + area.height {
        let t = (((y - MENU_BAR_HEIGHT) * 255) / height) as u8;
        let base = mix_color(theme.background_top, theme.background_bottom, t);
        framebuffer.fill_rect_clip(Rect::new(area.x, y, area.width, 1), base);
        y += 1;
    }

    let glow = framebuffer.color(126, 97, 61);
    draw_wallpaper_disc(
        framebuffer,
        dirty,
        screen.width as i32 - 220,
        MENU_BAR_HEIGHT + 160,
        180,
        glow,
    );

    let stripe = mix_color(theme.background_bottom, theme.accent, 80);
    let mut x = -(screen.height as i32);
    while x < screen.width as i32 {
        draw_wallpaper_stripe(
            framebuffer,
            dirty,
            x,
            MENU_BAR_HEIGHT,
            screen.height as i32,
            stripe,
        );
        x += 220;
    }
}

fn parse_pnm_p6(data: &[u8]) -> Option<PnmImage<'_>> {
    let mut index = 0usize;
    let magic = next_pnm_token(data, &mut index)?;
    if magic != b"P6" {
        return None;
    }
    let width = parse_usize_token(next_pnm_token(data, &mut index)?)?;
    let height = parse_usize_token(next_pnm_token(data, &mut index)?)?;
    let max_value = parse_usize_token(next_pnm_token(data, &mut index)?)?;
    if width == 0 || height == 0 || max_value != 255 {
        return None;
    }
    if index >= data.len() || !is_pnm_space(data[index]) {
        return None;
    }
    index += 1;
    let bytes = width.checked_mul(height)?.checked_mul(3)?;
    if index.checked_add(bytes)? > data.len() {
        return None;
    }
    Some(PnmImage {
        width,
        height,
        pixels: &data[index..index + bytes],
    })
}

fn next_pnm_token<'a>(data: &'a [u8], index: &mut usize) -> Option<&'a [u8]> {
    skip_pnm_space_and_comments(data, index);
    let start = *index;
    while *index < data.len() && !is_pnm_space(data[*index]) && data[*index] != b'#' {
        *index += 1;
    }
    if *index == start {
        return None;
    }
    Some(&data[start..*index])
}

fn skip_pnm_space_and_comments(data: &[u8], index: &mut usize) {
    loop {
        skip_pnm_space(data, index);
        if *index >= data.len() || data[*index] != b'#' {
            return;
        }
        while *index < data.len() && data[*index] != b'\n' {
            *index += 1;
        }
    }
}

fn skip_pnm_space(data: &[u8], index: &mut usize) {
    while *index < data.len() && is_pnm_space(data[*index]) {
        *index += 1;
    }
}

fn is_pnm_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t')
}

fn parse_usize_token(token: &[u8]) -> Option<usize> {
    let mut value = 0usize;
    if token.is_empty() {
        return None;
    }
    let mut i = 0usize;
    while i < token.len() {
        let digit = token[i].wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(digit as usize)?;
        i += 1;
    }
    Some(value)
}

fn draw_scaled_pnm(framebuffer: &Framebuffer, desktop: Rect, dirty: Rect, image: PnmImage<'_>) {
    let dst_w = desktop.width.max(1) as usize;
    let dst_h = desktop.height.max(1) as usize;
    let mut y = dirty.y;
    while y < dirty.y + dirty.height {
        let rel_y = y.saturating_sub(desktop.y) as usize;
        let src_y = rel_y.saturating_mul(image.height) / dst_h;
        let mut x = dirty.x;
        while x < dirty.x + dirty.width {
            let rel_x = x.saturating_sub(desktop.x) as usize;
            let src_x = rel_x.saturating_mul(image.width) / dst_w;
            let src = src_y
                .saturating_mul(image.width)
                .saturating_add(src_x)
                .saturating_mul(3);
            if src + 2 < image.pixels.len() {
                let color = framebuffer.color(
                    image.pixels[src],
                    image.pixels[src + 1],
                    image.pixels[src + 2],
                );
                framebuffer.put_pixel(x, y, color);
            }
            x += 1;
        }
        y += 1;
    }
}

fn draw_wallpaper_disc(
    framebuffer: &Framebuffer,
    dirty: Rect,
    cx: i32,
    cy: i32,
    radius: i32,
    color: u32,
) {
    let mut band = 0;
    while band < radius {
        let width = radius - band;
        let y0 = cy - band;
        let y1 = cy + band;
        fill_clipped(framebuffer, dirty, cx - width, y0, width * 2, 1, color);
        fill_clipped(framebuffer, dirty, cx - width, y1, width * 2, 1, color);
        band += 8;
    }
}

fn draw_wallpaper_stripe(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    height: i32,
    color: u32,
) {
    let mut row = 0;
    while row < height {
        fill_clipped(framebuffer, dirty, x + row, y + row, 42, 2, color);
        row += 18;
    }
}

fn draw_menu_bar(framebuffer: &Framebuffer, screen: ScreenInfo, theme: Theme, dirty: Rect) {
    let bar = Rect::new(0, 0, screen.width as i32, MENU_BAR_HEIGHT);
    let Some(r) = intersect_rect(bar, dirty) else {
        return;
    };
    framebuffer.fill_rect_clip(r, theme.menu_bar);
    fill_clipped(
        framebuffer,
        dirty,
        0,
        MENU_BAR_HEIGHT - 1,
        screen.width as i32,
        1,
        theme.menu_edge,
    );
    draw_shell_icon(framebuffer, dirty, theme);
}

fn draw_shell_icon(framebuffer: &Framebuffer, dirty: Rect, theme: Theme) {
    let Some(_) = intersect_rect(SHELL_ICON_RECT, dirty) else {
        return;
    };
    fill_clipped(
        framebuffer,
        dirty,
        SHELL_ICON_RECT.x,
        SHELL_ICON_RECT.y,
        SHELL_ICON_RECT.width,
        SHELL_ICON_RECT.height,
        theme.window_frame,
    );
    fill_clipped(
        framebuffer,
        dirty,
        SHELL_ICON_RECT.x + 5,
        SHELL_ICON_RECT.y + 6,
        7,
        2,
        theme.title_text,
    );
    fill_clipped(
        framebuffer,
        dirty,
        SHELL_ICON_RECT.x + 10,
        SHELL_ICON_RECT.y + 9,
        7,
        2,
        theme.title_text,
    );
    fill_clipped(
        framebuffer,
        dirty,
        SHELL_ICON_RECT.x + 5,
        SHELL_ICON_RECT.y + 14,
        16,
        2,
        theme.accent,
    );
}

fn draw_clock(
    framebuffer: &Framebuffer,
    text: &TextRenderer,
    screen: ScreenInfo,
    theme: Theme,
    dirty: Rect,
    clock_text: &[u8],
) {
    let rect = Rect::new(screen.width as i32 - 98, 0, 92, MENU_BAR_HEIGHT);
    let Some(clock_dirty) = intersect_rect(rect, dirty) else {
        return;
    };
    framebuffer.fill_rect_clip(clock_dirty, theme.menu_bar);
    text.draw_title(
        framebuffer,
        clock_dirty,
        rect.x + 8,
        8,
        clock_text,
        theme.title_text,
        theme.menu_bar,
        u8::MAX,
    );
}

fn write_two_digits(dst: &mut [u8], value: u8) {
    let v = value.min(99);
    dst[0] = b'0' + (v / 10);
    dst[1] = b'0' + (v % 10);
}

fn draw_window(
    framebuffer: &Framebuffer,
    text: &TextRenderer,
    window: Window,
    theme: Theme,
    active: bool,
    dirty: Rect,
) {
    let Some(area) = intersect_rect(window.rect(), dirty) else {
        return;
    };
    draw_window_surface(framebuffer, window, theme, active, area);
    let title_clip = Rect::new(
        window.x + 10,
        window.y + 6,
        window.width - 44,
        TITLE_BAR_HEIGHT - 8,
    );
    if let Some(title_dirty) = intersect_rect(title_clip, dirty) {
        text.draw_title(
            framebuffer,
            title_dirty,
            window.x + 12,
            window.y + TITLE_TEXT_Y_OFFSET,
            &window.title[..window.title_len],
            theme.title_text,
            theme.title_bar,
            window.opacity,
        );
    }
    draw_close_button(framebuffer, dirty, window, theme.title_text);

    let content = window.content_rect();
    if window.local_fb != 0 {
        draw_window_content(framebuffer, window, dirty);
    } else {
        fill_clipped_alpha(
            framebuffer,
            dirty,
            content.x + 18,
            content.y + 20,
            content.width - 36,
            3,
            theme.title_text,
            window.opacity,
        );
        fill_clipped_alpha(
            framebuffer,
            dirty,
            content.x + 18,
            content.y + 38,
            content.width - 84,
            3,
            theme.title_text,
            window.opacity,
        );
        fill_clipped_alpha(
            framebuffer,
            dirty,
            content.x + 18,
            content.y + 56,
            content.width - 140,
            3,
            theme.title_text,
            window.opacity,
        );
    }
}

fn close_button_rect(window: Window) -> Rect {
    Rect::new(
        window.x + window.width - CLOSE_BUTTON_RIGHT_MARGIN - CLOSE_BUTTON_SIZE,
        window.y + (TITLE_BAR_HEIGHT - CLOSE_BUTTON_SIZE) / 2,
        CLOSE_BUTTON_SIZE,
        CLOSE_BUTTON_SIZE,
    )
}

fn draw_close_button(framebuffer: &Framebuffer, dirty: Rect, window: Window, color: u32) {
    let rect = close_button_rect(window);
    let arm = rect.width.saturating_sub(6);
    let mut offset = 0i32;
    while offset < arm {
        fill_clipped_alpha(
            framebuffer,
            dirty,
            rect.x + 3 + offset,
            rect.y + 3 + offset,
            2,
            2,
            color,
            window.opacity,
        );
        fill_clipped_alpha(
            framebuffer,
            dirty,
            rect.x + rect.width - 5 - offset,
            rect.y + 3 + offset,
            2,
            2,
            color,
            window.opacity,
        );
        offset += 1;
    }
}

fn draw_window_surface(
    framebuffer: &Framebuffer,
    window: Window,
    theme: Theme,
    active: bool,
    dirty: Rect,
) {
    let outer = window.rect();
    let inner = Rect::new(outer.x + 2, outer.y + 2, outer.width - 4, outer.height - 4);
    let content = window.content_rect();
    let border = if active {
        theme.accent
    } else {
        theme.window_frame
    };
    let mut y = dirty.y;
    while y < dirty.y.saturating_add(dirty.height) {
        let Some((outer_x0, outer_x1)) = rounded_row_span(outer, WINDOW_CORNER_RADIUS, y) else {
            y += 1;
            continue;
        };
        let Some((inner_x0, inner_x1)) =
            rounded_row_span(inner, WINDOW_CORNER_RADIUS.saturating_sub(2), y)
        else {
            fill_window_span(
                framebuffer,
                dirty,
                y,
                outer_x0,
                outer_x1,
                border,
                window.opacity,
            );
            y += 1;
            continue;
        };

        fill_window_span(
            framebuffer,
            dirty,
            y,
            outer_x0,
            inner_x0,
            border,
            window.opacity,
        );
        fill_window_span(
            framebuffer,
            dirty,
            y,
            inner_x1,
            outer_x1,
            border,
            window.opacity,
        );
        if y >= content.y && y < content.y.saturating_add(content.height) {
            fill_window_span(
                framebuffer,
                dirty,
                y,
                inner_x0,
                content.x,
                theme.window_body,
                window.opacity,
            );
            if window.local_fb == 0 {
                fill_window_span(
                    framebuffer,
                    dirty,
                    y,
                    content.x,
                    content.x.saturating_add(content.width),
                    darken(theme.window_body),
                    window.opacity,
                );
            }
            fill_window_span(
                framebuffer,
                dirty,
                y,
                content.x.saturating_add(content.width),
                inner_x1,
                theme.window_body,
                window.opacity,
            );
        } else {
            let color = if y < content.y {
                theme.title_bar
            } else {
                theme.window_body
            };
            fill_window_span(
                framebuffer,
                dirty,
                y,
                inner_x0,
                inner_x1,
                color,
                window.opacity,
            );
        }
        y += 1;
    }
}

fn draw_window_content(framebuffer: &Framebuffer, window: Window, dirty: Rect) {
    let content = window.content_rect();
    let Some(area) = intersect_rect(content, dirty) else {
        return;
    };
    let mut y = area.y;
    while y < area.y.saturating_add(area.height) {
        if let Some((rounded_x0, rounded_x1)) =
            rounded_row_span(window.rect(), WINDOW_CORNER_RADIUS, y)
        {
            let x0 = area.x.max(rounded_x0);
            let x1 = area.x.saturating_add(area.width).min(rounded_x1);
            if x0 < x1 {
                framebuffer.blit_bgra32_from_alpha(
                    x0,
                    y,
                    x1 - x0,
                    1,
                    window.local_fb,
                    window.fb_size,
                    content.width.max(0) as usize,
                    (x0 - content.x) as usize,
                    (y - content.y) as usize,
                    window.opacity,
                );
            }
        }
        y += 1;
    }
}

fn fill_window_span(
    framebuffer: &Framebuffer,
    dirty: Rect,
    y: i32,
    x0: i32,
    x1: i32,
    color: u32,
    opacity: u8,
) {
    let x0 = x0.max(dirty.x);
    let x1 = x1.min(dirty.x.saturating_add(dirty.width));
    if y >= dirty.y && y < dirty.y.saturating_add(dirty.height) && x0 < x1 {
        framebuffer.fill_rect_alpha(x0, y, x1 - x0, 1, color, opacity);
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_clipped_alpha(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
    opacity: u8,
) {
    if let Some(r) = intersect_rect(Rect::new(x, y, width, height), dirty) {
        framebuffer.fill_rect_alpha(r.x, r.y, r.width, r.height, color, opacity);
    }
}

fn fill_clipped(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
) {
    if let Some(r) = intersect_rect(Rect::new(x, y, width, height), dirty) {
        framebuffer.fill_rect_clip(r, color);
    }
}

fn draw_rect_clipped(
    framebuffer: &Framebuffer,
    dirty: Rect,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    color: u32,
) {
    fill_clipped(framebuffer, dirty, x, y, width, 1, color);
    fill_clipped(framebuffer, dirty, x, y + height - 1, width, 1, color);
    fill_clipped(framebuffer, dirty, x, y, 1, height, color);
    fill_clipped(framebuffer, dirty, x + width - 1, y, 1, height, color);
}

fn draw_drag_outline(framebuffer: &Framebuffer, dirty: Rect, rect: Rect, color: u32) {
    let mut i = 0i32;
    while i < DRAG_OUTLINE_THICKNESS {
        draw_rect_clipped(
            framebuffer,
            dirty,
            rect.x - i,
            rect.y - i,
            rect.width + i * 2,
            rect.height + i * 2,
            color,
        );
        i += 1;
    }
}

fn make_default_title(_window_id: Word) -> [u8; TITLE_TEXT_MAX] {
    let mut title = [0u8; TITLE_TEXT_MAX];
    let text = b"Window";
    let mut i = 0usize;
    while i < text.len() {
        title[i] = text[i];
        i += 1;
    }
    title
}

fn default_title_len(_window_id: Word) -> usize {
    6
}

fn decode_title_chunks(chunks: &[Word], window_id: Word) -> ([u8; TITLE_TEXT_MAX], usize) {
    let mut title = [0u8; TITLE_TEXT_MAX];
    let mut len = 0usize;
    let max = chunks.len().saturating_mul(8).min(TITLE_TEXT_MAX);
    let mut i = 0usize;
    while i < max {
        let byte = ((chunks[i / 8] >> ((i % 8) * 8)) & 0xff) as u8;
        if byte == 0 {
            break;
        }
        title[i] = sanitize_title_byte(byte);
        len += 1;
        i += 1;
    }
    if len == 0 {
        (make_default_title(window_id), default_title_len(window_id))
    } else {
        (title, len)
    }
}

fn sanitize_title_byte(byte: u8) -> u8 {
    if byte.is_ascii_graphic() || byte == b' ' {
        byte
    } else {
        b'?'
    }
}

fn parse_theme(framebuffer: &Framebuffer, data: &[u8]) -> Option<Theme> {
    const FIELD_COUNT: usize = 11;
    let mut values = [None; FIELD_COUNT];
    let mut start = 0usize;
    while start < data.len() {
        let mut end = start;
        while end < data.len() && data[end] != b'\n' {
            end += 1;
        }
        let line = trim_ascii(&data[start..end]);
        if !line.is_empty() && line[0] != b';' {
            let separator = line.iter().position(|byte| *byte == b'=')?;
            let key = trim_ascii(&line[..separator]);
            let value = trim_ascii(&line[separator + 1..]);
            let index = theme_field_index(key)?;
            values[index] = Some(parse_hex_color(value)?);
        }
        start = end.saturating_add(1);
    }

    Some(Theme {
        background_top: framebuffer_theme_color(framebuffer, values[0]?),
        background_bottom: framebuffer_theme_color(framebuffer, values[1]?),
        menu_bar: framebuffer_theme_color(framebuffer, values[2]?),
        menu_edge: framebuffer_theme_color(framebuffer, values[3]?),
        window_body: framebuffer_theme_color(framebuffer, values[4]?),
        window_frame: framebuffer_theme_color(framebuffer, values[5]?),
        title_bar: framebuffer_theme_color(framebuffer, values[6]?),
        title_text: framebuffer_theme_color(framebuffer, values[7]?),
        accent: framebuffer_theme_color(framebuffer, values[8]?),
        cursor: framebuffer_theme_color(framebuffer, values[9]?),
        cursor_shadow: framebuffer_theme_color(framebuffer, values[10]?),
    })
}

fn theme_field_index(key: &[u8]) -> Option<usize> {
    match key {
        b"background_top" => Some(0),
        b"background_bottom" => Some(1),
        b"menu_bar" => Some(2),
        b"menu_edge" => Some(3),
        b"window_body" => Some(4),
        b"window_frame" => Some(5),
        b"title_bar" => Some(6),
        b"title_text" => Some(7),
        b"accent" => Some(8),
        b"cursor" => Some(9),
        b"cursor_shadow" => Some(10),
        _ => None,
    }
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn parse_hex_color(value: &[u8]) -> Option<u32> {
    if value.len() != 7 || value[0] != b'#' {
        return None;
    }
    let mut color = 0u32;
    let mut i = 1usize;
    while i < value.len() {
        color = (color << 4) | hex_digit(value[i])? as u32;
        i += 1;
    }
    Some(color)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn framebuffer_theme_color(framebuffer: &Framebuffer, color: u32) -> u32 {
    framebuffer.color(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
    )
}

fn clip_to_screen(rect: Rect, screen: ScreenInfo) -> Rect {
    let x0 = clamp_i32(rect.x, 0, screen.width as i32);
    let y0 = clamp_i32(rect.y, 0, screen.height as i32);
    let x1 = clamp_i32(rect.x.saturating_add(rect.width), 0, screen.width as i32);
    let y1 = clamp_i32(rect.y.saturating_add(rect.height), 0, screen.height as i32);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn intersects(a: Rect, b: Rect) -> bool {
    !intersect_rect(a, b).unwrap_or(Rect::EMPTY).is_empty()
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.x.saturating_add(a.width).min(b.x.saturating_add(b.width));
    let y1 =
        a.y.saturating_add(a.height)
            .min(b.y.saturating_add(b.height));
    let r = Rect::new(x0, y0, x1 - x0, y1 - y0);
    if r.is_empty() {
        None
    } else {
        Some(r)
    }
}

fn union_rect(a: Rect, b: Rect) -> Rect {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = a.x.saturating_add(a.width).max(b.x.saturating_add(b.width));
    let y1 =
        a.y.saturating_add(a.height)
            .max(b.y.saturating_add(b.height));
    Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

fn darken(color: u32) -> u32 {
    (color & 0xfefefe) >> 1
}

fn mix_color(a: u32, b: u32, amount: u8) -> u32 {
    let ia = 255u32.saturating_sub(amount as u32);
    let ib = amount as u32;
    let rb = (((a & 0x00ff00ff) * ia + (b & 0x00ff00ff) * ib) / 255) & 0x00ff00ff;
    let g = (((a & 0x0000ff00) * ia + (b & 0x0000ff00) * ib) / 255) & 0x0000ff00;
    rb | g
}
