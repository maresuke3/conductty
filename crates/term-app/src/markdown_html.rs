//! Drawer 本文プレビュー用の最小 Markdown → HTML 変換（```mermaid ブロックは Mermaid.js で描画）
//!
//! shortcut: 行単位の簡易パーサ（ネストしたリスト・リンク・画像は非対応）。必要になったら pulldown-cmark 等へ移行する。

use term_core::config::ResolvedTheme;

const MERMAID_JS: &str = "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js";

const STYLE: &str = r#"
body { background: var(--bg); color: var(--fg); margin: 0; padding: 16px 20px 40px;
  font: 14px/1.6 -apple-system, BlinkMacSystemFont, "Segoe UI", "Hiragino Sans", "Noto Sans JP", sans-serif; }
.doc-title { color: var(--accent); font-size: 12px; margin-bottom: 8px; }
h1, h2, h3, h4, h5, h6 { margin: 1.2em 0 0.5em; line-height: 1.3; }
h1, h2 { border-bottom: 1px solid var(--border); padding-bottom: 0.25em; }
a { color: var(--accent); }
code { background: var(--card); border-radius: 4px; padding: 0.1em 0.35em;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 0.92em; }
pre { background: var(--card); border: 1px solid var(--border); border-radius: 6px;
  padding: 10px 12px; overflow-x: auto; }
pre code { background: none; padding: 0; }
pre.frontmatter { background: var(--drawer); color: var(--fg); opacity: 0.85; font-size: 12px; }
pre.mermaid { text-align: center; white-space: pre; }
blockquote { margin: 0.5em 0; padding: 0 1em; border-left: 3px solid var(--accent); opacity: 0.85; }
table { border-collapse: collapse; margin: 0.8em 0; display: block; overflow-x: auto; }
th, td { border: 1px solid var(--border); padding: 4px 10px; text-align: left; }
th { background: var(--drawer); }
tr:nth-child(even) td { background: var(--card); }
hr { border: none; border-top: 1px solid var(--border); }
"#;

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// コードフェンス行なら (マーカー, info 文字列) を返す
fn fence(line: &str) -> Option<(&'static str, &str)> {
    let t = line.trim_start();
    ["```", "~~~"]
        .into_iter()
        .find_map(|m| t.strip_prefix(m).map(|info| (m, info.trim())))
}

fn is_mermaid_info(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
}

pub fn markdown_contains_mermaid(md: &str) -> bool {
    md.lines()
        .any(|l| fence(l).is_some_and(|(_, info)| is_mermaid_info(info)))
}

/// `delim` で挟まれた区間を `<tag>` で囲む。対になっていない末尾の区切りはそのまま残す
fn wrap_pairs(s: &str, delim: &str, tag: &str, outer: fn(&str) -> String) -> String {
    let parts: Vec<&str> = s.split(delim).collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i % 2 == 1 && i + 1 < parts.len() {
            out.push_str(&format!("<{tag}>{p}</{tag}>"));
        } else {
            if i % 2 == 1 {
                out.push_str(delim);
            }
            out.push_str(&outer(p));
        }
    }
    out
}

/// インライン要素（`code` と **bold**）。エスケープ後に処理するので生 HTML は通らない
fn inline(text: &str) -> String {
    wrap_pairs(&escape_html(text), "`", "code", |s| {
        wrap_pairs(s, "**", "strong", str::to_string)
    })
}

fn list_item(t: &str) -> Option<(&'static str, &str)> {
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| t.strip_prefix(m)) {
        return Some(("ul", rest));
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    t[digits..]
        .strip_prefix(". ")
        .or_else(|| t[digits..].strip_prefix(") "))
        .map(|rest| ("ol", rest))
}

fn is_hr(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&m| compact.chars().all(|c| c == m))
}

fn table_cells(row: &str) -> Vec<&str> {
    let r = row.trim();
    let r = r.strip_prefix('|').unwrap_or(r);
    let r = r.strip_suffix('|').unwrap_or(r);
    r.split('|').map(str::trim).collect()
}

