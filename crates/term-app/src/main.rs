#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod browser;
mod markdown_html;
mod theme_picker;
mod updater;
mod win_backdrop;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use browser::{BrowserBounds, EmbeddedBrowser};
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Stroke, Vec2};
use markdown_html::{markdown_contains_mermaid, render_markdown_to_html};
use term_core::agent::{AgentStatus, PaneAgentState, detect_pane_agent, snapshot_visible_lines};
use term_core::config::{AppConfig, LeftDrawerMode, ResolvedTheme, find_theme};
use term_core::grid::{GridSnapshot, TerminalGrid};
use term_core::input::{
    Action, GoToCommand, InputDecision, InputMode, InputRouter, KeyCode as CoreKeyCode,
    KeyEvent as CoreKeyEvent, Modifiers, PerformableContext,
};
use term_core::layout::{NavDirection, PaneId, Rect as CoreRect, SplitDirection, WorkspaceManager};
use term_core::pty::PtySession;
use term_drawer::{
    DrawerEffect, DrawerItem, DrawerSection, DrawerSnapshot, fetch_plugin_snapshot,
    format_item_for_pane, invoke_plugin_action, validate_http_url,
};
use term_mux::{
    AgentInfo, DEFAULT_IPC_ADDR, DEFAULT_MUX_SERVER_ADDR, MAX_IPC_REQUEST_BYTES,
    MUX_PROTOCOL_VERSION, MuxRequest, MuxResponse, PaneInfo, SpaceInfo, SpawnKind,
    parse_authenticated_request, read_bounded_line, read_or_create_ipc_token, send_request,
    split_dir_for_spawn,
};
use term_render::bg_from_rgb;
use winit::application::ApplicationHandler;
use winit::event::{
    ElementState, Ime, KeyEvent as WinitKeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode as WinitKeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowAttributes, WindowId};

const LEFT_DRAWER_WIDTH: f32 = 240.0;
const LEFT_RAIL_WIDTH: f32 = 32.0;
const TAB_BAR_HEIGHT: f32 = 32.0;
const RIGHT_DRAWER_WIDTH: f32 = 340.0;
const RIGHT_BROWSER_WIDTH: f32 = 560.0;

#[derive(Debug)]
enum UserEvent {
    PtyOutput,
    PaneExited(PaneId),
    DrawerUpdated(DrawerSnapshot),
    MuxStateUpdated(WorkspaceManager, HashMap<PaneId, GridSnapshot>),
    IpcRequest(MuxRequest, tokio::sync::oneshot::Sender<MuxResponse>),
}

struct PaneRuntime {
    pty: PtySession,
    grid: Arc<Mutex<TerminalGrid>>,
    last_size: (usize, usize),
}

fn panel_color([r, g, b]: [u8; 3], opacity: f32) -> Color32 {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn rgb([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn mix_rgb(a: [u8; 3], b: [u8; 3], k: f32) -> Color32 {
    rgb(std::array::from_fn(|i| {
        (a[i] as f32 + (b[i] as f32 - a[i] as f32) * k).round() as u8
    }))
}

fn is_light_theme(theme: &ResolvedTheme) -> bool {
    theme.bg.iter().map(|&c| c as u32).sum::<u32>() > 384
}

/// テーマから導出したドロワー等の UI 配色（状態色: 橙/緑/赤などはテーマ非依存のまま）
#[derive(Clone, Copy)]
struct UiPalette {
    fg: Color32,
    muted: Color32,
    dim: Color32,
    accent: Color32,
    card: Color32,
    card_active: Color32,
    drawer: Color32,
}

impl UiPalette {
    fn new(t: &ResolvedTheme) -> Self {
        Self {
            fg: rgb(t.fg),
            muted: mix_rgb(t.bg, t.fg, 0.6),
            dim: mix_rgb(t.bg, t.fg, 0.45),
            accent: rgb(t.accent),
            card: rgb(t.card_bg),
            card_active: mix_rgb(t.card_bg, t.fg, 0.12),
            drawer: rgb(t.drawer_bg),
        }
    }
}

/// egui 標準ウィジェット（ボタン・入力欄・区切り線など）をテーマの明暗に合わせる
fn apply_theme_visuals(ctx: &egui::Context, theme: &ResolvedTheme) {
    let (egui_theme, mut v) = if is_light_theme(theme) {
        (egui::Theme::Light, egui::Visuals::light())
    } else {
        (egui::Theme::Dark, egui::Visuals::dark())
    };
    v.panel_fill = rgb(theme.drawer_bg);
    v.window_fill = rgb(theme.drawer_bg);
    v.extreme_bg_color = rgb(theme.card_bg);
    v.hyperlink_color = rgb(theme.accent);
    ctx.set_theme(egui_theme);
    ctx.set_visuals_of(egui_theme, v);
}

enum RenderStatus {
    Ok,
    Reconfigure,
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui_renderer: egui_wgpu::Renderer,
}

impl GpuState {
    fn new(window: Arc<Window>, rt: &tokio::runtime::Runtime, transparent: bool) -> Self {
        let size = window.inner_size();
        let instance_desc = if cfg!(target_os = "windows") && transparent {
            wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                backend_options: wgpu::BackendOptions {
                    dx12: wgpu::Dx12BackendOptions {
                        presentation_system: wgpu::Dx12SwapchainKind::DxgiFromVisual,
                        ..wgpu::Dx12BackendOptions::from_env_or_default()
                    },
                    ..wgpu::BackendOptions::from_env_or_default()
                },
                ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
            }
        } else {
            wgpu::InstanceDescriptor::new_without_display_handle_from_env()
        };
        let instance = wgpu::Instance::new(instance_desc);
        let surface = instance
            .create_surface(window)
            .expect("failed to create wgpu surface");

        let adapter = rt
            .block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            }))
            .expect("failed to find suitable GPU adapter");

        let (device, queue) = rt
            .block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("failed to create wgpu device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let alpha_mode = if transparent {
            caps.alpha_modes
                .iter()
                .copied()
                .find(|m| *m == wgpu::CompositeAlphaMode::PreMultiplied)
                .or_else(|| {
                    caps.alpha_modes
                        .iter()
                        .copied()
                        .find(|m| *m == wgpu::CompositeAlphaMode::PostMultiplied)
                })
                .unwrap_or(caps.alpha_modes[0])
        } else {
            caps.alpha_modes
                .iter()
                .copied()
                .find(|m| *m == wgpu::CompositeAlphaMode::Opaque)
                .unwrap_or(caps.alpha_modes[0])
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::default(),
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let egui_renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());

        Self {
            surface,
            device,
            queue,
            config,
            egui_renderer,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn render_egui(
        &mut self,
        window: &Window,
        egui_ctx: &egui::Context,
        mut full_output: egui::FullOutput,
        bg: wgpu::Color,
    ) -> RenderStatus {
        let mut textures_delta = std::mem::take(&mut full_output.textures_delta);
        for (id, image_deltas) in &textures_delta.set {
            for image_delta in image_deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, image_delta);
            }
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                for id in &textures_delta.free {
                    self.egui_renderer.free_texture(id);
                }
                textures_delta.clear();
                return RenderStatus::Reconfigure;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                for id in &textures_delta.free {
                    self.egui_renderer.free_texture(id);
                }
                textures_delta.clear();
                return RenderStatus::Ok;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("wgpu surface validation error");
                for id in &textures_delta.free {
                    self.egui_renderer.free_texture(id);
                }
                textures_delta.clear();
                return RenderStatus::Ok;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("conductty-frame"),
            });

        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("conductty-main-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(bg),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                    multiview_mask: None,
                })
                .forget_lifetime();

            self.egui_renderer
                .render(&mut pass, &clipped_primitives, &screen_descriptor);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);

        for id in &textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        textures_delta.clear();
        RenderStatus::Ok
    }
}

struct TabNavCtx<'a> {
    manager: &'a WorkspaceManager,
    area: CoreRect,
}

impl PerformableContext for TabNavCtx<'_> {
    fn can_goto_split(&self, dir: NavDirection) -> bool {
        self.manager.active_tab().can_goto_split(dir, self.area)
    }
}

type PaneSelection = (PaneId, (usize, usize), (usize, usize));

struct App {
    config: AppConfig,
    theme: ResolvedTheme,
    rt: tokio::runtime::Runtime,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    manager: WorkspaceManager,
    panes: HashMap<PaneId, PaneRuntime>,
    mux_snapshots: HashMap<PaneId, GridSnapshot>,
    mux_last_sizes: HashMap<PaneId, (usize, usize)>,
    /// conductty-server の版が合わずつながなかったときの案内
    mux_error: Option<String>,
    next_pane_id: PaneId,
    pane_mode_active: bool,
    go_to_open: bool,
    go_to_filtering: bool,
    go_to_query: String,
    go_to_selected_idx: usize,
    keybind_hints_open: bool,
    keybind_hints_scroll_offset: f32,
    keybind_hints_max_scroll: f32,
    agent_states: HashMap<PaneId, PaneAgentState>,
    agent_scan_start: std::time::Instant,
    last_agent_scan: std::time::Instant,
    left_drawer_mode: LeftDrawerMode,
    left_drawer_selected_idx: usize,
    renaming_workspace_idx: Option<usize>,
    rename_buf: String,
    right_drawer_open: bool,
    active_plugin_idx: usize,
    drawer_snapshot: DrawerSnapshot,
    /// 非アクティブなプラグインの直近スナップショット（取得中フラグ含む）。切替時の再取得を避ける
    drawer_cache: HashMap<String, DrawerSnapshot>,
    drawer_selected_idx: usize,
    drawer_scroll_to_selected: bool,
    drawer_preview_item: Option<DrawerItem>,
    drawer_input_action: Option<(String, Option<String>, String)>,
    drawer_input_buf: String,
    cursor_pos: Pos2,
    cell_size: (f32, f32),
    selection: Option<PaneSelection>,
    selecting: bool,
    clipboard: Option<arboard::Clipboard>,
    browser: Option<EmbeddedBrowser>,
    pending_browser_url: Option<String>,
    pending_browser_html: Option<String>,
    browser_error: Option<String>,
    kitty_textures: HashMap<(PaneId, u32), (egui::TextureHandle, u64)>,
    router: InputRouter,
    modifiers: ModifiersState,
    suppress_next_ime_commit: bool,
    ime_preedit: Option<(String, Option<(usize, usize)>)>,
    last_terminal_area: Option<CoreRect>,
    pty_dirty: Arc<std::sync::atomic::AtomicBool>,
    window_focused: bool,
    window_occluded: bool,
    pending_occluded_redraw: bool,
    last_redraw_at: std::time::Instant,
    pending_redraw_deadline: Option<std::time::Instant>,
    last_agent_grid_gen: HashMap<PaneId, u64>,
}

impl App {
    fn terminal_area(&self) -> CoreRect {
        if let Some(area) = self.last_terminal_area {
            return area;
        }
        let (w, h) = if let Some(window) = &self.window {
            let size = window.inner_size();
            let scale = window.scale_factor() as f32;
            (
                (size.width as f32 / scale).max(100.0),
                (size.height as f32 / scale).max(100.0),
            )
        } else {
            (1200.0, 800.0)
        };

        let left_w = match self.left_drawer_mode {
            LeftDrawerMode::Always => LEFT_DRAWER_WIDTH.min(w * 0.30),
            LeftDrawerMode::Collapsed => LEFT_RAIL_WIDTH,
            LeftDrawerMode::Hidden => 0.0,
        };

        let right_w = if self.right_drawer_open {
            if self.browser.is_some() || self.drawer_preview_item.is_some() {
                RIGHT_BROWSER_WIDTH.min(w * 0.55)
            } else {
                RIGHT_DRAWER_WIDTH.min(w * 0.45)
            }
        } else {
            0.0
        };

        let top_y = if self.manager.active_workspace().tabs.len() > 1 {
            TAB_BAR_HEIGHT
        } else {
            0.0
        };

        CoreRect {
            x: left_w,
            y: top_y,
            width: (w - left_w - right_w).max(100.0),
            height: (h - top_y).max(80.0),
        }
    }

    fn grid_size_for_rect(&self, rect: CoreRect) -> (usize, usize) {
        let (char_w, row_h) = self.cell_size;
        let pad_x = self.config.appearance.window_padding_x;
        let pad_y = self.config.appearance.window_padding_y;
        let cols = ((rect.width - pad_x * 2.0) / char_w.max(1.0))
            .floor()
            .max(10.0) as usize;
        let rows = ((rect.height - pad_y * 2.0) / row_h.max(1.0))
            .floor()
            .max(4.0) as usize;
        (cols, rows)
    }

    fn expected_pane_grid_size(&self, split: Option<SplitDirection>) -> (usize, usize) {
        let area = self.terminal_area();
        if let Some(dir) = split {
            let focused = self.manager.active_tab().focused_pane;
            let base_rect = self
                .manager
                .active_tab()
                .visible_rects(area)
                .into_iter()
                .find(|(pid, _)| *pid == focused)
                .map(|(_, r)| r)
                .unwrap_or(area);
            let split_rect = match dir {
                SplitDirection::Horizontal => CoreRect {
                    width: base_rect.width * 0.5,
                    ..base_rect
                },
                SplitDirection::Vertical => CoreRect {
                    height: base_rect.height * 0.5,
                    ..base_rect
                },
            };
            self.grid_size_for_rect(split_rect)
        } else {
            self.grid_size_for_rect(area)
        }
    }

    fn sync_visible_pane_sizes(&mut self) {
        if self.config.features.multiplexer_mode {
            return;
        }
        let area = self.terminal_area();
        let visible = self.manager.active_tab().visible_rects(area);
        let sizes: Vec<(PaneId, (usize, usize))> = visible
            .into_iter()
            .map(|(pid, rect)| (pid, self.grid_size_for_rect(rect)))
            .collect();
        for (pid, (cols, rows)) in sizes {
            if let Some(runtime) = self.panes.get_mut(&pid)
                && runtime.last_size != (cols, rows)
            {
                runtime.last_size = (cols, rows);
                if let Ok(mut g) = runtime.grid.lock() {
                    g.resize(cols, rows);
                }
                let _ = runtime.pty.resize(cols as u16, rows as u16);
            }
        }
    }

    fn spawn_pane(&mut self) -> Option<PaneId> {
        self.spawn_pane_for_split(None)
    }

    fn spawn_pane_for_split(&mut self, split: Option<SplitDirection>) -> Option<PaneId> {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let (cols, rows) = self.expected_pane_grid_size(split);

        match PtySession::spawn(
            cols as u16,
            rows as u16,
            &self.config.shell.windows_default_prog,
        ) {
            Ok(mut pty) => {
                let (_, dummy_rx) = std::sync::mpsc::channel();
                let rx = std::mem::replace(&mut pty.output_rx, dummy_rx);
                let (_, dummy_exit_rx) = std::sync::mpsc::channel();
                let exit_rx = std::mem::replace(&mut pty.exit_rx, dummy_exit_rx);

                let mut grid = TerminalGrid::new(cols, rows);
                grid.set_theme(&self.theme);
                let grid = Arc::new(Mutex::new(grid));
                let grid_clone = Arc::clone(&grid);
                let proxy_out = self.proxy.clone();
                let pty_dirty = Arc::clone(&self.pty_dirty);
                thread::spawn(move || {
                    while let Ok(bytes) = rx.recv() {
                        let mut sync_active = false;
                        if let Ok(mut g) = grid_clone.lock() {
                            g.feed_bytes(&bytes);
                            sync_active = g.synchronized_output;
                        }
                        if !sync_active
                            && !pty_dirty.swap(true, std::sync::atomic::Ordering::AcqRel)
                        {
                            let _ = proxy_out.send_event(UserEvent::PtyOutput);
                        }
                    }
                });

                let proxy_exit = self.proxy.clone();
                thread::spawn(move || {
                    if exit_rx.recv().is_ok() {
                        let _ = proxy_exit.send_event(UserEvent::PaneExited(id));
                    }
                });
                self.panes.insert(
                    id,
                    PaneRuntime {
                        pty,
                        grid,
                        last_size: (cols, rows),
                    },
                );
                Some(id)
            }
            Err(err) => {
                eprintln!("PTY spawn error: {err}");
                None
            }
        }
    }

