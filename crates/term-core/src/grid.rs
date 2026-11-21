use std::collections::VecDeque;

use vte::{Params, Parser, Perform};

const MAX_SCROLLBACK: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
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
        0 => Rgb { r: 0x1b, g: 0x1d, b: 0x2b }, // Black
        1 => Rgb { r: 0xff, g: 0x75, b: 0x7f }, // Red
        2 => Rgb { r: 0xc3, g: 0xe8, b: 0x8d }, // Green
        3 => Rgb { r: 0xff, g: 0xc7, b: 0x77 }, // Yellow
        4 => Rgb { r: 0x82, g: 0xaa, b: 0xff }, // Blue
        5 => Rgb { r: 0xc0, g: 0x99, b: 0xff }, // Magenta
        6 => Rgb { r: 0x86, g: 0xe1, b: 0xfc }, // Cyan
        7 => Rgb { r: 0x82, g: 0x8b, b: 0xb8 }, // White
        8 => Rgb { r: 0x44, g: 0x4a, b: 0x73 }, // Bright Black
        9 => Rgb { r: 0xff, g: 0x8d, b: 0x94 }, // Bright Red
        10 => Rgb { r: 0xc7, g: 0xfb, b: 0x6d }, // Bright Green
        11 => Rgb { r: 0xff, g: 0xd8, b: 0xab }, // Bright Yellow
        12 => Rgb { r: 0x9a, g: 0xb8, b: 0xff }, // Bright Blue
        13 => Rgb { r: 0xca, g: 0xab, b: 0xff }, // Bright Magenta
        14 => Rgb { r: 0xb2, g: 0xeb, b: 0xff }, // Bright Cyan
        _ => Rgb { r: 0xc8, g: 0xd3, b: 0xf5 }, // Bright White
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
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub scroll_offset: usize,
    pub pending_replies: Vec<Vec<u8>>,
    current_fg: Rgb,
    current_bg: Option<Rgb>,
    current_bold: bool,
    current_reverse: bool,
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
            cells: vec![Cell::default(); cols * rows],
            scrollback: VecDeque::new(),
            scroll_offset: 0,
            pending_replies: Vec::new(),
            current_fg: Cell::default().fg,
            current_bg: None,
            current_bold: false,
            current_reverse: false,
            parser: Parser::new(),
        }
    }

    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::replace(&mut self.parser, Parser::new());
        parser.advance(self, bytes);
        self.parser = parser;
    }

    pub fn take_replies(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending_replies)
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

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        let new_cols = new_cols.max(1);
        let new_rows = new_rows.max(1);
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }

        let mut new_cells = vec![Cell::default(); new_cols * new_rows];
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
    }

    fn newline(&mut self) {
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else {
            let top_row = self.cells[..self.cols].to_vec();
            if self.scrollback.len() >= MAX_SCROLLBACK {
                self.scrollback.pop_front();
            }
            self.scrollback.push_back(top_row);
            if self.scroll_offset > 0 {
                self.scroll_offset = (self.scroll_offset + 1).min(self.scrollback.len());
            }

            self.cells.rotate_left(self.cols);
            let start = (self.rows - 1) * self.cols;
            for c in &mut self.cells[start..] {
                *c = Cell::default();
            }
        }
    }

    fn clear_all(&mut self) {
        for c in &mut self.cells {
            *c = Cell::default();
        }
    }

    fn apply_sgr(&mut self, params: &Params) {
        let list: Vec<u16> = params.iter().map(|p| p.first().copied().unwrap_or(0)).collect();
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

impl Perform for TerminalGrid {
    fn print(&mut self, c: char) {
        if self.cursor_col >= self.cols {
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
        let idx = self.cursor_row * self.cols + self.cursor_col;
        if let Some(cell) = self.cells.get_mut(idx) {
            *cell = Cell {
                ch: c,
                fg,
                bg,
                bold: self.current_bold,
            };
        }
        self.cursor_col += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cursor_col = 0,
            0x08 => {
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            b'\t' => {
                let next = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0) as usize;
        match action {
            'A' => self.cursor_row = self.cursor_row.saturating_sub(first.max(1)),
            'B' => self.cursor_row = (self.cursor_row + first.max(1)).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + first.max(1)).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(first.max(1)),
            'G' => self.cursor_col = first.saturating_sub(1).min(self.cols - 1),
            'd' => self.cursor_row = first.saturating_sub(1).min(self.rows - 1),
            'H' | 'f' => {
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
            'J' if first == 2 => self.clear_all(),
            'K' => {
                let start = self.cursor_row * self.cols + self.cursor_col;
                let end = (self.cursor_row + 1) * self.cols;
                for c in &mut self.cells[start..end] {
                    *c = Cell::default();
                }
            }
            'n' if first == 6 => {
                let reply = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                self.pending_replies.push(reply.into_bytes());
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
}
