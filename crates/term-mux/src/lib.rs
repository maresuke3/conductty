use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use term_core::grid::GridSnapshot;
use term_core::layout::{NavDirection, PaneId, SplitDirection, WorkspaceManager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub const DEFAULT_IPC_ADDR: &str = "127.0.0.1:47321";
pub const DEFAULT_MUX_SERVER_ADDR: &str = "127.0.0.1:47322";
pub const MAX_IPC_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_IPC_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEnvelope {
    pub token: String,
    #[serde(flatten)]
    pub req: MuxRequest,
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn ipc_token_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    home.map(|h| h.join(".config").join("conductty").join(".ipc_token"))
}

fn generate_random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("failed to generate random IPC token: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn read_verified_token_file(path: &Path) -> Result<Option<String>, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("failed to stat IPC token: {e}")),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(format!(
            "IPC token path is not a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read IPC token: {e}"))?;
    let trimmed = content.trim();
    if trimmed.len() >= 32 {
        Ok(Some(trimmed.to_string()))
    } else {
        Ok(None)
    }
}

pub fn read_or_create_ipc_token() -> Result<String, String> {
    let path =
        ipc_token_path().ok_or_else(|| "cannot resolve home dir for IPC token".to_string())?;
    read_or_create_ipc_token_at(&path)
}

pub fn read_or_create_ipc_token_at(path: &Path) -> Result<String, String> {
    if let Some(tok) = read_verified_token_file(path)? {
        return Ok(tok);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create token dir: {e}"))?;
    }
    let token = generate_random_token()?;

    let create_res = {
        let mut opts = std::fs::OpenOptions::new();
        opts.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(path)
    };

    match create_res {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(token.as_bytes())
                .map_err(|e| format!("failed to write IPC token: {e}"))?;
            f.sync_all()
                .map_err(|e| format!("failed to sync IPC token: {e}"))?;
            Ok(token)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // 別プロセスが同時作成中の可能性に備えて短時間リトライして読み直す
            for _ in 0..10 {
                if let Some(tok) = read_verified_token_file(path)? {
                    return Ok(tok);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(format!(
                "IPC token file exists but remained empty: {}",
                path.display()
            ))
        }
        Err(e) => Err(format!("failed to create IPC token: {e}")),
    }
}

pub async fn read_bounded_line<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let mut buf = Vec::new();
    let mut total = 0usize;
    loop {
        let avail = reader
            .fill_buf()
            .await
            .map_err(|e| format!("IPC 受信失敗: {e}"))?;
        if avail.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(pos) = avail.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            total = total.saturating_add(take);
            if total > max_bytes {
                return Err(format!("IPC line exceeds max {max_bytes} bytes"));
            }
            buf.extend_from_slice(&avail[..take]);
            reader.consume(take);
            break;
        } else {
            let len = avail.len();
            total = total.saturating_add(len);
            if total > max_bytes {
                return Err(format!("IPC line exceeds max {max_bytes} bytes"));
            }
            buf.extend_from_slice(avail);
            reader.consume(len);
        }
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| format!("IPC UTF-8 error: {e}"))
}