    fn start_ipc_server(&self) {
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            use std::sync::Arc;
            use std::time::Duration;
            use tokio::io::{AsyncWriteExt, BufReader};
            use tokio::net::TcpListener;

            let Ok(ipc_token) = read_or_create_ipc_token() else {
                return;
            };
            let ipc_token = Arc::new(ipc_token);
            let Ok(listener) = TcpListener::bind(DEFAULT_IPC_ADDR).await else {
                return;
            };
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let proxy = proxy.clone();
                let ipc_token = Arc::clone(&ipc_token);
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut buf_reader = BufReader::new(reader);
                    loop {
                        let read_res = tokio::time::timeout(
                            Duration::from_secs(30),
                            read_bounded_line(&mut buf_reader, MAX_IPC_REQUEST_BYTES),
                        )
                        .await;
                        let line = match read_res {
                            Ok(Ok(Some(line))) => line,
                            Ok(Ok(None)) => break,
                            Ok(Err(e)) => {
                                if let Ok(mut json) =
                                    serde_json::to_string(&MuxResponse::Error { message: e })
                                {
                                    json.push('\n');
                                    let _ = writer.write_all(json.as_bytes()).await;
                                }
                                break;
                            }
                            Err(_) => break,
                        };
                        let (resp, should_close) =
                            match parse_authenticated_request(&line, &ipc_token) {
                                Ok(req) => {
                                    let (tx, rx) = tokio::sync::oneshot::channel();
                                    let r = if proxy
                                        .send_event(UserEvent::IpcRequest(req, tx))
                                        .is_err()
                                    {
                                        MuxResponse::Error {
                                            message: "GUI event loop closed".to_string(),
                                        }
                                    } else {
                                        tokio::time::timeout(Duration::from_secs(5), rx)
                                            .await
                                            .ok()
                                            .and_then(|res| res.ok())
                                            .unwrap_or(MuxResponse::Error {
                                                message: "IPC timeout".to_string(),
                                            })
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
        });
    }

    fn handle_ipc_request(&mut self, req: MuxRequest) -> MuxResponse {
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
                self.refresh_agent_states();
                let mut agents = Vec::new();
                for ws in &self.manager.workspaces {
                    for (t_idx, tab) in ws.tabs.iter().enumerate() {
                        for pid in tab.all_panes() {
                            if let Some(st) = self.agent_states.get(&pid) {
                                agents.push(AgentInfo {
                                    pane_id: pid,
                                    workspace: ws.name.clone(),
                                    tab_index: t_idx + 1,
                                    kind: st.kind.label().to_string(),
                                    status: st.status.badge().to_string(),
                                    summary: st.summary.clone(),
                                });
                            }
                        }
                    }
                }
                MuxResponse::Agents { agents }
            }
            MuxRequest::CapturePane { pane_id } => {
                if let Some(pane) = self.panes.get(&pane_id)
                    && let Ok(g) = pane.grid.lock()
                {
                    let (lines, _) = snapshot_visible_lines(&g.snapshot());
                    return MuxResponse::Captured {
                        pane_id,
                        text: lines.join("\n"),
                    };
                }
                MuxResponse::Error {
                    message: format!("pane {pane_id} not found"),
                }
            }
            MuxRequest::SpawnPane { kind } => {
                let split_dir = split_dir_for_spawn(kind);
                let Some(new_id) = self.spawn_pane_for_split(split_dir) else {
                    return MuxResponse::Error {
                        message: "failed to spawn pane".to_string(),
                    };
                };
                match kind {
                    SpawnKind::SplitRight | SpawnKind::SplitDown => {
                        if let Some(dir) = split_dir {
                            self.manager.active_tab_mut().split_focused(new_id, dir);
                            self.sync_visible_pane_sizes();
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
                MuxResponse::Ok { delivered_to: 1 }
            }
            MuxRequest::FocusPane { pane_id } => {
                for (ws_idx, ws) in self.manager.workspaces.iter().enumerate() {
                    for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                        if tab.all_panes().contains(&pane_id) {
                            self.manager.switch_workspace(ws_idx);
                            self.manager.switch_tab(tab_idx);
                            self.manager.active_tab_mut().focused_pane = pane_id;
                            self.left_drawer_selected_idx = ws_idx;
                            return MuxResponse::Ok { delivered_to: 1 };
                        }
                    }
                }
                MuxResponse::Error {
                    message: format!("pane {pane_id} not found"),
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
                self.cleanup_closed_pane(pane_id);
                self.manager.close_pane(pane_id);
                MuxResponse::Ok { delivered_to: 1 }
            }
            MuxRequest::CloseWorkspace { idx } => {
                if idx == 0 {
                    return MuxResponse::Error {
                        message: "cannot close Master Space".to_string(),
                    };
                }
                if idx >= self.manager.workspaces.len() {
                    return MuxResponse::Error {
                        message: format!("space {idx} not found"),
                    };
                }
                let closed = self.manager.close_workspace(idx);
                for pid in closed {
                    self.cleanup_closed_pane(pid);
                }
                if self.left_drawer_selected_idx >= self.manager.workspaces.len() {
                    self.left_drawer_selected_idx = self.manager.workspaces.len() - 1;
                }
                MuxResponse::Ok { delivered_to: 1 }
            }
            MuxRequest::RenameWorkspace { idx, name } => {
                self.manager.rename_workspace(idx, name);
                MuxResponse::Ok { delivered_to: 1 }
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
            MuxRequest::SetTheme { theme } => {
                let Some((name, _)) = find_theme(&theme) else {
                    return MuxResponse::Error {
                        message: format!("unknown theme: {theme}"),
                    };
                };
                self.config.appearance.theme = name;
                // [theme] の個別上書き色も維持するため resolve_theme() 経由で再計算する
                self.theme = self.config.resolve_theme();
                apply_theme_visuals(&self.egui_ctx, &self.theme);
                self.apply_theme_to_grids();
                MuxResponse::Ok { delivered_to: 1 }
            }
            _ => MuxResponse::Error {
                message: "supported in conductty-server (--mux) mode only".to_string(),
            },
        }
    }

    fn enabled_plugins(&self) -> Vec<term_core::config::DrawerPluginConfig> {
        self.config
            .plugins
            .iter()
            .filter(|p| p.enabled && !p.command.trim().is_empty())
            .cloned()
            .collect()
    }

    fn trigger_drawer_refresh(&mut self) {
        let plugins = self.enabled_plugins();
        if plugins.is_empty() {
            self.drawer_snapshot = DrawerSnapshot {
                plugin_id: String::new(),
                title: "Drawer".to_string(),
                sections: Vec::new(),
                is_loading: false,
                last_error: None,
                ..Default::default()
            };
            return;
        }
        if self.active_plugin_idx >= plugins.len() {
            self.active_plugin_idx = 0;
        }
        let plugin = plugins[self.active_plugin_idx].clone();
        if self.drawer_snapshot.plugin_id == plugin.id && self.drawer_snapshot.is_loading {
            return;
        }
        self.drawer_snapshot.plugin_id = plugin.id.clone();
        self.drawer_snapshot.title = plugin.name.clone();
        self.drawer_snapshot.is_loading = true;
        let proxy = self.proxy.clone();
        let cfg_json = serde_json::to_value(&plugin.config).unwrap_or(serde_json::Value::Null);
        self.rt.spawn(async move {
            let snapshot = fetch_plugin_snapshot(
                &plugin.id,
                &plugin.name,
                &plugin.command,
                &plugin.args,
                cfg_json,
            )
            .await;
            let _ = proxy.send_event(UserEvent::DrawerUpdated(snapshot));
        });
    }

    /// 本文プレビューを開く。```mermaid を含む本文は WebView で図として描画する（Raw でテキスト表示に戻せる）
    fn open_body_preview(&mut self, item: DrawerItem) {
        if self.config.features.embedded_browser
            && let Some(body) = item
                .body
                .as_deref()
                .filter(|b| markdown_contains_mermaid(b))
        {
            self.pending_browser_html =
                Some(render_markdown_to_html(&item.title, body, &self.theme));
        }
        self.right_drawer_open = true;
        self.drawer_preview_item = Some(item);
    }

    fn switch_drawer_plugin(&mut self, idx: usize) {
        let prev = std::mem::take(&mut self.drawer_snapshot);
        if !prev.plugin_id.is_empty() {
            self.drawer_cache.insert(prev.plugin_id.clone(), prev);
        }
        self.active_plugin_idx = idx;
        self.drawer_selected_idx = 0;
        self.drawer_scroll_to_selected = true;
        self.drawer_preview_item = None;
        self.drawer_input_action = None;
        let cached = self
            .enabled_plugins()
            .get(idx)
            .and_then(|p| self.drawer_cache.remove(&p.id));
        match cached {
            Some(snapshot) => self.drawer_snapshot = snapshot,
            None => self.trigger_drawer_refresh(),
        }
    }

    fn trigger_drawer_action(
        &mut self,
        action_id: String,
        item_id: Option<String>,
        input: Option<String>,
    ) {
        let plugins = self.enabled_plugins();
        let Some(plugin) = plugins.get(self.active_plugin_idx).cloned() else {
            return;
        };
        self.drawer_snapshot.is_loading = true;
        let proxy = self.proxy.clone();
        let cfg_json = serde_json::to_value(&plugin.config).unwrap_or(serde_json::Value::Null);
        self.rt.spawn(async move {
            let snapshot = invoke_plugin_action(
                &plugin.id,
                &plugin.name,
                &plugin.command,
                &plugin.args,
                &action_id,
                item_id,
                input,
                cfg_json,
            )
            .await;
            let _ = proxy.send_event(UserEvent::DrawerUpdated(snapshot));
        });
    }

    fn ordered_drawer_items(&self) -> Vec<DrawerItem> {
        self.drawer_snapshot.ordered_items()
    }

    fn selected_drawer_item(&self) -> Option<DrawerItem> {
        let ordered = self.ordered_drawer_items();
        ordered.get(self.drawer_selected_idx).cloned()
    }

    fn hit_test_cell(&self, pos: Pos2) -> Option<(PaneId, usize, usize)> {
        let area = self.terminal_area();
        if pos.x < area.x || pos.x >= area.x + area.width {
            return None;
        }
        let (char_w, row_h) = self.cell_size;
        let pad_x = self.config.appearance.window_padding_x;
        let pad_y = self.config.appearance.window_padding_y;
        for (pane_id, rect) in self.manager.active_tab().visible_rects(area) {
            if pos.x >= rect.x
                && pos.x <= rect.x + rect.width
                && pos.y >= rect.y
                && pos.y <= rect.y + rect.height
            {
                let col = ((pos.x - rect.x - pad_x).max(0.0) / char_w).floor() as usize;
                let row = ((pos.y - rect.y - pad_y).max(0.0) / row_h).floor() as usize;
                return Some((pane_id, col, row));
            }
        }
        None
    }

    fn copy_selection(&mut self) {
        let Some((pane_id, start, end)) = self.selection else {
            return;
        };
        if start == end {
            return;
        }
        let text = if let Some(pane) = self.panes.get(&pane_id) {
            pane.grid
                .lock()
                .ok()
                .map(|g| g.extract_text(start, end))
                .unwrap_or_default()
        } else {
            String::new()
        };
        if !text.is_empty()
            && let Some(cb) = &mut self.clipboard
        {
            let _ = cb.set_text(text);
        }
    }

    fn paste_clipboard(&mut self) {
        let Some(cb) = &mut self.clipboard else {
            return;
        };
        if let Ok(text) = cb.get_text()
            && !text.is_empty()
        {
            let normalized = text.replace("\r\n", "\n");
            let focused = self.manager.active_tab().focused_pane;
            if let Some(pane) = self.panes.get_mut(&focused) {
                let bracketed = if let Ok(mut g) = pane.grid.lock() {
                    g.reset_scroll();
                    g.bracketed_paste
                } else {
                    false
                };
                if bracketed {
                    let mut payload = Vec::with_capacity(normalized.len() + 12);
                    payload.extend_from_slice(b"\x1b[200~");
                    payload.extend_from_slice(normalized.as_bytes());
                    payload.extend_from_slice(b"\x1b[201~");
                    let _ = pane.pty.write_bytes(&payload);
                } else {
                    let _ = pane.pty.write_bytes(normalized.as_bytes());
                }
            }
        }
    }

    fn right_browser_bounds(&self) -> BrowserBounds {
        let (w, h) = if let Some(window) = &self.window {
            let size = window.inner_size();
            let scale = window.scale_factor() as f32;
            (
                (size.width as f32 / scale).max(100.0),
                (size.height as f32 / scale).max(100.0),
            )
        } else {
            (1200.0, 800.0)
        };
        let drawer_w = RIGHT_BROWSER_WIDTH.min(w * 0.55);
        let top_bar_h = 46.0_f32;
        BrowserBounds {
            x: (w - drawer_w) as f64,
            y: top_bar_h as f64,
            width: drawer_w as f64,
            height: (h - top_bar_h).max(40.0) as f64,
        }
    }

    fn sync_browser_window(&self) {
        if let (Some(browser), Some(parent)) = (&self.browser, &self.window) {
            browser.sync_to_parent(parent, self.right_browser_bounds());
        }
    }

    fn save_session(&self) {
        if let Some(path) = session_file_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let names: Vec<String> = self
                .manager
                .workspaces
                .iter()
                .map(|w| w.name.clone())
                .collect();
            if let Ok(json) = serde_json::to_string_pretty(&names) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    /// grid が OSC の色の問い合わせに今のテーマで答えるよう、テーマを渡す
    fn apply_theme_to_grids(&self) {
        if self.config.features.multiplexer_mode {
            // grid は server にあるので、server にもテーマを切り替えてもらう
            let req = MuxRequest::SetTheme {
                theme: self.config.appearance.theme.clone(),
            };
            let _ = self
                .rt
                .block_on(send_request(DEFAULT_MUX_SERVER_ADDR, &req));
            return;
        }
        for pane in self.panes.values() {
            if let Ok(mut g) = pane.grid.lock() {
                g.set_theme(&self.theme);
            }
        }
    }

    fn ensure_mux_server_and_attach(&mut self) {
        // 起動したまま残った古い server とは画面のデータの形が違うため、つながずに案内を出す
        match self
            .rt
            .block_on(send_request(DEFAULT_MUX_SERVER_ADDR, &MuxRequest::Hello))
        {
            Ok(MuxResponse::Hello { protocol }) if protocol == MUX_PROTOCOL_VERSION => {}
            Ok(resp) => {
                // 版 1 の server は Hello を知らずエラーを返す
                let server = match resp {
                    MuxResponse::Hello { protocol } => protocol,
                    _ => 1,
                };
                let msg = format!(
                    "conductty-server の版 ({server}) がアプリ ({MUX_PROTOCOL_VERSION}) と合いません。\n古い conductty-server を終了してから開き直してください"
                );
                eprintln!("[conductty-mux] {msg}");
                self.mux_error = Some(msg);
                return;
            }
            // server が起動していない。下の Attach の失敗で起動する
            Err(_) => {}
        }
        if let Ok(MuxResponse::State { manager, snapshots }) = self
            .rt
            .block_on(send_request(DEFAULT_MUX_SERVER_ADDR, &MuxRequest::Attach))
        {
            self.manager = manager;
            self.mux_snapshots = snapshots.into_iter().collect();
        } else {
            // サーバーが未起動ならバックグラウンドで起動して再接続
            let server_bin = if cfg!(target_os = "windows") {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("conductty-server.exe")))
                    .filter(|p| p.is_file())
                    .unwrap_or_else(|| PathBuf::from("conductty-server.exe"))
            } else {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("conductty-server")))
                    .filter(|p| p.is_file())
                    .unwrap_or_else(|| PathBuf::from("conductty-server"))
            };
            let mut server_cmd = Command::new(&server_bin);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                server_cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            match server_cmd.spawn() {
                Ok(_) => {
                    for _ in 0..20 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        if let Ok(MuxResponse::State { manager, snapshots }) = self
                            .rt
                            .block_on(send_request(DEFAULT_MUX_SERVER_ADDR, &MuxRequest::Attach))
                        {
                            self.manager = manager;
                            self.mux_snapshots = snapshots.into_iter().collect();
                            break;
                        }
                    }
                }
                Err(err) => {
                    eprintln!("[conductty] failed to spawn {server_bin:?}: {err}");
                }
            }
        }

        // バックグラウンドでサーバーの画面状態を定期同期
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let mut logged_err = false;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                match send_request(DEFAULT_MUX_SERVER_ADDR, &MuxRequest::PollSnapshots).await {
                    Ok(MuxResponse::State { manager, snapshots }) => {
                        logged_err = false;
                        if proxy
                            .send_event(UserEvent::MuxStateUpdated(
                                manager,
                                snapshots.into_iter().collect(),
                            ))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(other) => {
                        if !logged_err {
                            eprintln!("[conductty-mux] PollSnapshots unexpected: {other:?}");
                            logged_err = true;
                        }
                    }
                    Err(e) => {
                        if !logged_err {
                            eprintln!("[conductty-mux] PollSnapshots error: {e}");
                            logged_err = true;
                        }
                    }
                }
            }
        });
    }

    fn send_mux_action(&mut self, req: MuxRequest) {
        if let Ok(MuxResponse::State { manager, snapshots }) = self
            .rt
            .block_on(send_request(DEFAULT_MUX_SERVER_ADDR, &req))
        {
            self.manager = manager;
            self.mux_snapshots = snapshots.into_iter().collect();
        }
    }

    fn jump_to_pane(&mut self, ws_idx: usize, tab_idx: usize, pane_id: PaneId) {
        if self.config.features.multiplexer_mode {
            self.send_mux_action(MuxRequest::SwitchWorkspace { idx: ws_idx });
            self.send_mux_action(MuxRequest::SwitchTab { idx: tab_idx });
            self.send_mux_action(MuxRequest::FocusPane { pane_id });
        } else {
            self.manager.switch_workspace(ws_idx);
            self.manager.switch_tab(tab_idx);
            self.manager.active_tab_mut().focused_pane = pane_id;
        }
        self.left_drawer_selected_idx = ws_idx;
    }

    fn jump_to_go_to_item(&mut self, item: &GoToItem) {
        self.jump_to_pane(item.ws_idx, item.tab_idx, item.pane_id);
        self.go_to_open = false;
        self.go_to_query.clear();
        self.go_to_selected_idx = 0;
    }

    fn send_text_to_focused_pane(&mut self, text: &str) {
        let focused = self.manager.active_tab().focused_pane;
        if self.config.features.multiplexer_mode {
            let bracketed = self
                .mux_snapshots
                .get(&focused)
                .is_some_and(|s| s.bracketed_paste);
            let payload = if bracketed {
                format!("\x1b[200~{text}\x1b[201~")
            } else {
                text.to_string()
            };
            self.send_mux_action(MuxRequest::WriteBytes {
                pane_id: focused,
                bytes: payload.into_bytes(),
            });
        } else if let Some(pane) = self.panes.get_mut(&focused) {
            let bracketed = pane.grid.lock().map(|g| g.bracketed_paste).unwrap_or(false);
            if bracketed {
                let wrapped = format!("\x1b[200~{text}\x1b[201~");
                let _ = pane.pty.write_bytes(wrapped.as_bytes());
            } else {
                let _ = pane.pty.write_bytes(text.as_bytes());
            }
        }
    }

    fn cleanup_closed_pane(&mut self, pane_id: PaneId) {
        self.panes.remove(&pane_id);
        self.agent_states.remove(&pane_id);
        self.last_agent_grid_gen.remove(&pane_id);
        self.mux_last_sizes.remove(&pane_id);
        self.kitty_textures.retain(|(pid, _), _| *pid != pane_id);
    }

    fn refresh_agent_states(&mut self) {
        if should_throttle_agent_scan(self.last_agent_scan.elapsed()) {
            return;
        }
        self.last_agent_scan = std::time::Instant::now();
        let now_ms = self.agent_scan_start.elapsed().as_millis() as u64;
        let grace_ms = term_core::agent::AGENT_RUNNING_GRACE.as_millis() as u64;
        let mut next_states = HashMap::new();
        let mut next_gens = HashMap::new();

        for ws in &self.manager.workspaces {
            for tab in &ws.tabs {
                for pane_id in tab.all_panes() {
                    let prev = self.agent_states.get(&pane_id);
                    let prev_gen = self.last_agent_grid_gen.get(&pane_id).copied();
                    let grace_expired = prev.is_some_and(|st| {
                        st.status == AgentStatus::Running
                            && now_ms.saturating_sub(st.last_changed_ms) >= grace_ms
                    });

                    let (snap_opt, gen_opt) = if self.config.features.multiplexer_mode {
                        let snap = self.mux_snapshots.get(&pane_id);
                        let cur_gen = snap.map(|s| s.generation);
                        if cur_gen.is_some() && cur_gen == prev_gen && !grace_expired {
                            if let Some(g) = cur_gen {
                                next_gens.insert(pane_id, g);
                            }
                            if let Some(st) = prev {
                                next_states.insert(pane_id, st.clone());
                            }
                            continue;
                        }
                        (snap.cloned(), cur_gen)
                    } else if let Some(rt) = self.panes.get(&pane_id) {
                        if let Ok(g) = rt.grid.lock() {
                            let cur_gen = g.generation;
                            if Some(cur_gen) == prev_gen && !grace_expired {
                                next_gens.insert(pane_id, cur_gen);
                                if let Some(st) = prev {
                                    next_states.insert(pane_id, st.clone());
                                }
                                continue;
                            }
                            (Some(g.snapshot()), Some(cur_gen))
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    };

                    if let Some(g) = gen_opt {
                        next_gens.insert(pane_id, g);
                    }
                    let Some(snap) = snap_opt else {
                        continue;
                    };
                    let (lines, hash) = snapshot_visible_lines(&snap);
                    if let Some(detected) = detect_pane_agent(&lines, hash, now_ms, prev) {
                        next_states.insert(pane_id, detected);
                    }
                }
            }
        }
        self.agent_states = next_states;
        self.last_agent_grid_gen = next_gens;
    }

    fn handle_action(&mut self, action: Action) {
        let area = self.terminal_area();
        if self.config.features.multiplexer_mode {
            match action {
                Action::SplitRight => {
                    self.send_mux_action(MuxRequest::SpawnPane {
                        kind: SpawnKind::SplitRight,
                    });
                    return;
                }
                Action::SplitDown => {
                    self.send_mux_action(MuxRequest::SpawnPane {
                        kind: SpawnKind::SplitDown,
                    });
                    return;
                }
                Action::ResizeSplit(dir) => {
                    self.send_mux_action(MuxRequest::ResizeSplit { dir });
                    return;
                }
                Action::NewTab => {
                    self.send_mux_action(MuxRequest::SpawnPane {
                        kind: SpawnKind::NewTab,
                    });
                    return;
                }
                Action::NewWorkspace => {
                    self.send_mux_action(MuxRequest::SpawnPane {
                        kind: SpawnKind::NewWorkspace,
                    });
                    self.left_drawer_selected_idx = self.manager.active_workspace;
                    self.router.mode = InputMode::Normal;
                    return;
                }
                Action::ToggleZoom => {
                    self.send_mux_action(MuxRequest::ToggleZoom);
                    return;
                }
                Action::LeftDrawerSelect => {
                    self.send_mux_action(MuxRequest::SwitchWorkspace {
                        idx: self.left_drawer_selected_idx,
                    });
                    self.router.mode = InputMode::Normal;
                    return;
                }
                Action::LeftDrawerCloseWorkspace => {
                    self.send_mux_action(MuxRequest::CloseWorkspace {
                        idx: self.left_drawer_selected_idx,
                    });
                    if self.left_drawer_selected_idx >= self.manager.workspaces.len() {
                        self.left_drawer_selected_idx = self.manager.workspaces.len() - 1;
                    }
                    return;
                }
                Action::GotoSplit(dir) => {
                    self.manager.active_tab_mut().goto_split(dir, area);
                    let focused = self.manager.active_tab().focused_pane;
                    self.send_mux_action(MuxRequest::FocusPane { pane_id: focused });
                    return;
                }
                _ => {}
            }
        }

        match action {
            Action::SplitRight => {
                if let Some(new_id) = self.spawn_pane_for_split(Some(SplitDirection::Horizontal)) {
                    self.manager
                        .active_tab_mut()
                        .split_focused(new_id, SplitDirection::Horizontal);
                    self.sync_visible_pane_sizes();
                }
            }
            Action::SplitDown => {
                if let Some(new_id) = self.spawn_pane_for_split(Some(SplitDirection::Vertical)) {
                    self.manager
                        .active_tab_mut()
                        .split_focused(new_id, SplitDirection::Vertical);
                    self.sync_visible_pane_sizes();
                }
            }
            Action::GotoSplit(dir) => {
                self.manager.active_tab_mut().goto_split(dir, area);
            }
            Action::ResizeSplit(dir) => {
                self.manager.active_tab_mut().resize_focused(dir, 0.05);
            }
            Action::ToggleZoom => {
                self.manager.active_tab_mut().toggle_zoom();
            }
            Action::OpenPaneMode => {
                self.pane_mode_active = !self.pane_mode_active;
            }
            Action::ToggleKeybindHints => {
                self.keybind_hints_open = !self.keybind_hints_open;
                if self.keybind_hints_open {
                    self.keybind_hints_scroll_offset = 0.0;
                }
            }
            Action::OpenGoTo => {
                self.go_to_open = !self.go_to_open;
                if self.go_to_open {
                    self.go_to_filtering = false;
                    self.go_to_query.clear();
                    self.go_to_selected_idx = 0;
                }
            }
            Action::ToggleLeftDrawer => {
                if self.config.features.left_drawer_mode == LeftDrawerMode::Hidden {
                    self.router.mode = InputMode::Normal;
                    return;
                }
                self.left_drawer_mode = LeftDrawerMode::Always;
                self.left_drawer_selected_idx = self.manager.active_workspace;
            }
            Action::ToggleLeftDrawerCollapse => {
                if self.config.features.left_drawer_mode == LeftDrawerMode::Collapsed {
                    self.left_drawer_mode = match self.left_drawer_mode {
                        LeftDrawerMode::Always => LeftDrawerMode::Collapsed,
                        LeftDrawerMode::Collapsed | LeftDrawerMode::Hidden => {
                            LeftDrawerMode::Always
                        }
                    };
                }
                self.router.mode = InputMode::Normal;
            }
            Action::ExitLeftDrawerMode => {
                self.router.mode = InputMode::Normal;
            }
            Action::LeftDrawerNext => {
                let len = self.manager.workspaces.len();
                if len > 0 {
                    self.left_drawer_selected_idx =
                        (self.left_drawer_selected_idx + 1).min(len - 1);
                }
            }
            Action::LeftDrawerPrev => {
                self.left_drawer_selected_idx = self.left_drawer_selected_idx.saturating_sub(1);
            }
            Action::LeftDrawerSelect => {
                self.manager.switch_workspace(self.left_drawer_selected_idx);
                self.router.mode = InputMode::Normal;
            }
            Action::NewWorkspace => {
                if let Some(new_id) = self.spawn_pane() {
                    let num = self.manager.workspaces.len();
                    let idx = self.manager.add_workspace(format!("Dev {num}"), new_id);
                    self.left_drawer_selected_idx = idx;
                    self.router.mode = InputMode::Normal;
                }
            }
            Action::LeftDrawerRename => {
                let idx = self.left_drawer_selected_idx;
                if let Some(ws) = self.manager.workspaces.get(idx) {
                    self.rename_buf = ws.name.clone();
                    self.renaming_workspace_idx = Some(idx);
                }
            }
            Action::LeftDrawerCloseWorkspace => {
                let idx = self.left_drawer_selected_idx;
                let closed = self.manager.close_workspace(idx);
                for pid in closed {
                    self.cleanup_closed_pane(pid);
                }
                if self.left_drawer_selected_idx >= self.manager.workspaces.len() {
                    self.left_drawer_selected_idx = self.manager.workspaces.len() - 1;
                }
            }
            Action::NewTab => {
                if let Some(new_id) = self.spawn_pane() {
                    self.manager.add_tab(new_id);
                }
            }
            Action::ToggleRightDrawer => {
                if !self.config.features.right_drawer {
                    self.router.mode = InputMode::Normal;
                    return;
                }
                self.right_drawer_open = true;
                self.drawer_selected_idx = 0;
                self.drawer_scroll_to_selected = true;
                if self.drawer_snapshot.sections.is_empty() && !self.drawer_snapshot.is_loading {
                    self.trigger_drawer_refresh();
                }
            }
            Action::CloseRightDrawer => {
                if self.browser.take().is_some() {
                    self.drawer_preview_item = None;
                    if let Some(window) = &self.window {
                        window.focus_window();
                    }
                    return;
                }
                if self.drawer_preview_item.take().is_some()
                    || self.drawer_input_action.take().is_some()
                {
                    return;
                }
                self.right_drawer_open = false;
                self.router.mode = InputMode::Normal;
            }
            Action::DrawerNext => {
                let len = self.drawer_snapshot.total_items();
                if len > 0 {
                    self.drawer_selected_idx = (self.drawer_selected_idx + 1).min(len - 1);
                    self.drawer_scroll_to_selected = true;
                }
            }
            Action::DrawerPrev => {
                self.drawer_selected_idx = self.drawer_selected_idx.saturating_sub(1);
                self.drawer_scroll_to_selected = true;
            }
            Action::DrawerNextPlugin => {
                let plugins = self.enabled_plugins();
                if plugins.len() > 1 {
                    self.switch_drawer_plugin((self.active_plugin_idx + 1) % plugins.len());
                }
            }
            Action::DrawerPrevPlugin => {
                let plugins = self.enabled_plugins();
                if plugins.len() > 1 {
                    self.switch_drawer_plugin(
                        (self.active_plugin_idx + plugins.len() - 1) % plugins.len(),
                    );
                }
            }
            Action::DrawerOpen => {
                if let Some(item) = self.selected_drawer_item() {
                    if !item.url.is_empty() {
                        open_url(&item.url);
                    } else if item.body.as_ref().is_some_and(|b| !b.is_empty()) {
                        self.open_body_preview(item);
                    } else if let Some(first_act) = item.actions.first() {
                        if first_act.needs_input {
                            self.drawer_input_action = Some((
                                first_act.id.clone(),
                                Some(item.id.clone()),
                                first_act.input_placeholder.clone().unwrap_or_default(),
                            ));
                            self.drawer_input_buf.clear();
                        } else {
                            self.trigger_drawer_action(
                                first_act.id.clone(),
                                Some(item.id.clone()),
                                None,
                            );
                        }
                    }
                }
            }
            Action::DrawerPreview => {
                if self.drawer_preview_item.take().is_some() {
                    if self.browser.take().is_some()
                        && let Some(window) = &self.window
                    {
                        window.focus_window();
                    }
                    return;
                }
                if let Some(item) = self.selected_drawer_item() {
                    if item.body.as_ref().is_some_and(|b| !b.is_empty()) {
                        self.open_body_preview(item);
                    } else if self.config.features.embedded_browser && !item.url.is_empty() {
                        self.right_drawer_open = true;
                        self.pending_browser_url = Some(item.url);
                    }
                }
            }
            Action::DrawerSendToPane => {
                if let Some(item) = self.selected_drawer_item() {
                    let text = format_item_for_pane(&item);
                    self.send_text_to_focused_pane(&text);
                    self.right_drawer_open = false;
                    self.router.mode = InputMode::Normal;
                }
            }
            Action::DrawerRefresh => {
                self.trigger_drawer_refresh();
            }
            Action::Copy => {
                self.copy_selection();
                self.selection = None;
            }
            Action::Paste => {
                self.paste_clipboard();
            }
            Action::ReloadConfig => {
                let new_cfg = AppConfig::load();
                self.theme = new_cfg.resolve_theme();
                self.apply_theme_to_grids();
                self.router = InputRouter::from_config(&new_cfg);
                self.left_drawer_mode = new_cfg.features.left_drawer_mode;
                if !new_cfg.features.right_drawer {
                    self.right_drawer_open = false;
                    self.browser = None;
                } else if !new_cfg.features.embedded_browser {
                    self.browser = None;
                }
                if !new_cfg.features.kitty_graphics {
                    self.kitty_textures.clear();
                }
                self.drawer_cache.clear();
                configure_fonts(&self.egui_ctx, &new_cfg.appearance.font_families);
                apply_theme_visuals(&self.egui_ctx, &self.theme);
                if let Some(window) = &self.window {
                    win_backdrop::apply(window, new_cfg.appearance.background_opacity);
                }
                self.config = new_cfg;
            }
            Action::EnterPaneMoveMode
            | Action::ExitPaneMoveMode
            | Action::EnterPaneResizeMode
            | Action::ExitPaneResizeMode => {}
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // 全 OS で背景・UI パネルのみをピクセル単位透過させる（Windows は DirectComposition + patched wgpu-hal を使用）
            let is_wslg =
                cfg!(target_os = "linux") && std::env::var_os("WSL_DISTRO_NAME").is_some();
            let transparent = self.config.appearance.background_opacity < 0.999 && !is_wslg;
            #[allow(unused_mut)]
            let mut attrs = WindowAttributes::default()
                .with_title("conductty")
                .with_transparent(transparent);
            #[cfg(not(target_os = "windows"))]
            {
                attrs = attrs.with_window_icon(load_window_icon(32));
            }
            #[cfg(target_os = "windows")]
            if transparent {
                use winit::platform::windows::WindowAttributesExtWindows;
                attrs = attrs.with_no_redirection_bitmap(true);
            }
            let window = Arc::new(
                event_loop
                    .create_window(attrs)
                    .expect("failed to create window"),
            );
            win_backdrop::apply(&window, self.config.appearance.background_opacity);

            configure_fonts(&self.egui_ctx, &self.config.appearance.font_families);
            apply_theme_visuals(&self.egui_ctx, &self.theme);
            let egui_state = egui_winit::State::new(
                self.egui_ctx.clone(),
                egui::ViewportId::ROOT,
                &window,
                Some(window.scale_factor() as f32),
                None,
                None,
            );

            self.gpu = Some(GpuState::new(window.clone(), &self.rt, transparent));
            self.egui_state = Some(egui_state);
            window.set_ime_allowed(true);
            self.window = Some(window);

            // 初回起動時にバックグラウンドで GitHub タスクをプリフェッチしておく
            if self.config.features.right_drawer {
                self.trigger_drawer_refresh();
            }
            self.start_ipc_server();
        }

        if self.config.features.multiplexer_mode {
            if self.mux_snapshots.is_empty() && self.mux_error.is_none() {
                self.ensure_mux_server_and_attach();
            }
        } else if self.panes.is_empty()
            && let Some(first_id) = self.spawn_pane()
        {
            self.manager = WorkspaceManager::new(first_id);
            // 前回セッションの Workspace 名を復元する (index 0 は Master Space 固定)
            if let Some(names) = load_saved_workspace_names() {
                for name in names.into_iter().skip(1) {
                    if let Some(pid) = self.spawn_pane() {
                        self.manager.add_workspace(name, pid);
                    }
                }
                self.manager.switch_workspace(0);
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput => {
                self.pty_dirty
                    .store(false, std::sync::atomic::Ordering::Release);
                // ConPTY からの DSR (\x1b[6n) 等に対する応答があれば PTY へ返す
                for pane in self.panes.values_mut() {
                    let replies = if let Ok(mut g) = pane.grid.lock() {
                        g.take_replies()
                    } else {
                        Vec::new()
                    };
                    for reply in replies {
                        let _ = pane.pty.write_bytes(&reply);
                    }
                }
                if let Some(window) = &self.window {
                    let is_minimized = window.is_minimized().unwrap_or(false);
                    if self.window_occluded || is_minimized {
                        self.pending_occluded_redraw = true;
                    } else {
                        let min_interval = if self.window_focused {
                            std::time::Duration::from_millis(16)
                        } else {
                            std::time::Duration::from_millis(100)
                        };
                        let elapsed = self.last_redraw_at.elapsed();
                        if elapsed >= min_interval {
                            self.pending_redraw_deadline = None;
                            window.request_redraw();
                        } else {
                            let deadline = self.last_redraw_at + min_interval;
                            self.pending_redraw_deadline =
                                Some(match self.pending_redraw_deadline {
                                    Some(existing) => existing.min(deadline),
                                    None => deadline,
                                });
                        }
                    }
                }
            }
            UserEvent::PaneExited(id) => {
                self.cleanup_closed_pane(id);
                if !self.manager.close_pane(id) {
                    self.save_session();
                    event_loop.exit();
                    return;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            UserEvent::DrawerUpdated(mut snapshot) => {
                if let Some(err) = &snapshot.last_error {
                    eprintln!("[conductty drawer error] {err}");
                }
                let matches_active = self
                    .enabled_plugins()
                    .get(self.active_plugin_idx)
                    .is_none_or(|p| p.id == snapshot.plugin_id);
                if matches_active {
                    let effects = std::mem::take(&mut snapshot.effects);
                    self.drawer_snapshot = snapshot;
                    let total = self.drawer_snapshot.total_items();
                    if total == 0 {
                        self.drawer_selected_idx = 0;
                    } else if self.drawer_selected_idx >= total {
                        self.drawer_selected_idx = total - 1;
                    }
                    for eff in effects {
                        match eff {
                            DrawerEffect::OpenUrl { url } => open_url(&url),
                            DrawerEffect::PreviewUrl { url } => {
                                if self.config.features.embedded_browser {
                                    self.right_drawer_open = true;
                                    self.pending_browser_url = Some(url);
                                }
                            }
                            DrawerEffect::SendToPane { text } => {
                                self.send_text_to_focused_pane(&text);
                            }
                            DrawerEffect::CopyToClipboard { text } => {
                                if let Some(cb) = &mut self.clipboard {
                                    let _ = cb.set_text(text);
                                }
                            }
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else {
                    self.drawer_cache
                        .insert(snapshot.plugin_id.clone(), snapshot);
                }
            }
            UserEvent::MuxStateUpdated(manager, snapshots) => {
                self.manager = manager;
                self.mux_snapshots = snapshots;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            UserEvent::IpcRequest(req, reply_tx) => {
                let resp = self.handle_ipc_request(req);
                let _ = reply_tx.send(resp);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let pending = (self.pending_browser_url.take().map(|u| (u, false)))
            .or_else(|| self.pending_browser_html.take().map(|h| (h, true)));
        if let Some((content, is_html)) = pending {
            self.right_drawer_open = true;
            self.browser_error = None;
            let bounds = self.right_browser_bounds();
            if let Some(browser) = &mut self.browser {
                if is_html {
                    browser.load_html(&content);
                } else {
                    browser.load_url(&content);
                }
                if let Some(parent) = &self.window {
                    browser.sync_to_parent(parent, bounds);
                }
            } else if let Some(parent) = &self.window {
                let opened = if is_html {
                    EmbeddedBrowser::open_html(event_loop, parent, &content, bounds)
                } else {
                    EmbeddedBrowser::open(event_loop, parent, &content, bounds)
                };
                match opened {
                    Ok(b) => self.browser = Some(b),
                    Err(err) => self.browser_error = Some(err),
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if let Some(deadline) = self.pending_redraw_deadline {
            if std::time::Instant::now() >= deadline {
                self.pending_redraw_deadline = None;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
            } else {
                event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(deadline));
            }
        } else {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.browser.as_ref().is_some_and(|b| b.window_id() == id) {
            let should_close = matches!(event, WindowEvent::CloseRequested)
                || matches!(
                    &event,
                    WindowEvent::KeyboardInput { event: key_ev, .. }
                        if key_ev.state == ElementState::Pressed
                            && matches!(key_ev.logical_key, Key::Named(NamedKey::Escape))
                );
            if should_close {
                self.browser = None;
                if let Some(window) = &self.window {
                    window.focus_window();
                    window.request_redraw();
                }
            }
            return;
        }

        let is_text_editing =
            self.renaming_workspace_idx.is_some() || self.drawer_input_action.is_some();
        if let (Some(window), Some(egui_state)) = (&self.window, &mut self.egui_state)
            && should_forward_event_to_egui(is_text_editing, &event)
        {
            let response = egui_state.on_window_event(window, &event);
            // egui_winit は RedrawRequested 自体にも repaint: true を返すため、ここで再要求すると
            // 描画ごとに次の描画が積まれ無限ループになる (表示中は 60fps、最小化中はビジーループ)
            if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
                window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.save_session();
                event_loop.exit();
            }
            WindowEvent::Focused(focused) => {
                self.window_focused = focused;
                if !focused {
                    self.modifiers = ModifiersState::empty();
                } else if self.pending_occluded_redraw && !self.window_occluded {
                    self.pending_occluded_redraw = false;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.window_occluded = occluded;
                if !occluded && self.pending_occluded_redraw {
                    self.pending_occluded_redraw = false;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                self.last_terminal_area = None;
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                self.sync_browser_window();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::Moved(_) => {
                self.sync_browser_window();
            }
            WindowEvent::ModifiersChanged(new_mods) => {
                self.modifiers = new_mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(window) = &self.window {
                    let scale = window.scale_factor() as f32;
                    self.cursor_pos =
                        Pos2::new(position.x as f32 / scale, position.y as f32 / scale);
                }
                if self.selecting
                    && let Some((pane_id, col, row)) = self.hit_test_cell(self.cursor_pos)
                    && let Some((sel_pane, start, _)) = self.selection
                    && sel_pane == pane_id
                {
                    self.selection = Some((sel_pane, start, (col, row)));
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if let Some((pane_id, col, row)) = self.hit_test_cell(self.cursor_pos) {
                            self.manager.active_tab_mut().focused_pane = pane_id;
                            self.selection = Some((pane_id, (col, row), (col, row)));
                            self.selecting = true;
                            if let Some(window) = &self.window {
                                // WebView2 子ウィンドウにフォーカスが移っていた場合に親端末へ戻す
                                window.focus_window();
                                window.request_redraw();
                            }
                        }
                    }
                    (MouseButton::Left, ElementState::Released) if self.selecting => {
                        self.selecting = false;
                        if self.config.features.copy_on_select {
                            self.copy_selection();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let area = self.terminal_area();
                if self.cursor_pos.x >= area.x && self.cursor_pos.x < area.x + area.width {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y.round() as i32 * 3,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / 16.0).round() as i32,
                    };
                    if lines != 0 {
                        let focused = self.manager.active_tab().focused_pane;
                        if let Some(pane) = self.panes.get(&focused)
                            && let Ok(mut g) = pane.grid.lock()
                        {
                            if lines > 0 {
                                g.scroll_up(lines as usize);
                            } else {
                                g.scroll_down((-lines) as usize);
                            }
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                if self.renaming_workspace_idx.is_some() || self.drawer_input_action.is_some() {
                    self.ime_preedit = None;
                    return;
                }
                if self.router.mode == InputMode::Normal
                    && !self.pane_mode_active
                    && !self.go_to_open
                {
                    if text.is_empty() {
                        self.ime_preedit = None;
                    } else {
                        self.ime_preedit = Some((text, cursor));
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::Ime(Ime::Disabled) => {
                if self.ime_preedit.take().is_some()
                    && let Some(window) = &self.window
                {
                    window.request_redraw();
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.ime_preedit = None;
                if self.suppress_next_ime_commit {
                    self.suppress_next_ime_commit = false;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }
                if self.modifiers.alt_key() && self.router.macos_left_option_as_alt {
                    return;
                }
                if self.renaming_workspace_idx.is_some() || self.drawer_input_action.is_some() {
                    return;
                }
                if self.go_to_open {
                    if self.go_to_filtering {
                        self.go_to_query.push_str(&text);
                        self.go_to_selected_idx = 0;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                } else if self.router.mode == InputMode::Normal && !self.pane_mode_active {
                    let focused = self.manager.active_tab().focused_pane;
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::WriteBytes {
                            pane_id: focused,
                            bytes: text.into_bytes(),
                        });
                    } else if let Some(pane) = self.panes.get_mut(&focused) {
                        let _ = pane.pty.write_bytes(text.as_bytes());
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Workspace 名のリネームや右 Drawer のテキスト入力中は egui TextEdit にキー入力を委ねる
                if self.renaming_workspace_idx.is_some() || self.drawer_input_action.is_some() {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                if self.go_to_open {
                    let items = filtered_go_to_items(&self.manager, &self.go_to_query);
                    let core_ev = to_core_key_event(&event, self.modifiers);
                    if let Some(cmd) = core_ev
                        .as_ref()
                        .and_then(|ev| self.router.route_go_to(ev, self.go_to_filtering))
                    {
                        match cmd {
                            GoToCommand::Next => {
                                if !items.is_empty() {
                                    self.go_to_selected_idx =
                                        (self.go_to_selected_idx + 1) % items.len();
                                }
                            }
                            GoToCommand::Prev => {
                                if !items.is_empty() {
                                    self.go_to_selected_idx = if self.go_to_selected_idx == 0 {
                                        items.len() - 1
                                    } else {
                                        self.go_to_selected_idx - 1
                                    };
                                }
                            }
                            GoToCommand::Select => {
                                if let Some(item) = items.get(self.go_to_selected_idx) {
                                    self.jump_to_go_to_item(item);
                                } else {
                                    self.go_to_open = false;
                                }
                                self.go_to_filtering = false;
                            }
                            GoToCommand::Close => {
                                if self.go_to_filtering {
                                    self.go_to_filtering = false;
                                } else {
                                    self.go_to_open = false;
                                }
                            }
                            GoToCommand::StartFilter => {
                                self.go_to_filtering = true;
                            }
                        }
                    } else if self.go_to_filtering {
                        let ctrl = self.modifiers.control_key();
                        match &event.logical_key {
                            Key::Named(NamedKey::Backspace) => {
                                self.go_to_query.pop();
                                self.go_to_selected_idx = 0;
                            }
                            Key::Named(NamedKey::Space) if !ctrl && !self.modifiers.alt_key() => {
                                self.go_to_query.push(' ');
                                self.go_to_selected_idx = 0;
                            }
                            Key::Character(s)
                                if !ctrl
                                    && !self.modifiers.alt_key()
                                    && !self.modifiers.super_key() =>
                            {
                                self.go_to_query.push_str(s.as_str());
                                self.go_to_selected_idx = 0;
                            }
                            _ => {}
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                if self.keybind_hints_open {
                    let ctrl = self.modifiers.control_key();
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            self.keybind_hints_open = false;
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            self.keybind_hints_scroll_offset = (self.keybind_hints_scroll_offset
                                + 28.0)
                                .min(self.keybind_hints_max_scroll);
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            self.keybind_hints_scroll_offset =
                                (self.keybind_hints_scroll_offset - 28.0).max(0.0);
                        }
                        Key::Character(s) if s == "q" || s == "Q" || s == "?" => {
                            self.keybind_hints_open = false;
                        }
                        Key::Character(s)
                            if (s == "j" && !ctrl)
                                || (ctrl
                                    && (s.eq_ignore_ascii_case("j")
                                        || s.eq_ignore_ascii_case("n"))) =>
                        {
                            self.keybind_hints_scroll_offset = (self.keybind_hints_scroll_offset
                                + 28.0)
                                .min(self.keybind_hints_max_scroll);
                        }
                        Key::Character(s)
                            if (s == "k" && !ctrl)
                                || (ctrl
                                    && (s.eq_ignore_ascii_case("k")
                                        || s.eq_ignore_ascii_case("p"))) =>
                        {
                            self.keybind_hints_scroll_offset =
                                (self.keybind_hints_scroll_offset - 28.0).max(0.0);
                        }
                        Key::Character(s) if s == "g" && !ctrl => {
                            self.keybind_hints_scroll_offset = 0.0;
                        }
                        Key::Character(s) if s == "G" && !ctrl => {
                            self.keybind_hints_scroll_offset = self.keybind_hints_max_scroll;
                        }
                        _ => {}
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                if self.pane_mode_active {
                    if let Key::Character(s) = &event.logical_key
                        && let Ok(num) = s.parse::<PaneId>()
                    {
                        let exists = if self.config.features.multiplexer_mode {
                            self.mux_snapshots.contains_key(&num)
                        } else {
                            self.panes.contains_key(&num)
                        };
                        if exists {
                            if self.config.features.multiplexer_mode {
                                self.send_mux_action(MuxRequest::FocusPane { pane_id: num });
                            } else {
                                self.manager.active_tab_mut().focused_pane = num;
                            }
                            self.pane_mode_active = false;
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                    if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                        self.pane_mode_active = false;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                }

                if let Some(core_ev) = to_core_key_event(&event, self.modifiers) {
                    let area = self.terminal_area();
                    let decision = {
                        let ctx = TabNavCtx {
                            manager: &self.manager,
                            area,
                        };
                        self.router.route(&core_ev, &ctx)
                    };

                    match decision {
                        InputDecision::SendToPty(bytes) => {
                            self.suppress_next_ime_commit = false;
                            let focused = self.manager.active_tab().focused_pane;
                            if self.config.features.multiplexer_mode {
                                let _ = self.rt.block_on(send_request(
                                    DEFAULT_MUX_SERVER_ADDR,
                                    &MuxRequest::WriteBytes {
                                        pane_id: focused,
                                        bytes,
                                    },
                                ));
                            } else if let Some(pane) = self.panes.get_mut(&focused) {
                                if let Ok(mut g) = pane.grid.lock() {
                                    g.reset_scroll();
                                }
                                let _ = pane.pty.write_bytes(&bytes);
                            }
                        }
                        // 長押しのキーリピートでプラグインを多重起動しない
                        InputDecision::Execute(Action::DrawerRefresh) if event.repeat => {
                            self.suppress_next_ime_commit = true;
                        }
                        InputDecision::Execute(action) => {
                            self.suppress_next_ime_commit = true;
                            self.handle_action(action);
                        }
                        InputDecision::Ignore => {
                            self.suppress_next_ime_commit = false;
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let is_minimized = self
                    .window
                    .as_ref()
                    .and_then(|w| w.is_minimized())
                    .unwrap_or(false);
                if self.window_occluded || is_minimized {
                    self.pending_occluded_redraw = true;
                    return;
                }
                self.last_redraw_at = std::time::Instant::now();
                self.pending_redraw_deadline = None;
                self.pending_occluded_redraw = false;
                self.refresh_agent_states();
                let area = self.terminal_area();
                let focused_pane = self.manager.active_tab().focused_pane;
                let pane_move_mode = self.router.mode == InputMode::PaneMove;
                let pane_resize_mode = self.router.mode == InputMode::PaneResize;
                let prefix_mode = self.router.mode == InputMode::Prefix;
                let zoomed = self.manager.active_tab().zoomed;
                let pane_mode_active = self.pane_mode_active;
                let go_to_open = self.go_to_open;
                let keybind_hints_open = self.keybind_hints_open;
                let mut hints_scroll_offset = self.keybind_hints_scroll_offset;
                let mut hints_max_scroll = self.keybind_hints_max_scroll;
                let kb_snapshot = if keybind_hints_open {
                    Some(self.config.keybinds.clone())
                } else {
                    None
                };
                let go_to_filtering = self.go_to_filtering;
                let go_to_query = self.go_to_query.clone();
                let go_to_items = if go_to_open {
                    filtered_go_to_items(&self.manager, &go_to_query)
                } else {
                    Vec::new()
                };
                let go_to_selected_idx = if go_to_items.is_empty() {
                    0
                } else {
                    self.go_to_selected_idx.min(go_to_items.len() - 1)
                };
                let mut go_to_click_target: Option<GoToItem> = None;
                let left_drawer_mode = self.left_drawer_mode;
                let left_selected = self.left_drawer_selected_idx;
                let active_ws_idx = self.manager.active_workspace;
                let active_tab_idx = self.manager.active_workspace().active_tab;
                let active_tabs: Vec<(u64, String)> = self
                    .manager
                    .active_workspace()
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        (
                            t.id,
                            t.name.clone().unwrap_or_else(|| format!("Tab {}", i + 1)),
                        )
                    })
                    .collect();
                let ws_summaries: Vec<(String, usize, usize, usize)> = self
                    .manager
                    .workspaces
                    .iter()
                    .map(|ws| {
                        (
                            ws.name.clone(),
                            ws.tabs.len(),
                            ws.active_tab,
                            ws.pane_count(),
                        )
                    })
                    .collect();
                let mut agent_entries: Vec<(usize, String, usize, PaneId, bool, PaneAgentState)> =
                    Vec::new();
                for (ws_idx, ws) in self.manager.workspaces.iter().enumerate() {
                    for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                        for pane_id in tab.all_panes() {
                            if let Some(st) = self.agent_states.get(&pane_id) {
                                let is_focused = ws_idx == active_ws_idx
                                    && tab_idx == ws.active_tab
                                    && pane_id == tab.focused_pane;
                                agent_entries.push((
                                    ws_idx,
                                    ws.name.clone(),
                                    tab_idx,
                                    pane_id,
                                    is_focused,
                                    st.clone(),
                                ));
                            }
                        }
                    }
                }
                let has_ask_agent = agent_entries
                    .iter()
                    .any(|(_, _, _, _, _, st)| st.status == AgentStatus::Ask);
                let mut jump_agent_target: Option<(usize, usize, PaneId)> = None;
                let right_drawer_open = self.right_drawer_open;
                let enabled_plugins = if right_drawer_open {
                    self.enabled_plugins()
                } else {
                    Vec::new()
                };
                let active_plugin_idx = self.active_plugin_idx;
                let drawer_snapshot = if right_drawer_open {
                    self.drawer_snapshot.clone()
                } else {
                    DrawerSnapshot::default()
                };

                let (Some(window), Some(gpu), Some(egui_state)) =
                    (&self.window, &mut self.gpu, &mut self.egui_state)
                else {
                    return;
                };

                let raw_input = egui_state.take_egui_input(window);
                let font_size = self.config.appearance.font_size;
                let pad_x = self.config.appearance.window_padding_x;
                let pad_y = self.config.appearance.window_padding_y;
                let cell_extra_h = self.config.appearance.adjust_cell_height;

                let mut refresh_requested = false;
                let mut switch_plugin_idx: Option<usize> = None;
                let mut preview_body_item: Option<DrawerItem> = None;
                let mut close_body_preview = false;
                let mut invoke_action_target: Option<(String, Option<String>, Option<String>)> =
                    None;
                let mut prompt_action_target: Option<(String, Option<String>, String)> = None;
                let mut cancel_action_prompt = false;
                let mut drawer_input_buf = self.drawer_input_buf.clone();
                let drawer_input_action = self.drawer_input_action.clone();
                let drawer_preview_item = self.drawer_preview_item.clone();
                let mut toggle_left_collapse = false;
                let can_collapse_left =
                    self.config.features.left_drawer_mode == LeftDrawerMode::Collapsed;
                let show_left_toggle =
                    can_collapse_left && self.config.features.left_drawer_toggle_button;
                let mut switch_ws_target: Option<usize> = None;
                let mut switch_tab_target: Option<(usize, usize)> = None;
                let mut start_rename_idx: Option<usize> = None;
                let mut commit_rename: Option<(usize, String)> = None;
                let mut cancel_rename = false;
                let mut close_ws_idx: Option<usize> = None;
                let mut renaming_idx = self.renaming_workspace_idx;
                let mut rename_buf = self.rename_buf.clone();
                let mut select_tab_idx: Option<usize> = None;
                let mut close_tab_idx: Option<usize> = None;
                let mut new_tab_requested = false;
                let mut close_drawer_requested = false;
                let mut close_browser_requested = false;
                let mut show_raw_requested = false;
                let mut open_url_target: Option<String> = None;
                let mut preview_url_target: Option<String> = None;
                let mut send_to_pane_text: Option<String> = None;
                let browser_active = self.browser.is_some();
                let browser_url = self.browser.as_ref().map(|b| b.url.clone());
                let browser_error = self.browser_error.clone();
                let drawer_panel_w = if browser_active || drawer_preview_item.is_some() {
                    RIGHT_BROWSER_WIDTH.min((area.width + RIGHT_BROWSER_WIDTH) * 0.55)
                } else {
                    RIGHT_DRAWER_WIDTH
                };
                let scroll_to_selected = self.drawer_scroll_to_selected;
                self.drawer_scroll_to_selected = false;
                let mut pending_resizes: Vec<(PaneId, usize, usize)> = Vec::new();

                let selection = self.selection;
                let ui_opacity = self.config.appearance.background_opacity;
                let mut computed_cell_size = self.cell_size;
                let mut computed_terminal_area: Option<CoreRect> = None;
                let ime_preedit = self.ime_preedit.clone();
                let mut ime_cursor_rect: Option<Rect> = None;
                let theme = self.theme;
                let pal = UiPalette::new(&theme);

                let mut full_output = self.egui_ctx.run_ui(raw_input, |ui| {
                    match left_drawer_mode {
                        LeftDrawerMode::Always => {
                        egui::Panel::left("conductty_left_drawer")
                            .exact_size(LEFT_DRAWER_WIDTH)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .inner_margin(12.0),
                            )
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.heading(
                                        egui::RichText::new("Workspaces")
                                            .color(pal.fg),
                                    );
                                });
                                ui.separator();

                                let footer_h = 32.0;
                                let avail_h = (ui.available_height() - footer_h).max(80.0);
                                egui::ScrollArea::vertical()
                                    .max_height(avail_h)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                    for (idx, (name, tab_count, active_t, pane_count)) in
                                        ws_summaries.iter().enumerate()
                                    {
                                        let is_active = idx == active_ws_idx;
                                        let is_selected = idx == left_selected;
                                        let is_master = idx == 0;
                                        let accent = if is_master {
                                            Color32::from_rgb(0xff, 0xc7, 0x77)
                                        } else {
                                            pal.accent
                                        };
                                        let fill = if is_active {
                                            pal.card_active
                                        } else {
                                            pal.card
                                        };
                                        let stroke = if is_selected {
                                            Stroke::new(1.5_f32, accent)
                                        } else {
                                            Stroke::NONE
                                        };

                                        egui::Frame::new()
                                            .fill(fill)
                                            .stroke(stroke)
                                            .corner_radius(4.0)
                                            .inner_margin(8.0)
                                            .show(ui, |ui| {
                                                let prefix = if is_master { "★ " } else { "" };
                                                if renaming_idx == Some(idx) {
                                                    let resp = ui.add(
                                                        egui::TextEdit::singleline(&mut rename_buf)
                                                            .desired_width(f32::INFINITY),
                                                    );
                                                    resp.request_focus();
                                                    if ui.input(|i| i.key_pressed(egui::Key::Enter))
                                                    {
                                                        commit_rename =
                                                            Some((idx, rename_buf.clone()));
                                                        renaming_idx = None;
                                                    } else if ui
                                                        .input(|i| i.key_pressed(egui::Key::Escape))
                                                    {
                                                        cancel_rename = true;
                                                        renaming_idx = None;
                                                    }
                                                } else {
                                                    ui.horizontal(|ui| {
                                                        if ui
                                                            .selectable_label(
                                                                is_active,
                                                                egui::RichText::new(format!(
                                                                    "{prefix}{name}"
                                                                ))
                                                                .strong()
                                                                .color(accent),
                                                            )
                                                            .clicked()
                                                        {
                                                            switch_ws_target = Some(idx);
                                                        }
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                if !is_master
                                                                    && ui.small_button("x").clicked()
                                                                {
                                                                    close_ws_idx = Some(idx);
                                                                }
                                                                if ui.small_button("Edit").clicked()
                                                                {
                                                                    start_rename_idx = Some(idx);
                                                                }
                                                            },
                                                        );
                                                    });
                                                }

                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{tab_count} tabs · {pane_count} panes"
                                                    ))
                                                    .small()
                                                    .color(pal.muted),
                                                );

                                                if *tab_count > 1 {
                                                    ui.horizontal_wrapped(|ui| {
                                                        for t_idx in 0..*tab_count {
                                                            let t_active =
                                                                is_active && t_idx == *active_t;
                                                            if ui
                                                                .selectable_label(
                                                                    t_active,
                                                                    format!("Tab {}", t_idx + 1),
                                                                )
                                                                .clicked()
                                                            {
                                                                switch_tab_target =
                                                                    Some((idx, t_idx));
                                                            }
                                                        }
                                                    });
                                                }
                                            });
                                        ui.add_space(4.0);
                                    }

                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "AI Agents ({})",
                                                agent_entries.len()
                                            ))
                                            .strong()
                                            .color(pal.fg),
                                        );
                                        if has_ask_agent {
                                            ui.label(
                                                egui::RichText::new("ASK")
                                                    .small()
                                                    .strong()
                                                    .color(Color32::from_rgb(0xff, 0x9e, 0x64)),
                                            );
                                        }
                                    });
                                    ui.add_space(4.0);
                                    if agent_entries.is_empty() {
                                        ui.label(
                                            egui::RichText::new("No active agents detected")
                                                .small()
                                                .color(pal.dim),
                                        );
                                    } else {
                                        for (ws_idx, ws_name, tab_idx, pane_id, is_focused, st) in
                                            &agent_entries
                                        {
                                            let badge_color = match st.status {
                                                AgentStatus::Ask => {
                                                    Color32::from_rgb(0xff, 0x9e, 0x64)
                                                }
                                                AgentStatus::Running => {
                                                    Color32::from_rgb(0x7d, 0xcf, 0xff)
                                                }
                                                AgentStatus::Waiting => {
                                                    Color32::from_rgb(0xff, 0xc7, 0x77)
                                                }
                                                AgentStatus::Done => {
                                                    Color32::from_rgb(0xc3, 0xe8, 0x8d)
                                                }
                                            };
                                            let card_fill = if *is_focused {
                                                pal.card_active
                                            } else {
                                                pal.card
                                            };
                                            let card_stroke = if st.status == AgentStatus::Ask {
                                                Stroke::new(1.5_f32, badge_color)
                                            } else if *is_focused {
                                                Stroke::new(
                                                    1.0_f32,
                                                    pal.accent,
                                                )
                                            } else {
                                                Stroke::NONE
                                            };
                                            let resp = egui::Frame::new()
                                                .fill(card_fill)
                                                .stroke(card_stroke)
                                                .corner_radius(4.0)
                                                .inner_margin(7.0)
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.horizontal(|ui| {
                                                        egui::Frame::new()
                                                            .fill(badge_color)
                                                            .corner_radius(3.0)
                                                            .inner_margin(egui::Margin::symmetric(
                                                                5, 1,
                                                            ))
                                                            .show(ui, |ui| {
                                                                ui.label(
                                                                    egui::RichText::new(
                                                                        st.status.badge(),
                                                                    )
                                                                    .small()
                                                                    .strong()
                                                                    .color(Color32::from_rgb(
                                                                        0x1e, 0x20, 0x30,
                                                                    )),
                                                                );
                                                            });
                                                        ui.label(
                                                            egui::RichText::new(st.kind.label())
                                                                .strong()
                                                                .color(pal.fg),
                                                        );
                                                    });
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{ws_name} / T{} · P{}",
                                                            tab_idx + 1,
                                                            pane_id
                                                        ))
                                                        .small()
                                                        .color(pal.muted),
                                                    );
                                                    if !st.summary.is_empty() {
                                                        ui.label(
                                                            egui::RichText::new(&st.summary)
                                                                .small()
                                                                .monospace()
                                                                .color(pal.muted),
                                                        );
                                                    }
                                                })
                                                .response;
                                            if resp.interact(egui::Sense::click()).clicked() {
                                                jump_agent_target =
                                                    Some((*ws_idx, *tab_idx, *pane_id));
                                            }
                                            ui.add_space(4.0);
                                        }
                                    }
                                });

                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(
                                            "j/k: select | Enter: switch | n: new | r: rename | d: close",
                                        )
                                        .small()
                                        .color(pal.muted),
                                    );
                                    if show_left_toggle {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.button("<<").clicked() {
                                                    toggle_left_collapse = true;
                                                }
                                            },
                                        );
                                    }
                                });
                            });
                        }
                        LeftDrawerMode::Collapsed => {
                        egui::Panel::left("conductty_left_rail")
                            .exact_size(LEFT_RAIL_WIDTH)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .inner_margin(4.0),
                            )
                            .show(ui, |ui| {
                                ui.vertical_centered(|ui| {
                                    if has_ask_agent {
                                        ui.label(
                                            egui::RichText::new("!")
                                                .strong()
                                                .color(Color32::from_rgb(0xff, 0x9e, 0x64)),
                                        );
                                    }
                                    ui.add_space(6.0);
                                    for (idx, (_, _, _, _)) in ws_summaries.iter().enumerate() {
                                        let is_active = idx == active_ws_idx;
                                        let label = if idx == 0 {
                                            "★".to_string()
                                        } else {
                                            format!("{idx}")
                                        };
                                        if ui.selectable_label(is_active, label).clicked() {
                                            switch_ws_target = Some(idx);
                                        }
                                    }
                                });
                                if show_left_toggle {
                                    ui.with_layout(
                                        egui::Layout::bottom_up(egui::Align::Center),
                                        |ui| {
                                            if ui.button(">").clicked() {
                                                toggle_left_collapse = true;
                                            }
                                        },
                                    );
                                }
                            });
                        }
                        LeftDrawerMode::Hidden => {}
                    }

                    if right_drawer_open {
                        egui::Panel::right("conductty_right_drawer")
                            .exact_size(drawer_panel_w)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .inner_margin(12.0),
                            )
                            .show(ui, |ui| {
                                if let Some(url) = &browser_url {
                                    ui.horizontal(|ui| {
                                        if ui.button("<- List").clicked() {
                                            close_browser_requested = true;
                                        }
                                        if url.starts_with("http://") || url.starts_with("https://") {
                                            if ui.button("External").clicked() {
                                                open_url_target = Some(url.clone());
                                            }
                                        } else if drawer_preview_item.is_some()
                                            && ui.button("Raw").clicked()
                                        {
                                            show_raw_requested = true;
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.button("x").clicked() {
                                                    close_drawer_requested = true;
                                                }
                                            },
                                        );
                                    });
                                    ui.separator();
                                    return;
                                }

                                if let Some(item) = &drawer_preview_item {
                                    ui.horizontal(|ui| {
                                        if ui.button("<- List").clicked() {
                                            close_body_preview = true;
                                        }
                                        if ui.small_button("Send to Pane").clicked() {
                                            send_to_pane_text = Some(format_item_for_pane(item));
                                        }
                                        if let Some(body) = &item.body
                                            && ui.small_button("Copy Body").clicked()
                                        {
                                            ui.ctx().copy_text(body.clone());
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.button("x").clicked() {
                                                    close_drawer_requested = true;
                                                }
                                            },
                                        );
                                    });
                                    ui.separator();
                                    if !item.subtitle.is_empty() {
                                        ui.label(
                                            egui::RichText::new(&item.subtitle)
                                                .small()
                                                .color(pal.accent),
                                        );
                                    }
                                    ui.heading(
                                        egui::RichText::new(&item.title)
                                            .color(pal.fg),
                                    );
                                    if !item.actions.is_empty() {
                                        ui.horizontal_wrapped(|ui| {
                                            for act in &item.actions {
                                                if ui.small_button(&act.label).clicked() {
                                                    if act.needs_input {
                                                        prompt_action_target = Some((
                                                            act.id.clone(),
                                                            Some(item.id.clone()),
                                                            act.input_placeholder
                                                                .clone()
                                                                .unwrap_or_default(),
                                                        ));
                                                    } else {
                                                        invoke_action_target = Some((
                                                            act.id.clone(),
                                                            Some(item.id.clone()),
                                                            None,
                                                        ));
                                                    }
                                                }
                                            }
                                        });
                                    }
                                    ui.separator();
                                    let avail_h = (ui.available_height() - 28.0).max(80.0);
                                    egui::ScrollArea::vertical()
                                        .max_height(avail_h)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            render_markdown_preview(
                                                ui,
                                                item.body.as_deref().unwrap_or(""),
                                                &pal,
                                            );
                                        });
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new("b/Esc: back to list | s: send to pane")
                                            .small()
                                            .color(pal.muted),
                                    );
                                    return;
                                }

                                if enabled_plugins.len() > 1 {
                                    ui.horizontal_wrapped(|ui| {
                                        for (p_idx, p_cfg) in enabled_plugins.iter().enumerate() {
                                            let is_active = p_idx == active_plugin_idx;
                                            let label = if p_cfg.category.is_empty() {
                                                p_cfg.name.clone()
                                            } else {
                                                format!("{} ({})", p_cfg.name, p_cfg.category)
                                            };
                                            if ui.selectable_label(is_active, label).clicked() {
                                                switch_plugin_idx = Some(p_idx);
                                            }
                                        }
                                    });
                                    ui.separator();
                                }

                                let header_title = if !drawer_snapshot.title.is_empty() {
                                    drawer_snapshot.title.as_str()
                                } else if let Some(p) = enabled_plugins.get(active_plugin_idx) {
                                    p.name.as_str()
                                } else {
                                    "Drawer"
                                };

                                ui.horizontal(|ui| {
                                    ui.heading(
                                        egui::RichText::new(header_title)
                                            .color(pal.fg),
                                    );
                                    if let Some(p) = enabled_plugins.get(active_plugin_idx)
                                        && !p.category.is_empty()
                                    {
                                        ui.label(
                                            egui::RichText::new(format!("[{}]", p.category))
                                                .small()
                                                .color(pal.accent),
                                        );
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("x").clicked() {
                                                close_drawer_requested = true;
                                            }
                                            if ui.button("Refresh").clicked() {
                                                refresh_requested = true;
                                            }
                                        },
                                    );
                                });
                                if !drawer_snapshot.actions.is_empty() {
                                    ui.horizontal_wrapped(|ui| {
                                        for act in &drawer_snapshot.actions {
                                            if ui.small_button(&act.label).clicked() {
                                                if act.needs_input {
                                                    prompt_action_target = Some((
                                                        act.id.clone(),
                                                        None,
                                                        act.input_placeholder
                                                            .clone()
                                                            .unwrap_or_default(),
                                                    ));
                                                } else {
                                                    invoke_action_target =
                                                        Some((act.id.clone(), None, None));
                                                }
                                            }
                                        }
                                    });
                                }
                                if let Some((act_id, item_id, placeholder)) = &drawer_input_action {
                                    ui.horizontal(|ui| {
                                        let edit = egui::TextEdit::singleline(&mut drawer_input_buf)
                                            .hint_text(placeholder)
                                            .desired_width(170.0);
                                        let resp = ui.add(edit);
                                        resp.request_focus();
                                        let submit = ui.small_button("OK").clicked()
                                            || ui.input(|i| i.key_pressed(egui::Key::Enter));
                                        let cancel = ui.small_button("Cancel").clicked()
                                            || ui.input(|i| i.key_pressed(egui::Key::Escape));
                                        if submit && !drawer_input_buf.trim().is_empty() {
                                            invoke_action_target = Some((
                                                act_id.clone(),
                                                item_id.clone(),
                                                Some(drawer_input_buf.trim().to_string()),
                                            ));
                                            cancel_action_prompt = true;
                                        } else if cancel {
                                            cancel_action_prompt = true;
                                        }
                                    });
                                }
                                ui.separator();

                                if let Some(err) = &browser_error {
                                    ui.label(
                                        egui::RichText::new(err)
                                            .small()
                                            .color(Color32::from_rgb(0xff, 0x75, 0x7f)),
                                    );
                                }

                                if drawer_snapshot.is_loading {
                                    ui.label(
                                        egui::RichText::new("Loading plugin view...")
                                            .color(pal.accent),
                                    );
                                }

                                let footer_h = 28.0;
                                let avail_h = (ui.available_height() - footer_h).max(80.0);
                                egui::ScrollArea::vertical()
                                    .max_height(avail_h)
                                    .auto_shrink([false, false])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                                    )
                                    .show(ui, |ui| {
                                        if enabled_plugins.is_empty() {
                                            ui.label(
                                                egui::RichText::new(
                                                    "No drawer plugins enabled in config.conductty",
                                                )
                                                .color(pal.muted),
                                            );
                                            return;
                                        }

                                        if let Some(err) = &drawer_snapshot.last_error {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(format!("Error: {err}"))
                                                            .color(Color32::from_rgb(0xff, 0x75, 0x7f)),
                                                    )
                                                    .selectable(true),
                                                );
                                                if ui.small_button("Copy").clicked() {
                                                    ui.ctx().copy_text(err.clone());
                                                }
                                            });
                                            ui.separator();
                                        }

                                        let show_send_to_pane = enabled_plugins
                                            .get(active_plugin_idx)
                                            .is_none_or(|p| p.show_send_to_pane);
                                        let mut global_idx = 0usize;
                                        for section in &drawer_snapshot.sections {
                                            render_drawer_section(
                                                ui,
                                                section,
                                                &mut global_idx,
                                                self.drawer_selected_idx,
                                                scroll_to_selected,
                                                show_send_to_pane,
                                                &mut open_url_target,
                                                &mut preview_url_target,
                                                &mut preview_body_item,
                                                &mut send_to_pane_text,
                                                &mut invoke_action_target,
                                                &mut prompt_action_target,
                                                &pal,
                                            );
                                        }
                                    });

                                ui.separator();
                                ui.label(
                                    egui::RichText::new(
                                        "h/l: plugin | j/k: select | b: preview | o: open | s: send | r: refresh | q/Esc: close",
                                    )
                                    .small()
                                    .color(pal.muted),
                                );
                            });
                    }

                    if active_tabs.len() > 1 {
                        egui::Panel::top("conductty_tab_bar")
                            .exact_size(TAB_BAR_HEIGHT)
                            .frame(
                                egui::Frame::new()
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .inner_margin(egui::Margin::symmetric(8, 4)),
                            )
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    for (t_idx, (_tab_id, title)) in active_tabs.iter().enumerate() {
                                        let is_active = t_idx == active_tab_idx;
                                        let fill = if is_active {
                                            pal.card_active
                                        } else {
                                            pal.card
                                        };
                                        let stroke = if is_active {
                                            Stroke::new(1.0_f32, pal.accent)
                                        } else {
                                            Stroke::NONE
                                        };

                                        egui::Frame::new()
                                            .fill(fill)
                                            .stroke(stroke)
                                            .corner_radius(4.0)
                                            .inner_margin(egui::Margin::symmetric(8, 2))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    let label_color = if is_active {
                                                        pal.fg
                                                    } else {
                                                        pal.muted
                                                    };
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new(title)
                                                                    .small()
                                                                    .color(label_color),
                                                            )
                                                            .frame(false),
                                                        )
                                                        .clicked()
                                                    {
                                                        select_tab_idx = Some(t_idx);
                                                    }
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new("x")
                                                                    .small()
                                                                    .color(pal.muted),
                                                            )
                                                            .frame(false),
                                                        )
                                                        .clicked()
                                                    {
                                                        close_tab_idx = Some(t_idx);
                                                    }
                                                });
                                            });
                                    }

                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("+")
                                                    .color(pal.accent),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        new_tab_requested = true;
                                    }
                                });
                            });
                    }

                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            let panel_rect = ui.max_rect();
                            let actual_area = CoreRect {
                                x: panel_rect.min.x,
                                y: panel_rect.min.y,
                                width: panel_rect.width().max(100.0),
                                height: panel_rect.height().max(80.0),
                            };
                            computed_terminal_area = Some(actual_area);
                            let visible_panes = self.manager.active_tab().visible_rects(actual_area);

                            let font_id = FontId::new(font_size, FontFamily::Monospace);
                            let (char_w, row_h) = ui.fonts_mut(|f| {
                                let w = f
                                    .glyph_width(&font_id, 'M')
                                    .max(f.glyph_width(&font_id, 'W'))
                                    .max(f.glyph_width(&font_id, '0'))
                                    .max(font_size * 0.5);
                                let h = (f.row_height(&font_id).max(font_size) + cell_extra_h)
                                    .ceil();
                                (w, h)
                            });
                            computed_cell_size = (char_w, row_h);
                            if let Some(msg) = &self.mux_error {
                                ui.painter().text(
                                    panel_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    msg,
                                    font_id.clone(),
                                    rgb(theme.fg),
                                );
                            }

                            for (pane_id, rect) in &visible_panes {
                                let egui_rect = Rect::from_min_size(
                                    Pos2::new(rect.x, rect.y),
                                    Vec2::new(rect.width, rect.height),
                                );
                                let is_focused = *pane_id == focused_pane;
                                let painter = ui.painter().with_clip_rect(egui_rect);

                                let border_color = if is_focused && prefix_mode {
                                    Color32::from_rgb(0xff, 0x9e, 0x64)
                                } else if is_focused && pane_resize_mode {
                                    Color32::from_rgb(0x7d, 0xcf, 0xff)
                                } else if is_focused {
                                    pal.accent
                                } else {
                                    pal.card_active
                                };
                                let border_width = if is_focused && (prefix_mode || pane_resize_mode) {
                                    2.5_f32
                                } else if is_focused {
                                    1.5_f32
                                } else {
                                    1.0_f32
                                };
                                painter.rect_stroke(
                                    egui_rect.shrink(1.0),
                                    2.0,
                                    Stroke::new(border_width, border_color),
                                    egui::StrokeKind::Inside,
                                );

                                let target_cols =
                                    ((rect.width - pad_x * 2.0) / char_w).floor().max(10.0)
                                        as usize;
                                let target_rows =
                                    ((rect.height - pad_y * 2.0) / row_h).floor().max(4.0)
                                        as usize;

                                if self.config.features.multiplexer_mode {
                                    if self.mux_last_sizes.get(pane_id).copied()
                                        != Some((target_cols, target_rows))
                                    {
                                        pending_resizes.push((*pane_id, target_cols, target_rows));
                                    }
                                    if let Some(snap) = self.mux_snapshots.get(pane_id) {
                                        render_grid_snapshot(
                                            &painter,
                                            snap,
                                            rect,
                                            pad_x,
                                            pad_y,
                                            char_w,
                                            row_h,
                                            &font_id,
                                            is_focused,
                                            selection.filter(|(p, _, _)| p == pane_id),
                                            &theme,
                                        );
                                        if is_focused {
                                            ime_cursor_rect = Some(render_ime_preedit(
                                                &painter,
                                                snap,
                                                rect,
                                                pad_x,
                                                pad_y,
                                                char_w,
                                                row_h,
                                                &font_id,
                                                &theme,
                                                ime_preedit.as_ref(),
                                            ));
                                        }
                                    }
                                } else if let Some(runtime) = self.panes.get(pane_id) {
                                    if (target_cols, target_rows) != runtime.last_size {
                                        pending_resizes.push((*pane_id, target_cols, target_rows));
                                    }

                                    if let Ok(g) = runtime.grid.lock() {
                                        let snap = g.snapshot();
                                        render_grid_snapshot(
                                            &painter,
                                            &snap,
                                            rect,
                                            pad_x,
                                            pad_y,
                                            char_w,
                                            row_h,
                                            &font_id,
                                            is_focused,
                                            selection.filter(|(p, _, _)| p == pane_id),
                                            &theme,
                                        );
                                        if is_focused {
                                            ime_cursor_rect = Some(render_ime_preedit(
                                                &painter,
                                                &snap,
                                                rect,
                                                pad_x,
                                                pad_y,
                                                char_w,
                                                row_h,
                                                &font_id,
                                                &theme,
                                                ime_preedit.as_ref(),
                                            ));
                                        }
                                        if self.config.features.kitty_graphics {
                                            let pid_val = *pane_id;
                                            self.kitty_textures.retain(|(pid, img_id), _| {
                                                *pid != pid_val || g.kitty.images.contains_key(img_id)
                                            });
                                        }
                                        if self.config.features.kitty_graphics
                                            && !g.kitty.placements.is_empty()
                                        {
                                            let clip_rect = Rect::from_min_size(
                                                Pos2::new(rect.x, rect.y),
                                                Vec2::new(rect.width, rect.height),
                                            );
                                            let clipped = painter.with_clip_rect(clip_rect);
                                            for p in &g.kitty.placements {
                                                let Some(img) = g.kitty.images.get(&p.image_id)
                                                else {
                                                    continue;
                                                };
                                                let display_row = p.row + g.scroll_offset as isize;
                                                let est_rows = p
                                                    .rows
                                                    .map(|r| r as isize)
                                                    .unwrap_or_else(|| {
                                                        ((img.height as f32 / row_h.max(1.0))
                                                            .ceil()
                                                            as isize)
                                                            .max(1)
                                                    });
                                                if display_row + est_rows <= 0
                                                    || display_row >= g.rows as isize
                                                {
                                                    continue;
                                                }
                                                let needs_upload = self
                                                    .kitty_textures
                                                    .get(&(*pane_id, p.image_id))
                                                    .is_none_or(|(_, gen_id)| *gen_id != img.generation);
                                                if needs_upload {
                                                    let expected = (img.width as usize)
                                                        .saturating_mul(img.height as usize)
                                                        .saturating_mul(4);
                                                    if img.width == 0
                                                        || img.height == 0
                                                        || img.rgba.len() != expected
                                                    {
                                                        continue;
                                                    }
                                                    let color_img =
                                                        egui::ColorImage::from_rgba_unmultiplied(
                                                            [
                                                                img.width as usize,
                                                                img.height as usize,
                                                            ],
                                                            &img.rgba,
                                                        );
                                                    let handle = ui.ctx().load_texture(
                                                        format!(
                                                            "kitty-{}-{}",
                                                            pane_id, p.image_id
                                                        ),
                                                        color_img,
                                                        egui::TextureOptions::LINEAR,
                                                    );
                                                    self.kitty_textures.insert(
                                                        (*pane_id, p.image_id),
                                                        (handle, img.generation),
                                                    );
                                                }
                                                let Some((tex, _)) =
                                                    self.kitty_textures.get(&(*pane_id, p.image_id))
                                                else {
                                                    continue;
                                                };

                                                let img_w = p
                                                    .cols
                                                    .map(|c| c as f32 * char_w)
                                                    .unwrap_or(img.width as f32);
                                                let img_h = p
                                                    .rows
                                                    .map(|r| r as f32 * row_h)
                                                    .unwrap_or(img.height as f32);
                                                let img_pos = Pos2::new(
                                                    rect.x + pad_x + p.col as f32 * char_w,
                                                    rect.y + pad_y + display_row as f32 * row_h,
                                                );
                                                let img_rect = Rect::from_min_size(
                                                    img_pos,
                                                    Vec2::new(img_w, img_h),
                                                );
                                                clipped.image(
                                                    tex.id(),
                                                    img_rect,
                                                    Rect::from_min_max(
                                                        Pos2::new(0.0, 0.0),
                                                        Pos2::new(1.0, 1.0),
                                                    ),
                                                    Color32::WHITE,
                                                );
                                            }
                                        }
                                    }
                                }

                                if pane_mode_active {
                                    let badge_pos = Pos2::new(rect.x + 14.0, rect.y + 14.0);
                                    let badge_rect =
                                        Rect::from_min_size(badge_pos, Vec2::new(36.0, 28.0));
                                    painter.rect_filled(
                                        badge_rect,
                                        4.0,
                                        Color32::from_rgb(0xff, 0x9e, 0x64),
                                    );
                                    painter.text(
                                        badge_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        format!("{pane_id}"),
                                        FontId::new(16.0, FontFamily::Monospace),
                                        Color32::from_rgb(0x1e, 0x20, 0x30),
                                    );
                                }
                            }

                            let mut status_parts = Vec::new();
                            if self.config.features.multiplexer_mode {
                                status_parts.push("MUX");
                            }
                            if pane_move_mode {
                                status_parts.push("PANE MOVE MODE (h/j/k/l: move, z: zoom, Enter/Esc: exit)");
                            }
                            if pane_resize_mode {
                                status_parts.push("PANE RESIZE MODE (h/j/k/l: resize, Enter/Esc: exit)");
                            }
                            if zoomed {
                                status_parts.push("ZOOMED");
                            }
                            if pane_mode_active {
                                status_parts.push("PANE MODE (press pane number, Esc: exit)");
                            }
                            if self.config.features.left_drawer_mode == LeftDrawerMode::Collapsed
                                && left_drawer_mode != LeftDrawerMode::Always
                            {
                                status_parts.push("Ctrl+Shift+E: Spaces");
                            }
                            if self.config.features.right_drawer && !right_drawer_open {
                                status_parts.push("Ctrl+Shift+G: GitHub");
                            }
                            let mut right_edge = actual_area.x + actual_area.width - 14.0;
                            if prefix_mode {
                                let badge_w = 64.0;
                                let badge_h = 22.0;
                                let badge_rect = Rect::from_min_size(
                                    Pos2::new(right_edge - badge_w, actual_area.y + 8.0),
                                    Vec2::new(badge_w, badge_h),
                                );
                                ui.painter().rect_filled(
                                    badge_rect,
                                    4.0,
                                    Color32::from_rgb(0xff, 0x9e, 0x64),
                                );
                                ui.painter().text(
                                    badge_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "PREFIX",
                                    FontId::new(12.0, FontFamily::Monospace),
                                    Color32::from_rgb(0x1e, 0x20, 0x30),
                                );
                                right_edge = badge_rect.left() - 10.0;
                            }
                            if !status_parts.is_empty() {
                                let label = status_parts.join(" | ");
                                let top_right = Pos2::new(right_edge, actual_area.y + 10.0);
                                ui.painter().text(
                                    top_right,
                                    egui::Align2::RIGHT_TOP,
                                    label,
                                    FontId::new(12.0, FontFamily::Monospace),
                                    pal.muted,
                                );
                            }
                        });

                    if go_to_open {
                        egui::Window::new("go_to_modal")
                            .title_bar(false)
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                            .fixed_size(Vec2::new(520.0, 420.0))
                            .frame(
                                egui::Frame::window(ui.style())
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .stroke(Stroke::new(1.5_f32, pal.accent))
                                    .corner_radius(8.0)
                                    .inner_margin(egui::Margin::same(14)),
                            )
                            .show(ui.ctx(), |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(if go_to_filtering {
                                            "Go To Space / Pane  [FILTER]"
                                        } else {
                                            "Go To Space / Pane  [VIM]"
                                        })
                                        .strong()
                                        .size(15.0)
                                        .color(pal.accent),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} pane(s)",
                                                    go_to_items.len()
                                                ))
                                                .small()
                                                .color(pal.muted),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                egui::Frame::NONE
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .stroke(Stroke::new(1.0_f32, pal.card_active))
                                    .corner_radius(4.0)
                                    .inner_margin(egui::Margin::symmetric(8, 6))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        let prompt = if go_to_query.is_empty() && !go_to_filtering {
                                            "Press / to filter spaces or panes...".to_string()
                                        } else if go_to_filtering {
                                            format!("/{go_to_query}_")
                                        } else {
                                            format!("/{go_to_query}")
                                        };
                                        let color = if go_to_query.is_empty() && !go_to_filtering {
                                            pal.dim
                                        } else {
                                            pal.fg
                                        };
                                        ui.label(
                                            egui::RichText::new(prompt)
                                                .monospace()
                                                .size(13.0)
                                                .color(color),
                                        );
                                    });
                                ui.add_space(8.0);

                                egui::ScrollArea::vertical()
                                    .max_height(300.0)
                                    .show(ui, |ui| {
                                        if go_to_items.is_empty() {
                                            ui.label(
                                                egui::RichText::new("No matching panes")
                                                    .color(pal.muted),
                                            );
                                            return;
                                        }
                                        let mut last_ws: Option<usize> = None;
                                        for (idx, item) in go_to_items.iter().enumerate() {
                                            if last_ws != Some(item.ws_idx) {
                                                if last_ws.is_some() {
                                                    ui.add_space(4.0);
                                                }
                                                let ws_label = if item.ws_idx == active_ws_idx {
                                                    format!("{} (current)", item.ws_name)
                                                } else {
                                                    item.ws_name.clone()
                                                };
                                                ui.label(
                                                    egui::RichText::new(ws_label)
                                                        .strong()
                                                        .size(13.0)
                                                        .color(Color32::from_rgb(0xc3, 0xe8, 0x8d)),
                                                );
                                                last_ws = Some(item.ws_idx);
                                            }
                                            let is_selected = idx == go_to_selected_idx;
                                            let row_bg = if is_selected {
                                                pal.card
                                            } else {
                                                Color32::TRANSPARENT
                                            };
                                            let resp = egui::Frame::NONE
                                                .fill(row_bg)
                                                .corner_radius(4.0)
                                                .inner_margin(egui::Margin::symmetric(10, 5))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.horizontal(|ui| {
                                                        let marker = if is_selected { ">" } else { " " };
                                                        let focus_tag = if item.is_focused {
                                                            " *"
                                                        } else {
                                                            ""
                                                        };
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{marker} Tab {}  ·  Pane {}{focus_tag}",
                                                                item.tab_idx + 1,
                                                                item.pane_id
                                                            ))
                                                            .monospace()
                                                            .size(13.0)
                                                            .color(if is_selected {
                                                                pal.accent
                                                            } else {
                                                                pal.fg
                                                            }),
                                                        );
                                                    });
                                                })
                                                .response;
                                            if is_selected {
                                                resp.scroll_to_me(None);
                                            }
                                            if resp.interact(egui::Sense::click()).clicked() {
                                                go_to_click_target = Some(item.clone());
                                            }
                                        }
                                    });

                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "j/k: move   /: filter   Enter: jump   q/Esc: close",
                                    )
                                    .small()
                                    .color(pal.muted),
                                );
                            });
                    }

                    if let Some(kb_snapshot) = kb_snapshot.as_ref() {
                        let prefix_label = if kb_snapshot.prefix.trim().is_empty() {
                            "(disabled)".to_string()
                        } else {
                            kb_snapshot.prefix.clone()
                        };
                        let sections: Vec<(&str, Vec<(&str, String)>)> = vec![
                            (
                                "Prefix & General",
                                vec![
                                    ("Prefix / Leader", prefix_label),
                                    ("Keybind Hints", kb_snapshot.keybind_hints.clone()),
                                    ("Reload Config", kb_snapshot.reload_config.clone()),
                                    ("Copy", kb_snapshot.copy.clone()),
                                    ("Paste", kb_snapshot.paste.clone()),
                                ],
                            ),
                            (
                                "Panes & Tabs",
                                vec![
                                    ("New Tab", kb_snapshot.new_tab.clone()),
                                    ("New Workspace", kb_snapshot.new_workspace.clone()),
                                    ("Split Right", kb_snapshot.split_right.clone()),
                                    ("Split Down", kb_snapshot.split_down.clone()),
                                    ("Toggle Zoom", kb_snapshot.toggle_zoom.clone()),
                                    ("Pane Mode (Number)", kb_snapshot.pane_mode.clone()),
                                    ("Pane Move Mode", kb_snapshot.pane_move_mode.clone()),
                                    ("Pane Resize Mode", kb_snapshot.pane_resize_mode.clone()),
                                    ("Focus Left", kb_snapshot.goto_left.clone()),
                                    ("Focus Down", kb_snapshot.goto_down.clone()),
                                    ("Focus Up", kb_snapshot.goto_up.clone()),
                                    ("Focus Right", kb_snapshot.goto_right.clone()),
                                ],
                            ),
                            (
                                "Navigation & Drawers",
                                vec![
                                    ("Go To (Space/Pane)", kb_snapshot.go_to.clone()),
                                    ("Left Drawer (Spaces/AI)", kb_snapshot.toggle_left_drawer.clone()),
                                    ("Right Drawer (Plugins)", kb_snapshot.toggle_right_drawer.clone()),
                                ],
                            ),
                            (
                                "Go To Modal",
                                vec![
                                    ("Next Item", kb_snapshot.go_to_next.clone()),
                                    ("Prev Item", kb_snapshot.go_to_prev.clone()),
                                    ("Jump / Select", kb_snapshot.go_to_select.clone()),
                                    ("Filter Mode", kb_snapshot.go_to_filter.clone()),
                                    ("Close Modal", kb_snapshot.go_to_close.clone()),
                                ],
                            ),
                            (
                                "Right Drawer (Plugins)",
                                vec![
                                    ("Next Item", kb_snapshot.drawer_next.clone()),
                                    ("Prev Item", kb_snapshot.drawer_prev.clone()),
                                    ("Next Plugin", kb_snapshot.drawer_next_plugin.clone()),
                                    ("Prev Plugin", kb_snapshot.drawer_prev_plugin.clone()),
                                    ("Preview (In-App)", kb_snapshot.drawer_preview.clone()),
                                    ("Open (Browser)", kb_snapshot.drawer_open.clone()),
                                    ("Send to Pane", kb_snapshot.drawer_send_to_pane.clone()),
                                    ("Refresh", kb_snapshot.drawer_refresh.clone()),
                                    ("Close Drawer", kb_snapshot.drawer_close.clone()),
                                ],
                            ),
                        ];
                        egui::Window::new("keybind_hints_modal")
                            .title_bar(false)
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                            .fixed_size(Vec2::new(560.0, 460.0))
                            .frame(
                                egui::Frame::window(ui.style())
                                    .fill(panel_color(theme.drawer_bg, ui_opacity))
                                    .stroke(Stroke::new(1.5_f32, Color32::from_rgb(0xff, 0x9e, 0x64)))
                                    .corner_radius(8.0)
                                    .inner_margin(egui::Margin::same(16)),
                            )
                            .show(ui.ctx(), |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Keybind Hints (Current Config)")
                                            .strong()
                                            .size(15.0)
                                            .color(Color32::from_rgb(0xff, 0x9e, 0x64)),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("j/k: scroll   Esc: close")
                                                    .small()
                                                    .color(pal.muted),
                                            );
                                        },
                                    );
                                });
                                ui.separator();
                                let scroll_out = egui::ScrollArea::vertical()
                                    .max_height(390.0)
                                    .auto_shrink([false, false])
                                    .vertical_scroll_offset(hints_scroll_offset)
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        for (heading, rows) in &sections {
                                            ui.add_space(4.0);
                                            ui.label(
                                                egui::RichText::new(*heading)
                                                    .strong()
                                                    .size(13.0)
                                                    .color(pal.accent),
                                            );
                                            for (label, keys) in rows {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!("{label:<24}"))
                                                            .monospace()
                                                            .size(12.5)
                                                            .color(pal.muted),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(keys)
                                                            .monospace()
                                                            .strong()
                                                            .size(12.5)
                                                            .color(Color32::from_rgb(0xc3, 0xe8, 0x8d)),
                                                    );
                                                });
                                            }
                                        }
                                    });
                                hints_scroll_offset = scroll_out.state.clamped_offset().y;
                                hints_max_scroll = (scroll_out.content_size.y
                                    - scroll_out.inner_rect.height())
                                .max(0.0);
                            });
                    }
                });

                self.cell_size = computed_cell_size;
                if let Some(actual_area) = computed_terminal_area {
                    self.last_terminal_area = Some(actual_area);
                }
                self.keybind_hints_scroll_offset = hints_scroll_offset;
                self.keybind_hints_max_scroll = hints_max_scroll;
                let egui_wants_repaint = full_output
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.repaint_delay.is_zero());

                // egui_winit が毎フレーム set_ime_allowed(false) を呼んで Windows IME (TSF) が
                // 「あ」固定になるのを防ぐため、端末用 IME 出力を常に Some に保つ
                let mut platform_output = std::mem::take(&mut full_output.platform_output);
                if platform_output.ime.is_none() {
                    let cursor_rect = ime_cursor_rect.unwrap_or_else(|| {
                        Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(10.0, 20.0))
                    });
                    platform_output.ime = Some(egui::output::IMEOutput {
                        purpose: egui::IMEPurpose::Terminal,
                        rect: cursor_rect,
                        cursor_rect,
                        should_interrupt_composition: false,
                    });
                }
                egui_state.handle_platform_output(window, platform_output);

                let bg_alpha = self.config.appearance.background_opacity;
                let mut bg = if gpu.config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied {
                    bg_from_rgb(self.theme.bg, 1.0)
                } else {
                    bg_from_rgb(self.theme.bg, bg_alpha)
                };
                bg.a = bg_alpha.clamp(0.0, 1.0) as f64;
                match gpu.render_egui(window, &self.egui_ctx, full_output, bg) {
                    RenderStatus::Ok => {}
                    RenderStatus::Reconfigure => {
                        let size = window.inner_size();
                        gpu.resize(size.width, size.height);
                    }
                }

                let mut need_redraw = egui_wants_repaint;
                if let Some((ws_idx, tab_idx, pane_id)) = jump_agent_target {
                    self.jump_to_pane(ws_idx, tab_idx, pane_id);
                    need_redraw = true;
                }
                if let Some(item) = go_to_click_target {
                    self.jump_to_go_to_item(&item);
                    need_redraw = true;
                }
                self.rename_buf = rename_buf;
                if cancel_rename {
                    self.renaming_workspace_idx = None;
                    need_redraw = true;
                }
                if let Some((idx, new_name)) = commit_rename {
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::RenameWorkspace {
                            idx,
                            name: new_name,
                        });
                    } else {
                        self.manager.rename_workspace(idx, new_name);
                    }
                    self.renaming_workspace_idx = None;
                    need_redraw = true;
                }
                if let Some(idx) = start_rename_idx
                    && let Some(ws) = self.manager.workspaces.get(idx)
                {
                    self.rename_buf = ws.name.clone();
                    self.renaming_workspace_idx = Some(idx);
                    need_redraw = true;
                }
                if let Some(idx) = close_ws_idx {
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::CloseWorkspace { idx });
                    } else {
                        let closed = self.manager.close_workspace(idx);
                        for pid in closed {
                            self.cleanup_closed_pane(pid);
                        }
                        if self.left_drawer_selected_idx >= self.manager.workspaces.len() {
                            self.left_drawer_selected_idx = self.manager.workspaces.len() - 1;
                        }
                    }
                    need_redraw = true;
                }
                if toggle_left_collapse {
                    self.handle_action(Action::ToggleLeftDrawerCollapse);
                    need_redraw = true;
                }
                if let Some(ws_idx) = switch_ws_target {
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::SwitchWorkspace { idx: ws_idx });
                    } else {
                        self.manager.switch_workspace(ws_idx);
                    }
                    self.left_drawer_selected_idx = ws_idx;
                    need_redraw = true;
                }
                if let Some((ws_idx, tab_idx)) = switch_tab_target {
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::SwitchWorkspace { idx: ws_idx });
                        self.send_mux_action(MuxRequest::SwitchTab { idx: tab_idx });
                    } else {
                        self.manager.switch_workspace(ws_idx);
                        self.manager.switch_tab(tab_idx);
                    }
                    self.left_drawer_selected_idx = ws_idx;
                    need_redraw = true;
                }
                if new_tab_requested {
                    self.handle_action(Action::NewTab);
                    need_redraw = true;
                }
                if let Some(t_idx) = select_tab_idx {
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::SwitchTab { idx: t_idx });
                    } else {
                        self.manager.switch_tab(t_idx);
                    }
                    need_redraw = true;
                }
                if let Some(t_idx) = close_tab_idx {
                    if self.config.features.multiplexer_mode {
                        self.send_mux_action(MuxRequest::CloseTab { idx: t_idx });
                    } else {
                        let (closed_panes, keep_running) = self.manager.close_tab(t_idx);
                        for pid in closed_panes {
                            self.cleanup_closed_pane(pid);
                        }
                        if !keep_running {
                            self.save_session();
                            event_loop.exit();
                            return;
                        }
                    }
                    need_redraw = true;
                }
                if close_drawer_requested {
                    self.right_drawer_open = false;
                    if self.browser.take().is_some()
                        && let Some(w) = &self.window
                    {
                        w.focus_window();
                    }
                    self.router.mode = InputMode::Normal;
                    need_redraw = true;
                }
                if close_browser_requested || show_raw_requested {
                    if self.browser.take().is_some()
                        && let Some(w) = &self.window
                    {
                        w.focus_window();
                    }
                    // Raw は本文テキスト表示へ、<- List はリストへ戻る
                    if close_browser_requested {
                        self.drawer_preview_item = None;
                    }
                    need_redraw = true;
                }
                self.drawer_input_buf = drawer_input_buf;
                if close_body_preview {
                    self.drawer_preview_item = None;
                    need_redraw = true;
                }
                if let Some(item) = preview_body_item {
                    self.open_body_preview(item);
                    need_redraw = true;
                }
                if cancel_action_prompt {
                    self.drawer_input_action = None;
                    self.drawer_input_buf.clear();
                    need_redraw = true;
                }
                if let Some(prompt) = prompt_action_target {
                    self.drawer_input_action = Some(prompt);
                    self.drawer_input_buf.clear();
                    need_redraw = true;
                }
                if let Some((act_id, item_id, input)) = invoke_action_target {
                    self.trigger_drawer_action(act_id, item_id, input);
                    need_redraw = true;
                }
                if let Some(p_idx) = switch_plugin_idx
                    && p_idx != self.active_plugin_idx
                {
                    self.switch_drawer_plugin(p_idx);
                    need_redraw = true;
                }
                if refresh_requested {
                    self.trigger_drawer_refresh();
                    need_redraw = true;
                }
                if let Some(url) = preview_url_target {
                    self.pending_browser_url = Some(url);
                    need_redraw = true;
                }
                if let Some(url) = open_url_target {
                    open_url(&url);
                }
                if let Some(text) = send_to_pane_text {
                    self.send_text_to_focused_pane(&text);
                }
                for (pane_id, cols, rows) in pending_resizes {
                    if self.config.features.multiplexer_mode {
                        if !self.mux_snapshots.is_empty() {
                            self.mux_last_sizes.insert(pane_id, (cols, rows));
                            self.send_mux_action(MuxRequest::ResizePane {
                                pane_id,
                                cols,
                                rows,
                            });
                            need_redraw = true;
                        }
                    } else if let Some(runtime) = self.panes.get_mut(&pane_id) {
                        runtime.last_size = (cols, rows);
                        if let Ok(mut g) = runtime.grid.lock() {
                            g.resize(cols, rows);
                        }
                        let _ = runtime.pty.resize(cols as u16, rows as u16);
                        need_redraw = true;
                    }
                }
                // Running は出力停止から AGENT_RUNNING_GRACE 経過で解除されるので、その時刻に 1 回だけ起こす
                // (+300ms は should_throttle_agent_scan の 250ms に弾かれないための余裕)
                let grace_ms = term_core::agent::AGENT_RUNNING_GRACE.as_millis() as u64;
                let now_ms = self.agent_scan_start.elapsed().as_millis() as u64;
                if let Some(expiry_ms) = self
                    .agent_states
                    .values()
                    .filter(|st| st.status == AgentStatus::Running)
                    .map(|st| st.last_changed_ms + grace_ms)
                    .filter(|&ms| ms > now_ms)
                    .min()
                {
                    self.pending_redraw_deadline = Some(
                        self.agent_scan_start + std::time::Duration::from_millis(expiry_ms + 300),
                    );
                }
                if need_redraw && let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn parse_drawer_accent(hex: Option<&str>, fallback: Color32) -> Color32 {
    if let Some(s) = hex {
        let h = s.trim().trim_start_matches('#');
        if h.len() == 6
            && let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&h[0..2], 16),
                u8::from_str_radix(&h[2..4], 16),
                u8::from_str_radix(&h[4..6], 16),
            )
        {
            return Color32::from_rgb(r, g, b);
        }
    }
    fallback
}

