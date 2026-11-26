//! `conductty theme` の TUI テーマピッカー（依存クレートなし、stty + ANSI エスケープのみ）

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

use term_core::config::{ResolvedTheme, THEME_PRESETS, resolve_theme_preset};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerKey {
    Next,
    Prev,
    Accept,
    Cancel,
    Other,
}

/// 1 回の read で届いたバイト列をキー列に分解する（キーリピートで複数キーがまとめて届くため）
// shortcut: ESC シーケンスが read 境界で分割されると Esc 扱いになる。SSH 越しで問題になれば待ち合わせを入れる
fn parse_keys(mut buf: &[u8]) -> Vec<PickerKey> {
    let mut keys = Vec::new();
    while let Some(&b) = buf.first() {
        let (key, len) = match buf {
            [0x1b, b'[' | b'O', b'A', ..] => (PickerKey::Prev, 3),
            [0x1b, b'[' | b'O', b'B', ..] => (PickerKey::Next, 3),
            [0x1b, b'[' | b'O', _, ..] => (PickerKey::Other, 3),
            _ => {
                let key = match b {
                    b'j' | 0x0e | 0x0a => PickerKey::Next, // j / Ctrl+n / Ctrl+j
                    b'k' | 0x10 | 0x0b => PickerKey::Prev, // k / Ctrl+p / Ctrl+k
                    b'\r' => PickerKey::Accept,
                    0x1b | b'q' | 0x03 => PickerKey::Cancel, // Esc / q / Ctrl+c
                    _ => PickerKey::Other,
                };
                (key, 1)
            }
        };
        keys.push(key);
        buf = &buf[len..];
    }
    keys
}

fn color_chips(t: &ResolvedTheme) -> String {
    let mut s: String = [t.bg, t.fg, t.accent, t.cursor]
        .iter()
        .map(|[r, g, b]| format!("\x1b[48;2;{r};{g};{b}m  "))
        .collect();
    s.push_str("\x1b[0m");
    s
}

fn row(name: &str, cursor: bool, current: bool) -> String {
    let chips = resolve_theme_preset(name)
        .map(|(_, t)| color_chips(&t))
        .unwrap_or_default();
    let cursor = if cursor { '>' } else { ' ' };
    let mark = if current { '*' } else { ' ' };
    format!("{cursor} {mark} {name:<18} {chips}")
}

pub fn print_list(current: &str) {
    let current_canonical = resolve_theme_preset(current).map(|(n, _)| n);
    println!("Available themes (current: {current}):");
    for name in THEME_PRESETS {
        println!("{}", row(name, false, current_canonical == Some(*name)));
    }
}

fn stty(args: &[&str]) -> io::Result<()> {
    let status = Command::new("stty").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("stty {} failed", args.join(" "))))
    }
}

/// 端末を 1 文字入力 + 代替スクリーンにし、Drop で必ず元に戻す
struct TermGuard {
    saved: String,
}

impl TermGuard {
    fn enter() -> io::Result<Self> {
        // output() は既定で stdin を継承しないため明示する（stty は stdin の端末を操作する）
        let out = Command::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()?;
        if !out.status.success() {
            return Err(io::Error::other("stty -g failed"));
        }
        let guard = Self {
            saved: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        };
        // -isig: Ctrl+c を SIGINT でなくキーとして受けて復元処理を通す / -icrnl: Enter(\r) と Ctrl+j(\n) を区別する
        stty(&[
            "-echo", "-icanon", "-isig", "-icrnl", "min", "1", "time", "0",
        ])?;
        let mut out = io::stdout();
        out.write_all(b"\x1b[?1049h\x1b[?25l")?;
        out.flush()?;
        Ok(guard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = stty(&[&self.saved]);
    }
}

fn draw(idx: usize, current: Option<&str>) -> io::Result<()> {
    let mut s = String::from(
        "\x1b[HSelect theme  (j/k ↑/↓: preview, Enter: save, Esc/q: cancel)\x1b[K\r\n\x1b[K\r\n",
    );
    for (i, name) in THEME_PRESETS.iter().enumerate() {
        s.push_str(&row(name, i == idx, current == Some(*name)));
        s.push_str("\x1b[K\r\n");
    }
    s.push_str("\x1b[J");
    let mut out = io::stdout();
    out.write_all(s.as_bytes())?;
    out.flush()
}

/// 選択されたテーマ名を返す。キャンセル時は None（元テーマへの復元は呼び出し側で行う）
pub fn run(current: &str, preview: &dyn Fn(&str)) -> io::Result<Option<&'static str>> {
    let _guard = TermGuard::enter()?;
    let current_canonical = resolve_theme_preset(current).map(|(n, _)| n);
    let len = THEME_PRESETS.len();
    let mut idx = THEME_PRESETS
        .iter()
        .position(|n| current_canonical == Some(*n))
        .unwrap_or(0);
    let mut stdin = io::stdin().lock();
    let mut buf = [0u8; 64];
    loop {
        draw(idx, current_canonical)?;
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        let before = idx;
        for key in parse_keys(&buf[..n]) {
            match key {
                PickerKey::Next => idx = (idx + 1) % len,
                PickerKey::Prev => idx = (idx + len - 1) % len,
                PickerKey::Accept => return Ok(Some(THEME_PRESETS[idx])),
                PickerKey::Cancel => return Ok(None),
                PickerKey::Other => {}
            }
        }
        if idx != before {
            preview(THEME_PRESETS[idx]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PickerKey::*;
    use super::*;

    #[test]
    fn parse_keys_maps_navigation_accept_and_cancel() {
        assert_eq!(parse_keys(b"\x1b[A"), vec![Prev]);
        assert_eq!(parse_keys(b"\x1b[B"), vec![Next]);
        assert_eq!(parse_keys(b"\x1bOB"), vec![Next]);
        assert_eq!(parse_keys(b"j"), vec![Next]);
        assert_eq!(parse_keys(b"k"), vec![Prev]);
        assert_eq!(
            parse_keys(&[0x0e, 0x0a, 0x10, 0x0b]),
            vec![Next, Next, Prev, Prev]
        );
        assert_eq!(parse_keys(b"\r"), vec![Accept]);
        assert_eq!(parse_keys(b"\x1b"), vec![Cancel]);
        assert_eq!(parse_keys(b"q"), vec![Cancel]);
        assert_eq!(parse_keys(&[0x03]), vec![Cancel]);
    }

    #[test]
    fn parse_keys_handles_batched_and_unknown_input() {
        assert_eq!(parse_keys(b"jj\x1b[Ak"), vec![Next, Next, Prev, Prev]);
        assert_eq!(parse_keys(b"\x1b[Cx"), vec![Other, Other]);
        assert!(parse_keys(b"").is_empty());
    }

    #[test]
    fn color_chips_render_truecolor_blocks() {
        let (_, t) = resolve_theme_preset("Solarized Dark").unwrap();
        let chips = color_chips(&t);
        assert!(chips.starts_with("\x1b[48;2;0;43;54m  ")); // bg #002b36
        assert!(chips.contains("\x1b[48;2;131;148;150m  ")); // fg #839496
        assert!(chips.contains("\x1b[48;2;38;139;210m  ")); // accent #268bd2
        assert!(chips.ends_with("\x1b[0m"));
        assert_eq!(chips.matches("48;2;").count(), 4);
    }

    #[test]
    fn row_shows_cursor_and_current_marks() {
        assert!(row("Dracula", true, true).starts_with("> * Dracula "));
        assert!(row("Dracula", false, false).starts_with("    Dracula "));
        assert!(row("Dracula", false, true).contains("\x1b[48;2;"));
    }
}
