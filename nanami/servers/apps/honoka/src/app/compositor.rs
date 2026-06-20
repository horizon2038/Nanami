use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::Word;

use crate::constants::{
    MAX_WINDOWS, MENU_BAR_HEIGHT, SLOT_WINDOW_INPUT_NOTIFICATION_BASE, TITLE_BAR_HEIGHT,
};
use crate::font::TextRenderer;
use crate::framebuffer::{clamp_i32, Framebuffer, Rect, ScreenInfo};
use crate::input::InputEvent;

const MAX_DIRTY_RECTS: usize = 256;
const CLIENT_PADDING: i32 = 4;
const DRAG_OUTLINE_THICKNESS: i32 = 2;
const CLOCK_TEXT_BYTES: usize = 8;
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
    active_theme: usize,
    dirty_rects: [Rect; MAX_DIRTY_RECTS],
    dirty_count: usize,
    focused_window_id: Word,
    next_input_notification_slot: Word,
    clock_text: [u8; CLOCK_TEXT_BYTES],
    clock_len: usize,
    text: TextRenderer,
}

impl Compositor {
    pub fn new(framebuffer: Framebuffer, text: TextRenderer) -> Self {
        let screen = framebuffer.screen();
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
            active_theme: 0,
            dirty_rects: [Rect::EMPTY; MAX_DIRTY_RECTS],
            dirty_count: 0,
            focused_window_id: 0,
            next_input_notification_slot: SLOT_WINDOW_INPUT_NOTIFICATION_BASE,
            clock_text: *b"--:--:--",
            clock_len: CLOCK_TEXT_BYTES,
            text,
        };
        this.dirty_count = 0;
        this.mark_dirty(this.screen_rect());
        this
    }

    pub fn render_if_needed(&mut self) -> bool {
        if self.dirty_count == 0 {
            return false;
        }

        if self.drag_outline_visible {
            while self.dirty_count != 0 {
                self.dirty_count -= 1;
                self.render_rect(self.dirty_rects[self.dirty_count]);
            }
            return false;
        }

        let mut rect = self.dirty_rects[0];
        let mut i = 1usize;
        while i < self.dirty_count {
            rect = union_rect(rect, self.dirty_rects[i]);
            i += 1;
        }
        self.dirty_count = 0;
        self.render_rect(rect);
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
                libnanami::print!("[honoka] drag begin\n");
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
            if code == 1 {
                libnanami::print!("[honoka] drag end\n");
            }
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

        if code == 0x01 {
            if pressed {
                self.active_theme ^= 1;
                self.mark_dirty(self.screen_rect());
                libnanami::print!("[honoka] esc pressed: theme toggled\n");
            }
            return true;
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
        let theme = make_theme(&self.framebuffer, self.active_theme);
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
            if w.used && w.visible && contains_rect(w.rect(), x, y) {
                return Some(i);
            }
        }
        None
    }

    fn point_in_title(&self, index: usize, x: i32, y: i32) -> bool {
        let w = self.windows[index];
        w.used
            && w.visible
            && x >= w.x
            && x < w.x + w.width
            && y >= w.y
            && y < w.y + TITLE_BAR_HEIGHT
    }

    fn find_window_content_at(&self, x: i32, y: i32) -> Option<usize> {
        let mut i = MAX_WINDOWS;
        while i > 0 {
            i -= 1;
            let w = self.windows[i];
            if w.used && w.visible && contains_rect(w.content_rect(), x, y) {
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
        let theme = make_theme(&self.framebuffer, self.active_theme);
        let mut offset = 0usize;
        while offset + 4 <= size {
            unsafe {
                core::ptr::write_volatile((local_vaddr + offset) as *mut u32, theme.window_body);
            }
            offset += 4;
        }
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
    fill_clipped(framebuffer, dirty, 14, 8, 12, 12, theme.accent);
    fill_clipped(framebuffer, dirty, 36, 9, 86, 3, theme.title_text);
    fill_clipped(framebuffer, dirty, 36, 17, 56, 3, theme.title_text);
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
    let Some(_) = intersect_rect(window.rect(), dirty) else {
        return;
    };

    fill_clipped(
        framebuffer,
        dirty,
        window.x,
        window.y,
        window.width,
        window.height,
        theme.window_frame,
    );
    fill_clipped(
        framebuffer,
        dirty,
        window.x + 2,
        window.y + 2,
        window.width - 4,
        window.height - 4,
        theme.window_body,
    );
    fill_clipped(
        framebuffer,
        dirty,
        window.x + 2,
        window.y + 2,
        window.width - 4,
        TITLE_BAR_HEIGHT,
        theme.title_bar,
    );
    draw_rect_clipped(
        framebuffer,
        dirty,
        window.x,
        window.y,
        window.width,
        window.height,
        if active {
            theme.accent
        } else {
            theme.window_frame
        },
    );
    fill_clipped(
        framebuffer,
        dirty,
        window.x + 12,
        window.y + 10,
        8,
        8,
        theme.accent,
    );
    let title_clip = Rect::new(
        window.x + 26,
        window.y + 4,
        window.width - 48,
        TITLE_BAR_HEIGHT - 6,
    );
    if let Some(title_dirty) = intersect_rect(title_clip, dirty) {
        text.draw_title(
            framebuffer,
            title_dirty,
            window.x + 28,
            window.y + 8,
            &window.title[..window.title_len],
            theme.title_text,
            theme.title_bar,
        );
    }
    fill_clipped(
        framebuffer,
        dirty,
        window.x + window.width - 24,
        window.y + 11,
        8,
        8,
        if active {
            theme.title_text
        } else {
            theme.window_frame
        },
    );

    let content = window.content_rect();
    if window.local_fb != 0 {
        if let Some(r) = intersect_rect(content, dirty) {
            framebuffer.blit_bgra32_from(
                r.x,
                r.y,
                r.width,
                r.height,
                window.local_fb,
                window.fb_size,
                content.width.max(0) as usize,
                (r.x - content.x) as usize,
                (r.y - content.y) as usize,
            );
        }
    } else {
        fill_clipped(
            framebuffer,
            dirty,
            content.x,
            content.y,
            content.width,
            content.height,
            darken(theme.window_body),
        );
        fill_clipped(
            framebuffer,
            dirty,
            content.x + 18,
            content.y + 20,
            content.width - 36,
            3,
            theme.title_text,
        );
        fill_clipped(
            framebuffer,
            dirty,
            content.x + 18,
            content.y + 38,
            content.width - 84,
            3,
            theme.title_text,
        );
        fill_clipped(
            framebuffer,
            dirty,
            content.x + 18,
            content.y + 56,
            content.width - 140,
            3,
            theme.title_text,
        );
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

fn make_theme(framebuffer: &Framebuffer, index: usize) -> Theme {
    if index == 0 {
        return Theme {
            background_top: framebuffer.color(101, 101, 101),
            background_bottom: framebuffer.color(32, 32, 32),
            menu_bar: framebuffer.color(64, 64, 64),
            menu_edge: framebuffer.color(32, 32, 32),
            window_body: framebuffer.color(136, 136, 136),
            window_frame: framebuffer.color(48, 48, 48),
            title_bar: framebuffer.color(112, 113, 111),
            title_text: framebuffer.color(224, 224, 224),
            accent: framebuffer.color(170, 132, 82),
            cursor: framebuffer.color(250, 248, 238),
            cursor_shadow: framebuffer.color(23, 25, 24),
        };
    }

    Theme {
        background_top: framebuffer.color(28, 23, 20),
        background_bottom: framebuffer.color(63, 43, 32),
        menu_bar: framebuffer.color(30, 24, 22),
        menu_edge: framebuffer.color(130, 88, 66),
        window_body: framebuffer.color(166, 148, 126),
        window_frame: framebuffer.color(54, 36, 32),
        title_bar: framebuffer.color(104, 62, 52),
        title_text: framebuffer.color(236, 222, 202),
        accent: framebuffer.color(108, 150, 122),
        cursor: framebuffer.color(255, 244, 220),
        cursor_shadow: framebuffer.color(29, 20, 18),
    }
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