fn build_markdown_inline_job(text: &str, size: f32, base_color: Color32) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};

    let mut job = LayoutJob::default();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0usize;
    let mut plain_buf = String::new();
    // 太字は本文色より一段強く: 暗色テーマでは白、明色テーマでは黒
    let strong_color =
        if base_color.r() as u16 + base_color.g() as u16 + base_color.b() as u16 > 384 {
            Color32::WHITE
        } else {
            Color32::BLACK
        };

    let flush_plain = |buf: &mut String, job: &mut LayoutJob| {
        if !buf.is_empty() {
            job.append(
                buf,
                0.0,
                TextFormat {
                    font_id: FontId::new(size, FontFamily::Proportional),
                    color: base_color,
                    ..Default::default()
                },
            );
            buf.clear();
        }
    };

    while i < len {
        // Inline code: `...`
        if chars[i] == '`'
            && let Some(end_offset) = chars[i + 1..].iter().position(|&c| c == '`')
        {
            flush_plain(&mut plain_buf, &mut job);
            let code_str: String = chars[i + 1..i + 1 + end_offset].iter().collect();
            job.append(
                &code_str,
                0.0,
                TextFormat {
                    font_id: FontId::new((size - 1.0).max(11.0), FontFamily::Monospace),
                    color: Color32::from_rgb(0xff, 0x9e, 0x64),
                    background: base_color.gamma_multiply(0.15),
                    ..Default::default()
                },
            );
            i += end_offset + 2;
            continue;
        }

        // Bold: **...**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            let rest = &chars[i + 2..];
            if let Some(end_offset) = rest.windows(2).position(|w| w[0] == '*' && w[1] == '*') {
                flush_plain(&mut plain_buf, &mut job);
                let bold_str: String = rest[..end_offset].iter().collect();
                job.append(
                    &bold_str,
                    0.0,
                    TextFormat {
                        font_id: FontId::new(size + 0.5, FontFamily::Proportional),
                        color: strong_color,
                        ..Default::default()
                    },
                );
                i += end_offset + 4;
                continue;
            }
        }

        // Strikethrough: ~~...~~
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            let rest = &chars[i + 2..];
            if let Some(end_offset) = rest.windows(2).position(|w| w[0] == '~' && w[1] == '~') {
                flush_plain(&mut plain_buf, &mut job);
                let strike_str: String = rest[..end_offset].iter().collect();
                job.append(
                    &strike_str,
                    0.0,
                    TextFormat {
                        font_id: FontId::new(size, FontFamily::Proportional),
                        color: base_color.gamma_multiply(0.6),
                        strikethrough: Stroke::new(1.0, base_color.gamma_multiply(0.75)),
                        ..Default::default()
                    },
                );
                i += end_offset + 4;
                continue;
            }
        }

        // Link: [label](url)
        if chars[i] == '['
            && let Some(close_br) = chars[i + 1..].iter().position(|&c| c == ']')
        {
            let after_br = i + 1 + close_br + 1;
            if after_br < len
                && chars[after_br] == '('
                && let Some(close_par) = chars[after_br + 1..].iter().position(|&c| c == ')')
            {
                flush_plain(&mut plain_buf, &mut job);
                let label_str: String = chars[i + 1..i + 1 + close_br].iter().collect();
                job.append(
                    &label_str,
                    0.0,
                    TextFormat {
                        font_id: FontId::new(size, FontFamily::Proportional),
                        color: Color32::from_rgb(0x7d, 0xcf, 0xff),
                        underline: Stroke::new(1.0, Color32::from_rgb(0x7d, 0xcf, 0xff)),
                        ..Default::default()
                    },
                );
                i = after_br + 1 + close_par + 1;
                continue;
            }
        }

        plain_buf.push(chars[i]);
        i += 1;
    }

    flush_plain(&mut plain_buf, &mut job);
    job
}

