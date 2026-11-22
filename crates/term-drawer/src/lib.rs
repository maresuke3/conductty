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
            config,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub badge: Option<String>,
    pub url: String,
    pub send_text: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerSection {
    pub id: String,
    pub title: String,
    pub accent: Option<String>,
    pub items: Vec<DrawerItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DrawerPluginView {
    pub title: String,
    pub sections: Vec<DrawerSection>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DrawerSnapshot {
    pub plugin_id: String,
    pub title: String,
    pub sections: Vec<DrawerSection>,
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

/// 実行ファイルと同じディレクトリにプラグインバイナリが存在すればそのパスを返す。
pub fn resolve_plugin_command(command: &str) -> PathBuf {
    let cmd_path = Path::new(command);
    if cmd_path.is_absolute() || command.contains('/') || command.contains('\\') {
        return cmd_path.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return candidate;
        }
        #[cfg(target_os = "windows")]
        if !command.ends_with(".exe") {
            let exe_candidate = dir.join(format!("{command}.exe"));
            if exe_candidate.is_file() {
                return exe_candidate;
            }
        }
    }
    cmd_path.to_path_buf()
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
        "PATH=\"$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.local/share/mise/shims:/home/linuxbrew/.linuxbrew/bin:$PATH\" exec {}",
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

/// 外部プラグインバイナリを stdio (JSON Lines) で呼び出し、右 Drawer 用のスナップショットを取得する。
pub async fn fetch_plugin_snapshot(
    plugin_id: &str,
    fallback_title: &str,
    command: &str,
    args: &[String],
    config: serde_json::Value,
) -> DrawerSnapshot {
    let req = PluginRequest::fetch(config);
    let mut req_bytes = serde_json::to_vec(&req).unwrap_or_default();
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
                is_loading: false,
                last_error: view.error,
            },
            Err(parse_err) => DrawerSnapshot {
                plugin_id: plugin_id.to_string(),
                title: fallback_title.to_string(),
                sections: Vec::new(),
                is_loading: false,
                last_error: Some(parse_err),
            },
        },
        Err(err) => DrawerSnapshot {
            plugin_id: plugin_id.to_string(),
            title: fallback_title.to_string(),
            sections: Vec::new(),
            is_loading: false,
            last_error: Some(err),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sanitizes_plugin_view_json() {
        let raw = r##"{
            "title": "GitHub",
            "sections": [
                {
                    "id": "assigned_prs",
                    "title": "Assigned PRs",
                    "accent": "#7aa2f7",
                    "items": [
                        {
                            "id": "maresuke3/conductty#42",
                            "title": "Add right drawer plugin host",
                            "subtitle": "maresuke3/conductty #42",
                            "badge": "PR",
                            "url": "https://github.com/maresuke3/conductty/pull/42"
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
        assert_eq!(view.title, "GitHub");
        assert_eq!(view.sections.len(), 1);
        assert_eq!(view.sections[0].items.len(), 2);
        assert_eq!(
            view.sections[0].items[0].url,
            "https://github.com/maresuke3/conductty/pull/42"
        );
        assert_eq!(view.sections[0].items[1].url, "");
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
            send_text: None,
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
