//! Configure events propose content dimensions. The owner's synchronous
//! acknowledgement commits geometry and a matching surface together.
use super::*;
use libnanami::RequestError;
use nanami_services::gfx::honoka::{publish_resize, HONOKA_DAMAGE_QUEUE_BYTES};

impl Compositor {
    pub(super) fn resize_edges(&self, index: usize) -> u8 {
        let w = self.windows[index];
        // Right/bottom borders; a wider lower-right grip remains usable with
        // rounded corners. Title/close-button hit areas retain their behavior.
        if self.cursor_y < w.y + TITLE_BAR_HEIGHT {
            return 0;
        }
        let right = self.cursor_x >= w.x + w.width - CLIENT_PADDING;
        let bottom = self.cursor_y >= w.y + w.height - CLIENT_PADDING;
        let corner = self.cursor_x >= w.x + w.width - 14 && self.cursor_y >= w.y + w.height - 14;
        u8::from(right || corner) | (u8::from(bottom || corner) << 1)
    }

    pub(super) fn update_resize_preview(&mut self, index: usize) {
        let w = self.windows[index];
        let screen = self.framebuffer.screen();
        if self.resizing_edges & 1 != 0 {
            self.drag_preview_width = (w.width + self.cursor_x - w.x - self.drag_origin_x)
                .clamp(80, (screen.width as i32).min(i16::MAX as i32));
        }
        if self.resizing_edges & 2 != 0 {
            self.drag_preview_height = (w.height + self.cursor_y - w.y - self.drag_origin_y).clamp(
                TITLE_BAR_HEIGHT + 36,
                (screen.height as i32).min(i16::MAX as i32),
            );
        }
    }

    pub(super) fn send_resize(&mut self, index: usize) {
        let width = (self.drag_preview_width - CLIENT_PADDING * 2) as Word;
        let height = (self.drag_preview_height - TITLE_BAR_HEIGHT - CLIENT_PADDING) as Word;
        let w = self.windows[index];
        if w.input_queue == 0 {
            if w.local_fb == 0 {
                let _ = self.resize_viewport(w.owner_pid, w.id, width, height);
            }
            return;
        }
        publish_resize(w.input_queue, width, height);
        if w.input_notify != 0 {
            let _ = libnanami::ipc::notification_notify(w.input_notify);
        }
    }

    fn validate_resize(&self, width: Word, height: Word) -> Result<(), RequestError> {
        let screen = self.framebuffer.screen();
        if width < 72
            || height < 32
            || width > (screen.width as usize).saturating_sub(CLIENT_PADDING as usize * 2)
            || height
                > (screen.height as usize)
                    .saturating_sub((TITLE_BAR_HEIGHT + CLIENT_PADDING) as usize)
            || width > i16::MAX as usize
            || height > i16::MAX as usize
        {
            return Err(RequestError::InvalidArgument);
        }
        Ok(())
    }

    fn commit_size(&mut self, index: usize, width: Word, height: Word) {
        let old = self.windows[index].rect();
        self.windows[index].width = width as i32 + CLIENT_PADDING * 2;
        self.windows[index].height = height as i32 + TITLE_BAR_HEIGHT + CLIENT_PADDING;
        self.mark_dirty(old);
        self.mark_dirty(self.windows[index].rect());
    }

    pub fn resize_surface(
        &mut self,
        owner: Word,
        id: Word,
        width: Word,
        height: Word,
    ) -> Result<(Word, Word), RequestError> {
        let index = self.find_owned_window(owner, id)?;
        self.validate_resize(width, height)?;
        let old = self.windows[index];
        if old.damage_queue == 0 {
            return Err(RequestError::InvalidArgument);
        }
        if let Some((base, bytes)) = old.retired_fb {
            libnanami::request_mapping_release(base, bytes)?;
            self.windows[index].retired_fb = None;
        }
        let pixels = width
            .checked_mul(height)
            .and_then(|v| v.checked_mul(4))
            .ok_or(RequestError::InvalidArgument)?;
        let bytes = pixels + HONOKA_DAMAGE_QUEUE_BYTES;
        let (local, peer) = libnanami::request_shared_memory(owner, bytes)?;
        self.init_damage_queue(local);
        self.clear_logical_framebuffer(local + HONOKA_DAMAGE_QUEUE_BYTES, pixels);
        let w = &mut self.windows[index];
        w.damage_queue = local;
        w.local_fb = local + HONOKA_DAMAGE_QUEUE_BYTES;
        w.fb_size = pixels;
        w.fb_width = width;
        w.fb_height = height;
        self.commit_size(index, width, height);
        let old_bytes = old.fb_size + HONOKA_DAMAGE_QUEUE_BYTES;
        if libnanami::request_mapping_release(old.damage_queue, old_bytes).is_err() {
            self.windows[index].retired_fb = Some((old.damage_queue, old_bytes));
        }
        // Each side releases its own mapping. Do not revoke a client's pages
        // while it may still be painting the preceding frame on another CPU.
        Ok((peer, bytes))
    }

    pub fn resize_viewport(
        &mut self,
        owner: Word,
        id: Word,
        width: Word,
        height: Word,
    ) -> Result<(), RequestError> {
        let index = self.find_owned_window(owner, id)?;
        self.validate_resize(width, height)?;
        // Linux fbdev mmaps retain their resolution/stride until a mode change.
        // Resize the viewport without invalidating those guest mappings.
        if self.windows[index].damage_queue != 0 {
            return Err(RequestError::InvalidArgument);
        }
        self.commit_size(index, width, height);
        Ok(())
    }
}