fn render_markdown_inline(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
    let job = build_markdown_inline_job(text, size, color);
    ui.add(egui::Label::new(job).selectable(true).wrap());
}

fn is_markdown_table_separator(cells: &[&str]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| {
            let t = c.trim();
            !t.is_empty() && t.chars().all(|ch| matches!(ch, '-' | ':'))
        })
}

fn split_markdown_table_row(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner.split('|').map(str::trim).collect()
}

fn render_markdown_preview(ui: &mut egui::Ui, markdown: &str, pal: &UiPalette) {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut idx = 0usize;

    // 1. YAML Frontmatter (--- ... ---)
    if lines.first().map(|l| l.trim()) == Some("---")
        && let Some(end_rel) = lines[1..].iter().position(|l| l.trim() == "---")
    {
        let fm_lines = &lines[1..1 + end_rel];
        if !fm_lines.is_empty() {
            egui::Frame::new()
                .fill(pal.card)
                .stroke(Stroke::new(1.0, pal.card_active))
                .corner_radius(4.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for &fl in fm_lines {
                            let t = fl.trim();
                            if t.is_empty() {
                                continue;
                            }
                            ui.label(
                                egui::RichText::new(t)
                                    .small()
                                    .monospace()
                                    .color(Color32::from_rgb(0x7d, 0xcf, 0xff)),
                            );
                            ui.add_space(6.0);
                        }
                    });
                });
            ui.add_space(6.0);
        }
        idx = 1 + end_rel + 1;
    }

    let body_color = pal.fg;

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            ui.add_space(4.0);
            idx += 1;
            continue;
        }

        // Fenced code block: ```lang ... ```
        if let Some(fence_lang) = trimmed.strip_prefix("```") {
            let lang = fence_lang.trim();
            idx += 1;
            let mut code_lines = Vec::new();
            while idx < lines.len() && !lines[idx].trim().starts_with("```") {
                code_lines.push(lines[idx]);
                idx += 1;
            }
            if idx < lines.len() {
                idx += 1;
            }
            let code_text = code_lines.join("\n");
            egui::Frame::new()
                .fill(pal.drawer)
                .stroke(Stroke::new(1.0, pal.card_active))
                .corner_radius(4.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if !lang.is_empty() {
                            ui.label(egui::RichText::new(lang).small().strong().color(pal.accent));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(code_text.clone());
                            }
                        });
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&code_text)
                                .monospace()
                                .size(13.0)
                                .color(Color32::from_rgb(0xc3, 0xe8, 0x8d)),
                        )
                        .selectable(true),
                    );
                });
            ui.add_space(4.0);
            continue;
        }

        // Markdown Table: | ... |
        if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 2 {
            let table_start = idx;
            let mut rows: Vec<Vec<&str>> = Vec::new();
            while idx < lines.len() {
                let tl = lines[idx].trim();
                if tl.starts_with('|') && tl.contains('|') {
                    let parsed = split_markdown_table_row(tl);
                    if !is_markdown_table_separator(&parsed) {
                        rows.push(parsed);
                    }
                    idx += 1;
                } else {
                    break;
                }
            }
            if !rows.is_empty() {
                egui::Frame::new()
                    .fill(pal.card)
                    .stroke(Stroke::new(1.0, pal.card_active))
                    .corner_radius(4.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        egui::Grid::new(format!("md_table_{table_start}"))
                            .striped(true)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                for (r_idx, row) in rows.iter().enumerate() {
                                    for cell in row {
                                        let color =
                                            if r_idx == 0 { pal.accent } else { body_color };
                                        let size = if r_idx == 0 { 13.5 } else { 13.0 };
                                        render_markdown_inline(ui, cell, size, color);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(4.0);
            }
            continue;
        }

        // Horizontal rule
        if matches!(trimmed, "---" | "***" | "___") {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            idx += 1;
            continue;
        }

        // Headings
        if let Some(h1) = trimmed.strip_prefix("# ") {
            ui.add_space(6.0);
            render_markdown_inline(ui, h1.trim(), 19.0, pal.accent);
            ui.separator();
            idx += 1;
            continue;
        }
        if let Some(h2) = trimmed.strip_prefix("## ") {
            ui.add_space(6.0);
            render_markdown_inline(ui, h2.trim(), 16.5, pal.accent);
            ui.separator();
            idx += 1;
            continue;
        }
        if let Some(h3) = trimmed.strip_prefix("### ") {
            ui.add_space(4.0);
            render_markdown_inline(ui, h3.trim(), 15.0, Color32::from_rgb(0xbb, 0x9a, 0xf7));
            idx += 1;
            continue;
        }
        if let Some(h4) = trimmed.strip_prefix("#### ") {
            ui.add_space(2.0);
            render_markdown_inline(ui, h4.trim(), 14.0, Color32::from_rgb(0x7d, 0xcf, 0xff));
            idx += 1;
            continue;
        }

        // Blockquote: > ...
        if let Some(quote) = trimmed.strip_prefix('>') {
            let quote_text = quote.trim_start();
            egui::Frame::new()
                .fill(pal.card)
                .stroke(Stroke::new(1.0, pal.accent))
                .corner_radius(3.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    render_markdown_inline(ui, quote_text, 13.5, pal.muted);
                });
            idx += 1;
            continue;
        }

        // Checkbox / Bullet / Numbered list
        let leading_spaces = line.chars().take_while(|&c| c == ' ').count();
        let indent_px = (leading_spaces / 2) as f32 * 12.0;

        if let Some(unchecked) = trimmed
            .strip_prefix("- [ ] ")
            .or_else(|| trimmed.strip_prefix("* [ ] "))
        {
            ui.horizontal_top(|ui| {
                if indent_px > 0.0 {
                    ui.add_space(indent_px);
                }
                ui.label(egui::RichText::new("☐").color(pal.muted).strong());
                render_markdown_inline(ui, unchecked, 13.5, body_color);
            });
            idx += 1;
            continue;
        }
        if let Some(checked) = trimmed
            .strip_prefix("- [x] ")
            .or_else(|| trimmed.strip_prefix("- [X] "))
            .or_else(|| trimmed.strip_prefix("* [x] "))
        {
            ui.horizontal_top(|ui| {
                if indent_px > 0.0 {
                    ui.add_space(indent_px);
                }
                ui.label(
                    egui::RichText::new("☑")
                        .color(Color32::from_rgb(0xc3, 0xe8, 0x8d))
                        .strong(),
                );
                render_markdown_inline(ui, checked, 13.5, pal.muted);
            });
            idx += 1;
            continue;
        }
        if let Some(bullet) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            ui.horizontal_top(|ui| {
                if indent_px > 0.0 {
                    ui.add_space(indent_px);
                }
                ui.label(egui::RichText::new("•").color(pal.accent).strong());
                render_markdown_inline(ui, bullet, 13.5, body_color);
            });
            idx += 1;
            continue;
        }

        render_markdown_inline(ui, trimmed, 13.5, body_color);
        idx += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_drawer_section(
    ui: &mut egui::Ui,
    section: &DrawerSection,
    global_idx: &mut usize,
    selected_idx: usize,
    scroll_to_selected: bool,
    show_send_to_pane: bool,
    _open_url_target: &mut Option<String>,
    _preview_url_target: &mut Option<String>,
    preview_body_item: &mut Option<DrawerItem>,
    send_to_pane_text: &mut Option<String>,
    invoke_action_target: &mut Option<(String, Option<String>, Option<String>)>,
    prompt_action_target: &mut Option<(String, Option<String>, String)>,
    pal: &UiPalette,
) {
    let accent = parse_drawer_accent(section.accent.as_deref(), pal.accent);
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!("{} ({})", section.title, section.items.len()))
                .strong()
                .color(accent),
        );
        for act in &section.actions {
            if ui.small_button(&act.label).clicked() {
                if act.needs_input {
                    *prompt_action_target = Some((
                        act.id.clone(),
                        Some(section.id.clone()),
                        act.input_placeholder.clone().unwrap_or_default(),
                    ));
                } else {
                    *invoke_action_target = Some((act.id.clone(), Some(section.id.clone()), None));
                }
            }
        }
    });
    if section.items.is_empty() {
        ui.label(egui::RichText::new("  None").small().color(pal.dim));
        return;
    }

    for item in &section.items {
        let is_selected = *global_idx == selected_idx;
        *global_idx += 1;

        let fill = if is_selected {
            pal.card_active
        } else {
            pal.card
        };
        let stroke = if is_selected {
            Stroke::new(1.5_f32, pal.accent)
        } else {
            Stroke::NONE
        };

        let frame_resp = egui::Frame::new()
            .fill(fill)
            .stroke(stroke)
            .corner_radius(4.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !item.subtitle.is_empty() || item.badge.is_some() {
                    ui.horizontal(|ui| {
                        if let Some(badge) = &item.badge
                            && !badge.is_empty()
                        {
                            ui.label(
                                egui::RichText::new(format!("[{badge}]"))
                                    .small()
                                    .strong()
                                    .color(accent),
                            );
                        }
                        if !item.subtitle.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&item.subtitle).small().color(accent),
                                )
                                .truncate(),
                            );
                        }
                    });
                }
                ui.add(
                    egui::Label::new(egui::RichText::new(&item.title).color(pal.fg))
                        .truncate()
                        .selectable(true),
                )
                .on_hover_text(&item.title);
                let has_body = item.body.as_ref().is_some_and(|b| !b.is_empty());
                if has_body || show_send_to_pane || !item.actions.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        if has_body && ui.small_button("View").clicked() {
                            *preview_body_item = Some(item.clone());
                        }
                        if show_send_to_pane && ui.small_button("Send to Pane").clicked() {
                            *send_to_pane_text = Some(format_item_for_pane(item));
                        }
                        for act in &item.actions {
                            if ui.small_button(&act.label).clicked() {
                                if act.needs_input {
                                    *prompt_action_target = Some((
                                        act.id.clone(),
                                        Some(item.id.clone()),
                                        act.input_placeholder.clone().unwrap_or_default(),
                                    ));
                                } else {
                                    *invoke_action_target =
                                        Some((act.id.clone(), Some(item.id.clone()), None));
                                }
                            }
                        }
                    });
                }
            });
        if is_selected && scroll_to_selected {
            frame_resp.response.scroll_to_me(None);
        }
        ui.add_space(4.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_grid_snapshot(
    painter: &egui::Painter,
    snap: &GridSnapshot,
    rect: &CoreRect,
    pad_x: f32,
    pad_y: f32,
    char_w: f32,
    row_h: f32,
    font_id: &FontId,
    is_focused: bool,
    selection: Option<PaneSelection>,
    theme: &ResolvedTheme,
) {
    let sel_range = selection.map(|(_, a, b)| if a <= b { (a, b) } else { (b, a) });
    // セルの色は番号・既定色のまま届くので、ここで今のテーマ（OSC で変えた色を含む）の色に決める
    let theme = &theme.with_overrides(&snap.color_overrides);
    // OSC 11 で背景色を変えたときは、ウィンドウの背景の上からペインを塗る
    if let Some(bg) = snap.color_overrides.bg {
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(rect.x, rect.y),
                Vec2::new(rect.width, rect.height),
            ),
            0.0,
            rgb(bg),
        );
    }
    let cell_bg = |c: usize| snap.cells.get(c).and_then(|cell| theme.cell_colors(cell).1);
    let is_selected = |pos: (usize, usize)| sel_range.is_some_and(|(s, e)| pos >= s && pos <= e);
    // cursor-text があるときは Ghostty と同じく、塗りつぶしたカーソルの上に文字をその色で描く
    let solid_cursor = is_focused && snap.cursor_visible && theme.cursor_text.is_some();

    for row in 0..snap.rows {
        let y = rect.y + pad_y + row as f32 * row_h;
        let row_start = row * snap.cols;
        let mut col = 0;

        while col < snap.cols {
            if snap.cells.get(row_start + col).is_none() {
                break;
            }
            if let Some(bg_rgb) = cell_bg(row_start + col) {
                let mut end_col = col + 1;
                while end_col < snap.cols && cell_bg(row_start + end_col) == Some(bg_rgb) {
                    end_col += 1;
                }
                let bg_rect = Rect::from_min_size(
                    Pos2::new(rect.x + pad_x + col as f32 * char_w, y),
                    Vec2::new((end_col - col) as f32 * char_w, row_h),
                );
                painter.rect_filled(bg_rect, 0.0, rgb(bg_rgb));
                col = end_col;
            } else {
                col += 1;
            }
        }

        let sel_color = theme
            .selection_bg
            .map(rgb)
            .unwrap_or(Color32::from_rgba_unmultiplied(0x3d, 0x59, 0xa1, 120));
        for col in (0..snap.cols).filter(|&col| is_selected((row, col))) {
            let sel_rect = Rect::from_min_size(
                Pos2::new(rect.x + pad_x + col as f32 * char_w, y),
                Vec2::new(char_w, row_h),
            );
            painter.rect_filled(sel_rect, 0.0, sel_color);
        }
        if solid_cursor && row == snap.cursor_row {
            let cursor_rect = Rect::from_min_size(
                Pos2::new(rect.x + pad_x + snap.cursor_col as f32 * char_w, y),
                Vec2::new(char_w, row_h),
            );
            painter.rect_filled(cursor_rect, 0.0, rgb(theme.cursor));
        }

        col = 0;
        while col < snap.cols {
            let Some(cell) = snap.cells.get(row_start + col) else {
                break;
            };
            let ch = cell.ch;
            if ch == ' ' || ch == '\0' {
                col += 1;
                continue;
            }
            let fg_color = if solid_cursor && (row, col) == (snap.cursor_row, snap.cursor_col) {
                theme.cursor_text.map(rgb)
            } else if is_selected((row, col)) {
                theme.selection_fg.map(rgb)
            } else {
                None
            }
            .unwrap_or_else(|| rgb(theme.cell_colors(cell).0));
            if is_box_drawing_char(ch) {
                let cell_rect = Rect::from_min_size(
                    Pos2::new(rect.x + pad_x + col as f32 * char_w, y),
                    Vec2::new(char_w, row_h),
                );
                if !draw_box_drawing(painter, ch, cell_rect, fg_color) {
                    painter.text(
                        Pos2::new(rect.x + pad_x + col as f32 * char_w, y),
                        egui::Align2::LEFT_TOP,
                        ch.to_string(),
                        font_id.clone(),
                        fg_color,
                    );
                }
                col += 1;
            } else {
                let mut ch_buf = [0u8; 4];
                let s = ch.encode_utf8(&mut ch_buf);
                painter.text(
                    Pos2::new(rect.x + pad_x + col as f32 * char_w, y),
                    egui::Align2::LEFT_TOP,
                    s,
                    font_id.clone(),
                    fg_color,
                );
                col += 1;
            }
        }
    }

    if is_focused && snap.cursor_visible && !solid_cursor {
        let cx = rect.x + pad_x + snap.cursor_col as f32 * char_w;
        let cy = rect.y + pad_y + snap.cursor_row as f32 * row_h;
        let cursor_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(char_w, row_h));
        painter.rect_filled(
            cursor_rect,
            0.0,
            Color32::from_rgba_unmultiplied(theme.cursor[0], theme.cursor[1], theme.cursor[2], 160),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_ime_preedit(
    painter: &egui::Painter,
    snap: &GridSnapshot,
    rect: &CoreRect,
    pad_x: f32,
    pad_y: f32,
    char_w: f32,
    row_h: f32,
    font_id: &FontId,
    theme: &term_core::config::ResolvedTheme,
    preedit: Option<&(String, Option<(usize, usize)>)>,
) -> Rect {
    let cx = rect.x + pad_x + snap.cursor_col as f32 * char_w;
    let cy = rect.y + pad_y + snap.cursor_row as f32 * row_h;
    let base_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(char_w, row_h));

    let Some((text, cursor_range)) = preedit else {
        return base_rect;
    };
    if text.is_empty() {
        return base_rect;
    }

    let fg = Color32::from_rgb(theme.fg[0], theme.fg[1], theme.fg[2]);
    let bg = Color32::from_rgb(theme.bg[0], theme.bg[1], theme.bg[2]);
    let accent = Color32::from_rgb(theme.accent[0], theme.accent[1], theme.accent[2]);

    let galley = painter.layout_no_wrap(text.clone(), font_id.clone(), fg);
    let text_w = galley.size().x.max(char_w);
    let box_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(text_w, row_h));

    // 未確定文字列の背景を塗りつぶし、下線とテキストを描画する
    painter.rect_filled(box_rect, 0.0, bg);
    painter.galley(Pos2::new(cx, cy), galley, fg);

    let underline_y = cy + row_h - 1.5;
    painter.line_segment(
        [
            Pos2::new(cx, underline_y),
            Pos2::new(cx + text_w, underline_y),
        ],
        Stroke::new(1.5, accent),
    );

    // 変換カーソル／選択範囲の強調表示
    let ime_x = if let Some((start, end)) = cursor_range {
        let prefix = text.get(..*start).unwrap_or("");
        let prefix_w = if prefix.is_empty() {
            0.0
        } else {
            painter
                .layout_no_wrap(prefix.to_string(), font_id.clone(), fg)
                .size()
                .x
        };
        if end > start
            && let Some(seg) = text.get(*start..*end)
        {
            let seg_w = painter
                .layout_no_wrap(seg.to_string(), font_id.clone(), fg)
                .size()
                .x
                .max(2.0);
            painter.line_segment(
                [
                    Pos2::new(cx + prefix_w, underline_y),
                    Pos2::new(cx + prefix_w + seg_w, underline_y),
                ],
                Stroke::new(2.5, fg),
            );
        } else {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(cx + prefix_w, cy), Vec2::new(2.0, row_h)),
                0.0,
                accent,
            );
        }
        cx + prefix_w
    } else {
        cx + text_w
    };

    Rect::from_min_size(Pos2::new(ime_x, cy), Vec2::new(char_w, row_h))
}

