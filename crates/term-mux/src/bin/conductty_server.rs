#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use term_core::agent::{PaneAgentState, detect_pane_agent, snapshot_visible_lines};
use term_core::config::AppConfig;
use term_core::grid::{GridSnapshot, TerminalGrid};
use term_core::layout::{PaneId, WorkspaceManager};
use term_core::pty::PtySession;
use term_mux::{
    AgentInfo, DEFAULT_MUX_SERVER_ADDR, MAX_IPC_REQUEST_BYTES, MuxRequest, MuxResponse, PaneInfo,
    SpaceInfo, SpawnKind, parse_authenticated_request, read_bounded_line, read_or_create_ipc_token,
    split_dir_for_spawn,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

struct ServerPane {
    pty: PtySession,
    grid: Arc<Mutex<TerminalGrid>>,
}

struct ServerState {
    config: AppConfig,
    manager: WorkspaceManager,
    panes: HashMap<PaneId, ServerPane>,
    agent_states: HashMap<PaneId, PaneAgentState>,
    started_at: std::time::Instant,
    next_pane_id: PaneId,
}

impl ServerState {
    fn new(config: AppConfig) -> Self {
        let mut state = Self {
            config,
            manager: WorkspaceManager::new(1),
            panes: HashMap::new(),
            agent_states: HashMap::new(),
            started_at: std::time::Instant::now(),
            next_pane_id: 1,
        };
        if let Some(first_id) = state.spawn_pane() {
            state.manager = WorkspaceManager::new(first_id);
        }
        state
    }

    fn spawn_pane(&mut self) -> Option<PaneId> {
        let id = self.next_pane_id;
        self.next_pane_id += 1;

        let mut pty = match PtySession::spawn(120, 36, &self.config.shell.windows_default_prog) {
            Ok(p) => p,
            Err(err) => {
                eprintln!("[conductty-server] PtySession::spawn failed: {err}");
                return None;
            }
        };
        let (_, dummy_rx) = std::sync::mpsc::channel();
        let rx = std::mem::replace(&mut pty.output_rx, dummy_rx);
        let grid = Arc::new(Mutex::new(TerminalGrid::new(120, 36)));
        let grid_clone = Arc::clone(&grid);

        thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if let Ok(mut g) = grid_clone.lock() {
                    g.feed_bytes(&bytes);
                }
            }
        });

        self.panes.insert(id, ServerPane { pty, grid });
        Some(id)
    }

    fn cleanup_exited_panes(&mut self) {
        let mut exited = Vec::new();
        for (&id, pane) in &self.panes {
            if pane.pty.exit_rx.try_recv().is_ok() {
                exited.push(id);
            }
        }
        for id in exited {
            self.panes.remove(&id);
            self.agent_states.remove(&id);
            if !self.manager.close_pane(id) {
                if let Some(new_id) = self.spawn_pane() {
                    self.manager = WorkspaceManager::new(new_id);
                }
            }
        }
    }

    fn collect_snapshots(&mut self) -> Vec<(PaneId, GridSnapshot)> {
        self.cleanup_exited_panes();
        let mut list = Vec::with_capacity(self.panes.len());
        for (&id, pane) in &mut self.panes {
            if let Ok(mut g) = pane.grid.lock() {
                let replies = g.take_replies();
                for reply in replies {
                    let _ = pane.pty.write_bytes(&reply);
                }
                list.push((id, g.snapshot()));
            }
        }
        list
    }

    fn handle_request(&mut self, req: MuxRequest) -> MuxResponse {
        self.cleanup_exited_panes();
        match req {
            MuxRequest::Ping => MuxResponse::Pong,
            MuxRequest::ListPanes => {
                let focused = self.manager.active_tab().focused_pane;
                let mut panes = Vec::new();
                for ws in &self.manager.workspaces {
                    for (t_idx, tab) in ws.tabs.iter().enumerate() {
                        for pid in tab.all_panes() {
                            panes.push(PaneInfo {
                                pane_id: pid,
                                workspace: ws.name.clone(),
                                tab_index: t_idx + 1,
                                focused: pid == focused,
                            });
                        }
                    }
                }
                MuxResponse::Panes { panes }
            }
            MuxRequest::ListSpaces => {
                let active_ws = self.manager.active_workspace;
                let spaces = self
                    .manager
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, ws)| SpaceInfo {
                        index,
                        name: ws.name.clone(),
                        tabs: ws.tabs.len(),
                        panes: ws.pane_count(),
                        active: index == active_ws,
                    })
                    .collect();
                MuxResponse::Spaces { spaces }
            }
            MuxRequest::ListAgents => {
                let now_ms = self.started_at.elapsed().as_millis() as u64;
                let mut next_states = HashMap::new();
                let mut agents = Vec::new();
                for ws in &self.manager.workspaces {
                    for (t_idx, tab) in ws.tabs.iter().enumerate() {
                        for pid in tab.all_panes() {
                            let Some(pane) = self.panes.get(&pid) else {
                                continue;
                            };
                            let Ok(g) = pane.grid.lock() else {
                                continue;
                            };
                            let (lines, hash) = snapshot_visible_lines(&g.snapshot());
                            let prev = self.agent_states.get(&pid);
                            if let Some(st) = detect_pane_agent(&lines, hash, now_ms, prev) {
                                agents.push(AgentInfo {
                                    pane_id: pid,
                                    workspace: ws.name.clone(),
                                    tab_index: t_idx + 1,
                                    kind: st.kind.label().to_string(),
                                    status: st.status.badge().to_string(),
                                    summary: st.summary.clone(),
                                });
                                next_states.insert(pid, st);
                            }
                        }
                    }
                }
                self.agent_states = next_states;
                MuxResponse::Agents { agents }
            }
            MuxRequest::CapturePane { pane_id } => {
                if let Some(pane) = self.panes.get(&pane_id) {
                    if let Ok(g) = pane.grid.lock() {
                        let (lines, _) = snapshot_visible_lines(&g.snapshot());
                        return MuxResponse::Captured {
                            pane_id,
                            text: lines.join("\n"),
                        };
                    }
                }
                MuxResponse::Error {
                    message: format!("pane {pane_id} not found"),
                }
            }
            MuxRequest::SendKeys {
                pane_id,
                text,
                broadcast,
            } => {
                let mut delivered = 0;
                if broadcast {
                    for pane in self.panes.values_mut() {
                        if pane.pty.write_bytes(text.as_bytes()).is_ok() {
                            delivered += 1;
                        }
                    }
                } else {
                    let target = pane_id.unwrap_or_else(|| self.manager.active_tab().focused_pane);
                    if let Some(pane) = self.panes.get_mut(&target) {
                        if pane.pty.write_bytes(text.as_bytes()).is_ok() {
                            delivered = 1;
                        }
                    } else {
                        return MuxResponse::Error {
                            message: format!("pane {target} not found"),
                        };
                    }
                }
                MuxResponse::Ok {
                    delivered_to: delivered,
                }
            }
            MuxRequest::Attach | MuxRequest::PollSnapshots => MuxResponse::State {
                manager: self.manager.clone(),
                snapshots: self.collect_snapshots(),
            },
            MuxRequest::WriteBytes { pane_id, bytes } => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    let _ = pane.pty.write_bytes(&bytes);
                    MuxResponse::Ok { delivered_to: 1 }
                } else {
                    MuxResponse::Error {
                        message: format!("pane {pane_id} not found"),
                    }
                }
            }
            MuxRequest::ResizePane {
                pane_id,
                cols,
                rows,
            } => {
                let cols = cols.clamp(1, 1000);
                let rows = rows.clamp(1, 1000);
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    if let Ok(mut g) = pane.grid.lock() {
                        g.resize(cols, rows);
                    }
                    let _ = pane.pty.resize(cols as u16, rows as u16);
                }
                MuxResponse::Ok { delivered_to: 1 }
            }
            MuxRequest::SpawnPane { kind } => {
                if let Some(new_id) = self.spawn_pane() {
                    match kind {
                        SpawnKind::SplitRight | SpawnKind::SplitDown => {
                            if let Some(dir) = split_dir_for_spawn(kind) {
                                self.manager.active_tab_mut().split_focused(new_id, dir);
                            }
                        }
                        SpawnKind::NewTab => {
                            self.manager.add_tab(new_id);
                        }
                        SpawnKind::NewWorkspace => {
                            let num = self.manager.workspaces.len();
                            self.manager.add_workspace(format!("Dev {num}"), new_id);
                        }
                    }
                }
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::SwitchWorkspace { idx } => {
                self.manager.switch_workspace(idx);
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::SwitchTab { idx } => {
                self.manager.switch_tab(idx);
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::FocusPane { pane_id } => {
                if !self.panes.contains_key(&pane_id) {
                    return MuxResponse::Error {
                        message: format!("pane {pane_id} not found"),
                    };
                }
                for (ws_idx, ws) in self.manager.workspaces.iter().enumerate() {
                    for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                        if tab.all_panes().contains(&pane_id) {
                            self.manager.switch_workspace(ws_idx);
                            self.manager.switch_tab(tab_idx);
                            self.manager.active_tab_mut().focused_pane = pane_id;
                            return MuxResponse::State {
                                manager: self.manager.clone(),
                                snapshots: self.collect_snapshots(),
                            };
                        }
                    }
                }
                self.manager.active_tab_mut().focused_pane = pane_id;
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::RenameWorkspace { idx, name } => {
                self.manager.rename_workspace(idx, name);
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::CloseWorkspace { idx } => {
                if idx == 0 {
                    return MuxResponse::Error {
                        message: "cannot close Master Space".to_string(),
                    };
                }
                let closed = self.manager.close_workspace(idx);
                for pid in closed {
                    self.panes.remove(&pid);
                    self.agent_states.remove(&pid);
                }
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::CloseTab { idx } => {
                if self.manager.active_workspace().tabs.len() <= 1
                    && self.manager.workspaces.len() <= 1
                {
                    return MuxResponse::Error {
                        message: "cannot close last tab".to_string(),
                    };
                }
                let (closed, keep_running) = self.manager.close_tab(idx);
                if !keep_running {
                    return MuxResponse::Error {
                        message: "cannot close last tab".to_string(),
                    };
                }
                for pid in closed {
                    self.panes.remove(&pid);
                    self.agent_states.remove(&pid);
                }
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::ClosePane { pane_id } => {
                if !self.panes.contains_key(&pane_id) {
                    return MuxResponse::Error {
                        message: format!("pane {pane_id} not found"),
                    };
                }
                if self.panes.len() <= 1 {
                    return MuxResponse::Error {
                        message: "cannot close the last pane".to_string(),
                    };
                }
                self.panes.remove(&pane_id);
                self.agent_states.remove(&pane_id);
                self.manager.close_pane(pane_id);
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::ResizeSplit { dir } => {
                self.manager.active_tab_mut().resize_focused(dir, 0.05);
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
            MuxRequest::ToggleZoom => {
                self.manager.active_tab_mut().toggle_zoom();
                MuxResponse::State {
                    manager: self.manager.clone(),
                    snapshots: self.collect_snapshots(),
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let config = AppConfig::load();
    let ipc_token = Arc::new(read_or_create_ipc_token().expect("failed to init IPC token"));
    let state = Arc::new(Mutex::new(ServerState::new(config)));
    let listener = TcpListener::bind(DEFAULT_MUX_SERVER_ADDR)
        .await
        .expect("failed to bind conductty-server");

    eprintln!("[conductty-server] listening on {DEFAULT_MUX_SERVER_ADDR}");

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let state = Arc::clone(&state);
        let ipc_token = Arc::clone(&ipc_token);
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            loop {
                let read_res = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    read_bounded_line(&mut buf_reader, MAX_IPC_REQUEST_BYTES),
                )
                .await;
                let line = match read_res {
                    Ok(Ok(Some(line))) => line,
                    Ok(Ok(None)) => break,
                    Ok(Err(e)) => {
                        if let Ok(mut json) = serde_json::to_string(&MuxResponse::Error { message: e }) {
                            json.push('\n');
                            let _ = writer.write_all(json.as_bytes()).await;
                        }
                        break;
                    }
                    Err(_) => break,
                };
                let (resp, should_close) = match parse_authenticated_request(&line, &ipc_token) {
                    Ok(req) => {
                        let r = if let Ok(mut guard) = state.lock() {
                            guard.handle_request(req)
                        } else {
                            MuxResponse::Error {
                                message: "server state poisoned".to_string(),
                            }
                        };
                        (r, false)
                    }
                    Err(e) => (MuxResponse::Error { message: e }, true),
                };
                if let Ok(mut json) = serde_json::to_string(&resp) {
                    json.push('\n');
                    if writer.write_all(json.as_bytes()).await.is_err() {
                        break;
                    }
                }
                if should_close {
                    break;
                }
            }
        });
    }
}
