use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use vte::{Params, Parser, Perform};

use crate::kitty::KittyState;

const MAX_SCROLLBACK: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub cursor_visible: bool,
    pub bracketed_paste: bool,
    pub cells: Vec<Cell>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            // TokyoNight Moon foreground (#c8d3f5)
            fg: Rgb {
                r: 0xc8,
                g: 0xd3,
                b: 0xf5,
            },
            bg: None,
            bold: false,
        }
    }
}

/// TokyoNight Moon 準拠の ANSI 16色パレット
fn ansi_color(idx: u8) -> Rgb {
    match idx {
        0 => Rgb {
            r: 0x1b,
            g: 0x1d,
            b: 0x2b,
        }, // Black
        1 => Rgb {
            r: 0xff,
            g: 0x75,
            b: 0x7f,
        }, // Red
        2 => Rgb {
            r: 0xc3,
            g: 0xe8,
            b: 0x8d,
        }, // Green
        3 => Rgb {
            r: 0xff,
            g: 0xc7,
            b: 0x77,
        }, // Yellow
        4 => Rgb {
            r: 0x82,
            g: 0xaa,
            b: 0xff,
        }, // Blue
        5 => Rgb {
            r: 0xc0,
            g: 0x99,
            b: 0xff,
        }, // Magenta
        6 => Rgb {
            r: 0x86,
            g: 0xe1,
            b: 0xfc,
        }, // Cyan
        7 => Rgb {
            r: 0x82,
            g: 0x8b,
            b: 0xb8,
        }, // White
        8 => Rgb {
            r: 0x44,
            g: 0x4a,
            b: 0x73,
        }, // Bright Black
        9 => Rgb {
            r: 0xff,
            g: 0x8d,
            b: 0x94,
        }, // Bright Red
        10 => Rgb {
            r: 0xc7,
            g: 0xfb,
            b: 0x6d,
        }, // Bright Green
        11 => Rgb {
            r: 0xff,
            g: 0xd8,
            b: 0xab,
        }, // Bright Yellow
        12 => Rgb {
            r: 0x9a,
            g: 0xb8,
            b: 0xff,
        }, // Bright Blue
        13 => Rgb {
            r: 0xca,
            g: 0xab,
            b: 0xff,
        }, // Bright Magenta
        14 => Rgb {
            r: 0xb2,
            g: 0xeb,
            b: 0xff,
        }, // Bright Cyan
        _ => Rgb {
            r: 0xc8,
            g: 0xd3,
            b: 0xf5,
        }, // Bright White
    }
}

fn color_256(idx: u8) -> Rgb {
    match idx {
        0..=15 => ansi_color(idx),
        16..=231 => {
            let n = idx - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let to_u8 = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb {
                r: to_u8(r),
                g: to_u8(g),
                b: to_u8(b),
            }
        }
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            Rgb { r: v, g: v, b: v }
        }
    }
}

/// 副作用ゼロの端末グリッド状態マシン。
/// VT バイト列を `feed_bytes` で食わせると、セル配列・スクロールバック・カーソル位置を更新する。
pub struct TerminalGrid {
    pub cols: usize,
    pub rows: usize,
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub cursor_visible: bool,
    pub bracketed_paste: bool,
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub scroll_offset: usize,
    pub pending_replies: Vec<Vec<u8>>,
    pub kitty: KittyState,
    current_fg: Rgb,
    current_bg: Option<Rgb>,
    current_bold: bool,
    current_reverse: bool,
    pending_wrap: bool,
    scroll_top: usize,
    scroll_bottom: usize,
    saved_cursor: (usize, usize),
    alt_screen: Option<(Vec<Cell>, usize, usize)>,
    parser: Parser,
}