#[derive(Debug, Clone)]
struct GoToItem {
    ws_idx: usize,
    ws_name: String,
    tab_idx: usize,
    pane_id: PaneId,
    is_focused: bool,
}

fn filtered_go_to_items(manager: &WorkspaceManager, query: &str) -> Vec<GoToItem> {
    let q = query.trim().to_lowercase();
    let mut items = Vec::new();
    for (ws_idx, ws) in manager.workspaces.iter().enumerate() {
        for (tab_idx, tab) in ws.tabs.iter().enumerate() {
            for pane_id in tab.all_panes() {
                if !q.is_empty() {
                    let hay =
                        format!("{} tab {} pane {}", ws.name, tab_idx + 1, pane_id).to_lowercase();
                    if !hay.contains(&q) {
                        continue;
                    }
                }
                items.push(GoToItem {
                    ws_idx,
                    ws_name: ws.name.clone(),
                    tab_idx,
                    pane_id,
                    is_focused: ws_idx == manager.active_workspace
                        && tab_idx == ws.active_tab
                        && pane_id == tab.focused_pane,
                });
            }
        }
    }
    items
}

fn is_box_drawing_char(ch: char) -> bool {
    matches!(
        ch,
        '│' | '┃'
            | '─'
            | '━'
            | '┌'
            | '╭'
            | '┐'
            | '╮'
            | '└'
            | '╰'
            | '┘'
            | '╯'
            | '├'
            | '┤'
            | '┬'
            | '┴'
            | '┼'
            | '\u{e0b4}'
            | '\u{e0b6}'
    )
}

