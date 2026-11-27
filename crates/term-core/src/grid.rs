use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use vte::{Params, Parser, Perform};

use crate::config::ResolvedTheme;
use crate::kitty::{KittySegment, KittyState};

const MAX_SCROLLBACK: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// セルの色。番号と既定色は描画のときにテーマで RGB に決めるため、テーマを変えると表示済みの文字の色も変わる
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Color {
    /// テーマの前景色・背景色（背景は塗らない）
    #[default]
    Default,
    /// パレットの番号（0〜255）
    Indexed(u8),
    Rgb(Rgb),
}

/// 下線の種類 (SGR 4 / 4:N / 21)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

// NOTE: server からは毎回全セルを JSON で送るため、既定値の項目は書き出さずに小さくする
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub ch: char,
    #[serde(default, skip_serializing_if = "is_default")]
    pub fg: Color,
    #[serde(default, skip_serializing_if = "is_default")]
    pub bg: Color,
    #[serde(default, skip_serializing_if = "is_default")]
    pub bold: bool,
    /// 薄字 (SGR 2)
    #[serde(default, skip_serializing_if = "is_default")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub underline: Underline,
    /// 下線の色 (SGR 58)。既定色なら文字色で引く
    #[serde(default, skip_serializing_if = "is_default")]
    pub underline_color: Color,
    /// 反転表示 (SGR 7)。前景と背景は描画のときに入れ替える
    #[serde(default, skip_serializing_if = "is_default")]
    pub reverse: bool,
    /// 非表示 (SGR 8)。文字を描かない
    #[serde(default, skip_serializing_if = "is_default")]
    pub invisible: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub strikethrough: bool,
    /// 上線 (SGR 53)
    #[serde(default, skip_serializing_if = "is_default")]
    pub overline: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: Underline::None,
            underline_color: Color::Default,
            reverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub cursor_visible: bool,
    pub bracketed_paste: bool,
    #[serde(default)]
    pub modes: TerminalModes,
    #[serde(default)]
    pub synchronized_output: bool,
    #[serde(default)]
    pub generation: u64,
    pub cells: Vec<Cell>,
    #[serde(default)]
    pub color_overrides: ColorOverrides,
}

/// マウスの報告の範囲 (CSI ?9 / ?1000 / ?1002 / ?1003 h)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MouseTracking {
    #[default]
    Off,
    /// ボタンを押したときだけ（修飾キーなし）
    X10,
    /// 押したときと離したとき
    Normal,
    /// それに加えて、ボタンを押したまま動かしたとき
    ButtonEvent,
    /// それに加えて、ボタンを押していなくても動かしたとき
    AnyEvent,
}

/// アプリが CSI ? h / l で切り替える、キー・マウスの送り方のモード
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalModes {
    /// カーソルキーのアプリケーションモード (?1)。矢印・Home・End を SS3 で送る
    pub app_cursor_keys: bool,
    pub mouse_tracking: MouseTracking,
    /// マウスの報告を SGR の形（CSI < b ; x ; y M）で送る (?1006)
    pub mouse_sgr: bool,
    /// フォーカスの出入りを CSI I / CSI O で知らせる (?1004)
    pub focus_events: bool,
    /// 代替画面でマウスの報告がオフのとき、ホイールを矢印キーとして送る (?1007。Ghostty と同じく既定でオン)
    pub alt_scroll: bool,
    /// 代替画面 (?1049) を使っているか
    pub alt_screen: bool,
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self {
            app_cursor_keys: false,
            mouse_tracking: MouseTracking::Off,
            mouse_sgr: false,
            focus_events: false,
            alt_scroll: true,
            alt_screen: false,
        }
    }
}

/// OSC 4 / 10 / 11 / 12 で変えた色。テーマの色より優先し、OSC 104 / 110 / 111 / 112 で戻す
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorOverrides {
    pub palette: BTreeMap<u8, [u8; 3]>,
    pub fg: Option<[u8; 3]>,
    pub bg: Option<[u8; 3]>,
    pub cursor: Option<[u8; 3]>,
}

