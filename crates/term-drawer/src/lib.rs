use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;
const MAX_PLUGIN_STDOUT_BYTES: usize = 1024 * 1024;
const PLUGIN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginRequest {
    pub protocol_version: u32,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
}

impl PluginRequest {
    pub fn fetch(config: serde_json::Value) -> Self {
        Self {
            protocol_version: PLUGIN_PROTOCOL_VERSION,
            method: "fetch".to_string(),
            action_id: None,
            item_id: None,
            input: None,
            config,
        }
    }

    pub fn action(
        action_id: impl Into<String>,
        item_id: Option<String>,
        input: Option<String>,
        config: serde_json::Value,
    ) -> Self {
        Self {
            protocol_version: PLUGIN_PROTOCOL_VERSION,
            method: "action".to_string(),
            action_id: Some(action_id.into()),
            item_id,
            input,
            config,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerCustomAction {
    pub id: String,
    pub label: String,
    pub key: Option<String>,
    pub needs_input: bool,
    pub input_placeholder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DrawerEffect {
    OpenUrl { url: String },
    PreviewUrl { url: String },
    SendToPane { text: String },
    CopyToClipboard { text: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub badge: Option<String>,
    pub url: String,
    pub body: Option<String>,
    pub send_text: Option<String>,
    pub actions: Vec<DrawerCustomAction>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerSection {
    pub id: String,
    pub title: String,
    pub accent: Option<String>,
    pub actions: Vec<DrawerCustomAction>,
    pub items: Vec<DrawerItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerPluginView {
    pub title: String,
    pub sections: Vec<DrawerSection>,
    pub actions: Vec<DrawerCustomAction>,
    pub effects: Vec<DrawerEffect>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DrawerSnapshot {
    pub plugin_id: String,
    pub title: String,
    pub sections: Vec<DrawerSection>,
    pub actions: Vec<DrawerCustomAction>,
    pub effects: Vec<DrawerEffect>,
    pub is_loading: bool,
    pub last_error: Option<String>,
}

impl DrawerSnapshot {
    pub fn ordered_items(&self) -> Vec<DrawerItem> {
        let total = self.sections.iter().map(|s| s.items.len()).sum();
        let mut out = Vec::with_capacity(total);
        for sec in &self.sections {
            out.extend(sec.items.iter().cloned());
        }
        out
    }

    pub fn total_items(&self) -> usize {
        self.sections.iter().map(|s| s.items.len()).sum()
    }
}

pub fn validate_http_url(url: &str) -> Result<&str, String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("URL must use http:// or https://".to_string());
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
    {
        return Err("URL contains invalid characters".to_string());
    }
    let after_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or("");
    if after_scheme.is_empty() || after_scheme.starts_with('/') {
        return Err("URL host is empty".to_string());
    }
    Ok(trimmed)
}

pub fn sanitize_terminal_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\r' | '\n' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub fn format_item_for_pane(item: &DrawerItem) -> String {
    if let Some(custom) = &item.send_text
        && !custom.trim().is_empty()
    {
        return sanitize_terminal_text(custom);
    }
    let sub = sanitize_terminal_text(&item.subtitle);
    let title = sanitize_terminal_text(&item.title);
    let url = sanitize_terminal_text(&item.url);
    if !sub.is_empty() && !url.is_empty() {
        format!("[{sub}] {title} ({url})")
    } else if !url.is_empty() {
        format!("{title} ({url})")
    } else if !sub.is_empty() {
        format!("[{sub}] {title}")
    } else {
        title
    }
}

pub fn sanitize_plugin_view(mut view: DrawerPluginView) -> DrawerPluginView {
    for section in &mut view.sections {
        for item in &mut section.items {
            item.url = validate_http_url(&item.url)
                .map(str::to_string)
                .unwrap_or_default();
            if let Some(send) = &item.send_text {
                item.send_text = Some(sanitize_terminal_text(send));
            }
        }
    }
    view.effects.retain_mut(|eff| match eff {
        DrawerEffect::OpenUrl { url } | DrawerEffect::PreviewUrl { url } => {
            if let Ok(valid) = validate_http_url(url) {
                *url = valid.to_string();
                true
            } else {
                false
            }
        }
        DrawerEffect::SendToPane { text } => {
            *text = sanitize_terminal_text(text);
            !text.is_empty()
        }
        DrawerEffect::CopyToClipboard { text } => !text.is_empty(),
    });
    view
}

pub fn parse_plugin_view_json(json_str: &str) -> Result<DrawerPluginView, String> {
    let raw: DrawerPluginView = serde_json::from_str(json_str.trim())
        .map_err(|e| format!("plugin JSON parse error: {e}"))?;
    Ok(sanitize_plugin_view(raw))
}

fn apply_no_window(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cmd;
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    }
}

pub fn expand_tilde(path_str: &str) -> PathBuf {
    if let Some(rest) = path_str
        .strip_prefix("~/")
        .or_else(|| path_str.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    if path_str == "~"
        && let Some(home) = home_dir()
    {
        return home;
    }
    PathBuf::from(path_str)
}

fn check_dir_for_binary(dir: &Path, command: &str) -> Option<PathBuf> {
    let candidate = dir.join(command);
    if candidate.is_file() {
        return Some(candidate);
    }
    #[cfg(target_os = "windows")]
    if !command.ends_with(".exe") {
        let exe_candidate = dir.join(format!("{command}.exe"));
        if exe_candidate.is_file() {
            return Some(exe_candidate);
        }
    }
    None
}

/// プラグインコマンドを解決する。
/// 1. `~/` を展開し、パス区切りを含む場合はそのパスを返す
/// 2. `conductty` 実行ファイルと同じディレクトリ
/// 3. `~/.config/conductty/plugins/`（サードパーティ・自作プラグインの標準配置先）
/// 4. `~/.local/bin/` および `~/.cargo/bin/`
/// 5. OS の `PATH`
pub fn resolve_plugin_command(command: &str) -> PathBuf {
    let expanded = expand_tilde(command);
    if expanded.is_absolute() || command.contains('/') || command.contains('\\') {
        return expanded;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && let Some(found) = check_dir_for_binary(dir, command)
    {
        return found;
    }
    if let Some(home) = home_dir() {
        let search_dirs = [
            home.join(".config").join("conductty").join("plugins"),
            home.join(".conductty").join("plugins"),
            home.join(".local").join("bin"),
            home.join(".cargo").join("bin"),
        ];
        for dir in &search_dirs {
            if let Some(found) = check_dir_for_binary(dir, command) {
                return found;
            }
        }
    }
    expanded
}

/// GUI 起動時（Finder / Dock 等）は PATH が最小限になるため、標準的なツール配置先を後ろに補完する。
#[cfg(unix)]
fn enriched_path(current: Option<std::ffi::OsString>, home: Option<&Path>) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = current
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let system = [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/home/linuxbrew/.linuxbrew/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]
    .map(PathBuf::from);
    let user = home
        .into_iter()
        .flat_map(|h| [".local/share/mise/shims", ".local/bin", ".cargo/bin"].map(|d| h.join(d)));
    for dir in system.into_iter().chain(user) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| current.unwrap_or_default())
}

async fn run_plugin_process(
    command: &str,
    args: &[String],
    request_line: &[u8],
) -> Result<String, String> {
    let resolved = resolve_plugin_command(command);
    let mut cmd = Command::new(&resolved);
    cmd.args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.env(
        "PATH",
        enriched_path(std::env::var_os("PATH"), home_dir().as_deref()),
    );
    apply_no_window(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if cfg!(target_os = "windows") && e.kind() == std::io::ErrorKind::NotFound => {
            return run_plugin_via_wsl(command, args, request_line).await;
        }
        Err(e) => {
            return Err(format!("プラグイン `{command}` の起動に失敗しました: {e}"));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(request_line).await;
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "plugin stdout unavailable".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "plugin stderr unavailable".to_string())?;

    let io_fut = async {
        let mut out_buf = Vec::new();
        let mut err_buf = Vec::new();
        let mut out_take = (&mut stdout).take((MAX_PLUGIN_STDOUT_BYTES + 1) as u64);
        let mut err_take = (&mut stderr).take(64 * 1024);
        let (out_res, err_res, status_res) = tokio::join!(
            out_take.read_to_end(&mut out_buf),
            err_take.read_to_end(&mut err_buf),
            child.wait()
        );
        out_res.map_err(|e| format!("plugin stdout read error: {e}"))?;
        let _ = err_res;
        let status = status_res.map_err(|e| format!("plugin wait error: {e}"))?;
        if out_buf.len() > MAX_PLUGIN_STDOUT_BYTES {
            return Err("plugin output exceeded 1 MiB limit".to_string());
        }
        if !status.success() {
            let err_text = String::from_utf8_lossy(&err_buf).trim().to_string();
            return Err(if err_text.is_empty() {
                format!("プラグイン `{command}` 終了コード: {status}")
            } else {
                err_text
            });
        }
        Ok(String::from_utf8_lossy(&out_buf).into_owned())
    };

    tokio::time::timeout(PLUGIN_TIMEOUT, io_fut)
        .await
        .map_err(|_| format!("プラグイン `{command}` がタイムアウトしました (15s)"))?
}

async fn run_plugin_via_wsl(
    command: &str,
    args: &[String],
    request_line: &[u8],
) -> Result<String, String> {
    let mut all_parts = Vec::with_capacity(args.len() + 1);
    all_parts.push(command.to_string());
    all_parts.extend(args.iter().cloned());
    let escaped: Vec<String> = all_parts
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect();
    let script = format!(
        "PATH=\"$HOME/.config/conductty/plugins:$HOME/.conductty/plugins:$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.local/share/mise/shims:/home/linuxbrew/.linuxbrew/bin:$PATH\" exec {}",
        escaped.join(" ")
    );

    let mut wsl_cmd = Command::new("wsl.exe");
    wsl_cmd
        .args(["sh", "-c", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    apply_no_window(&mut wsl_cmd);

    let mut child = wsl_cmd
        .spawn()
        .map_err(|e| format!("WSL 経由のプラグイン `{command}` 起動失敗: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(request_line).await;
    }

    let output = tokio::time::timeout(PLUGIN_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("WSL プラグイン `{command}` がタイムアウトしました (15s)"))?
        .map_err(|e| format!("WSL プラグイン `{command}` 待機失敗: {e}"))?;

    if output.stdout.len() > MAX_PLUGIN_STDOUT_BYTES {
        return Err("plugin output exceeded 1 MiB limit".to_string());
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("WSL プラグイン `{command}` 終了コード: {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn execute_plugin_request(
    plugin_id: &str,
    fallback_title: &str,
    command: &str,
    args: &[String],
    req: &PluginRequest,
) -> DrawerSnapshot {
    let mut req_bytes = serde_json::to_vec(req).unwrap_or_default();
    req_bytes.push(b'\n');

    match run_plugin_process(command, args, &req_bytes).await {
        Ok(stdout) => match parse_plugin_view_json(&stdout) {
            Ok(view) => DrawerSnapshot {
                plugin_id: plugin_id.to_string(),
                title: if view.title.trim().is_empty() {
                    fallback_title.to_string()
                } else {
                    view.title
                },
                sections: view.sections,
                actions: view.actions,
                effects: view.effects,
                is_loading: false,
                last_error: view.error,
            },
            Err(parse_err) => DrawerSnapshot {
                plugin_id: plugin_id.to_string(),
                title: fallback_title.to_string(),
                sections: Vec::new(),
                actions: Vec::new(),
                effects: Vec::new(),
                is_loading: false,
                last_error: Some(parse_err),
            },
        },
        Err(err) => DrawerSnapshot {
            plugin_id: plugin_id.to_string(),
            title: fallback_title.to_string(),
            sections: Vec::new(),
            actions: Vec::new(),
            effects: Vec::new(),
            is_loading: false,
            last_error: Some(err),
        },
    }
}

/// 外部プラグインバイナリを stdio (JSON Lines) で呼び出し、右 Drawer 用のスナップショットを取得する。
pub async fn fetch_plugin_snapshot(
    plugin_id: &str,
    fallback_title: &str,
    command: &str,
    args: &[String],
    config: serde_json::Value,
) -> DrawerSnapshot {
    let req = PluginRequest::fetch(config);
    execute_plugin_request(plugin_id, fallback_title, command, args, &req).await
}

/// プラグイン定義のカスタムアクション (`method = "action"`) を呼び出し、更新後の View と Effect を返す。
#[allow(clippy::too_many_arguments)]
pub async fn invoke_plugin_action(
    plugin_id: &str,
    fallback_title: &str,
    command: &str,
    args: &[String],
    action_id: &str,
    item_id: Option<String>,
    input: Option<String>,
    config: serde_json::Value,
) -> DrawerSnapshot {
    let req = PluginRequest::action(action_id, item_id, input, config);
    execute_plugin_request(plugin_id, fallback_title, command, args, &req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn enriched_path_keeps_order_adds_standard_dirs_without_duplicates() {
        let out = enriched_path(
            Some("/custom/bin:/opt/homebrew/bin".into()),
            Some(Path::new("/home/u")),
        );
        let dirs: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(dirs[0], Path::new("/custom/bin"));
        assert_eq!(dirs[1], Path::new("/opt/homebrew/bin"));
        assert_eq!(
            dirs.iter()
                .filter(|d| *d == Path::new("/opt/homebrew/bin"))
                .count(),
            1
        );
        for d in [
            "/opt/homebrew/sbin",
            "/usr/local/bin",
            "/usr/bin",
            "/home/u/.local/share/mise/shims",
            "/home/u/.local/bin",
            "/home/u/.cargo/bin",
        ] {
            assert!(dirs.contains(&PathBuf::from(d)), "{d} missing");
        }

        let fallback: Vec<PathBuf> = std::env::split_paths(&enriched_path(None, None)).collect();
        assert!(fallback.contains(&PathBuf::from("/usr/bin")));
    }

    #[test]
    fn parses_and_sanitizes_plugin_view_json_with_body_actions_and_effects() {
        let raw = r##"{
            "title": "Docs",
            "actions": [
                {
                    "id": "new_note",
                    "label": "+ New Note",
                    "needs_input": true,
                    "input_placeholder": "note-title.md"
                }
            ],
            "effects": [
                { "type": "send_to_pane", "text": "nvim note.md\r\n" },
                { "type": "open_url", "url": "javascript:alert(1)" },
                { "type": "open_url", "url": "https://example.com/doc" }
            ],
            "sections": [
                {
                    "id": "notes",
                    "title": "Local Notes",
                    "accent": "#7aa2f7",
                    "items": [
                        {
                            "id": "todo.md",
                            "title": "TODO List",
                            "subtitle": "notes/todo.md",
                            "badge": "MD",
                            "body": "# TODO\n- Item 1",
                            "url": "",
                            "actions": [
                                { "id": "open_editor", "label": "Edit" }
                            ]
                        },
                        {
                            "id": "bad",
                            "title": "Malicious URL",
                            "subtitle": "a/b #1",
                            "url": "javascript:alert(1)"
                        }
                    ]
                }
            ]
        }"##;

        let view = parse_plugin_view_json(raw).unwrap();
        assert_eq!(view.title, "Docs");
        assert_eq!(view.actions.len(), 1);
        assert!(view.actions[0].needs_input);
        assert_eq!(view.effects.len(), 2);
        assert_eq!(
            view.effects[0],
            DrawerEffect::SendToPane {
                text: "nvim note.md  ".to_string()
            }
        );
        assert_eq!(
            view.effects[1],
            DrawerEffect::OpenUrl {
                url: "https://example.com/doc".to_string()
            }
        );
        assert_eq!(
            view.sections[0].items[0].body.as_deref(),
            Some("# TODO\n- Item 1")
        );
        assert_eq!(view.sections[0].items[0].actions.len(), 1);
        assert_eq!(view.sections[0].items[1].url, "");
    }

    #[test]
    fn expands_tilde_paths() {
        let expanded = expand_tilde("~/my-plugins/custom-bin");
        assert!(
            expanded
                .to_string_lossy()
                .ends_with("my-plugins/custom-bin")
        );
        assert!(!expanded.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn sanitizes_pty_text_and_validates_urls() {
        assert!(validate_http_url("https://github.com/a/b").is_ok());
        assert!(validate_http_url("http://localhost:8080/x").is_ok());
        assert!(validate_http_url("javascript:alert(1)").is_err());
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("https://example.com/a\"&calc").is_err());
        assert!(validate_http_url("https://example.com/\nfoo").is_err());

        let item = DrawerItem {
            id: "org/repo#99".to_string(),
            title: "Fix bug\r\nrm -rf /\x1b[31m".to_string(),
            subtitle: "org/repo #99\x07".to_string(),
            badge: Some("PR".to_string()),
            url: "https://github.com/org/repo/pull/99".to_string(),
            body: None,
            send_text: None,
            actions: Vec::new(),
        };
        let formatted = format_item_for_pane(&item);
        assert!(!formatted.contains('\n'));
        assert!(!formatted.contains('\r'));
        assert!(!formatted.contains('\x1b'));
        assert!(!formatted.contains('\x07'));
        assert_eq!(
            formatted,
            "[org/repo #99] Fix bug  rm -rf /[31m (https://github.com/org/repo/pull/99)"
        );
    }
}