/// 罫線文字（Box Drawing）はフォントに任せずセル全幅・全高で直接描画して隙間をなくす
fn draw_box_drawing(painter: &egui::Painter, ch: char, rect: Rect, color: Color32) -> bool {
    let cx = rect.center().x.round();
    let cy = rect.center().y.round();
    let t = 1.5_f32;
    let ht = t * 0.5;
    let bt = 2.5_f32;
    let hbt = bt * 0.5;

    let v_full = Rect::from_min_max(
        Pos2::new(cx - ht, rect.top()),
        Pos2::new(cx + ht, rect.bottom()),
    );
    let h_full = Rect::from_min_max(
        Pos2::new(rect.left(), cy - ht),
        Pos2::new(rect.right(), cy + ht),
    );
    let v_up = Rect::from_min_max(Pos2::new(cx - ht, rect.top()), Pos2::new(cx + ht, cy + ht));
    let v_down = Rect::from_min_max(
        Pos2::new(cx - ht, cy - ht),
        Pos2::new(cx + ht, rect.bottom()),
    );
    let h_left = Rect::from_min_max(Pos2::new(rect.left(), cy - ht), Pos2::new(cx + ht, cy + ht));
    let h_right = Rect::from_min_max(
        Pos2::new(cx - ht, cy - ht),
        Pos2::new(rect.right(), cy + ht),
    );

    match ch {
        '│' => {
            painter.rect_filled(v_full, 0.0, color);
        }
        '┃' => {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(cx - hbt, rect.top()),
                    Pos2::new(cx + hbt, rect.bottom()),
                ),
                0.0,
                color,
            );
        }
        '─' => {
            painter.rect_filled(h_full, 0.0, color);
        }
        '━' => {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(rect.left(), cy - hbt),
                    Pos2::new(rect.right(), cy + hbt),
                ),
                0.0,
                color,
            );
        }
        '┌' | '╭' => {
            painter.rect_filled(v_down, 0.0, color);
            painter.rect_filled(h_right, 0.0, color);
        }
        '┐' | '╮' => {
            painter.rect_filled(v_down, 0.0, color);
            painter.rect_filled(h_left, 0.0, color);
        }
        '└' | '╰' => {
            painter.rect_filled(v_up, 0.0, color);
            painter.rect_filled(h_right, 0.0, color);
        }
        '┘' | '╯' => {
            painter.rect_filled(v_up, 0.0, color);
            painter.rect_filled(h_left, 0.0, color);
        }
        '├' => {
            painter.rect_filled(v_full, 0.0, color);
            painter.rect_filled(h_right, 0.0, color);
        }
        '┤' => {
            painter.rect_filled(v_full, 0.0, color);
            painter.rect_filled(h_left, 0.0, color);
        }
        '┬' => {
            painter.rect_filled(h_full, 0.0, color);
            painter.rect_filled(v_down, 0.0, color);
        }
        '┴' => {
            painter.rect_filled(h_full, 0.0, color);
            painter.rect_filled(v_up, 0.0, color);
        }
        '┼' => {
            painter.rect_filled(v_full, 0.0, color);
            painter.rect_filled(h_full, 0.0, color);
        }
        // Powerline 丸角キャップ (U+E0B6 , U+E0B4 ) をセル全高にピッタリ合わせて描画
        '\u{e0b6}' => {
            let r = (rect.height() * 0.5).min(rect.width()).round() as u8;
            painter.rect_filled(
                rect,
                egui::CornerRadius {
                    nw: r,
                    sw: r,
                    ne: 0,
                    se: 0,
                },
                color,
            );
        }
        '\u{e0b4}' => {
            let r = (rect.height() * 0.5).min(rect.width()).round() as u8;
            painter.rect_filled(
                rect,
                egui::CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: r,
                    se: r,
                },
                color,
            );
        }
        _ => return false,
    }
    true
}