/// 既定の256色。0〜15 は TokyoNight Moon、16〜231 は 6x6x6 の色の立方体、232〜255 はグレー（xterm と同じ計算）
pub const DEFAULT_PALETTE: [[u8; 3]; 256] = {
    const BASE: [[u8; 3]; 16] = [
        [0x1b, 0x1d, 0x2b], // Black
        [0xff, 0x75, 0x7f], // Red
        [0xc3, 0xe8, 0x8d], // Green
        [0xff, 0xc7, 0x77], // Yellow
        [0x82, 0xaa, 0xff], // Blue
        [0xc0, 0x99, 0xff], // Magenta
        [0x86, 0xe1, 0xfc], // Cyan
        [0x82, 0x8b, 0xb8], // White
        [0x44, 0x4a, 0x73], // Bright Black
        [0xff, 0x8d, 0x94], // Bright Red
        [0xc7, 0xfb, 0x6d], // Bright Green
        [0xff, 0xd8, 0xab], // Bright Yellow
        [0x9a, 0xb8, 0xff], // Bright Blue
        [0xca, 0xab, 0xff], // Bright Magenta
        [0xb2, 0xeb, 0xff], // Bright Cyan
        [0xc8, 0xd3, 0xf5], // Bright White
    ];
    const fn cube(v: usize) -> u8 {
        if v == 0 { 0 } else { (55 + v * 40) as u8 }
    }
    let mut p = [[0u8; 3]; 256];
    let mut i = 0;
    while i < 16 {
        p[i] = BASE[i];
        i += 1;
    }
    let mut n = 0;
    while n < 216 {
        p[16 + n] = [cube(n / 36), cube((n % 36) / 6), cube(n % 6)];
        n += 1;
    }
    let mut g = 0;
    while g < 24 {
        let v = (8 + g * 10) as u8;
        p[232 + g] = [v, v, v];
        g += 1;
    }
    p
};

/// SGR 38 / 48 / 58 の色を読み、使った引数のぶん i を進める
/// `38;5;N` `38;2;R;G;B`（; 区切り）と `38:5:N` `38:2:R:G:B` `38:2::R:G:B`（: 区切り）を受け付ける
fn parse_sgr_color(params: &[&[u16]], i: &mut usize) -> Option<Color> {
    let rgb = |v: &[u16]| {
        Color::Rgb(Rgb {
            r: v[0] as u8,
            g: v[1] as u8,
            b: v[2] as u8,
        })
    };
    let p = params[*i];
    if p.len() > 1 {
        return match (p[1], p.len()) {
            (5, 3..) => Some(Color::Indexed(p[2] as u8)),
            // 色空間の ID が入る形（38:2::R:G:B）と入らない形（38:2:R:G:B）
            (2, 6..) => Some(rgb(&p[3..6])),
            (2, 5) => Some(rgb(&p[2..5])),
            _ => None,
        };
    }
    let next = |n: usize| params.get(*i + n).map(|v| v[0]);
    match next(1)? {
        5 => {
            let index = next(2)?;
            *i += 2;
            Some(Color::Indexed(index as u8))
        }
        2 => {
            let (r, g, b) = (next(2)?, next(3)?, next(4)?);
            *i += 4;
            Some(rgb(&[r, g, b]))
        }
        _ => None,
    }
}

fn parse_number<T: std::str::FromStr>(bytes: &[u8]) -> Option<T> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

/// OSC の色の指定を読む。`rgb:R/G/B`（各 1〜4 桁、桁数に合わせて拡大する）と
/// `#RGB` / `#RRGGBB` / `#RRRGGGBBB` / `#RRRRGGGGBBBB`（上位の桁を使う。XParseColor と同じ）
/// shortcut: X11 の色名（red など）は読まない。色名で指定するアプリが出てきたら対応する
pub fn parse_color_spec(spec: &[u8]) -> Option<[u8; 3]> {
    let spec = std::str::from_utf8(spec).ok()?;
    if let Some(rest) = spec.strip_prefix("rgb:") {
        let mut parts = rest.split('/');
        let mut color = [0u8; 3];
        for c in &mut color {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 4 {
                return None;
            }
            let max = (1u32 << (4 * part.len())) - 1;
            *c = (u32::from_str_radix(part, 16).ok()? * 255 / max) as u8;
        }
        return parts.next().is_none().then_some(color);
    }
    let hex = spec.strip_prefix('#')?;
    if hex.is_empty() || hex.len() % 3 != 0 || hex.len() > 12 {
        return None;
    }
    let n = hex.len() / 3;
    let mut color = [0u8; 3];
    for (i, c) in color.iter_mut().enumerate() {
        let v = u32::from_str_radix(&hex[i * n..(i + 1) * n], 16).ok()?;
        // 上位 8 ビットを取る。1 桁のときは左へずらす（#f00 は #f00000）
        *c = if n == 1 {
            (v << 4) as u8
        } else {
            (v >> (4 * (n - 2))) as u8
        };
    }
    Some(color)
}

