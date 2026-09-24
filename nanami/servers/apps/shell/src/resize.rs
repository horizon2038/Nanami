use super::*;

impl Shell {
    pub(super) fn resize(&mut self, width: usize, height: usize) {
        match self
            .surface
            .resize(self.honoka_port, self.window_id, width, height)
        {
            Ok(true) => {
                self.foreground.resize(self.cols(), self.rows());
                self.scroll_to_bottom();
                if !self.foreground.is_active() {
                    self.refresh_prompt_line();
                }
                self.repaint_all();
                self.present_full();
            }
            Ok(false) => {}
            Err(error) => log_request_error("[shell] resize failed: ", error),
        }
    }

    pub(super) fn repaint_terminal(&mut self) {
        let width = self.surface.width;
        let height = self.surface.height;
        let pixels = self.surface.pixels();
        fill_rect(pixels, (width, height), (0, 0, width, height), 0x0010_1418);
        let terminal = &self.foreground.terminal;
        let cursor = terminal.cursor();
        for y in 0..terminal.rows().min(height / FONT_H) {
            for (x, cell) in terminal.view_row(y).iter().enumerate().take(width / FONT_W) {
                let mut cell = *cell;
                if terminal.cursor_visible && terminal.at_bottom() && cursor == (x, y) {
                    core::mem::swap(&mut cell.fg, &mut cell.bg);
                }
                fill_rect(
                    pixels,
                    (width, height),
                    (x * FONT_W, y * FONT_H, FONT_W, FONT_H),
                    cell.bg,
                );
                self.text.draw_cell(
                    pixels,
                    width,
                    height,
                    x * FONT_W,
                    y * FONT_H,
                    cell.byte,
                    cell.fg,
                );
                if cell.underline {
                    fill_rect(
                        pixels,
                        (width, height),
                        (x * FONT_W, (y + 1) * FONT_H - 1, FONT_W, 1),
                        cell.fg,
                    );
                }
            }
        }
    }

    pub(super) fn save_terminal_output(&mut self) {
        let rows = self.foreground.terminal.history_len() + self.foreground.terminal.rows();
        for row in 0..rows {
            let mut line = [0; COLS];
            let mut colors = [DEFAULT_TEXT_COLOR; COLS];
            let cells = self.foreground.terminal.output_row(row);
            for (column, cell) in cells.iter().enumerate().take(COLS) {
                line[column] = cell.byte;
                colors[column] = cell.fg;
            }
            if line.iter().any(|b| *b != 0 && *b != b' ') {
                self.push_colored_line(line, colors);
            }
        }
    }
}