fn session_file_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(
            PathBuf::from(appdata)
                .join("conductty")
                .join("session.json"),
        );
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".config/conductty/session.json"))
}

fn load_saved_workspace_names() -> Option<Vec<String>> {
    let path = session_file_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(not(target_os = "windows"))]
fn load_window_icon(size: u32) -> Option<winit::window::Icon> {
    let bytes = include_bytes!("../../../assets/app-icon-256.png");
    let img = image::load_from_memory(bytes).ok()?.resize_exact(
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    winit::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

fn open_url(url: &str) {
    let Ok(safe_url) = validate_http_url(url) else {
        return;
    };

    #[cfg(target_os = "macos")]
    let _ = Command::new("open").args(["--", safe_url]).spawn();

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", safe_url])
            .creation_flags(0x0800_0000)
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WSL_DISTRO_NAME").is_some() {
            let _ = Command::new("rundll32.exe")
                .args(["url.dll,FileProtocolHandler", safe_url])
                .spawn();
        } else {
            let _ = Command::new("xdg-open").arg(safe_url).spawn();
        }
    }
}

/// `Cica` -> `FiraCode Nerd Font` -> `Fira Code` の順でシステムフォントを探索し、
/// Monospace と Proportional（日本語フォールバック）の両方へ登録する。
fn configure_fonts(ctx: &egui::Context, families: &[String]) {
    let mut fonts = FontDefinitions::default();
    let search_dirs = font_search_dirs();

    let mut loaded_names = Vec::new();
    for family_name in families {
        if let Some(bytes) = find_font_bytes(&search_dirs, family_name) {
            let key = format!("conductty-{family_name}");
            fonts
                .font_data
                .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
            loaded_names.push(key);
        }
    }

    // Nerd Font アイコン用フォールバック（FiraCodeNF / SymbolsNerdFont 等）
    for nf_name in ["FiraCodeNF", "SymbolsNerdFont", "NerdFont"] {
        let key = format!("conductty-nf-{nf_name}");
        if let Some(bytes) = find_font_bytes(&search_dirs, nf_name) {
            fonts
                .font_data
                .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
            loaded_names.push(key);
        }
    }

    // 日本語 UI（GitHub カードの日本語タイトル等）用にフォールバック日本語フォントも探す
    let mut jp_fallbacks = Vec::new();
    for jp_name in [
        "Cica",
        "NotoSansCJK",
        "NotoSansJP",
        "BIZUDGothic",
        "Meiryo",
        "YuGothic",
        "Hiragino",
    ] {
        let key = format!("conductty-jp-{jp_name}");
        if fonts
            .font_data
            .contains_key(&format!("conductty-{jp_name}"))
        {
            jp_fallbacks.push(format!("conductty-{jp_name}"));
            break;
        }
        if let Some(bytes) = find_font_bytes(&search_dirs, jp_name) {
            fonts
                .font_data
                .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
            jp_fallbacks.push(key);
            break;
        }
    }

    if !loaded_names.is_empty()
        && let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace)
    {
        for (idx, name) in loaded_names.into_iter().enumerate() {
            mono.insert(idx, name);
        }
    }

    if !jp_fallbacks.is_empty() {
        if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
            for name in &jp_fallbacks {
                if !prop.contains(name) {
                    prop.push(name.clone());
                }
            }
        }
        if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
            for name in jp_fallbacks {
                if !mono.contains(&name) {
                    mono.push(name);
                }
            }
        }
    }

    ctx.set_fonts(fonts);
}

fn font_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/fonts"));
        dirs.push(PathBuf::from(&home).join("Library/Fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("/Library/Fonts"));
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local_app_data).join("Microsoft\\Windows\\Fonts"));
    }
    dirs.push(PathBuf::from("/mnt/c/Windows/Fonts"));
    if let Ok(entries) = std::fs::read_dir("/mnt/c/Users") {
        for entry in entries.flatten() {
            let user_fonts = entry.path().join("AppData/Local/Microsoft/Windows/Fonts");
            if user_fonts.is_dir() {
                dirs.push(user_fonts);
            }
        }
    }
    dirs.push(PathBuf::from("C:\\Windows\\Fonts"));
    dirs
}

fn font_candidate_score(fname: &str) -> i32 {
    let mut score = 0;
    if fname.contains("regular") {
        score += 40;
    }
    if fname.contains("mono") {
        score += 15;
    }
    if fname.contains("propo") {
        score -= 100;
    }
    if fname.contains("italic") || fname.contains("oblique") {
        score -= 50;
    }
    if fname.contains("bold")
        || fname.contains("semibold")
        || fname.contains("extrabold")
        || fname.contains("black")
        || fname.contains("heavy")
    {
        score -= 30;
    }
    if fname.contains("light")
        || fname.contains("thin")
        || fname.contains("medium")
        || fname.contains("retina")
    {
        score -= 15;
    }
    if fname.ends_with("vf") {
        score -= 40;
    }
    score
}

fn find_font_bytes(dirs: &[PathBuf], family: &str) -> Option<Vec<u8>> {
    let normalized = family.replace(' ', "").to_lowercase();
    let mut best: Option<(i32, PathBuf)> = None;
    for dir in dirs {
        collect_best_font(dir, &normalized, 0, &mut best);
    }
    let (_, path) = best?;
    std::fs::read(&path).ok()
}

fn collect_best_font(dir: &PathBuf, target: &str, depth: usize, best: &mut Option<(i32, PathBuf)>) {
    if depth > 3 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_best_font(&path, target, depth + 1, best);
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            let ext_lower = ext.to_lowercase();
            if ext_lower == "ttf" || ext_lower == "otf" {
                let fname = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .replace([' ', '-', '_'], "")
                    .to_lowercase();
                if fname.contains(target) {
                    let score = font_candidate_score(&fname);
                    let replace = match best {
                        Some((best_score, _)) => score > *best_score,
                        None => true,
                    };
                    if replace {
                        *best = Some((score, path));
                    }
                }
            }
        }
    }
}

fn key_to_core_code(key: &Key, mods: ModifiersState) -> Option<CoreKeyCode> {
    match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let ch = chars.next()?;
            if ch == ' ' {
                Some(CoreKeyCode::Space)
            } else if mods.control_key() && ('\x01'..='\x1a').contains(&ch) {
                // Windows winit は Ctrl+文字 を制御文字 (\x01..\x1a) として渡すことがあるため英字に戻す
                let letter = (b'a' + (ch as u8 - 1)) as char;
                Some(CoreKeyCode::Char(letter))
            } else {
                Some(CoreKeyCode::Char(ch))
            }
        }
        Key::Named(NamedKey::Enter) => Some(CoreKeyCode::Enter),
        Key::Named(NamedKey::Escape) => Some(CoreKeyCode::Escape),
        Key::Named(NamedKey::Backspace) => Some(CoreKeyCode::Backspace),
        Key::Named(NamedKey::Tab) => Some(CoreKeyCode::Tab),
        Key::Named(NamedKey::Space) => Some(CoreKeyCode::Space),
        Key::Named(NamedKey::ArrowUp) => Some(CoreKeyCode::Up),
        Key::Named(NamedKey::ArrowDown) => Some(CoreKeyCode::Down),
        Key::Named(NamedKey::ArrowLeft) => Some(CoreKeyCode::Left),
        Key::Named(NamedKey::ArrowRight) => Some(CoreKeyCode::Right),
        _ => None,
    }
}

fn physical_key_to_core_code(physical_key: PhysicalKey) -> Option<CoreKeyCode> {
    let PhysicalKey::Code(code) = physical_key else {
        return None;
    };
    let ch = match code {
        WinitKeyCode::KeyA => 'a',
        WinitKeyCode::KeyB => 'b',
        WinitKeyCode::KeyC => 'c',
        WinitKeyCode::KeyD => 'd',
        WinitKeyCode::KeyE => 'e',
        WinitKeyCode::KeyF => 'f',
        WinitKeyCode::KeyG => 'g',
        WinitKeyCode::KeyH => 'h',
        WinitKeyCode::KeyI => 'i',
        WinitKeyCode::KeyJ => 'j',
        WinitKeyCode::KeyK => 'k',
        WinitKeyCode::KeyL => 'l',
        WinitKeyCode::KeyM => 'm',
        WinitKeyCode::KeyN => 'n',
        WinitKeyCode::KeyO => 'o',
        WinitKeyCode::KeyP => 'p',
        WinitKeyCode::KeyQ => 'q',
        WinitKeyCode::KeyR => 'r',
        WinitKeyCode::KeyS => 's',
        WinitKeyCode::KeyT => 't',
        WinitKeyCode::KeyU => 'u',
        WinitKeyCode::KeyV => 'v',
        WinitKeyCode::KeyW => 'w',
        WinitKeyCode::KeyX => 'x',
        WinitKeyCode::KeyY => 'y',
        WinitKeyCode::KeyZ => 'z',
        WinitKeyCode::Digit0 => '0',
        WinitKeyCode::Digit1 => '1',
        WinitKeyCode::Digit2 => '2',
        WinitKeyCode::Digit3 => '3',
        WinitKeyCode::Digit4 => '4',
        WinitKeyCode::Digit5 => '5',
        WinitKeyCode::Digit6 => '6',
        WinitKeyCode::Digit7 => '7',
        WinitKeyCode::Digit8 => '8',
        WinitKeyCode::Digit9 => '9',
        WinitKeyCode::Backquote => return Some(CoreKeyCode::Backquote),
        WinitKeyCode::Space => return Some(CoreKeyCode::Space),
        _ => return None,
    };
    Some(CoreKeyCode::Char(ch))
}

