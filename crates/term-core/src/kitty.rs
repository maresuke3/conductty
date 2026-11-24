use std::collections::{HashMap, VecDeque};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

const MAX_KITTY_APC_BYTES: usize = 32 * 1024 * 1024;
const MAX_KITTY_CHUNK_BYTES: usize = 32 * 1024 * 1024;
const MAX_KITTY_DIM: u32 = 4096;
const MAX_KITTY_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_KITTY_CACHE_BYTES: usize = 128 * 1024 * 1024;
const MAX_KITTY_PLACEMENTS: usize = 256;

#[derive(Debug, Clone)]
pub struct KittyImage {
    pub id: u32,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct KittyPlacement {
    pub image_id: u32,
    pub col: usize,
    pub row: isize,
    pub cols: Option<usize>,
    pub rows: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittySegment {
    Normal(Vec<u8>),
    Command(Vec<u8>),
    ITerm(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApcScanState {
    Normal,
    EscSeen,
    InApcStart,
    InKittyApc,
    InKittyEsc,
    InOscStart(usize),
    InITermOsc,
    InITermEsc,
}

/// バイトストリームから Kitty Graphics APC (`ESC _ G ... ESC \`) を抽出し、
/// 通常の VT バイト列と分離するステートマシン（チャンク跨ぎ対応）。
#[derive(Debug, Default)]
pub struct KittyState {
    scan_state: Option<ApcScanState>,
    apc_buf: Vec<u8>,
    chunk_buf: Vec<u8>,
    chunk_params: HashMap<String, String>,
    next_auto_id: u32,
    next_generation: u64,
    image_order: VecDeque<u32>,
    total_image_bytes: usize,
    pub images: HashMap<u32, KittyImage>,
    pub placements: Vec<KittyPlacement>,
}

impl KittyState {
    pub fn new() -> Self {
        Self {
            scan_state: Some(ApcScanState::Normal),
            next_auto_id: 1,
            ..Default::default()
        }
    }

    /// 入力バイト列から Kitty APC を分離し、通常の VT バイト列と Kitty コマンドを出現順に返す。
    pub fn split_stream(&mut self, bytes: &[u8]) -> Vec<KittySegment> {
        let mut segments = Vec::new();
        let mut normal_buf = Vec::new();
        let mut state = self.scan_state.unwrap_or(ApcScanState::Normal);

        for &b in bytes {
            match state {
                ApcScanState::Normal => {
                    if b == 0x1b {
                        state = ApcScanState::EscSeen;
                    } else {
                        normal_buf.push(b);
                    }
                }
                ApcScanState::EscSeen => {
                    if b == b'_' {
                        state = ApcScanState::InApcStart;
                    } else if b == b']' {
                        state = ApcScanState::InOscStart(0);
                    } else {
                        normal_buf.push(0x1b);
                        normal_buf.push(b);
                        state = ApcScanState::Normal;
                    }
                }
                ApcScanState::InOscStart(pos) => {
                    const PREFIX: &[u8] = b"1337;";
                    if b == PREFIX[pos] {
                        if pos + 1 == PREFIX.len() {
                            if !normal_buf.is_empty() {
                                segments
                                    .push(KittySegment::Normal(std::mem::take(&mut normal_buf)));
                            }
                            self.apc_buf.clear();
                            state = ApcScanState::InITermOsc;
                        } else {
                            state = ApcScanState::InOscStart(pos + 1);
                        }
                    } else {
                        normal_buf.extend_from_slice(&[0x1b, b']']);
                        normal_buf.extend_from_slice(&PREFIX[..pos]);
                        normal_buf.push(b);
                        state = ApcScanState::Normal;
                    }
                }
                ApcScanState::InITermOsc => {
                    if b == 0x07 {
                        let cmd = std::mem::take(&mut self.apc_buf);
                        segments.push(KittySegment::ITerm(cmd));
                        state = ApcScanState::Normal;
                    } else if b == 0x1b {
                        state = ApcScanState::InITermEsc;
                    } else if self.apc_buf.len() >= MAX_KITTY_APC_BYTES {
                        self.apc_buf = Vec::new();
                        state = ApcScanState::Normal;
                    } else {
                        self.apc_buf.push(b);
                    }
                }
                ApcScanState::InITermEsc => {
                    if b == b'\\' {
                        let cmd = std::mem::take(&mut self.apc_buf);
                        segments.push(KittySegment::ITerm(cmd));
                        state = ApcScanState::Normal;
                    } else if self.apc_buf.len().saturating_add(2) > MAX_KITTY_APC_BYTES {
                        self.apc_buf = Vec::new();
                        state = ApcScanState::Normal;
                    } else {
                        self.apc_buf.push(0x1b);
                        self.apc_buf.push(b);
                        state = ApcScanState::InITermOsc;
                    }
                }
                ApcScanState::InApcStart => {
                    if b == b'G' {
                        if !normal_buf.is_empty() {
                            segments.push(KittySegment::Normal(std::mem::take(&mut normal_buf)));
                        }
                        self.apc_buf.clear();
                        state = ApcScanState::InKittyApc;
                    } else {
                        normal_buf.extend_from_slice(&[0x1b, b'_', b]);
                        state = ApcScanState::Normal;
                    }
                }
                ApcScanState::InKittyApc => {
                    if b == 0x1b {
                        state = ApcScanState::InKittyEsc;
                    } else if self.apc_buf.len() >= MAX_KITTY_APC_BYTES {
                        self.apc_buf = Vec::new();
                        state = ApcScanState::Normal;
                    } else {
                        self.apc_buf.push(b);
                    }
                }
                ApcScanState::InKittyEsc => {
                    if b == b'\\' {
                        let cmd = std::mem::take(&mut self.apc_buf);
                        segments.push(KittySegment::Command(cmd));
                        state = ApcScanState::Normal;
                    } else if self.apc_buf.len().saturating_add(2) > MAX_KITTY_APC_BYTES {
                        self.apc_buf = Vec::new();
                        state = ApcScanState::Normal;
                    } else {
                        self.apc_buf.push(0x1b);
                        self.apc_buf.push(b);
                        state = ApcScanState::InKittyApc;
                    }
                }
            }
        }

        if !normal_buf.is_empty() {
            segments.push(KittySegment::Normal(normal_buf));
        }
        self.scan_state = Some(state);
        segments
    }

    /// 入力バイト列から Kitty APC を分離し、通常の VT バイト列を返す。
    /// 受信した Kitty コマンドの (応答バイト列, 配置する際の現在カーソル位置) を処理する。
    pub fn filter_and_process(
        &mut self,
        bytes: &[u8],
        cursor_col: usize,
        cursor_row: usize,
        replies: &mut Vec<Vec<u8>>,
    ) -> Vec<u8> {
        let mut normal_out = Vec::with_capacity(bytes.len());
        for seg in self.split_stream(bytes) {
            match seg {
                KittySegment::Normal(chunk) => normal_out.extend_from_slice(&chunk),
                KittySegment::Command(cmd) => {
                    self.handle_kitty_command(&cmd, cursor_col, cursor_row, replies);
                }
                KittySegment::ITerm(cmd) => {
                    self.handle_iterm_command(&cmd, cursor_col, cursor_row);
                }
            }
        }
        normal_out
    }

    pub fn scroll_up_placements(&mut self, lines: usize, max_scrollback: usize) {
        let delta = lines as isize;
        let min_row = -(max_scrollback as isize);
        for p in &mut self.placements {
            p.row -= delta;
        }
        self.placements.retain(|p| {
            let h = p.rows.unwrap_or(1) as isize;
            p.row + h > min_row
        });
    }

    pub fn handle_iterm_command(&mut self, raw: &[u8], cursor_col: usize, cursor_row: usize) {
        let text = match std::str::from_utf8(raw) {
            Ok(s) => s,
            Err(_) => return,
        };
        let Some(rest) = text.strip_prefix("File=") else {
            return;
        };
        let Some((header, b64_payload)) = rest.split_once(':') else {
            return;
        };

        let mut is_inline = false;
        let mut cols: Option<usize> = None;
        for pair in header.split(';') {
            if let Some((k, v)) = pair.split_once('=') {
                match k.trim() {
                    "inline" => is_inline = v.trim() == "1",
                    "width" => {
                        if let Ok(w) = v.trim().parse::<usize>()
                            && w > 0
                        {
                            cols = Some(w);
                        }
                    }
                    _ => {}
                }
            }
        }
        if !is_inline {
            return;
        }

        let clean_b64: Vec<u8> = b64_payload
            .bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        let id = self.next_auto_id;
        self.next_auto_id = self.next_auto_id.wrapping_add(1).max(1);
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;

        let mut params = HashMap::new();
        params.insert("f".to_string(), "100".to_string());
        let Some(img) = decode_kitty_image(id, generation, &params, &clean_b64) else {
            return;
        };
        let rows = cols.map(|c| {
            ((img.height as f64 * c as f64) / (img.width.max(1) as f64 * 2.0))
                .ceil()
                .max(1.0) as usize
        });
        self.insert_image_with_budget(img);

        if self.placements.len() >= MAX_KITTY_PLACEMENTS {
            let excess = self.placements.len() + 1 - MAX_KITTY_PLACEMENTS;
            self.placements.drain(..excess);
        }
        self.placements.push(KittyPlacement {
            image_id: id,
            col: cursor_col,
            row: cursor_row as isize,
            cols,
            rows,
        });
    }

    pub fn handle_kitty_command(
        &mut self,
        raw: &[u8],
        cursor_col: usize,
        cursor_row: usize,
        replies: &mut Vec<Vec<u8>>,
    ) {
        let text = match std::str::from_utf8(raw) {
            Ok(s) => s,
            Err(_) => return,
        };
        let (header, payload) = match text.split_once(';') {
            Some((h, p)) => (h, p),
            None => (text, ""),
        };

        let mut params = HashMap::new();
        for pair in header.split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                params.insert(k.to_string(), v.to_string());
            }
        }

        let more_chunks = params.get("m").map(String::as_str) == Some("1");
        if self.chunk_params.is_empty() {
            self.chunk_params = params;
        }
        if self.chunk_buf.len().saturating_add(payload.len()) > MAX_KITTY_CHUNK_BYTES {
            let quiet: u8 = self
                .chunk_params
                .get("q")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if quiet < 2
                && let Some(id) = self
                    .chunk_params
                    .get("i")
                    .and_then(|s| s.parse::<u32>().ok())
            {
                replies.push(format!("\x1b_Gi={id};EINVAL:payload too large\x1b\\").into_bytes());
            }
            self.chunk_buf = Vec::new();
            self.chunk_params.clear();
            return;
        }
        self.chunk_buf.extend_from_slice(payload.as_bytes());
        if more_chunks {
            return;
        }

        let full_payload = std::mem::take(&mut self.chunk_buf);
        let eff_params = std::mem::take(&mut self.chunk_params);
        let action = eff_params.get("a").map(String::as_str).unwrap_or("t");

        if action == "d" {
            let delete_mode = eff_params.get("d").map(String::as_str).unwrap_or("a");
            match delete_mode {
                "a" | "A" => {
                    self.placements.clear();
                    if delete_mode == "A" {
                        self.images.clear();
                        self.image_order.clear();
                        self.total_image_bytes = 0;
                    }
                }
                "i" | "I" => {
                    if let Some(id) = eff_params.get("i").and_then(|s| s.parse::<u32>().ok()) {
                        self.placements.retain(|p| p.image_id != id);
                        if delete_mode == "I" {
                            self.remove_image(id);
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        let quiet: u8 = eff_params
            .get("q")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let id = eff_params
            .get("i")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or_else(|| {
                let id = self.next_auto_id;
                self.next_auto_id = self.next_auto_id.wrapping_add(1).max(1);
                id
            });

        if action == "t" || action == "T" {
            self.next_generation = self.next_generation.wrapping_add(1);
            let generation = self.next_generation;
            if let Some(img) = decode_kitty_image(id, generation, &eff_params, &full_payload) {
                self.insert_image_with_budget(img);
            } else {
                if quiet < 2 {
                    replies.push(format!("\x1b_Gi={id};EINVAL:decode failed\x1b\\").into_bytes());
                }
                return;
            }
        }

        if action == "T" || action == "p" {
            let cols = eff_params.get("c").and_then(|s| s.parse::<usize>().ok());
            let rows = eff_params.get("r").and_then(|s| s.parse::<usize>().ok());
            if self.placements.len() >= MAX_KITTY_PLACEMENTS {
                let excess = self.placements.len() + 1 - MAX_KITTY_PLACEMENTS;
                self.placements.drain(..excess);
            }
            self.placements.push(KittyPlacement {
                image_id: id,
                col: cursor_col,
                row: cursor_row as isize,
                cols,
                rows,
            });
        }

        if quiet == 0 && eff_params.contains_key("i") {
            replies.push(format!("\x1b_Gi={id};OK\x1b\\").into_bytes());
        }
    }

    fn remove_image(&mut self, id: u32) {
        if let Some(old) = self.images.remove(&id) {
            self.total_image_bytes = self.total_image_bytes.saturating_sub(old.rgba.len());
        }
        self.image_order.retain(|&eid| eid != id);
    }

    fn insert_image_with_budget(&mut self, img: KittyImage) {
        self.remove_image(img.id);
        let new_bytes = img.rgba.len();
        while self.total_image_bytes.saturating_add(new_bytes) > MAX_KITTY_CACHE_BYTES {
            let Some(oldest_id) = self.image_order.pop_front() else {
                break;
            };
            if let Some(old) = self.images.remove(&oldest_id) {
                self.total_image_bytes = self.total_image_bytes.saturating_sub(old.rgba.len());
            }
            self.placements.retain(|p| p.image_id != oldest_id);
        }
        self.total_image_bytes = self.total_image_bytes.saturating_add(new_bytes);
        self.image_order.push_back(img.id);
        self.images.insert(img.id, img);
    }
}

fn checked_rgba_len(width: u32, height: u32) -> Option<(usize, usize)> {
    if width == 0 || height == 0 || width > MAX_KITTY_DIM || height > MAX_KITTY_DIM {
        return None;
    }
    let pixels = (width as usize).checked_mul(height as usize)?;
    let rgba_len = pixels.checked_mul(4)?;
    if rgba_len > MAX_KITTY_IMAGE_BYTES {
        return None;
    }
    Some((pixels, rgba_len))
}

fn decode_kitty_image(
    id: u32,
    generation: u64,
    params: &HashMap<String, String>,
    b64_payload: &[u8],
) -> Option<KittyImage> {
    let raw = BASE64.decode(b64_payload).ok()?;
    let fmt = params.get("f").map(String::as_str).unwrap_or("32");

    match fmt {
        "100" => {
            // PNG: デコード前に寸法・メモリ上限を設定して展開ボムを防止する
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_KITTY_DIM);
            limits.max_image_height = Some(MAX_KITTY_DIM);
            limits.max_alloc = Some(MAX_KITTY_IMAGE_BYTES as u64);
            let mut reader = image::ImageReader::with_format(
                std::io::Cursor::new(&raw),
                image::ImageFormat::Png,
            );
            reader.limits(limits);
            let img = reader.decode().ok()?.to_rgba8();
            let (width, height) = img.dimensions();
            checked_rgba_len(width, height)?;
            Some(KittyImage {
                id,
                generation,
                width,
                height,
                rgba: img.into_raw(),
            })
        }
        "32" => {
            let width = params.get("s").and_then(|s| s.parse::<u32>().ok())?;
            let height = params.get("v").and_then(|s| s.parse::<u32>().ok())?;
            let (_, expected_len) = checked_rgba_len(width, height)?;
            if raw.len() != expected_len {
                return None;
            }
            Some(KittyImage {
                id,
                generation,
                width,
                height,
                rgba: raw,
            })
        }
        "24" => {
            let width = params.get("s").and_then(|s| s.parse::<u32>().ok())?;
            let height = params.get("v").and_then(|s| s.parse::<u32>().ok())?;
            let (pixels, rgba_len) = checked_rgba_len(width, height)?;
            let expected_rgb_len = pixels.checked_mul(3)?;
            if raw.len() != expected_rgb_len {
                return None;
            }
            let mut rgba = Vec::with_capacity(rgba_len);
            for chunk in raw.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            Some(KittyImage {
                id,
                generation,
                width,
                height,
                rgba,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_mismatched_dimensions_without_panic() {
        let mut kitty = KittyState::new();
        let mut replies = Vec::new();
        // s=100, v=100 なのに 4 バイトしか送らない不正ペイロード
        let payload = BASE64.encode([1u8, 2, 3, 4]);
        let seq = format!("\x1b_Gi=7,a=T,f=32,s=100,v=100;{payload}\x1b\\");
        kitty.filter_and_process(seq.as_bytes(), 0, 0, &mut replies);

        assert!(kitty.images.is_empty());
        assert!(kitty.placements.is_empty());
        assert_eq!(replies.len(), 1);
        assert!(String::from_utf8_lossy(&replies[0]).contains("EINVAL"));
    }

    #[test]
    fn rejects_overflow_or_zero_dimensions() {
        let mut kitty = KittyState::new();
        let mut replies = Vec::new();
        let payload = BASE64.encode([0u8; 3]);
        let seq = format!("\x1b_Gi=8,a=t,f=24,s=4294967295,v=4294967295;{payload}\x1b\\");
        kitty.filter_and_process(seq.as_bytes(), 0, 0, &mut replies);
        assert!(kitty.images.is_empty());

        let zero_seq = format!("\x1b_Gi=9,a=t,f=32,s=0,v=10;{payload}\x1b\\");
        kitty.filter_and_process(zero_seq.as_bytes(), 0, 0, &mut replies);
        assert!(kitty.images.is_empty());
    }

    #[test]
    fn decodes_valid_rgb_and_rgba_and_evicts_on_budget() {
        let mut kitty = KittyState::new();
        let mut replies = Vec::new();
        let rgb = BASE64.encode([10u8, 20, 30, 40, 50, 60]);
        let seq = format!("\x1b_Gi=1,a=T,f=24,s=2,v=1;{rgb}\x1b\\");
        kitty.filter_and_process(seq.as_bytes(), 1, 2, &mut replies);

        let img = kitty.images.get(&1).expect("image 1 stored");
        assert_eq!(img.rgba, vec![10, 20, 30, 255, 40, 50, 60, 255]);
        assert_eq!(kitty.placements.len(), 1);
        assert_eq!(kitty.total_image_bytes, 8);

        // 上書き時は generation が進み、バイト数が二重加算されない
        let rgba = BASE64.encode([1u8, 2, 3, 4, 5, 6, 7, 8]);
        let seq2 = format!("\x1b_Gi=1,a=t,f=32,s=2,v=1;{rgba}\x1b\\");
        kitty.filter_and_process(seq2.as_bytes(), 0, 0, &mut replies);
        assert_eq!(kitty.images.get(&1).unwrap().generation, 2);
        assert_eq!(kitty.total_image_bytes, 8);
    }

    #[test]
    fn caps_oversized_apc_buffer() {
        let mut kitty = KittyState::new();
        let mut replies = Vec::new();
        kitty.filter_and_process(b"\x1b_G", 0, 0, &mut replies);
        let chunk = vec![b'A'; MAX_KITTY_APC_BYTES + 16];
        let out = kitty.filter_and_process(&chunk, 0, 0, &mut replies);
        assert!(kitty.apc_buf.is_empty());
        assert_eq!(kitty.scan_state, Some(ApcScanState::Normal));
        assert!(!out.is_empty());
    }

    #[test]
    fn extracts_chunked_kitty_rgba_and_replies_ok() {
        let mut kitty = KittyState::new();
        let mut replies = Vec::new();
        // 1x1 赤ピクセル (RGBA: 255, 0, 0, 255 -> base64 "/wAA/w==")
        let seq = b"hello\x1b_Ga=T,f=32,s=1,v=1,i=7,m=1;/wAA\x1b\\\x1b_Gm=0;/w==\x1b\\world";
        let normal = kitty.filter_and_process(seq, 2, 3, &mut replies);

        assert_eq!(normal, b"helloworld");
        assert_eq!(kitty.images.len(), 1);
        assert_eq!(kitty.images[&7].rgba, vec![255, 0, 0, 255]);
        assert_eq!(kitty.placements.len(), 1);
        assert_eq!((kitty.placements[0].col, kitty.placements[0].row), (2, 3));
        assert_eq!(replies, vec![b"\x1b_Gi=7;OK\x1b\\".to_vec()]);
    }

    #[test]
    fn evicts_oldest_images_on_budget_and_handles_delete_and_caps() {
        let mut kitty = KittyState::new();
        let mut replies = Vec::new();
        // 32 MiB * 4 = 128 MiB -> 5枚目で最古の画像が LRU 退避される
        for i in 1..=5u32 {
            let img = KittyImage {
                id: i,
                generation: i as u64,
                width: 4096,
                height: 2048,
                rgba: vec![0u8; 32 * 1024 * 1024],
            };
            kitty.insert_image_with_budget(img);
        }
        assert!(!kitty.images.contains_key(&1));
        assert!(kitty.images.contains_key(&5));
        assert!(kitty.total_image_bytes <= MAX_KITTY_CACHE_BYTES);

        // a=d,d=I で個別削除・バイト数減算
        kitty.filter_and_process(b"\x1b_Ga=d,d=I,i=5\x1b\\", 0, 0, &mut replies);
        assert!(!kitty.images.contains_key(&5));
        assert_eq!(kitty.total_image_bytes, 96 * 1024 * 1024);

        // a=d,d=A で全削除
        kitty.filter_and_process(b"\x1b_Ga=d,d=A\x1b\\", 0, 0, &mut replies);
        assert!(kitty.images.is_empty());
        assert_eq!(kitty.total_image_bytes, 0);

        // placements 上限キャップ
        for _ in 0..(MAX_KITTY_PLACEMENTS + 20) {
            kitty.filter_and_process(b"\x1b_Ga=p,i=1,q=2\x1b\\", 0, 0, &mut replies);
        }
        assert_eq!(kitty.placements.len(), MAX_KITTY_PLACEMENTS);

        // f=100 は非 PNG データを拒否する
        let non_png = BASE64.encode(b"NOT_A_PNG_HEADER");
        let seq = format!("\x1b_Gi=77,a=t,f=100;{non_png}\x1b\\");
        kitty.filter_and_process(seq.as_bytes(), 0, 0, &mut replies);
        assert!(!kitty.images.contains_key(&77));
    }

    #[test]
    fn tracks_cursor_and_scrollback_across_split_stream() {
        let mut grid = crate::grid::TerminalGrid::new(20, 4);
        let payload = BASE64.encode([255u8, 0, 0, 255, 0, 255, 0, 255]);
        let stream = format!(
            "line1\r\nhello   \r\n\x1b_Ga=T,f=32,s=2,v=1,c=10,r=2,i=1,q=2;{payload}\x1b\\\r\n\r\n\r\nbottom"
        );
        for chunk in stream.as_bytes().chunks(16) {
            grid.feed_bytes(chunk);
        }
        assert_eq!(grid.kitty.images.len(), 1);
        assert_eq!(grid.kitty.placements.len(), 1);
        assert_eq!(grid.kitty.placements[0].col, 0);
        // 5 newlines total on a 4-row screen: image placed at row 2, then scrolled up by 2 rows -> row 0
        assert_eq!(grid.kitty.placements[0].row, 0);
    }

    #[test]
    fn decodes_iterm_osc_1337_inline_png() {
        let mut png_bytes = Vec::new();
        let img = image::RgbaImage::from_pixel(4, 2, image::Rgba([12, 34, 56, 255]));
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let b64 = BASE64.encode(&png_bytes);
        let stream =
            format!("hi\r\n\x1b]1337;File=inline=1;width=40;preserveAspectRatio=1:{b64}\x07\r\n");

        let mut grid = crate::grid::TerminalGrid::new(80, 20);
        for chunk in stream.as_bytes().chunks(32) {
            grid.feed_bytes(chunk);
        }
        assert_eq!(grid.kitty.images.len(), 1);
        assert_eq!(grid.kitty.placements.len(), 1);
        assert_eq!(grid.kitty.placements[0].col, 0);
        assert_eq!(grid.kitty.placements[0].row, 1);
        assert_eq!(grid.kitty.placements[0].cols, Some(40));
    }
}