impl TerminalGrid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            cursor_visible: true,
            bracketed_paste: false,
            cells: vec![Cell::default(); cols * rows],
            scrollback: VecDeque::new(),
            scroll_offset: 0,
            pending_replies: Vec::new(),
            kitty: KittyState::new(),
            current_fg: Cell::default().fg,
            current_bg: None,
            current_bold: false,
            current_reverse: false,
            pending_wrap: false,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            saved_cursor: (0, 0),
            alt_screen: None,
            parser: Parser::new(),
        }
    }

    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let mut replies = Vec::new();
        let normal_bytes =
            self.kitty
                .filter_and_process(bytes, self.cursor_col, self.cursor_row, &mut replies);
        const MAX_PENDING_REPLIES: usize = 256;
        if !replies.is_empty() {
            self.pending_replies.extend(replies);
            if self.pending_replies.len() > MAX_PENDING_REPLIES {
                let excess = self.pending_replies.len() - MAX_PENDING_REPLIES;
                self.pending_replies.drain(..excess);
            }
        }

        let mut parser = std::mem::replace(&mut self.parser, Parser::new());
        parser.advance(self, &normal_bytes);
        self.parser = parser;
    }

    pub fn take_replies(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending_replies)
    }

    pub fn snapshot(&self) -> GridSnapshot {
        let mut cells = Vec::with_capacity(self.cols * self.rows);
        for row in 0..self.rows {
            for col in 0..self.cols {
                cells.push(self.visible_cell(col, row).copied().unwrap_or_default());
            }
        }
        GridSnapshot {
            cols: self.cols,
            rows: self.rows,
            cursor_col: self.cursor_col,
            cursor_row: self.cursor_row,
            cursor_visible: self.cursor_visible,
            bracketed_paste: self.bracketed_paste,
            cells,
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = (self.scroll_offset + lines).min(self.scrollback.len());
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    /// 現在のスクロール位置を考慮した表示セルを返す。
    pub fn visible_cell(&self, col: usize, row: usize) -> Option<&Cell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        if self.scroll_offset == 0 {
            return self.cell(col, row);
        }
        let sb_len = self.scrollback.len();
        let top_sb_idx = sb_len.saturating_sub(self.scroll_offset);
        let abs_row = top_sb_idx + row;
        if abs_row < sb_len {
            self.scrollback.get(abs_row).and_then(|r| r.get(col))
        } else {
            self.cell(col, abs_row - sb_len)
        }
    }

    pub fn cell(&self, col: usize, row: usize) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            Some(&self.cells[row * self.cols + col])
        } else {
            None
        }
    }

    /// 選択範囲 `(start_col, start_row)..=(end_col, end_row)` の文字列を抽出する。
    pub fn extract_text(&self, a: (usize, usize), b: (usize, usize)) -> String {
        let ((sc, sr), (ec, er)) = if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        };
        let mut lines = Vec::new();
        for row in sr..=er.min(self.rows.saturating_sub(1)) {
            let c_start = if row == sr { sc } else { 0 };
            let c_end = if row == er {
                ec.min(self.cols.saturating_sub(1))
            } else {
                self.cols.saturating_sub(1)
            };
            let mut line = String::new();
            for col in c_start..=c_end {
                if let Some(cell) = self.visible_cell(col, row)
                    && cell.ch != '\0'
                {
                    line.push(cell.ch);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        const MAX_GRID_DIM: usize = 1000;
        let new_cols = new_cols.clamp(1, MAX_GRID_DIM);
        let new_rows = new_rows.clamp(1, MAX_GRID_DIM);
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }

        let Some(total) = new_cols.checked_mul(new_rows) else {
            return;
        };

        if self.alt_screen.is_some() {
            let mut new_cells = vec![Cell::default(); total];
            let copy_cols = self.cols.min(new_cols);
            let copy_rows = self.rows.min(new_rows);
            for r in 0..copy_rows {
                for c in 0..copy_cols {
                    new_cells[r * new_cols + c] = self.cells[r * self.cols + c];
                }
            }
            self.cols = new_cols;
            self.rows = new_rows;
            self.cells = new_cells;
            self.cursor_col = self.cursor_col.min(new_cols - 1);
            self.cursor_row = self.cursor_row.min(new_rows - 1);
            self.scroll_top = 0;
            self.scroll_bottom = new_rows - 1;
            self.pending_wrap = false;
            return;
        }

        let default_cell = Cell::default();
        let mut last_active_row = self.cursor_row.min(self.rows.saturating_sub(1));
        for r in (0..self.rows).rev() {
            let row_slice = &self.cells[r * self.cols..(r + 1) * self.cols];
            if row_slice.iter().any(|c| *c != default_cell) {
                last_active_row = last_active_row.max(r);
                break;
            }
        }

        let mut reflowed: Vec<Vec<Cell>> = Vec::with_capacity(last_active_row + 1);
        let mut new_cursor_row = 0usize;
        let mut new_cursor_col = self.cursor_col.min(new_cols - 1);

        for r in 0..=last_active_row {
            let row_slice = &self.cells[r * self.cols..(r + 1) * self.cols];
            let mut content_len = row_slice
                .iter()
                .rposition(|c| *c != default_cell)
                .map(|idx| idx + 1)
                .unwrap_or(0);
            let is_cursor_row = r == self.cursor_row;
            if is_cursor_row {
                content_len = content_len.max(self.cursor_col.min(self.cols));
            }

            if content_len == 0 {
                if is_cursor_row {
                    new_cursor_row = reflowed.len();
                    new_cursor_col = 0;
                }
                reflowed.push(vec![default_cell; new_cols]);
                continue;
            }

            let mut cur_line = vec![default_cell; new_cols];
            let mut dst_col = 0usize;
            let mut src_col = 0usize;
            let mut cursor_placed = false;

            while src_col < content_len {
                let is_wide_start = src_col + 1 < self.cols
                    && row_slice[src_col + 1].ch == '\0'
                    && row_slice[src_col].ch != '\0';
                let cell_width = if is_wide_start { 2 } else { 1 };

                if dst_col + cell_width > new_cols && dst_col > 0 {
                    reflowed.push(cur_line);
                    cur_line = vec![default_cell; new_cols];
                    dst_col = 0;
                }

                if is_cursor_row && !cursor_placed && src_col == self.cursor_col {
                    new_cursor_row = reflowed.len();
                    new_cursor_col = dst_col.min(new_cols - 1);
                    cursor_placed = true;
                }

                cur_line[dst_col] = row_slice[src_col];
                if is_wide_start && dst_col + 1 < new_cols {
                    if is_cursor_row && !cursor_placed && src_col + 1 == self.cursor_col {
                        new_cursor_row = reflowed.len();
                        new_cursor_col = (dst_col + 1).min(new_cols - 1);
                        cursor_placed = true;
                    }
                    cur_line[dst_col + 1] = row_slice[src_col + 1];
                    dst_col += 2;
                    src_col += 2;
                } else {
                    dst_col += 1;
                    src_col += 1;
                }
            }

            if is_cursor_row && !cursor_placed {
                if dst_col >= new_cols {
                    reflowed.push(cur_line);
                    cur_line = vec![default_cell; new_cols];
                    dst_col = 0;
                }
                new_cursor_row = reflowed.len();
                new_cursor_col = dst_col.min(new_cols - 1);
            }

            reflowed.push(cur_line);
        }

        if reflowed.len() > new_rows {
            let overflow = reflowed.len() - new_rows;
            for line in reflowed.drain(..overflow) {
                if self.scrollback.len() >= MAX_SCROLLBACK {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(line);
            }
            new_cursor_row = new_cursor_row.saturating_sub(overflow);
        }

        let mut new_cells = vec![default_cell; total];
        for (r, line) in reflowed.into_iter().take(new_rows).enumerate() {
            let start = r * new_cols;
            new_cells[start..start + new_cols].copy_from_slice(&line);
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.cells = new_cells;
        self.cursor_col = new_cursor_col.min(new_cols - 1);
        self.cursor_row = new_cursor_row.min(new_rows - 1);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
        self.pending_wrap = false;
    }

    fn clear_wide_overlap(&mut self, row: usize, col: usize) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let idx = row * self.cols + col;
        if self.cells[idx].ch == '\0' && col > 0 {
            self.cells[idx - 1].ch = ' ';
            self.cells[idx].ch = ' ';
        } else if col + 1 < self.cols && self.cells[idx + 1].ch == '\0' {
            self.cells[idx + 1].ch = ' ';
        }
    }

    fn scroll_up_region(&mut self, top: usize, bottom: usize, count: usize) {
        let top = top.min(self.rows.saturating_sub(1));
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        let region_rows = bottom - top + 1;
        let n = count.clamp(1, region_rows);

        if top == 0 && bottom + 1 == self.rows && self.alt_screen.is_none() {
            for r in 0..n {
                let row_slice = self.cells[r * self.cols..(r + 1) * self.cols].to_vec();
                if self.scrollback.len() >= MAX_SCROLLBACK {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(row_slice);
                if self.scroll_offset > 0 {
                    self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
                }
            }
        }

        let start = top * self.cols;
        let end = (bottom + 1) * self.cols;
        let shift = n * self.cols;
        self.cells[start..end].rotate_left(shift);
        for c in &mut self.cells[end - shift..end] {
            *c = Cell::default();
        }
    }

    fn scroll_down_region(&mut self, top: usize, bottom: usize, count: usize) {
        let top = top.min(self.rows.saturating_sub(1));
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        let region_rows = bottom - top + 1;
        let n = count.clamp(1, region_rows);
        let start = top * self.cols;
        let end = (bottom + 1) * self.cols;
        let shift = n * self.cols;
        self.cells[start..end].rotate_right(shift);
        for c in &mut self.cells[start..start + shift] {
            *c = Cell::default();
        }
    }

    fn newline(&mut self) {
        let bottom = self.scroll_bottom.min(self.rows.saturating_sub(1));
        if self.cursor_row == bottom {
            self.scroll_up_region(self.scroll_top, bottom, 1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.pending_wrap = false;
        let top = self.scroll_top.min(self.rows.saturating_sub(1));
        let bottom = self.scroll_bottom.min(self.rows.saturating_sub(1));
        if self.cursor_row == top {
            self.scroll_down_region(top, bottom, 1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    fn clear_all(&mut self) {
        for c in &mut self.cells {
            *c = Cell::default();
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        let list: Vec<u16> = params
            .iter()
            .map(|p| p.first().copied().unwrap_or(0))
            .collect();
        if list.is_empty() {
            self.reset_sgr();
            return;
        }
        let mut i = 0;
        while i < list.len() {
            match list[i] {
                0 => self.reset_sgr(),
                1 => self.current_bold = true,
                7 => self.current_reverse = true,
                22 => self.current_bold = false,
                27 => self.current_reverse = false,
                30..=37 => self.current_fg = ansi_color((list[i] - 30) as u8),
                39 => self.current_fg = Cell::default().fg,
                40..=47 => self.current_bg = Some(ansi_color((list[i] - 40) as u8)),
                49 => self.current_bg = None,
                90..=97 => self.current_fg = ansi_color((list[i] - 90 + 8) as u8),
                100..=107 => self.current_bg = Some(ansi_color((list[i] - 100 + 8) as u8)),
                38 if i + 1 < list.len() => {
                    if list[i + 1] == 5 && i + 2 < list.len() {
                        self.current_fg = color_256(list[i + 2] as u8);
                        i += 2;
                    } else if list[i + 1] == 2 && i + 4 < list.len() {
                        self.current_fg = Rgb {
                            r: list[i + 2] as u8,
                            g: list[i + 3] as u8,
                            b: list[i + 4] as u8,
                        };
                        i += 4;
                    }
                }
                48 if i + 1 < list.len() => {
                    if list[i + 1] == 5 && i + 2 < list.len() {
                        self.current_bg = Some(color_256(list[i + 2] as u8));
                        i += 2;
                    } else if list[i + 1] == 2 && i + 4 < list.len() {
                        self.current_bg = Some(Rgb {
                            r: list[i + 2] as u8,
                            g: list[i + 3] as u8,
                            b: list[i + 4] as u8,
                        });
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn reset_sgr(&mut self) {
        self.current_fg = Cell::default().fg;
        self.current_bg = None;
        self.current_bold = false;
        self.current_reverse = false;
    }
}

fn char_display_width(c: char) -> usize {
    let cp = c as u32;
    // 制御文字・ゼロ幅文字・結合文字
    if cp < 0x20
        || (0x7f..=0x9f).contains(&cp)
        || (0x0300..=0x036f).contains(&cp)
        || (0x1ab0..=0x1aff).contains(&cp)
        || (0x1dc0..=0x1dff).contains(&cp)
        || (0x200b..=0x200f).contains(&cp)
        || (0x20d0..=0x20ff).contains(&cp)
        || (0xfe00..=0xfe0f).contains(&cp)
        || (0xfe20..=0xfe2f).contains(&cp)
        || cp == 0xfeff
    {
        return 0;
    }
    // 東アジア全角文字 (CJK / ひらがな / カタカナ / 全角英数記号 / ハングル / 絵文字)
    let is_wide = (0x1100..=0x115f).contains(&cp)
        || cp == 0x2329
        || cp == 0x232a
        || (0x2e80..=0x303e).contains(&cp)
        || (0x3040..=0x33ff).contains(&cp)
        || (0x3400..=0x4dbf).contains(&cp)
        || (0x4e00..=0x9fff).contains(&cp)
        || (0xa000..=0xa4cf).contains(&cp)
        || (0xac00..=0xd7a3).contains(&cp)
        || (0xf900..=0xfaff).contains(&cp)
        || (0xfe10..=0xfe19).contains(&cp)
        || (0xfe30..=0xfe6f).contains(&cp)
        || (0xff00..=0xff60).contains(&cp)
        || (0xffe0..=0xffe6).contains(&cp)
        || (0x1f300..=0x1f9ff).contains(&cp)
        || (0x1fa00..=0x1faff).contains(&cp)
        || (0x20000..=0x3fffd).contains(&cp);
    if is_wide { 2 } else { 1 }
}

impl Perform for TerminalGrid {
    fn print(&mut self, c: char) {
        let width = char_display_width(c);
        if width == 0 {
            return;
        }
        if self.pending_wrap || self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.newline();
            self.pending_wrap = false;
        }
        if width == 2 && self.cols >= 2 && self.cursor_col + 1 >= self.cols {
            let idx = self.cursor_row * self.cols + self.cursor_col;
            if let Some(cell) = self.cells.get_mut(idx) {
                *cell = Cell::default();
            }
            self.cursor_col = 0;
            self.newline();
        }
        let (fg, bg) = if self.current_reverse {
            let bg_or_default = self.current_bg.unwrap_or(Rgb {
                r: 0x22,
                g: 0x24,
                b: 0x36,
            });
            (bg_or_default, Some(self.current_fg))
        } else {
            (self.current_fg, self.current_bg)
        };

        self.clear_wide_overlap(self.cursor_row, self.cursor_col);
        let idx = self.cursor_row * self.cols + self.cursor_col;
        if let Some(cell) = self.cells.get_mut(idx) {
            *cell = Cell {
                ch: c,
                fg,
                bg,
                bold: self.current_bold,
            };
        }

        if width == 2 && self.cursor_col + 1 < self.cols {
            self.clear_wide_overlap(self.cursor_row, self.cursor_col + 1);
            if let Some(cont) = self.cells.get_mut(idx + 1) {
                *cont = Cell {
                    ch: '\0',
                    fg,
                    bg,
                    bold: self.current_bold,
                };
            }
            if self.cursor_col + 2 >= self.cols {
                self.cursor_col = self.cols - 1;
                self.pending_wrap = true;
            } else {
                self.cursor_col += 2;
            }
        } else if self.cursor_col + 1 >= self.cols {
            self.pending_wrap = true;
        } else {
            self.cursor_col += 1;
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.pending_wrap = false;
                self.newline();
            }
            b'\r' => {
                self.pending_wrap = false;
                self.cursor_col = 0;
            }
            0x08 => {
                self.pending_wrap = false;
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            b'\t' => {
                self.pending_wrap = false;
                let next = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => {
                self.saved_cursor = (self.cursor_col, self.cursor_row);
            }
            b'8' => {
                self.cursor_col = self.saved_cursor.0.min(self.cols - 1);
                self.cursor_row = self.saved_cursor.1.min(self.rows - 1);
                self.pending_wrap = false;
            }
            b'D' => {
                self.pending_wrap = false;
                self.newline();
            }
            b'E' => {
                self.pending_wrap = false;
                self.cursor_col = 0;
                self.newline();
            }
            b'M' => {
                self.reverse_index();
            }
            b'c' => {
                self.clear_all();
                self.cursor_col = 0;
                self.cursor_row = 0;
                self.scroll_top = 0;
                self.scroll_bottom = self.rows.saturating_sub(1);
                self.pending_wrap = false;
                self.reset_sgr();
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0) as usize;
        let is_private = intermediates.contains(&b'?');
        match action {
            'A' => {
                self.pending_wrap = false;
                self.cursor_row = self.cursor_row.saturating_sub(first.max(1));
            }
            'B' => {
                self.pending_wrap = false;
                self.cursor_row = (self.cursor_row + first.max(1)).min(self.rows - 1);
            }
            'C' => {
                self.pending_wrap = false;
                self.cursor_col = (self.cursor_col + first.max(1)).min(self.cols - 1);
            }
            'D' => {
                self.pending_wrap = false;
                self.cursor_col = self.cursor_col.saturating_sub(first.max(1));
            }
            'E' => {
                self.pending_wrap = false;
                self.cursor_row = (self.cursor_row + first.max(1)).min(self.rows - 1);
                self.cursor_col = 0;
            }
            'F' => {
                self.pending_wrap = false;
                self.cursor_row = self.cursor_row.saturating_sub(first.max(1));
                self.cursor_col = 0;
            }
            'G' | '`' => {
                self.pending_wrap = false;
                self.cursor_col = first.saturating_sub(1).min(self.cols - 1);
            }
            'd' => {
                self.pending_wrap = false;
                self.cursor_row = first.saturating_sub(1).min(self.rows - 1);
            }
            'H' | 'f' => {
                self.pending_wrap = false;
                let mut it = params.iter();
                let r = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                let c = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                self.cursor_row = (r - 1).min(self.rows - 1);
                self.cursor_col = (c - 1).min(self.cols - 1);
            }
            's' if !is_private => {
                self.saved_cursor = (self.cursor_col, self.cursor_row);
            }
            'u' if !is_private => {
                self.cursor_col = self.saved_cursor.0.min(self.cols - 1);
                self.cursor_row = self.saved_cursor.1.min(self.rows - 1);
                self.pending_wrap = false;
            }
            'P' => {
                // DCH: Delete N characters at cursor, shifting remaining cells left
                let n = first.max(1).min(self.cols - self.cursor_col);
                let row_start = self.cursor_row * self.cols;
                let start = row_start + self.cursor_col;
                let row_end = row_start + self.cols;
                self.cells[start..row_end].rotate_left(n);
                for c in &mut self.cells[row_end - n..row_end] {
                    *c = Cell::default();
                }
            }
            '@' => {
                // ICH: Insert N blank characters at cursor, shifting remaining cells right
                let n = first.max(1).min(self.cols - self.cursor_col);
                let row_start = self.cursor_row * self.cols;
                let start = row_start + self.cursor_col;
                let row_end = row_start + self.cols;
                self.cells[start..row_end].rotate_right(n);
                for c in &mut self.cells[start..start + n] {
                    *c = Cell::default();
                }
            }
            'J' => match first {
                0 => {
                    let start = self.cursor_row * self.cols + self.cursor_col;
                    for c in &mut self.cells[start..] {
                        *c = Cell::default();
                    }
                }
                1 => {
                    let end =
                        (self.cursor_row * self.cols + self.cursor_col + 1).min(self.cells.len());
                    for c in &mut self.cells[..end] {
                        *c = Cell::default();
                    }
                }
                2 | 3 => self.clear_all(),
                _ => {}
            },
            'K' => {
                let row_start = self.cursor_row * self.cols;
                let row_end = (self.cursor_row + 1) * self.cols;
                match first {
                    0 => {
                        for c in &mut self.cells[row_start + self.cursor_col..row_end] {
                            *c = Cell::default();
                        }
                    }
                    1 => {
                        let end = (row_start + self.cursor_col + 1).min(row_end);
                        for c in &mut self.cells[row_start..end] {
                            *c = Cell::default();
                        }
                    }
                    2 => {
                        for c in &mut self.cells[row_start..row_end] {
                            *c = Cell::default();
                        }
                    }
                    _ => {}
                }
            }
            'X' => {
                // ECH: Erase N characters
                let count = first.max(1);
                let start = self.cursor_row * self.cols + self.cursor_col;
                let end = (start + count).min((self.cursor_row + 1) * self.cols);
                for c in &mut self.cells[start..end] {
                    *c = Cell::default();
                }
            }
            'L' => {
                // IL: Insert N blank lines at cursor_row
                let n = first.max(1).min(self.rows - self.cursor_row);
                let start = self.cursor_row * self.cols;
                self.cells[start..].rotate_right(n * self.cols);
                for c in &mut self.cells[start..start + n * self.cols] {
                    *c = Cell::default();
                }
            }
            'M' => {
                // DL: Delete N lines at cursor_row within scroll region
                let bottom = self.scroll_bottom.min(self.rows - 1);
                if self.cursor_row <= bottom {
                    self.scroll_up_region(self.cursor_row, bottom, first.max(1));
                }
            }
            'S' if !is_private => {
                // SU: Scroll Up N lines
                self.scroll_up_region(self.scroll_top, self.scroll_bottom, first.max(1));
            }
            'T' if !is_private => {
                // SD: Scroll Down N lines
                self.scroll_down_region(self.scroll_top, self.scroll_bottom, first.max(1));
            }
            'r' if !is_private => {
                // DECSTBM: Set Top and Bottom Margins
                let mut it = params.iter();
                let top = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                let bottom = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(self.rows as u16) as usize;
                let top_idx = (top - 1).min(self.rows - 1);
                let bottom_idx = if bottom == 0 {
                    self.rows - 1
                } else {
                    (bottom - 1).min(self.rows - 1)
                };
                if top_idx < bottom_idx {
                    self.scroll_top = top_idx;
                    self.scroll_bottom = bottom_idx;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows - 1;
                }
                self.cursor_col = 0;
                self.cursor_row = 0;
                self.pending_wrap = false;
            }
            'h' if is_private && first == 25 => {
                self.cursor_visible = true;
            }
            'l' if is_private && first == 25 => {
                self.cursor_visible = false;
            }
            'h' if is_private && first == 2004 => {
                self.bracketed_paste = true;
            }
            'l' if is_private && first == 2004 => {
                self.bracketed_paste = false;
            }
            'h' if is_private && first == 1049 => {
                if self.alt_screen.is_none() {
                    let saved = std::mem::replace(
                        &mut self.cells,
                        vec![Cell::default(); self.cols * self.rows],
                    );
                    self.alt_screen = Some((saved, self.cursor_col, self.cursor_row));
                    self.cursor_col = 0;
                    self.cursor_row = 0;
                }
            }
            'l' if is_private && first == 1049 => {
                if let Some((saved, col, row)) = self.alt_screen.take() {
                    if saved.len() == self.cols * self.rows {
                        self.cells = saved;
                    }
                    self.cursor_col = col.min(self.cols - 1);
                    self.cursor_row = row.min(self.rows - 1);
                }
            }
            'n' if first == 6 => {
                let reply = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                const MAX_PENDING_REPLIES: usize = 256;
                self.pending_replies.push(reply.into_bytes());
                if self.pending_replies.len() > MAX_PENDING_REPLIES {
                    let excess = self.pending_replies.len() - MAX_PENDING_REPLIES;
                    self.pending_replies.drain(..excess);
                }
            }
            'm' => self.apply_sgr(params),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_text_and_handles_cr_lf() {
        let mut grid = TerminalGrid::new(10, 4);
        grid.feed_bytes(b"hi\r\nrust");
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'h');
        assert_eq!(grid.cell(1, 0).unwrap().ch, 'i');
        assert_eq!(grid.cell(0, 1).unwrap().ch, 'r');
        assert_eq!(grid.cell(3, 1).unwrap().ch, 't');
        assert_eq!((grid.cursor_col, grid.cursor_row), (4, 1));
    }

    #[test]
    fn responds_to_dsr_cursor_position_query() {
        let mut grid = TerminalGrid::new(10, 4);
        grid.feed_bytes(b"abc\x1b[6n");
        let replies = grid.take_replies();
        assert_eq!(replies, vec![b"\x1b[1;4R".to_vec()]);
    }

    #[test]
    fn parses_sgr_truecolor_and_scrollback() {
        let mut grid = TerminalGrid::new(10, 2);
        grid.feed_bytes(b"\x1b[38;2;10;20;30mA\r\nB\r\nC");
        assert_eq!(grid.scrollback.len(), 1);
        assert_eq!(grid.scrollback[0][0].ch, 'A');
        assert_eq!(
            grid.scrollback[0][0].fg,
            Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
        grid.scroll_up(1);
        assert_eq!(grid.visible_cell(0, 0).unwrap().ch, 'A');
        grid.scroll_down(1);
        assert_eq!(grid.visible_cell(0, 0).unwrap().ch, 'B');
    }

    #[test]
    fn extracts_selected_text_across_lines() {
        let mut grid = TerminalGrid::new(10, 3);
        grid.feed_bytes(b"hello\r\nworld");
        assert_eq!(grid.extract_text((1, 0), (3, 1)), "ello\nworl");
    }

    #[test]
    fn wide_chars_occupy_two_cells_and_dch_ich_work() {
        let mut grid = TerminalGrid::new(10, 2);
        grid.feed_bytes("あa".as_bytes());
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'あ');
        assert_eq!(grid.cell(1, 0).unwrap().ch, '\0');
        assert_eq!(grid.cell(2, 0).unwrap().ch, 'a');
        assert_eq!(grid.cursor_col, 3);
        assert_eq!(grid.extract_text((0, 0), (2, 0)), "あa");

        // DCH (CSI P) deletes at cursor and shifts left
        grid.feed_bytes(b"\r\x1b[2P");
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'a');
    }

    #[test]
    fn resize_clamps_extreme_dimensions_safely() {
        let mut grid = TerminalGrid::new(10, 5);
        grid.resize(usize::MAX, usize::MAX);
        assert_eq!(grid.cols, 1000);
        assert_eq!(grid.rows, 1000);
        grid.resize(0, 0);
        assert_eq!(grid.cols, 1);
        assert_eq!(grid.rows, 1);
    }

    #[test]
    fn pending_replies_is_bounded_when_unconsumed() {
        let mut grid = TerminalGrid::new(10, 5);
        for _ in 0..400 {
            grid.feed_bytes(b"\x1b[6n");
        }
        assert!(grid.pending_replies.len() <= 256);
    }

    #[test]
    fn resize_reflows_long_lines_and_keeps_cursor_aligned_with_prompt() {
        let mut grid = TerminalGrid::new(20, 6);
        grid.feed_bytes(b"0123456789abcdefghij\r\n> ");
        assert_eq!((grid.cursor_col, grid.cursor_row), (2, 1));
        assert_eq!(grid.cell(0, 1).unwrap().ch, '>');

        // SplitRight (width halved from 20 -> 10): row 0 wraps into 2 rows, so `> ` moves to row 2
        grid.resize(10, 6);
        assert_eq!(grid.cell(0, 2).unwrap().ch, '>');
        assert_eq!((grid.cursor_col, grid.cursor_row), (2, 2));

        // Typing after resize prints right next to `> ` on row 2
        grid.feed_bytes(b"ls");
        assert_eq!(grid.extract_text((0, 2), (4, 2)), "> ls");
    }

    #[test]
    fn resize_vertical_shrink_scrolls_top_rows_to_preserve_cursor_row() {
        let mut grid = TerminalGrid::new(10, 6);
        grid.feed_bytes(b"line1\r\nline2\r\nline3\r\nline4\r\n> ");
        assert_eq!((grid.cursor_col, grid.cursor_row), (2, 4));

        // SplitDown (height halved from 6 -> 3): top rows scroll into scrollback and `> ` stays on bottom row 2
        grid.resize(10, 3);
        assert_eq!(grid.scrollback.len(), 2);
        assert_eq!(grid.cell(0, 2).unwrap().ch, '>');
        assert_eq!((grid.cursor_col, grid.cursor_row), (2, 2));
    }

    #[test]
    fn reverse_index_and_scroll_commands_keep_cursor_aligned() {
        let mut grid = TerminalGrid::new(10, 4);
        grid.feed_bytes(b"\r\n> \r\nstatus\x1bM\r\x1b[2Cok");
        assert_eq!(grid.extract_text((0, 1), (4, 1)), "> ok");
    }
}