fn resolve_core_key_code(
    logical_key: &Key,
    unmodified_key: &Key,
    physical_key: PhysicalKey,
    mods: ModifiersState,
) -> Option<CoreKeyCode> {
    // macOS では Option(Alt)+d が '∂'、Option+Shift+d が 'Î' のように
    // logical_key が特殊文字や Dead キーに変化するため、修飾キー押下時は
    // key_without_modifiers() を優先し、さらに物理キーへフォールバックする。
    let has_shortcut_mod = mods.alt_key() || mods.control_key() || mods.super_key();
    if has_shortcut_mod || matches!(logical_key, Key::Dead(_)) {
        if let Some(code) = key_to_core_code(unmodified_key, mods)
            && !matches!(code, CoreKeyCode::Char(c) if !c.is_ascii())
        {
            return Some(code);
        }
        if let Some(code) = physical_key_to_core_code(physical_key) {
            return Some(code);
        }
    }
    key_to_core_code(logical_key, mods)
}

fn to_core_key_event(event: &WinitKeyEvent, mods: ModifiersState) -> Option<CoreKeyEvent> {
    let unmodified = event.key_without_modifiers();
    let code = resolve_core_key_code(&event.logical_key, &unmodified, event.physical_key, mods)?;

    Some(CoreKeyEvent {
        code,
        mods: Modifiers {
            ctrl: mods.control_key(),
            alt: mods.alt_key(),
            shift: mods.shift_key(),
            logo: mods.super_key(),
        },
        left_alt: mods.alt_key(),
    })
}

fn send_ipc_with_wsl_fallback(
    args: &[String],
    req: &MuxRequest,
    rt: &tokio::runtime::Runtime,
) -> Result<MuxResponse, String> {
    if let Ok(resp) = rt.block_on(send_request(DEFAULT_MUX_SERVER_ADDR, req)) {
        return Ok(resp);
    }
    match rt.block_on(send_request(DEFAULT_IPC_ADDR, req)) {
        Ok(resp) => Ok(resp),
        Err(err) if cfg!(target_os = "linux") && std::env::var_os("WSL_DISTRO_NAME").is_some() => {
            // WSL2 上で Windows 版 conductty.exe が動いている場合、PATH 上の Windows 側バイナリへフォワードする
            if let Ok(status) = Command::new("conductty.exe").args(&args[1..]).status() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Err(err)
        }
        Err(err) => Err(err),
    }
}

fn print_cli_help() {
    println!(
        "conductty CLI subcommands:
  panes | pane list                 List all panes
  spaces | space list               List all spaces (workspaces)
  agents | agent list               List detected AI agents and statuses
  capture <pane_id>                 Print visible text of a pane
  send <pane_id> <text>             Send keys/command to a pane
  broadcast <text>                  Broadcast keys/command to all panes
  split [--right | --down]          Split focused pane
  new-tab                           Create a new tab
  new-space [name]                  Create a new space (workspace)
  focus <pane_id>                   Focus a pane
  kill-pane <pane_id>               Close a pane
  kill-space <idx_or_name>          Close a space (except Master Space)
  theme [list]                      Pick a theme with live preview (plain list when piped)
  theme <name>                      Save theme to config and apply it to running conductty
  update [--check] [--force]        Update conductty from the latest GitHub release
  version | --version | -V          Print version"
    );
}

fn send_cli_action_ok(
    args: &[String],
    req: &MuxRequest,
    rt: &tokio::runtime::Runtime,
    ok_msg: &str,
) -> i32 {
    match send_ipc_with_wsl_fallback(args, req, rt) {
        Ok(MuxResponse::Ok { .. }) | Ok(MuxResponse::State { .. }) => {
            println!("{ok_msg}");
            0
        }
        Ok(MuxResponse::Error { message }) => {
            eprintln!("error: {message}");
            1
        }
        Ok(other) => {
            eprintln!("unexpected response: {other:?}");
            1
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn run_theme_cli(args: &[String], rt: &tokio::runtime::Runtime) -> i32 {
    // `conductty theme Tokyo Night Storm` のように引用符なしの複数語も受け付ける
    let name = args[2..].join(" ");
    let canonical = if name.is_empty() || name == "list" {
        use std::io::IsTerminal;
        let current = AppConfig::load().appearance.theme;
        if !(cfg!(unix) && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
            theme_picker::print_list(&current);
            return 0;
        }
        // ライブプレビュー: conductty 未起動なら接続失敗を無視して色見本だけで操作できる
        let preview = |theme: &str| {
            let req = MuxRequest::SetTheme {
                theme: theme.to_string(),
            };
            let _ = rt.block_on(send_request(DEFAULT_IPC_ADDR, &req));
        };
        match theme_picker::run(&current, &preview) {
            Ok(Some(selected)) => selected.to_string(),
            result => {
                preview(&current);
                if let Err(err) = result {
                    eprintln!("error: {err}");
                    return 1;
                }
                println!("theme unchanged: {current}");
                return 0;
            }
        }
    } else {
        let Some((canonical, _)) = find_theme(&name) else {
            eprintln!("error: unknown theme: {name}");
            theme_picker::print_list(&AppConfig::load().appearance.theme);
            return 1;
        };
        canonical
    };
    match AppConfig::save_theme_to_config(&canonical) {
        Ok(path) => println!("saved theme \"{canonical}\" to {}", path.display()),
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    }
    // テーマは GUI 側で描画するため、mux サーバーを経由せず GUI の IPC に直接送る
    let req = MuxRequest::SetTheme { theme: canonical };
    match rt.block_on(send_request(DEFAULT_IPC_ADDR, &req)) {
        Ok(MuxResponse::Ok { .. }) => println!("applied to running conductty"),
        Ok(MuxResponse::Error { message }) => {
            eprintln!("warning: running conductty did not apply the theme: {message}")
        }
        Ok(other) => eprintln!("warning: unexpected response: {other:?}"),
        Err(_) => println!("conductty is not running; the theme will be used on next launch"),
    }
    0
}

fn run_cli_subcommand(args: &[String], rt: &tokio::runtime::Runtime) -> Option<i32> {
    let sub = args.get(1)?.as_str();
    let sub2 = args.get(2).map(String::as_str);
    match (sub, sub2) {
        ("help" | "--help" | "-h", _) => {
            print_cli_help();
            Some(0)
        }
        ("version" | "--version" | "-V", _) => {
            updater::print_version();
            Some(0)
        }
        ("update", _) => Some(updater::run(&args[2..])),
        ("theme", _) => Some(run_theme_cli(args, rt)),
        ("panes", _) | ("pane", Some("list")) => {
            match send_ipc_with_wsl_fallback(args, &MuxRequest::ListPanes, rt) {
                Ok(MuxResponse::Panes { panes }) => {
                    for p in panes {
                        let mark = if p.focused { " *" } else { "" };
                        println!(
                            "[{}] {} / Tab {}{}",
                            p.pane_id, p.workspace, p.tab_index, mark
                        );
                    }
                    Some(0)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        ("spaces", _) | ("space", Some("list")) => {
            match send_ipc_with_wsl_fallback(args, &MuxRequest::ListSpaces, rt) {
                Ok(MuxResponse::Spaces { spaces }) => {
                    for s in spaces {
                        let mark = if s.active { " *" } else { "" };
                        println!(
                            "[{}] {} ({} tabs, {} panes){}",
                            s.index, s.name, s.tabs, s.panes, mark
                        );
                    }
                    Some(0)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        ("agents", _) | ("agent", Some("list")) => {
            match send_ipc_with_wsl_fallback(args, &MuxRequest::ListAgents, rt) {
                Ok(MuxResponse::Agents { agents }) => {
                    if agents.is_empty() {
                        println!("no active agents detected");
                    } else {
                        for a in agents {
                            println!(
                                "[{}] {} ({}) - {} / Tab {} | {}",
                                a.pane_id, a.kind, a.status, a.workspace, a.tab_index, a.summary
                            );
                        }
                    }
                    Some(0)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        ("capture", _) => {
            let Some(pane_str) = args.get(2) else {
                eprintln!("usage: conductty capture <pane_id>");
                return Some(2);
            };
            let Ok(pane_id) = pane_str.parse::<PaneId>() else {
                eprintln!("invalid pane_id: {pane_str}");
                return Some(2);
            };
            match send_ipc_with_wsl_fallback(args, &MuxRequest::CapturePane { pane_id }, rt) {
                Ok(MuxResponse::Captured { text, .. }) => {
                    println!("{text}");
                    Some(0)
                }
                Ok(MuxResponse::Error { message }) => {
                    eprintln!("error: {message}");
                    Some(1)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        ("split", _) => {
            let kind = match sub2 {
                Some("--down") | Some("-d") | Some("down") => SpawnKind::SplitDown,
                _ => SpawnKind::SplitRight,
            };
            Some(send_cli_action_ok(
                args,
                &MuxRequest::SpawnPane { kind },
                rt,
                "pane split",
            ))
        }
        ("new-tab", _) => Some(send_cli_action_ok(
            args,
            &MuxRequest::SpawnPane {
                kind: SpawnKind::NewTab,
            },
            rt,
            "created new tab",
        )),
        ("new-space" | "new-workspace", _) => {
            let req = MuxRequest::SpawnPane {
                kind: SpawnKind::NewWorkspace,
            };
            match send_ipc_with_wsl_fallback(args, &req, rt) {
                Ok(MuxResponse::State { manager, .. }) => {
                    let new_idx = manager.workspaces.len().saturating_sub(1);
                    if let Some(name) = args.get(2) {
                        let _ = send_ipc_with_wsl_fallback(
                            args,
                            &MuxRequest::RenameWorkspace {
                                idx: new_idx,
                                name: name.clone(),
                            },
                            rt,
                        );
                    }
                    println!("created space [{new_idx}]");
                    Some(0)
                }
                Ok(MuxResponse::Ok { .. }) => {
                    if let Some(name) = args.get(2)
                        && let Ok(MuxResponse::Spaces { spaces }) =
                            send_ipc_with_wsl_fallback(args, &MuxRequest::ListSpaces, rt)
                    {
                        let new_idx = spaces.len().saturating_sub(1);
                        let _ = send_ipc_with_wsl_fallback(
                            args,
                            &MuxRequest::RenameWorkspace {
                                idx: new_idx,
                                name: name.clone(),
                            },
                            rt,
                        );
                    }
                    println!("created new space");
                    Some(0)
                }
                Ok(MuxResponse::Error { message }) => {
                    eprintln!("error: {message}");
                    Some(1)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        ("focus", _) => {
            let Some(pane_str) = args.get(2) else {
                eprintln!("usage: conductty focus <pane_id>");
                return Some(2);
            };
            let Ok(pane_id) = pane_str.parse::<PaneId>() else {
                eprintln!("invalid pane_id: {pane_str}");
                return Some(2);
            };
            Some(send_cli_action_ok(
                args,
                &MuxRequest::FocusPane { pane_id },
                rt,
                &format!("focused pane {pane_id}"),
            ))
        }
        ("kill-pane", _) => {
            let Some(pane_str) = args.get(2) else {
                eprintln!("usage: conductty kill-pane <pane_id>");
                return Some(2);
            };
            let Ok(pane_id) = pane_str.parse::<PaneId>() else {
                eprintln!("invalid pane_id: {pane_str}");
                return Some(2);
            };
            Some(send_cli_action_ok(
                args,
                &MuxRequest::ClosePane { pane_id },
                rt,
                &format!("killed pane {pane_id}"),
            ))
        }
        ("kill-space" | "kill-workspace", _) => {
            let Some(target) = args.get(2) else {
                eprintln!("usage: conductty kill-space <index_or_name>");
                return Some(2);
            };
            let idx = if let Ok(n) = target.parse::<usize>() {
                n
            } else {
                match send_ipc_with_wsl_fallback(args, &MuxRequest::ListSpaces, rt) {
                    Ok(MuxResponse::Spaces { spaces }) => {
                        let Some(found) = spaces
                            .into_iter()
                            .find(|s| s.name.eq_ignore_ascii_case(target))
                        else {
                            eprintln!("space not found: {target}");
                            return Some(1);
                        };
                        found.index
                    }
                    Ok(other) => {
                        eprintln!("unexpected response: {other:?}");
                        return Some(1);
                    }
                    Err(err) => {
                        eprintln!("{err}");
                        return Some(1);
                    }
                }
            };
            Some(send_cli_action_ok(
                args,
                &MuxRequest::CloseWorkspace { idx },
                rt,
                &format!("killed space {idx}"),
            ))
        }
        ("send", _) => {
            let Some(pane_str) = args.get(2) else {
                eprintln!("usage: conductty send <pane_id> <text>");
                return Some(2);
            };
            let Ok(pane_id) = pane_str.parse::<PaneId>() else {
                eprintln!("invalid pane_id: {pane_str}");
                return Some(2);
            };
            let raw_text = args.get(3).map(String::as_str).unwrap_or("");
            let text = raw_text
                .replace("\\r\\n", "\r")
                .replace("\\n", "\r")
                .replace("\\r", "\r")
                .replace('\n', "\r");
            let req = MuxRequest::SendKeys {
                pane_id: Some(pane_id),
                text,
                broadcast: false,
            };
            match send_ipc_with_wsl_fallback(args, &req, rt) {
                Ok(MuxResponse::Ok { delivered_to }) => {
                    println!("sent to {delivered_to} pane(s)");
                    Some(0)
                }
                Ok(MuxResponse::Error { message }) => {
                    eprintln!("error: {message}");
                    Some(1)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        ("broadcast", _) => {
            let raw_text = args.get(2).map(String::as_str).unwrap_or("");
            let text = raw_text
                .replace("\\r\\n", "\r")
                .replace("\\n", "\r")
                .replace("\\r", "\r")
                .replace('\n', "\r");
            let req = MuxRequest::SendKeys {
                pane_id: None,
                text,
                broadcast: true,
            };
            match send_ipc_with_wsl_fallback(args, &req, rt) {
                Ok(MuxResponse::Ok { delivered_to }) => {
                    println!("broadcast to {delivered_to} pane(s)");
                    Some(0)
                }
                Ok(other) => {
                    eprintln!("unexpected response: {other:?}");
                    Some(1)
                }
                Err(err) => {
                    eprintln!("{err}");
                    Some(1)
                }
            }
        }
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn attach_parent_console_for_cli(args: &[String]) {
    if args.get(1).is_some_and(|s| s != "--mux") {
        unsafe extern "system" {
            fn AttachConsole(dw_process_id: u32) -> i32;
        }
        unsafe {
            let _ = AttachConsole(u32::MAX);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(target_os = "windows")]
    attach_parent_console_for_cli(&args);
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    if let Some(code) = run_cli_subcommand(&args, &rt) {
        std::process::exit(code);
    }

    let mut config = AppConfig::load();
    if args.iter().any(|a| a == "--mux") {
        config.features.multiplexer_mode = true;
    }
    let theme = config.resolve_theme();
    let router = InputRouter::from_config(&config);
    let left_drawer_mode = config.features.left_drawer_mode;
    let right_drawer_open =
        config.features.right_drawer && config.features.right_drawer_default_open;
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();

    let mut app = App {
        config,
        theme,
        rt,
        proxy,
        window: None,
        gpu: None,
        egui_ctx: egui::Context::default(),
        egui_state: None,
        manager: WorkspaceManager::new(1),
        panes: HashMap::new(),
        mux_snapshots: HashMap::new(),
        mux_last_sizes: HashMap::new(),
        mux_error: None,
        next_pane_id: 1,
        pane_mode_active: false,
        go_to_open: false,
        go_to_filtering: false,
        go_to_query: String::new(),
        go_to_selected_idx: 0,
        keybind_hints_open: false,
        keybind_hints_scroll_offset: 0.0,
        keybind_hints_max_scroll: 0.0,
        agent_states: HashMap::new(),
        agent_scan_start: std::time::Instant::now(),
        last_agent_scan: std::time::Instant::now(),
        left_drawer_mode,
        left_drawer_selected_idx: 0,
        renaming_workspace_idx: None,
        rename_buf: String::new(),
        right_drawer_open,
        active_plugin_idx: 0,
        drawer_snapshot: DrawerSnapshot::default(),
        drawer_cache: HashMap::new(),
        drawer_selected_idx: 0,
        drawer_scroll_to_selected: false,
        drawer_preview_item: None,
        drawer_input_action: None,
        drawer_input_buf: String::new(),
        cursor_pos: Pos2::ZERO,
        cell_size: (10.0, 22.0),
        selection: None,
        selecting: false,
        clipboard: arboard::Clipboard::new().ok(),
        browser: None,
        pending_browser_url: None,
        pending_browser_html: None,
        browser_error: None,
        kitty_textures: HashMap::new(),
        router,
        modifiers: ModifiersState::empty(),
        suppress_next_ime_commit: false,
        ime_preedit: None,
        last_terminal_area: None,
        pty_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        window_focused: true,
        window_occluded: false,
        pending_occluded_redraw: false,
        last_redraw_at: std::time::Instant::now(),
        pending_redraw_deadline: None,
        last_agent_grid_gen: HashMap::new(),
    };
    let _ = event_loop.run_app(&mut app);
}

/// キー入力/IME を常に egui へ渡すと Tab でウィジェット間フォーカス移動が起きるため、TextEdit 編集中のみ渡す
fn should_forward_event_to_egui(is_text_editing: bool, event: &WindowEvent) -> bool {
    is_text_editing
        || !matches!(
            event,
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_)
        )
}

fn should_throttle_agent_scan(elapsed: std::time::Duration) -> bool {
    elapsed < std::time::Duration::from_millis(250)
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_core::config::{THEME_PRESETS, resolve_theme_preset};

    #[test]
    fn egui_receives_key_and_ime_events_only_while_text_editing() {
        // winit の KeyEvent は外部から構築できないため、同じ分岐の Ime で検証する
        let ime = WindowEvent::Ime(winit::event::Ime::Commit("a".into()));
        assert!(!should_forward_event_to_egui(false, &ime));
        assert!(should_forward_event_to_egui(true, &ime));

        let non_key = [
            WindowEvent::ModifiersChanged(winit::event::Modifiers::default()),
            WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600)),
            WindowEvent::CursorLeft {
                device_id: winit::event::DeviceId::dummy(),
            },
        ];
        for ev in &non_key {
            assert!(should_forward_event_to_egui(false, ev));
            assert!(should_forward_event_to_egui(true, ev));
        }
    }

    struct NoNav;
    impl PerformableContext for NoNav {
        fn can_goto_split(&self, _dir: NavDirection) -> bool {
            false
        }
    }

    #[test]
    fn macos_option_d_and_option_shift_d_trigger_split_actions() {
        let mut router = InputRouter::default();

        // macOS Option+d produces logical_key = "∂", key_without_modifiers = "d"
        let alt_mods = ModifiersState::ALT;
        let code_right = resolve_core_key_code(
            &Key::Character("∂".into()),
            &Key::Character("d".into()),
            PhysicalKey::Code(WinitKeyCode::KeyD),
            alt_mods,
        )
        .unwrap();
        let ev_right = CoreKeyEvent {
            code: code_right,
            mods: Modifiers {
                ctrl: false,
                alt: true,
                shift: false,
                logo: false,
            },
            left_alt: true,
        };
        assert_eq!(
            router.route(&ev_right, &NoNav),
            InputDecision::Execute(Action::SplitRight)
        );

        // macOS Option+Shift+d produces logical_key = "Î", key_without_modifiers = "d"
        let alt_shift_mods = ModifiersState::ALT | ModifiersState::SHIFT;
        let code_down = resolve_core_key_code(
            &Key::Character("Î".into()),
            &Key::Character("d".into()),
            PhysicalKey::Code(WinitKeyCode::KeyD),
            alt_shift_mods,
        )
        .unwrap();
        let ev_down = CoreKeyEvent {
            code: code_down,
            mods: Modifiers {
                ctrl: false,
                alt: true,
                shift: true,
                logo: false,
            },
            left_alt: true,
        };
        assert_eq!(
            router.route(&ev_down, &NoNav),
            InputDecision::Execute(Action::SplitDown)
        );
    }

    #[test]
    fn shift_symbol_preserves_logical_key_for_prefix_hints() {
        let code = resolve_core_key_code(
            &Key::Character("?".into()),
            &Key::Character("/".into()),
            PhysicalKey::Code(WinitKeyCode::Slash),
            ModifiersState::SHIFT,
        )
        .unwrap();
        assert_eq!(code, CoreKeyCode::Char('?'));
    }

    #[test]
    fn font_scoring_prefers_regular_mono_over_bold_italic_propo() {
        assert!(font_candidate_score("cicaregular") > font_candidate_score("cicabold"));
        assert!(font_candidate_score("cicaregular") > font_candidate_score("cicabolditalic"));
        assert!(
            font_candidate_score("firacodenerdfontmonoregular")
                > font_candidate_score("firacodenerdfontproporegular")
        );
        assert!(
            font_candidate_score("firacodenerdfontmonoregular")
                > font_candidate_score("firacodenerdfontmonolight")
        );
    }

    #[test]
    fn agent_scan_throttles_under_250ms_even_when_empty() {
        assert!(should_throttle_agent_scan(
            std::time::Duration::from_millis(0)
        ));
        assert!(should_throttle_agent_scan(
            std::time::Duration::from_millis(249)
        ));
        assert!(!should_throttle_agent_scan(
            std::time::Duration::from_millis(250)
        ));
    }

    #[test]
    fn light_presets_use_light_visuals() {
        for name in THEME_PRESETS {
            let (_, theme) = resolve_theme_preset(name).unwrap();
            let expect_light = matches!(
                *name,
                "Solarized Light" | "GitHub Light" | "Gruvbox Light" | "Tokyo Night Day"
            );
            assert_eq!(is_light_theme(&theme), expect_light, "{name}");
        }
    }
}
