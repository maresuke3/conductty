use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

#[derive(Debug, Clone)]
pub struct KittyImage {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct KittyPlacement {
    pub image_id: u32,
    pub col: usize,
    pub row: usize,
    pub cols: Option<usize>,
    pub rows: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApcScanState {
    Normal,
    EscSeen,
    InApcStart,
    InKittyApc,
    InKittyEsc,
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
        let mut state = self.scan_state.unwrap_or(ApcScanState::Normal);

        for &b in bytes {
            match state {
                ApcScanState::Normal => {
                    if b == 0x1b {
                        state = ApcScanState::EscSeen;
                    } else {
                        normal_out.push(b);
                    }
                }
                ApcScanState::EscSeen => {
                    if b == b'_' {
                        state = ApcScanState::InApcStart;
                    } else {
                        normal_out.push(0x1b);
                        normal_out.push(b);
                        state = ApcScanState::Normal;
                    }
                }
                ApcScanState::InApcStart => {
                    if b == b'G' {
                        self.apc_buf.clear();
                        state = ApcScanState::InKittyApc;
                    } else {
                        normal_out.extend_from_slice(&[0x1b, b'_', b]);
                        state = ApcScanState::Normal;
                    }
                }
                ApcScanState::InKittyApc => {
                    if b == 0x1b {
                        state = ApcScanState::InKittyEsc;
                    } else {
                        self.apc_buf.push(b);
                    }
                }
                ApcScanState::InKittyEsc => {
                    if b == b'\\' {
                        let cmd = std::mem::take(&mut self.apc_buf);
                        self.handle_kitty_command(&cmd, cursor_col, cursor_row, replies);
                        state = ApcScanState::Normal;
                    } else {
                        self.apc_buf.push(0x1b);
                        self.apc_buf.push(b);
                        state = ApcScanState::InKittyApc;
                    }
                }
            }
        }

        self.scan_state = Some(state);
        normal_out
    }

    fn handle_kitty_command(
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
        if self.chunk_buf.is_empty() {
            self.chunk_params = params;
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
                    }
                }
                "i" | "I" => {
                    if let Some(id) = eff_params.get("i").and_then(|s| s.parse::<u32>().ok()) {
                        self.placements.retain(|p| p.image_id != id);
                        if delete_mode == "I" {
                            self.images.remove(&id);
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
                self.next_auto_id += 1;
                id
            });

        if action == "t" || action == "T" {
            if let Some(img) = decode_kitty_image(id, &eff_params, &full_payload) {
                self.images.insert(id, img);
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
            self.placements.push(KittyPlacement {
                image_id: id,
                col: cursor_col,
                row: cursor_row,
                cols,
                rows,
            });
        }

        if quiet == 0 && eff_params.contains_key("i") {
            replies.push(format!("\x1b_Gi={id};OK\x1b\\").into_bytes());
        }
    }
}

fn decode_kitty_image(
    id: u32,
    params: &HashMap<String, String>,
    b64_payload: &[u8],
) -> Option<KittyImage> {
    let raw = BASE64.decode(b64_payload).ok()?;
    let fmt = params.get("f").map(String::as_str).unwrap_or("32");

    match fmt {
        "100" => {
            // PNG
            let img = image::load_from_memory(&raw).ok()?.to_rgba8();
            let (width, height) = img.dimensions();
            Some(KittyImage {
                id,
                width,
                height,
                rgba: img.into_raw(),
            })
        }
        "32" => {
            let width = params.get("s").and_then(|s| s.parse::<u32>().ok())?;
            let height = params.get("v").and_then(|s| s.parse::<u32>().ok())?;
            Some(KittyImage {
                id,
                width,
                height,
                rgba: raw,
            })
        }
        "24" => {
            let width = params.get("s").and_then(|s| s.parse::<u32>().ok())?;
            let height = params.get("v").and_then(|s| s.parse::<u32>().ok())?;
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for chunk in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            Some(KittyImage {
                id,
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
}
