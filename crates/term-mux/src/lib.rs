use serde::{Deserialize, Serialize};
use term_core::grid::GridSnapshot;
use term_core::layout::{PaneId, SplitDirection, WorkspaceManager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub const DEFAULT_IPC_ADDR: &str = "127.0.0.1:47321";
pub const DEFAULT_MUX_SERVER_ADDR: &str = "127.0.0.1:47322";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: PaneId,
    pub workspace: String,
    pub tab_index: usize,
    pub focused: bool,
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
    ToggleZoom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuxResponse {
    Pong,
    Panes {
        panes: Vec<PaneInfo>,
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
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("IPC 接続失敗 ({addr}): {e}"))?;
    let (reader, mut writer) = stream.into_split();

    let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("IPC 送信失敗: {e}"))?;

    let mut buf_reader = BufReader::new(reader);
    let mut resp_line = String::new();
    buf_reader
        .read_line(&mut resp_line)
        .await
        .map_err(|e| format!("IPC 受信失敗: {e}"))?;

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
}