fn render_table(rows: &[&str], out: &mut String) {
    let is_sep = |r: &str| {
        table_cells(r)
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':')))
    };
    out.push_str("<table>\n");
    let mut body = rows;
    if rows.len() > 1 && is_sep(rows[1]) {
        out.push_str("<thead><tr>");
        for c in table_cells(rows[0]) {
            out.push_str(&format!("<th>{}</th>", inline(c)));
        }
        out.push_str("</tr></thead>\n");
        body = &rows[2..];
    }
    out.push_str("<tbody>\n");
    for r in body {
        out.push_str("<tr>");
        for c in table_cells(r) {
            out.push_str(&format!("<td>{}</td>", inline(c)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody></table>\n");
}

fn css_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn render_markdown_to_html(title: &str, markdown: &str, theme: &ResolvedTheme) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut body = String::new();
    let mut i = 0;

    if lines.first().is_some_and(|l| l.trim_end() == "---")
        && let Some(end) = lines[1..]
            .iter()
            .position(|l| matches!(l.trim_end(), "---" | "..."))
    {
        body.push_str(&format!(
            "<pre class=\"frontmatter\">{}</pre>\n",
            escape_html(&lines[1..1 + end].join("\n"))
        ));
        i = end + 2;
    }

    let mut para: Vec<&str> = Vec::new();
    let mut list: Option<&str> = None;
    let flush = |body: &mut String, para: &mut Vec<&str>, list: &mut Option<&str>| {
        if !para.is_empty() {
            let text: Vec<String> = para.iter().map(|l| inline(l)).collect();
            body.push_str(&format!("<p>{}</p>\n", text.join("\n")));
            para.clear();
        }
        if let Some(tag) = list.take() {
            body.push_str(&format!("</{tag}>\n"));
        }
    };

    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        i += 1;

        if let Some((marker, info)) = fence(line) {
            flush(&mut body, &mut para, &mut list);
            let start = i;
            while i < lines.len() && !lines[i].trim_start().starts_with(marker) {
                i += 1;
            }
            let code = escape_html(&lines[start..i].join("\n"));
            i += 1; // 閉じフェンス
            if is_mermaid_info(info) {
                body.push_str(&format!("<pre class=\"mermaid\">{code}</pre>\n"));
            } else {
                body.push_str(&format!("<pre><code>{code}</code></pre>\n"));
            }
            continue;
        }
        if t.is_empty() {
            flush(&mut body, &mut para, &mut list);
            continue;
        }
        let level = t.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&level) && t[level..].starts_with(' ') {
            flush(&mut body, &mut para, &mut list);
            body.push_str(&format!(
                "<h{level}>{}</h{level}>\n",
                inline(t[level..].trim())
            ));
            continue;
        }
        if t.starts_with('|') {
            flush(&mut body, &mut para, &mut list);
            let start = i - 1;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                i += 1;
            }
            render_table(&lines[start..i], &mut body);
            continue;
        }
        if is_hr(t) {
            flush(&mut body, &mut para, &mut list);
            body.push_str("<hr>\n");
            continue;
        }
        if let Some((tag, text)) = list_item(t) {
            if list != Some(tag) {
                flush(&mut body, &mut para, &mut list);
                body.push_str(&format!("<{tag}>\n"));
                list = Some(tag);
            }
            body.push_str(&format!("<li>{}</li>\n", inline(text)));
            continue;
        }
        if let Some(quote) = t.strip_prefix('>') {
            flush(&mut body, &mut para, &mut list);
            body.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                inline(quote.trim())
            ));
            continue;
        }
        if list.is_some() {
            flush(&mut body, &mut para, &mut list);
        }
        para.push(t);
    }
    flush(&mut body, &mut para, &mut list);

    let mermaid_theme = if crate::is_light_theme(theme) {
        "default"
    } else {
        "dark"
    };
    let border = format!(
        "rgba({}, {}, {}, 0.22)",
        theme.fg[0], theme.fg[1], theme.fg[2]
    );
    let title = escape_html(title);
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
:root {{ --bg: {bg}; --fg: {fg}; --accent: {accent}; --card: {card}; --drawer: {drawer}; --border: {border}; color-scheme: {scheme}; }}
{STYLE}</style>
</head>
<body>
<div class="doc-title">{title}</div>
{body}<script src="{MERMAID_JS}"></script>
<script>if (window.mermaid) {{ mermaid.initialize({{ startOnLoad: true, theme: '{mermaid_theme}' }}); }}</script>
</body>
</html>
"#,
        bg = css_hex(theme.bg),
        fg = css_hex(theme.fg),
        accent = css_hex(theme.accent),
        card = css_hex(theme.card_bg),
        drawer = css_hex(theme.drawer_bg),
        scheme = if mermaid_theme == "dark" {
            "dark"
        } else {
            "light"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(bg: [u8; 3], fg: [u8; 3]) -> ResolvedTheme {
        ResolvedTheme {
            bg,
            fg,
            accent: [0x12, 0x34, 0x56],
            drawer_bg: [1, 2, 3],
            card_bg: [4, 5, 6],
            cursor: fg,
        }
    }

    fn dark() -> ResolvedTheme {
        theme([0x10, 0x10, 0x10], [0xee, 0xee, 0xee])
    }

    #[test]
    fn detects_mermaid_fences() {
        assert!(markdown_contains_mermaid(
            "# t\n\n```mermaid\ngraph TD\n```\n"
        ));
        assert!(markdown_contains_mermaid("  ~~~ Mermaid\nA-->B\n~~~"));
        assert!(!markdown_contains_mermaid("```rust\nfn main() {}\n```"));
        assert!(!markdown_contains_mermaid(
            "mermaid in prose, `mermaid` inline"
        ));
        assert!(!markdown_contains_mermaid(""));
    }

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape_html(r#"<a href="x">&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn mermaid_block_is_escaped_pre() {
        let md = "```mermaid\ngraph TD\n  A-->B[\"<b>x</b>\"]\n```\n";
        let html = render_markdown_to_html("T", md, &dark());
        assert!(html.contains(
            "<pre class=\"mermaid\">graph TD\n  A--&gt;B[&quot;&lt;b&gt;x&lt;/b&gt;&quot;]</pre>"
        ));
        assert!(html.contains(MERMAID_JS));
        assert!(html.contains("theme: 'dark'"));
        assert!(!html.contains("<b>x</b>"));
    }

    #[test]
    fn mermaid_theme_follows_light_dark() {
        let light = theme([0xfa, 0xfa, 0xfa], [0x20, 0x20, 0x20]);
        let html = render_markdown_to_html("T", "x", &light);
        assert!(html.contains("theme: 'default'"));
        assert!(html.contains("--bg: #fafafa"));
        assert!(html.contains("--card: #040506"));
    }

    #[test]
    fn renders_basic_blocks() {
        let md = "---\ntitle: <Doc>\n---\n# Head\n## Sub `c`\n### Third\n\n\
                  - one\n- **two**\n\n1. first\n2. second\n\n\
                  | A | B |\n|---|:-:|\n| 1 | <2> |\n\n\
                  ```rust\nlet x = a < b;\n```\ntext <script>alert(1)</script>\n";
        let html = render_markdown_to_html("<T>", md, &dark());
        assert!(html.contains("<pre class=\"frontmatter\">title: &lt;Doc&gt;</pre>"));
        assert!(html.contains("<h1>Head</h1>"));
        assert!(html.contains("<h2>Sub <code>c</code></h2>"));
        assert!(html.contains("<h3>Third</h3>"));
        assert!(html.contains("<ul>\n<li>one</li>\n<li><strong>two</strong></li>\n</ul>"));
        assert!(html.contains("<ol>\n<li>first</li>\n<li>second</li>\n</ol>"));
        assert!(html.contains("<thead><tr><th>A</th><th>B</th></tr></thead>"));
        assert!(html.contains("<tr><td>1</td><td>&lt;2&gt;</td></tr>"));
        assert!(html.contains("<pre><code>let x = a &lt; b;</code></pre>"));
        assert!(html.contains("<p>text &lt;script&gt;alert(1)&lt;/script&gt;</p>"));
        assert!(html.contains("<title>&lt;T&gt;</title>"));
        assert!(!html.contains("<script>alert"));
    }

    #[test]
    fn unclosed_inline_markers_stay_literal() {
        assert_eq!(inline("a `b"), "a `b");
        assert_eq!(inline("**x** and ** y"), "<strong>x</strong> and ** y");
    }
}
