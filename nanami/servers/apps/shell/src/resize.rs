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
        terminal_render::paint(
            &self.foreground.terminal,
            &mut self.text,
            self.surface.pixels(),
            self.surface.width,
            self.surface.height,
            0,
            self.surface.height,
        );
        for row in 0..self.foreground.terminal.rows() {
            self.foreground.terminal.take_damage(row);
        }
    }

    pub(super) fn repaint_output(&mut self) {
        if !self.foreground.is_active() {
            self.repaint_all();
            self.present_full();
            return;
        }
        let rows = self.foreground.terminal.rows();
        let mut row = 0;
        while row < rows {
            if self.foreground.terminal.take_damage(row).is_none() {
                row += 1;
                continue;
            }
            let first = row;
            row += 1;
            while row < rows && self.foreground.terminal.take_damage(row).is_some() {
                row += 1;
            }
            let (top, bottom) =
                terminal_render::damage_band(&self.text, first, row, self.surface.height);
            if top == bottom {
                continue;
            }
            terminal_render::paint(
                &self.foreground.terminal,
                &mut self.text,
                self.surface.pixels(),
                self.surface.width,
                self.surface.height,
                top,
                bottom,
            );
            self.present_band(top, bottom - top);
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