pub fn parse_authenticated_request(line: &str, expected_token: &str) -> Result<MuxRequest, String> {
    let env: AuthEnvelope =
        serde_json::from_str(line).map_err(|e| format!("invalid request: {e}"))?;
    if !constant_time_eq(&env.token, expected_token) {
        return Err("unauthorized IPC request".to_string());
    }
    Ok(env.req)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: PaneId,
    pub workspace: String,
    pub tab_index: usize,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpaceInfo {
    pub index: usize,
    pub name: String,
    pub tabs: usize,
    pub panes: usize,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentInfo {
    pub pane_id: PaneId,
    pub workspace: String,
    pub tab_index: usize,
    pub kind: String,
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpawnKind {
    SplitRight,
    SplitDown,
    NewTab,
    NewWorkspace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuxRequest {
    Ping,
    ListPanes,
    ListSpaces,
    ListAgents,
    CapturePane {
        pane_id: PaneId,
    },
    SendKeys {
        pane_id: Option<PaneId>,
        text: String,
        broadcast: bool,
    },
    Attach,
    PollSnapshots,
    WriteBytes {
        pane_id: PaneId,
        bytes: Vec<u8>,
    },
    ResizePane {
        pane_id: PaneId,
        cols: usize,
        rows: usize,
    },
    SpawnPane {
        kind: SpawnKind,
    },
    SwitchWorkspace {
        idx: usize,
    },
    SwitchTab {
        idx: usize,
    },
    FocusPane {
        pane_id: PaneId,
    },
    RenameWorkspace {
        idx: usize,
        name: String,
    },
    CloseWorkspace {
        idx: usize,
    },
    CloseTab {
        idx: usize,
    },
    ClosePane {
        pane_id: PaneId,
    },
    ResizeSplit {
        dir: NavDirection,
    },
    ToggleZoom,
    SetTheme {
        theme: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuxResponse {
    Pong,
    Panes {
        panes: Vec<PaneInfo>,
    },
    Spaces {
        spaces: Vec<SpaceInfo>,
    },
    Agents {
        agents: Vec<AgentInfo>,
    },
    Captured {
        pane_id: PaneId,
        text: String,
    },
    Ok {
        delivered_to: usize,
    },
    State {
        manager: WorkspaceManager,
        snapshots: Vec<(PaneId, GridSnapshot)>,
    },
    Error {
        message: String,
    },
}

/// CLI / GUI から稼働中の conductty / conductty-server へ JSON-lines リクエストを送信する。
pub async fn send_request(addr: &str, req: &MuxRequest) -> Result<MuxResponse, String> {
    let token = read_or_create_ipc_token()?;
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("IPC 接続失敗 ({addr}): {e}"))?;
    let (reader, mut writer) = stream.into_split();

    let env = AuthEnvelope {
        token,
        req: req.clone(),
    };
    let mut line = serde_json::to_string(&env).map_err(|e| e.to_string())?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("IPC 送信失敗: {e}"))?;

    let mut buf_reader = BufReader::new(reader);
    let resp_line = read_bounded_line(&mut buf_reader, MAX_IPC_RESPONSE_BYTES)
        .await?
        .unwrap_or_default();

    serde_json::from_str(resp_line.trim()).map_err(|e| format!("IPC 応答パース失敗: {e}"))
}

pub fn split_dir_for_spawn(kind: SpawnKind) -> Option<SplitDirection> {
    match kind {
        SpawnKind::SplitRight => Some(SplitDirection::Horizontal),
        SpawnKind::SplitDown => Some(SplitDirection::Vertical),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_mux_protocol_json() {
        let req = MuxRequest::SendKeys {
            pane_id: Some(2),
            text: "ls -la\n".to_string(),
            broadcast: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: MuxRequest = serde_json::from_str(&json).unwrap();
        match decoded {
            MuxRequest::SendKeys {
                pane_id,
                text,
                broadcast,
            } => {
                assert_eq!(pane_id, Some(2));
                assert_eq!(text, "ls -la\n");
                assert!(!broadcast);
            }
            _ => panic!("unexpected variant"),
        }

        let resp = MuxResponse::State {
            manager: WorkspaceManager::new(1),
            snapshots: vec![(1, term_core::grid::TerminalGrid::new(4, 2).snapshot())],
        };
        let resp_json = serde_json::to_string(&resp).unwrap();
        let decoded_resp: MuxResponse = serde_json::from_str(&resp_json).unwrap();
        match decoded_resp {
            MuxResponse::State { snapshots, .. } => {
                assert_eq!(snapshots.len(), 1);
                assert_eq!(snapshots[0].0, 1);
            }
            _ => panic!("unexpected response variant"),
        }
    }

    #[test]
    fn auth_envelope_rejects_unauthenticated_and_http_smuggling() {
        let req = MuxRequest::Ping;
        let env = AuthEnvelope {
            token: "secret-token-123".to_string(),
            req,
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(matches!(
            parse_authenticated_request(&json, "secret-token-123"),
            Ok(MuxRequest::Ping)
        ));

        // 誤ったトークンやトークン無しリクエストを拒否する
        assert!(parse_authenticated_request(&json, "wrong-token").is_err());
        let unauth_json = serde_json::to_string(&MuxRequest::Ping).unwrap();
        assert!(parse_authenticated_request(&unauth_json, "secret-token-123").is_err());

        // HTTP ヘッダ行など非 JSON 行を拒否する
        assert!(parse_authenticated_request("POST / HTTP/1.1", "secret-token-123").is_err());
    }

    #[tokio::test]
    async fn read_bounded_line_rejects_oversized_input() {
        let oversized = vec![b'a'; 128];
        let mut reader = BufReader::new(std::io::Cursor::new(oversized));
        let err = read_bounded_line(&mut reader, 64).await.unwrap_err();
        assert!(err.contains("exceeds max"));

        let valid = b"{\"type\":\"ping\"}\n".to_vec();
        let mut reader2 = BufReader::new(std::io::Cursor::new(valid));
        let line = read_bounded_line(&mut reader2, 64).await.unwrap().unwrap();
        assert_eq!(line.trim(), "{\"type\":\"ping\"}");
    }

    #[test]
    fn ipc_token_creation_is_exclusive_and_reused() {
        let dir = std::env::temp_dir().join("conductty_test_ipc_token");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let token_path = dir.join(".ipc_token");

        let t1 = read_or_create_ipc_token_at(&token_path).unwrap();
        assert_eq!(t1.len(), 64);
        let t2 = read_or_create_ipc_token_at(&token_path).unwrap();
        assert_eq!(t1, t2);

        #[cfg(unix)]
        {
            let link_path = dir.join(".symlink_token");
            std::os::unix::fs::symlink(&token_path, &link_path).unwrap();
            assert!(read_or_create_ipc_token_at(&link_path).is_err());
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