/// xterm と同じ `rgb:rrrr/gggg/bbbb` の形にする
fn rgb_spec([r, g, b]: [u8; 3]) -> String {
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        r as u16 * 257,
        g as u16 * 257,
        b as u16 * 257
    )
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
    pub modes: TerminalModes,
    pub synchronized_output: bool,
    pub generation: u64,
    pub cells: Vec<Cell>,
    pub scrollback: VecDeque<Vec<Cell>>,
    pub scroll_offset: usize,
    pub pending_replies: Vec<Vec<u8>>,
    pub kitty: KittyState,
    pub color_overrides: ColorOverrides,
    /// OSC の問い合わせに答えるためのテーマ。描画するテーマと同じものを set_theme で渡す
    theme: ResolvedTheme,
    /// これから書く文字の色と属性（SGR で変わる）。ch は使わない
    pen: Cell,
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
            modes: TerminalModes::default(),
            synchronized_output: false,
            generation: 0,
            cells: vec![Cell::default(); cols * rows],
            scrollback: VecDeque::new(),
            scroll_offset: 0,
            pending_replies: Vec::new(),
            kitty: KittyState::new(),
            color_overrides: ColorOverrides::default(),
            theme: ResolvedTheme::default(),
            pen: Cell::default(),
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
        let segments = self.kitty.split_stream(bytes);
        let mut parser = std::mem::replace(&mut self.parser, Parser::new());

        for seg in segments {
            match seg {
                KittySegment::Normal(normal_bytes) => {
                    parser.advance(self, &normal_bytes);
                }
                KittySegment::Command(cmd) => {
                    self.kitty.handle_kitty_command(
                        &cmd,
                        self.cursor_col,
                        self.cursor_row,
                        &mut replies,
                    );
                }
                KittySegment::ITerm(cmd) => {
                    self.kitty
                        .handle_iterm_command(&cmd, self.cursor_col, self.cursor_row);
                }
            }
        }
        self.parser = parser;

        const MAX_PENDING_REPLIES: usize = 256;
        if !replies.is_empty() {
            self.pending_replies.extend(replies);
            if self.pending_replies.len() > MAX_PENDING_REPLIES {
                let excess = self.pending_replies.len() - MAX_PENDING_REPLIES;
                self.pending_replies.drain(..excess);
            }
        }
        if !bytes.is_empty() {
            self.generation = self.generation.wrapping_add(1);
        }
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
            modes: self.modes,
            synchronized_output: self.synchronized_output,
            generation: self.generation,
            cells,
            color_overrides: self.color_overrides.clone(),
        }
    }

    pub fn set_theme(&mut self, theme: &ResolvedTheme) {
        self.theme = *theme;
    }

    fn push_reply(&mut self, reply: Vec<u8>) {
        const MAX_PENDING_REPLIES: usize = 256;
        self.pending_replies.push(reply);
        if self.pending_replies.len() > MAX_PENDING_REPLIES {
            let excess = self.pending_replies.len() - MAX_PENDING_REPLIES;
            self.pending_replies.drain(..excess);
        }
    }

    /// OSC 4 / 104 / 10〜12 / 110〜112（パレットと前景・背景・カーソルの色の変更・問い合わせ・リセット）
    fn handle_color_osc(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let Some(code) = params.first().and_then(|p| parse_number::<u16>(p)) else {
            return;
        };
        // 問い合わせには、問い合わせと同じ終端（BEL か ST）で返す
        let st = if bell_terminated { "\x07" } else { "\x1b\\" };
        let args = &params[1..];
        match code {
            // OSC 4 ; 番号 ; 色 [; 番号 ; 色 ...]。色が ? なら今の色を返す
            4 => {
                for pair in args.chunks(2) {
                    let [index, spec] = pair else { break };
                    let Some(i) = parse_number::<u8>(index) else {
                        continue;
                    };
                    if *spec == b"?" {
                        let color = self.current_theme().palette[i as usize];
                        let reply = format!("\x1b]4;{i};{}{st}", rgb_spec(color));
                        self.push_reply(reply.into_bytes());
                    } else if let Some(color) = parse_color_spec(spec) {
                        self.color_overrides.palette.insert(i, color);
                    }
                }
            }
            104 if args.iter().all(|a| a.is_empty()) => self.color_overrides.palette.clear(),
            104 => {
                for i in args.iter().filter_map(|a| parse_number::<u8>(a)) {
                    self.color_overrides.palette.remove(&i);
                }
            }
            // OSC 10 ; 色 ; 色 ... は 10, 11, 12 の順に続けて指定できる
            10..=12 => {
                for (code, spec) in (code..=12).zip(args) {
                    if *spec == b"?" {
                        let theme = self.current_theme();
                        let color = match code {
                            10 => theme.fg,
                            11 => theme.bg,
                            _ => theme.cursor,
                        };
                        let reply = format!("\x1b]{code};{}{st}", rgb_spec(color));
                        self.push_reply(reply.into_bytes());
                    } else if let Some(color) = parse_color_spec(spec) {
                        *self.dynamic_color(code) = Some(color);
                    }
                }
            }
            110..=112 => *self.dynamic_color(code - 100) = None,
            _ => {}
        }
    }

    fn dynamic_color(&mut self, code: u16) -> &mut Option<[u8; 3]> {
        match code {
            10 => &mut self.color_overrides.fg,
            11 => &mut self.color_overrides.bg,
            _ => &mut self.color_overrides.cursor,
        }
    }

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.modes.app_cursor_keys = on,
            9 | 1000 | 1002 | 1003 => {
                self.modes.mouse_tracking = match (on, mode) {
                    (false, _) => MouseTracking::Off,
                    (true, 9) => MouseTracking::X10,
                    (true, 1000) => MouseTracking::Normal,
                    (true, 1002) => MouseTracking::ButtonEvent,
                    (true, _) => MouseTracking::AnyEvent,
                }
            }
            25 => self.cursor_visible = on,
            1004 => self.modes.focus_events = on,
            1006 => self.modes.mouse_sgr = on,
            1007 => self.modes.alt_scroll = on,
            1049 if on => {
                if self.alt_screen.is_none() {
                    let saved = std::mem::replace(
                        &mut self.cells,
                        vec![Cell::default(); self.cols * self.rows],
                    );
                    self.alt_screen = Some((saved, self.cursor_col, self.cursor_row));
                    self.cursor_col = 0;
                    self.cursor_row = 0;
                }
                self.modes.alt_screen = true;
            }
            1049 => {
                if let Some((saved, col, row)) = self.alt_screen.take() {
                    if saved.len() == self.cols * self.rows {
                        self.cells = saved;
                    }
                    self.cursor_col = col.min(self.cols - 1);
                    self.cursor_row = row.min(self.rows - 1);
                }
                self.modes.alt_screen = false;
            }
            2004 => self.bracketed_paste = on,
            2026 => self.synchronized_output = on,
            _ => {}
        }
    }

    fn current_theme(&self) -> ResolvedTheme {
        self.theme.with_overrides(&self.color_overrides)
    }

    pub fn scroll_up(&mut self, lines: usize) {
        let next = (self.scroll_offset + lines).min(self.scrollback.len());
        if next != self.scroll_offset {
            self.scroll_offset = next;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn scroll_down(&mut self, lines: usize) {
        let next = self.scroll_offset.saturating_sub(lines);
        if next != self.scroll_offset {
            self.scroll_offset = next;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn reset_scroll(&mut self) {
        if self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.generation = self.generation.wrapping_add(1);
        }
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
        self.generation = self.generation.wrapping_add(1);
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
            self.kitty.scroll_up_placements(n, MAX_SCROLLBACK);
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
        // NOTE: 1 つの引数に `:` で区切った値が入ることがある（4:3、38:2::r:g:b など）
        let params: Vec<&[u16]> = params.iter().collect();
        if params.is_empty() {
            self.reset_sgr();
            return;
        }
        let pen = &mut self.pen;
        let mut i = 0;
        while i < params.len() {
            let p = params[i];
            match p[0] {
                0 => *pen = Cell::default(),
                1 => pen.bold = true,
                2 => pen.dim = true,
                3 => pen.italic = true,
                4 => {
                    pen.underline = match p.get(1) {
                        Some(0) => Underline::None,
                        Some(2) => Underline::Double,
                        Some(3) => Underline::Curly,
                        Some(4) => Underline::Dotted,
                        Some(5) => Underline::Dashed,
                        _ => Underline::Single,
                    }
                }
                7 => pen.reverse = true,
                8 => pen.invisible = true,
                9 => pen.strikethrough = true,
                21 => pen.underline = Underline::Double,
                22 => (pen.bold, pen.dim) = (false, false),
                23 => pen.italic = false,
                24 => pen.underline = Underline::None,
                27 => pen.reverse = false,
                28 => pen.invisible = false,
                29 => pen.strikethrough = false,
                30..=37 => pen.fg = Color::Indexed((p[0] - 30) as u8),
                39 => pen.fg = Color::Default,
                40..=47 => pen.bg = Color::Indexed((p[0] - 40) as u8),
                49 => pen.bg = Color::Default,
                53 => pen.overline = true,
                55 => pen.overline = false,
                59 => pen.underline_color = Color::Default,
                90..=97 => pen.fg = Color::Indexed((p[0] - 90 + 8) as u8),
                100..=107 => pen.bg = Color::Indexed((p[0] - 100 + 8) as u8),
                38 | 48 | 58 => {
                    if let Some(color) = parse_sgr_color(&params, &mut i) {
                        match p[0] {
                            38 => pen.fg = color,
                            48 => pen.bg = color,
                            _ => pen.underline_color = color,
                        }
                    }
                }
                // NOTE: 点滅 (5 / 6 / 25) は Ghostty と同じく描かない
                _ => {}
            }
            i += 1;
        }
    }

    fn reset_sgr(&mut self) {
        self.pen = Cell::default();
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
        self.clear_wide_overlap(self.cursor_row, self.cursor_col);
        let idx = self.cursor_row * self.cols + self.cursor_col;
        if let Some(cell) = self.cells.get_mut(idx) {
            *cell = Cell { ch: c, ..self.pen };
        }

        if width == 2 && self.cursor_col + 1 < self.cols {
            self.clear_wide_overlap(self.cursor_row, self.cursor_col + 1);
            if let Some(cont) = self.cells.get_mut(idx + 1) {
                *cont = Cell {
                    ch: '\0',
                    ..self.pen
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

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.handle_color_osc(params, bell_terminated);
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
                self.color_overrides = ColorOverrides::default();
                self.modes = TerminalModes {
                    alt_screen: self.alt_screen.is_some(),
                    ..TerminalModes::default()
                };
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
            // NOTE: CSI ?1000;1006h のように 1 回で複数のモードを切り替えることがある
            'h' | 'l' if is_private => {
                for mode in params.iter().map(|p| p[0]) {
                    self.set_private_mode(mode, action == 'h');
                }
            }
            'n' if first == 6 => {
                let reply = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                self.push_reply(reply.into_bytes());
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
            Color::Rgb(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        grid.scroll_up(1);
        assert_eq!(grid.visible_cell(0, 0).unwrap().ch, 'A');
        grid.scroll_down(1);
        assert_eq!(grid.visible_cell(0, 0).unwrap().ch, 'B');
    }

    #[test]
    fn keeps_sgr_colors_as_palette_index_and_reverse_flag() {
        let mut grid = TerminalGrid::new(10, 1);
        grid.feed_bytes(b"\x1b[31;42mA\x1b[38;5;200;48;5;17mB\x1b[39;49;7mC\x1b[0mD");
        let cell = |col| *grid.cell(col, 0).unwrap();
        assert_eq!(
            (cell(0).fg, cell(0).bg),
            (Color::Indexed(1), Color::Indexed(2))
        );
        assert_eq!(
            (cell(1).fg, cell(1).bg),
            (Color::Indexed(200), Color::Indexed(17))
        );
        assert_eq!(
            (cell(2).fg, cell(2).bg, cell(2).reverse),
            (Color::Default, Color::Default, true)
        );
        assert_eq!(
            cell(3),
            Cell {
                ch: 'D',
                ..Cell::default()
            }
        );
    }

    #[test]
    fn default_palette_matches_xterm_cube_and_grays() {
        assert_eq!(DEFAULT_PALETTE[16], [0, 0, 0]);
        assert_eq!(DEFAULT_PALETTE[17], [0, 0, 95]);
        assert_eq!(DEFAULT_PALETTE[231], [255, 255, 255]);
        assert_eq!(DEFAULT_PALETTE[232], [8, 8, 8]);
        assert_eq!(DEFAULT_PALETTE[255], [238, 238, 238]);
    }

    #[test]
    fn osc_4_sets_queries_and_104_resets_palette() {
        let mut grid = TerminalGrid::new(10, 1);
        grid.feed_bytes(b"\x1b]4;1;rgb:ff/00/55;2;#00ff9f\x07");
        assert_eq!(
            grid.color_overrides.palette.get(&1),
            Some(&[0xff, 0x00, 0x55])
        );
        assert_eq!(
            grid.color_overrides.palette.get(&2),
            Some(&[0x00, 0xff, 0x9f])
        );

        // 問い合わせと同じ終端で返す（BEL と ST）
        grid.feed_bytes(b"\x1b]4;1;?\x07\x1b]4;3;?\x1b\\");
        let expected_3 = format!("\x1b]4;3;{}\x1b\\", rgb_spec(DEFAULT_PALETTE[3]));
        assert_eq!(
            grid.take_replies(),
            vec![
                b"\x1b]4;1;rgb:ffff/0000/5555\x07".to_vec(),
                expected_3.into_bytes()
            ]
        );

        grid.feed_bytes(b"\x1b]104;1\x07");
        assert_eq!(
            grid.color_overrides.palette.keys().collect::<Vec<_>>(),
            vec![&2]
        );
        grid.feed_bytes(b"\x1b]104\x07");
        assert!(grid.color_overrides.palette.is_empty());
    }

    #[test]
    fn osc_10_to_12_set_query_and_reset_dynamic_colors() {
        let mut grid = TerminalGrid::new(10, 1);
        grid.set_theme(&ResolvedTheme {
            bg: [0x0a, 0x0a, 0x0f],
            ..ResolvedTheme::default()
        });

        // 変えていない背景色はテーマの色を返す
        grid.feed_bytes(b"\x1b]11;?\x07");
        assert_eq!(
            grid.take_replies(),
            vec![b"\x1b]11;rgb:0a0a/0a0a/0f0f\x07".to_vec()]
        );

        // OSC 10 に続けて書いた色は 11, 12 に入る
        grid.feed_bytes(b"\x1b]10;#d4b3f7;#000000;#00ffff\x07");
        assert_eq!(grid.color_overrides.fg, Some([0xd4, 0xb3, 0xf7]));
        assert_eq!(grid.color_overrides.bg, Some([0, 0, 0]));
        assert_eq!(grid.color_overrides.cursor, Some([0x00, 0xff, 0xff]));
        grid.feed_bytes(b"\x1b]10;?\x07");
        assert_eq!(
            grid.take_replies(),
            vec![b"\x1b]10;rgb:d4d4/b3b3/f7f7\x07".to_vec()]
        );

        grid.feed_bytes(b"\x1b]111\x07");
        assert_eq!(grid.color_overrides.bg, None);
        // 全体のリセット (ESC c) で OSC の色も戻る
        grid.feed_bytes(b"\x1bc");
        assert_eq!(grid.color_overrides, ColorOverrides::default());
    }

    #[test]
    fn parses_osc_color_specs() {
        assert_eq!(parse_color_spec(b"rgb:f/0/5"), Some([0xff, 0x00, 0x55]));
        assert_eq!(
            parse_color_spec(b"rgb:ffff/0000/5555"),
            Some([0xff, 0x00, 0x55])
        );
        assert_eq!(parse_color_spec(b"#f05"), Some([0xf0, 0x00, 0x50]));
        assert_eq!(parse_color_spec(b"#ff0055"), Some([0xff, 0x00, 0x55]));
        assert_eq!(parse_color_spec(b"#ff0000555"), Some([0xff, 0x00, 0x55]));
        assert_eq!(parse_color_spec(b"#ffff00005555"), Some([0xff, 0x00, 0x55]));
        assert_eq!(parse_color_spec(b"rgb:ff/00"), None);
        assert_eq!(parse_color_spec(b"rgb:ff/00/55/00"), None);
        assert_eq!(parse_color_spec(b"#ff00"), None);
        assert_eq!(parse_color_spec(b"red"), None);
    }

    #[test]
    fn private_modes_toggle_mouse_focus_and_alt_screen() {
        let mut grid = TerminalGrid::new(4, 2);
        assert!(grid.modes.alt_scroll);
        // 1 回で複数のモードを切り替える
        grid.feed_bytes(b"\x1b[?1000;1006;1004h");
        assert_eq!(grid.modes.mouse_tracking, MouseTracking::Normal);
        assert!(grid.modes.mouse_sgr && grid.modes.focus_events);
        grid.feed_bytes(b"\x1b[?1003h");
        assert_eq!(grid.modes.mouse_tracking, MouseTracking::AnyEvent);
        grid.feed_bytes(b"\x1b[?1003l\x1b[?1007l");
        assert_eq!(grid.modes.mouse_tracking, MouseTracking::Off);
        assert!(!grid.modes.alt_scroll);

        grid.feed_bytes(b"\x1b[?1049;25l");
        assert!(!grid.modes.alt_screen && !grid.cursor_visible);
        grid.feed_bytes(b"\x1b[?1049h");
        assert!(grid.snapshot().modes.alt_screen);
        // 全体のリセットでモードは戻るが、代替画面にいることは変わらない
        grid.feed_bytes(b"\x1bc");
        assert_eq!(
            grid.modes,
            TerminalModes {
                alt_screen: true,
                ..TerminalModes::default()
            }
        );
        grid.feed_bytes(b"\x1b[?1049l");
        assert!(!grid.modes.alt_screen);
    }

    #[test]
    fn decckm_toggles_app_cursor_keys() {
        let mut grid = TerminalGrid::new(4, 1);
        grid.feed_bytes(b"\x1b[?1h");
        assert!(grid.snapshot().modes.app_cursor_keys);
        grid.feed_bytes(b"\x1b[?1l");
        assert!(!grid.modes.app_cursor_keys);
    }

    #[test]
    fn sgr_sets_and_clears_text_attributes() {
        let mut grid = TerminalGrid::new(10, 1);
        grid.feed_bytes(b"\x1b[1;2;3;4;7;8;9;53mA\x1b[22;23;24;27;28;29;55mB");
        let a = *grid.cell(0, 0).unwrap();
        assert!(
            a.bold
                && a.dim
                && a.italic
                && a.reverse
                && a.invisible
                && a.strikethrough
                && a.overline
        );
        assert_eq!(a.underline, Underline::Single);
        assert_eq!(
            *grid.cell(1, 0).unwrap(),
            Cell {
                ch: 'B',
                ..Cell::default()
            }
        );

        grid.feed_bytes(b"\r\x1b[4:3mC\x1b[21mD\x1b[4:0mE\x1b[4:5mF");
        let underline = |col| grid.cell(col, 0).unwrap().underline;
        assert_eq!(
            [underline(0), underline(1), underline(2), underline(3)],
            [
                Underline::Curly,
                Underline::Double,
                Underline::None,
                Underline::Dashed
            ]
        );
    }

    #[test]
    fn sgr_reads_colon_and_semicolon_extended_colors() {
        let mut grid = TerminalGrid::new(10, 1);
        grid.feed_bytes(
            b"\x1b[38:2::10:20:30mA\x1b[38:2:1:2:3mB\x1b[38:5:200mC\x1b[58;5;196mD\x1b[58:2::4:5:6;59mE",
        );
        let cell = |col| *grid.cell(col, 0).unwrap();
        assert_eq!(
            cell(0).fg,
            Color::Rgb(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        assert_eq!(cell(1).fg, Color::Rgb(Rgb { r: 1, g: 2, b: 3 }));
        assert_eq!(cell(2).fg, Color::Indexed(200));
        assert_eq!(cell(3).underline_color, Color::Indexed(196));
        // 58 のあとの 59 で下線の色を既定に戻す
        assert_eq!(cell(4).underline_color, Color::Default);
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
