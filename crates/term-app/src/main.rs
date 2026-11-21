mod browser;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use browser::{BrowserBounds, EmbeddedBrowser};
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Stroke, Vec2};
use term_core::config::AppConfig;
use term_core::grid::TerminalGrid;
use term_core::input::{
    Action, InputDecision, InputMode, InputRouter, KeyCode as CoreKeyCode, KeyEvent as CoreKeyEvent,
    Modifiers, PerformableContext,
};
use term_core::layout::{NavDirection, PaneId, Rect as CoreRect, SplitDirection, WorkspaceManager};
use term_core::pty::PtySession;
use term_drawer::{DrawerSnapshot, GitHubItem, GitHubItemKind, fetch_github_snapshot};
use term_render::tokyonight_moon_bg;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const LEFT_DRAWER_WIDTH: f32 = 240.0;
const LEFT_RAIL_WIDTH: f32 = 32.0;
const TAB_BAR_HEIGHT: f32 = 32.0;
const RIGHT_DRAWER_WIDTH: f32 = 340.0;
const RIGHT_BROWSER_WIDTH: f32 = 560.0;

#[derive(Debug, Clone)]
enum UserEvent {
    PtyOutput,
    PaneExited(PaneId),
    DrawerUpdated(DrawerSnapshot),
}

struct PaneRuntime {
    pty: PtySession,
    grid: Arc<Mutex<TerminalGrid>>,
    last_size: (usize, usize),
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
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let surface = instance
            .create_surface(window)
            .expect("failed to create wgpu surface");

        let adapter = rt
            .block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            }))
            .expect("failed to find suitable GPU adapter");

        let (device, queue) = rt
            .block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
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
                .find(|m| {
                    matches!(
                        m,
                        wgpu::CompositeAlphaMode::PreMultiplied
                            | wgpu::CompositeAlphaMode::PostMultiplied
                    )
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
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

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
        full_output: egui::FullOutput,
        bg: wgpu::Color,
    ) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

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
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(bg),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                })
                .forget_lifetime();

            self.egui_renderer
                .render(&mut pass, &clipped_primitives, &screen_descriptor);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        Ok(())
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

struct App {
    config: AppConfig,
    rt: tokio::runtime::Runtime,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    manager: WorkspaceManager,
    panes: HashMap<PaneId, PaneRuntime>,
    next_pane_id: PaneId,
    go_to_active: bool,
    left_drawer_open: bool,
    left_drawer_selected_idx: usize,
    renaming_workspace_idx: Option<usize>,
    rename_buf: String,
    right_drawer_open: bool,
    drawer_snapshot: DrawerSnapshot,
    drawer_selected_idx: usize,
    drawer_scroll_to_selected: bool,
    cursor_pos: Pos2,
    cell_size: (f32, f32),
    selection: Option<(PaneId, (usize, usize), (usize, usize))>,
    selecting: bool,
    clipboard: Option<arboard::Clipboard>,
    browser: Option<EmbeddedBrowser>,
    pending_browser_url: Option<String>,
    browser_error: Option<String>,
    kitty_textures: HashMap<(PaneId, u32), egui::TextureHandle>,
    router: InputRouter,
    modifiers: ModifiersState,
}

impl App {
    fn terminal_area(&self) -> CoreRect {
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

        let left_w = if self.left_drawer_open {
            LEFT_DRAWER_WIDTH.min(w * 0.30)
        } else {
            LEFT_RAIL_WIDTH
        };

        let right_w = if self.right_drawer_open {
            if self.browser.is_some() {
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

    fn spawn_pane(&mut self) -> Option<PaneId> {
        let id = self.next_pane_id;
        self.next_pane_id += 1;

        match PtySession::spawn(120, 36, &self.config.shell.windows_default_prog) {
            Ok(mut pty) => {
                let (_, dummy_rx) = std::sync::mpsc::channel();
                let rx = std::mem::replace(&mut pty.output_rx, dummy_rx);
                let (_, dummy_exit_rx) = std::sync::mpsc::channel();
                let exit_rx = std::mem::replace(&mut pty.exit_rx, dummy_exit_rx);

                let grid = Arc::new(Mutex::new(TerminalGrid::new(120, 36)));
                let grid_clone = Arc::clone(&grid);
                let proxy_out = self.proxy.clone();
                thread::spawn(move || {
                    while let Ok(bytes) = rx.recv() {
                        if let Ok(mut g) = grid_clone.lock() {
                            g.feed_bytes(&bytes);
                        }
                        let _ = proxy_out.send_event(UserEvent::PtyOutput);
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
                        last_size: (120, 36),
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

    fn trigger_drawer_refresh(&mut self) {
        self.drawer_snapshot.is_loading = true;
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let snapshot = fetch_github_snapshot().await;
            let _ = proxy.send_event(UserEvent::DrawerUpdated(snapshot));
        });
    }

    fn ordered_drawer_items(&self) -> Vec<GitHubItem> {
        let mut ordered = Vec::with_capacity(self.drawer_snapshot.items.len());
        for kind in [
            GitHubItemKind::ReviewRequestedPr,
            GitHubItemKind::AssignedPr,
            GitHubItemKind::AssignedIssue,
        ] {
            ordered.extend(
                self.drawer_snapshot
                    .items
                    .iter()
                    .filter(|i| i.kind == kind)
                    .cloned(),
            );
        }
        ordered
    }

    fn selected_drawer_item(&self) -> Option<GitHubItem> {
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
        if !text.is_empty() {
            if let Some(cb) = &mut self.clipboard {
                let _ = cb.set_text(text);
            }
        }
    }

    fn paste_clipboard(&mut self) {
        let Some(cb) = &mut self.clipboard else {
            return;
        };
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                let focused = self.manager.active_tab().focused_pane;
                if let Some(pane) = self.panes.get_mut(&focused) {
                    if let Ok(mut g) = pane.grid.lock() {
                        g.reset_scroll();
                    }
                    let _ = pane.pty.write_bytes(text.as_bytes());
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

    fn handle_action(&mut self, action: Action) {
        let area = self.terminal_area();
        match action {
            Action::SplitRight => {
                if let Some(new_id) = self.spawn_pane() {
                    self.manager
                        .active_tab_mut()
                        .split_focused(new_id, SplitDirection::Horizontal);
                }
            }
            Action::SplitDown => {
                if let Some(new_id) = self.spawn_pane() {
                    self.manager
                        .active_tab_mut()
                        .split_focused(new_id, SplitDirection::Vertical);
                }
            }
            Action::GotoSplit(dir) => {
                self.manager.active_tab_mut().goto_split(dir, area);
            }
            Action::ToggleZoom => {
                self.manager.active_tab_mut().toggle_zoom();
            }
            Action::OpenGoTo => {
                self.go_to_active = !self.go_to_active;
            }
            Action::ToggleLeftDrawer => {
                self.left_drawer_open = true;
                self.left_drawer_selected_idx = self.manager.active_workspace;
            }
            Action::ToggleLeftDrawerCollapse => {
                self.left_drawer_open = !self.left_drawer_open;
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
                    self.panes.remove(&pid);
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
                self.right_drawer_open = true;
                self.drawer_selected_idx = 0;
                self.drawer_scroll_to_selected = true;
                if self.drawer_snapshot.items.is_empty() && !self.drawer_snapshot.is_loading {
                    self.trigger_drawer_refresh();
                }
            }
            Action::CloseRightDrawer => {
                self.right_drawer_open = false;
                if self.browser.take().is_some() {
                    if let Some(window) = &self.window {
                        window.focus_window();
                    }
                }
                self.router.mode = InputMode::Normal;
            }
            Action::DrawerNext => {
                let len = self.drawer_snapshot.items.len();
                if len > 0 {
                    self.drawer_selected_idx = (self.drawer_selected_idx + 1).min(len - 1);
                    self.drawer_scroll_to_selected = true;
                }
            }
            Action::DrawerPrev => {
                self.drawer_selected_idx = self.drawer_selected_idx.saturating_sub(1);
                self.drawer_scroll_to_selected = true;
            }
            Action::DrawerOpen => {
                if let Some(item) = self.selected_drawer_item() {
                    open_url(&item.url);
                }
            }
            Action::DrawerPreview => {
                if let Some(item) = self.selected_drawer_item() {
                    self.right_drawer_open = true;
                    self.pending_browser_url = Some(item.url);
                }
            }
            Action::DrawerSendToPane => {
                if let Some(item) = self.selected_drawer_item() {
                    let text = format!("[{} #{}] {} ({})", item.repo, item.number, item.title, item.url);
                    let focused = self.manager.active_tab().focused_pane;
                    if let Some(pane) = self.panes.get_mut(&focused) {
                        let _ = pane.pty.write_bytes(text.as_bytes());
                    }
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
            Action::EnterPaneMode | Action::ExitPaneMode => {}
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // デザイン方針として不透明 (TokyoNight Moon #222436) を基本とする
            let transparent = false;
            let attrs = WindowAttributes::default()
                .with_title("conductty")
                .with_transparent(transparent);
            let window = Arc::new(
                event_loop
                    .create_window(attrs)
                    .expect("failed to create window"),
            );

            configure_fonts(&self.egui_ctx, &self.config.appearance.font_families);
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
            self.trigger_drawer_refresh();
        }

        if self.panes.is_empty() {
            if let Some(first_id) = self.spawn_pane() {
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
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput => {
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
                    window.request_redraw();
                }
            }
            UserEvent::PaneExited(id) => {
                self.panes.remove(&id);
                if !self.manager.close_pane(id) {
                    self.save_session();
                    event_loop.exit();
                    return;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            UserEvent::DrawerUpdated(snapshot) => {
                if let Some(err) = &snapshot.last_error {
                    eprintln!("[conductty drawer error] {err}");
                }
                self.drawer_snapshot = snapshot;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(url) = self.pending_browser_url.take() {
            self.right_drawer_open = true;
            self.browser_error = None;
            let bounds = self.right_browser_bounds();
            if let Some(browser) = &mut self.browser {
                browser.load_url(&url);
                if let Some(parent) = &self.window {
                    browser.sync_to_parent(parent, bounds);
                }
            } else if let Some(parent) = &self.window {
                match EmbeddedBrowser::open(event_loop, parent, &url, bounds) {
                    Ok(b) => self.browser = Some(b),
                    Err(err) => self.browser_error = Some(err),
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self
            .browser
            .as_ref()
            .is_some_and(|b| b.window_id() == id)
        {
            if matches!(event, WindowEvent::CloseRequested) {
                self.browser = None;
                if let Some(window) = &self.window {
                    window.focus_window();
                    window.request_redraw();
                }
            }
            return;
        }

        if let (Some(window), Some(egui_state)) = (&self.window, &mut self.egui_state) {
            let response = egui_state.on_window_event(window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.save_session();
                event_loop.exit();
            }
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
            }
            WindowEvent::Resized(size) => {
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
                if self.selecting {
                    if let Some((pane_id, col, row)) = self.hit_test_cell(self.cursor_pos) {
                        if let Some((sel_pane, start, _)) = self.selection {
                            if sel_pane == pane_id {
                                self.selection = Some((sel_pane, start, (col, row)));
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                            }
                        }
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
                    (MouseButton::Left, ElementState::Released) => {
                        if self.selecting {
                            self.selecting = false;
                            // copy-on-select = true
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
                        if let Some(pane) = self.panes.get(&focused) {
                            if let Ok(mut g) = pane.grid.lock() {
                                if lines > 0 {
                                    g.scroll_up(lines as usize);
                                } else {
                                    g.scroll_down((-lines) as usize);
                                }
                            }
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if self.router.mode == InputMode::Normal && !self.go_to_active {
                    let focused = self.manager.active_tab().focused_pane;
                    if let Some(pane) = self.panes.get_mut(&focused) {
                        let _ = pane.pty.write_bytes(text.as_bytes());
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Workspace 名のリネーム入力中は egui TextEdit にキー入力を委ねる
                if self.renaming_workspace_idx.is_some() {
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                if self.go_to_active {
                    if let Key::Character(s) = &event.logical_key {
                        if let Ok(num) = s.parse::<PaneId>() {
                            if self.panes.contains_key(&num) {
                                self.manager.active_tab_mut().focused_pane = num;
                                self.go_to_active = false;
                                if let Some(window) = &self.window {
                                    window.request_redraw();
                                }
                                return;
                            }
                        }
                    }
                    if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                        self.go_to_active = false;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                }

                if let Some(core_ev) = to_core_key_event(&event.logical_key, self.modifiers) {
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
                            let focused = self.manager.active_tab().focused_pane;
                            if let Some(pane) = self.panes.get_mut(&focused) {
                                if let Ok(mut g) = pane.grid.lock() {
                                    g.reset_scroll();
                                }
                                let _ = pane.pty.write_bytes(&bytes);
                            }
                        }
                        InputDecision::Execute(action) => {
                            self.handle_action(action);
                        }
                        InputDecision::Ignore => {}
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let area = self.terminal_area();
                let visible_panes = self.manager.active_tab().visible_rects(area);
                let focused_pane = self.manager.active_tab().focused_pane;
                let pane_mode = self.router.mode == InputMode::Pane;
                let zoomed = self.manager.active_tab().zoomed;
                let go_to_active = self.go_to_active;
                let left_drawer_open = self.left_drawer_open;
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
                    .map(|ws| (ws.name.clone(), ws.tabs.len(), ws.active_tab, ws.pane_count()))
                    .collect();
                let right_drawer_open = self.right_drawer_open;
                let drawer_snapshot = self.drawer_snapshot.clone();

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
                let mut toggle_left_collapse = false;
                let mut switch_ws_target: Option<usize> = None;
                let mut switch_tab_target: Option<(usize, usize)> = None;
                let mut new_ws_requested = false;
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
                let mut open_url_target: Option<String> = None;
                let mut preview_url_target: Option<String> = None;
                let mut send_to_pane_text: Option<String> = None;
                let browser_active = self.browser.is_some();
                let browser_url = self.browser.as_ref().map(|b| b.url.clone());
                let browser_error = self.browser_error.clone();
                let drawer_panel_w = if browser_active {
                    RIGHT_BROWSER_WIDTH.min((area.width + RIGHT_BROWSER_WIDTH) * 0.55)
                } else {
                    RIGHT_DRAWER_WIDTH
                };
                let scroll_to_selected = self.drawer_scroll_to_selected;
                self.drawer_scroll_to_selected = false;
                let mut pending_resizes: Vec<(PaneId, usize, usize)> = Vec::new();

                let selection = self.selection;
                let mut computed_cell_size = self.cell_size;

                let full_output = self.egui_ctx.run(raw_input, |ctx| {
                    if left_drawer_open {
                        egui::SidePanel::left("conductty_left_drawer")
                            .exact_width(LEFT_DRAWER_WIDTH)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(0x1e, 0x20, 0x30))
                                    .inner_margin(12.0),
                            )
                            .show(ctx, |ui| {
                                ui.horizontal(|ui| {
                                    ui.heading(
                                        egui::RichText::new("Workspaces")
                                            .color(Color32::from_rgb(0xc8, 0xd3, 0xf5)),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("<<").clicked() {
                                                toggle_left_collapse = true;
                                            }
                                            if ui.button("+ New").clicked() {
                                                new_ws_requested = true;
                                            }
                                        },
                                    );
                                });
                                ui.separator();

                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for (idx, (name, tab_count, active_t, pane_count)) in
                                        ws_summaries.iter().enumerate()
                                    {
                                        let is_active = idx == active_ws_idx;
                                        let is_selected = idx == left_selected;
                                        let is_master = idx == 0;
                                        let accent = if is_master {
                                            Color32::from_rgb(0xff, 0xc7, 0x77)
                                        } else {
                                            Color32::from_rgb(0x7a, 0xa2, 0xf7)
                                        };
                                        let fill = if is_active {
                                            Color32::from_rgb(0x3b, 0x42, 0x61)
                                        } else {
                                            Color32::from_rgb(0x2f, 0x33, 0x4d)
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
                                                    .color(Color32::from_rgb(0x82, 0x8b, 0xb8)),
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
                                });

                                ui.separator();
                                ui.label(
                                    egui::RichText::new(
                                        "j/k: select | Enter: switch | n: new | r: rename | d: close",
                                    )
                                    .small()
                                    .color(Color32::from_rgb(0x82, 0x8b, 0xb8)),
                                );
                            });
                    } else {
                        egui::SidePanel::left("conductty_left_rail")
                            .exact_width(LEFT_RAIL_WIDTH)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(0x1e, 0x20, 0x30))
                                    .inner_margin(4.0),
                            )
                            .show(ctx, |ui| {
                                ui.vertical_centered(|ui| {
                                    if ui.button(">").clicked() {
                                        toggle_left_collapse = true;
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
                            });
                    }

                    if right_drawer_open {
                        egui::SidePanel::right("conductty_right_drawer")
                            .exact_width(drawer_panel_w)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(0x1e, 0x20, 0x30))
                                    .inner_margin(12.0),
                            )
                            .show(ctx, |ui| {
                                if let Some(url) = &browser_url {
                                    ui.horizontal(|ui| {
                                        if ui.button("<- List").clicked() {
                                            close_browser_requested = true;
                                        }
                                        if ui.button("External").clicked() {
                                            open_url_target = Some(url.clone());
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

                                ui.horizontal(|ui| {
                                    ui.heading(
                                        egui::RichText::new("GitHub")
                                            .color(Color32::from_rgb(0xc8, 0xd3, 0xf5)),
                                    );
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
                                        egui::RichText::new("Fetching via gh CLI...")
                                            .color(Color32::from_rgb(0x82, 0xaa, 0xff)),
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

                                        let mut global_idx = 0usize;
                                        render_github_section(
                                            ui,
                                            "Review Requested",
                                            Color32::from_rgb(0xff, 0x9e, 0x64),
                                            &drawer_snapshot.items,
                                            GitHubItemKind::ReviewRequestedPr,
                                            &mut global_idx,
                                            self.drawer_selected_idx,
                                            scroll_to_selected,
                                            &mut open_url_target,
                                            &mut preview_url_target,
                                            &mut send_to_pane_text,
                                        );
                                        render_github_section(
                                            ui,
                                            "Assigned PRs",
                                            Color32::from_rgb(0x7a, 0xa2, 0xf7),
                                            &drawer_snapshot.items,
                                            GitHubItemKind::AssignedPr,
                                            &mut global_idx,
                                            self.drawer_selected_idx,
                                            scroll_to_selected,
                                            &mut open_url_target,
                                            &mut preview_url_target,
                                            &mut send_to_pane_text,
                                        );
                                        render_github_section(
                                            ui,
                                            "Assigned Issues",
                                            Color32::from_rgb(0xc3, 0xe8, 0x8d),
                                            &drawer_snapshot.items,
                                            GitHubItemKind::AssignedIssue,
                                            &mut global_idx,
                                            self.drawer_selected_idx,
                                            scroll_to_selected,
                                            &mut open_url_target,
                                            &mut preview_url_target,
                                            &mut send_to_pane_text,
                                        );
                                    });

                                ui.separator();
                                ui.label(
                                    egui::RichText::new(
                                        "j/k: select | b: preview | o: open | s: send | r: refresh | q/Esc: close",
                                    )
                                    .small()
                                    .color(Color32::from_rgb(0x82, 0x8b, 0xb8)),
                                );
                            });
                    }

                    if active_tabs.len() > 1 {
                        egui::TopBottomPanel::top("conductty_tab_bar")
                            .exact_height(TAB_BAR_HEIGHT)
                            .frame(
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(0x18, 0x1a, 0x28))
                                    .inner_margin(egui::Margin::symmetric(8, 4)),
                            )
                            .show(ctx, |ui| {
                                ui.horizontal(|ui| {
                                    for (t_idx, (_tab_id, title)) in active_tabs.iter().enumerate() {
                                        let is_active = t_idx == active_tab_idx;
                                        let fill = if is_active {
                                            Color32::from_rgb(0x3b, 0x42, 0x61)
                                        } else {
                                            Color32::from_rgb(0x24, 0x28, 0x3b)
                                        };
                                        let stroke = if is_active {
                                            Stroke::new(1.0_f32, Color32::from_rgb(0x7a, 0xa2, 0xf7))
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
                                                        Color32::from_rgb(0xc8, 0xd3, 0xf5)
                                                    } else {
                                                        Color32::from_rgb(0x82, 0x8b, 0xb8)
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
                                                                    .color(Color32::from_rgb(
                                                                        0x82, 0x8b, 0xb8,
                                                                    )),
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
                                                    .color(Color32::from_rgb(0x7a, 0xa2, 0xf7)),
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
                        .show(ctx, |ui| {
                            let font_id = FontId::new(font_size, FontFamily::Monospace);
                            let char_w = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
                            let row_h = font_size + cell_extra_h + 2.0;
                            computed_cell_size = (char_w, row_h);

                            for (pane_id, rect) in &visible_panes {
                                let egui_rect = Rect::from_min_size(
                                    Pos2::new(rect.x, rect.y),
                                    Vec2::new(rect.width, rect.height),
                                );
                                let is_focused = *pane_id == focused_pane;
                                let painter = ui.painter().with_clip_rect(egui_rect);

                                let border_color = if is_focused {
                                    Color32::from_rgb(0x7a, 0xa2, 0xf7)
                                } else {
                                    Color32::from_rgb(0x3b, 0x42, 0x61)
                                };
                                painter.rect_stroke(
                                    egui_rect.shrink(1.0),
                                    2.0,
                                    Stroke::new(
                                        if is_focused { 1.5_f32 } else { 1.0_f32 },
                                        border_color,
                                    ),
                                    egui::StrokeKind::Inside,
                                );

                                if let Some(runtime) = self.panes.get(pane_id) {
                                    let target_cols =
                                        ((rect.width - pad_x * 2.0) / char_w).floor().max(10.0)
                                            as usize;
                                    let target_rows =
                                        ((rect.height - pad_y * 2.0) / row_h).floor().max(4.0)
                                            as usize;
                                    if (target_cols, target_rows) != runtime.last_size {
                                        pending_resizes.push((*pane_id, target_cols, target_rows));
                                    }

                                    if let Ok(g) = runtime.grid.lock() {
                                        // 選択範囲のハイライト描画 (#3d59a1)
                                        if let Some((sel_pane, a, b)) = selection {
                                            if sel_pane == *pane_id && a != b {
                                                let ((sc, sr), (ec, er)) = if (a.1, a.0) <= (b.1, b.0) {
                                                    (a, b)
                                                } else {
                                                    (b, a)
                                                };
                                                for row in sr..=er.min(g.rows.saturating_sub(1)) {
                                                    let c_start = if row == sr { sc } else { 0 };
                                                    let c_end = if row == er {
                                                        ec.min(g.cols.saturating_sub(1))
                                                    } else {
                                                        g.cols.saturating_sub(1)
                                                    };
                                                    if c_end >= c_start {
                                                        let sel_rect = Rect::from_min_size(
                                                            Pos2::new(
                                                                rect.x
                                                                    + pad_x
                                                                    + c_start as f32 * char_w,
                                                                rect.y + pad_y + row as f32 * row_h,
                                                            ),
                                                            Vec2::new(
                                                                (c_end - c_start + 1) as f32
                                                                    * char_w,
                                                                row_h,
                                                            ),
                                                        );
                                                        painter.rect_filled(
                                                            sel_rect,
                                                            0.0,
                                                            Color32::from_rgba_unmultiplied(
                                                                0x3d, 0x59, 0xa1, 140,
                                                            ),
                                                        );
                                                    }
                                                }
                                            }
                                        }

                                        for row in 0..g.rows {
                                            let y = rect.y + pad_y + row as f32 * row_h;
                                            let mut col = 0;
                                            while col < g.cols {
                                                let Some(first) = g.visible_cell(col, row) else {
                                                    break;
                                                };
                                                let fg = first.fg;
                                                let bg = first.bg;
                                                let mut run = String::new();
                                                let mut box_chars: Vec<(usize, char)> = Vec::new();
                                                let mut non_ascii_chars: Vec<(usize, char)> =
                                                    Vec::new();
                                                let start_col = col;
                                                while col < g.cols {
                                                    if let Some(cell) = g.visible_cell(col, row) {
                                                        if cell.fg == fg && cell.bg == bg {
                                                            if is_box_drawing_char(cell.ch) {
                                                                box_chars.push((col, cell.ch));
                                                                run.push(' ');
                                                            } else if !cell.ch.is_ascii() {
                                                                non_ascii_chars.push((col, cell.ch));
                                                                run.push(' ');
                                                            } else {
                                                                run.push(cell.ch);
                                                            }
                                                            col += 1;
                                                            continue;
                                                        }
                                                    }
                                                    break;
                                                }

                                                if let Some(bg_rgb) = bg {
                                                    let x0 = (rect.x
                                                        + pad_x
                                                        + start_col as f32 * char_w)
                                                        .round();
                                                    let x1 =
                                                        (rect.x + pad_x + col as f32 * char_w)
                                                            .round();
                                                    let bg_rect = Rect::from_min_max(
                                                        Pos2::new(x0, y.round()),
                                                        Pos2::new(x1, (y + row_h).round()),
                                                    );
                                                    painter.rect_filled(
                                                        bg_rect,
                                                        0.0,
                                                        Color32::from_rgb(
                                                            bg_rgb.r, bg_rgb.g, bg_rgb.b,
                                                        ),
                                                    );
                                                }

                                                let fg_color = Color32::from_rgb(fg.r, fg.g, fg.b);
                                                for (b_col, b_ch) in box_chars {
                                                    let cell_rect = Rect::from_min_size(
                                                        Pos2::new(
                                                            (rect.x + pad_x + b_col as f32 * char_w)
                                                                .round(),
                                                            y.round(),
                                                        ),
                                                        Vec2::new(char_w.round(), row_h.round()),
                                                    );
                                                    draw_box_drawing(
                                                        &painter,
                                                        b_ch,
                                                        cell_rect,
                                                        fg_color,
                                                    );
                                                }

                                                let trimmed = run.trim_end();
                                                if !trimmed.is_empty() {
                                                    let pos = Pos2::new(
                                                        (rect.x + pad_x + start_col as f32 * char_w)
                                                            .round(),
                                                        y.round(),
                                                    );
                                                    painter.text(
                                                        pos,
                                                        egui::Align2::LEFT_TOP,
                                                        trimmed,
                                                        font_id.clone(),
                                                        fg_color,
                                                    );
                                                }

                                                // 日本語や Nerd Font アイコンはグリッド座標に厳密固定して描画する
                                                let mut buf = [0u8; 4];
                                                for (n_col, n_ch) in non_ascii_chars {
                                                    let pos = Pos2::new(
                                                        (rect.x + pad_x + n_col as f32 * char_w)
                                                            .round(),
                                                        y.round(),
                                                    );
                                                    let s = n_ch.encode_utf8(&mut buf);
                                                    painter.text(
                                                        pos,
                                                        egui::Align2::LEFT_TOP,
                                                        s,
                                                        font_id.clone(),
                                                        fg_color,
                                                    );
                                                }
                                            }
                                        }

                                        if is_focused && g.scroll_offset == 0 && g.cursor_visible {
                                            let cx = rect.x + pad_x + g.cursor_col as f32 * char_w;
                                            let cy = rect.y + pad_y + g.cursor_row as f32 * row_h;
                                            let cursor_rect = Rect::from_min_size(
                                                Pos2::new(cx, cy),
                                                Vec2::new(char_w, row_h),
                                            );
                                            painter.rect_filled(
                                                cursor_rect,
                                                1.0,
                                                Color32::from_rgba_unmultiplied(
                                                    0x82, 0xaa, 0xff, 160,
                                                ),
                                            );
                                        }

                                        // Kitty Graphics 画像の描画
                                        if g.scroll_offset == 0 && !g.kitty.placements.is_empty() {
                                            for p in &g.kitty.placements {
                                                let Some(img) = g.kitty.images.get(&p.image_id)
                                                else {
                                                    continue;
                                                };
                                                let tex = self
                                                    .kitty_textures
                                                    .entry((*pane_id, p.image_id))
                                                    .or_insert_with(|| {
                                                        let color_img =
                                                            egui::ColorImage::from_rgba_unmultiplied(
                                                                [
                                                                    img.width as usize,
                                                                    img.height as usize,
                                                                ],
                                                                &img.rgba,
                                                            );
                                                        ctx.load_texture(
                                                            format!(
                                                                "kitty-{}-{}",
                                                                pane_id, p.image_id
                                                            ),
                                                            color_img,
                                                            egui::TextureOptions::LINEAR,
                                                        )
                                                    });

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
                                                    rect.y + pad_y + p.row as f32 * row_h,
                                                );
                                                let img_rect = Rect::from_min_size(
                                                    img_pos,
                                                    Vec2::new(img_w, img_h),
                                                );
                                                painter.image(
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

                                if go_to_active {
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
                            if pane_mode {
                                status_parts.push("PANE MODE (h/j/k/l: move, z: zoom, Enter/Esc: exit)");
                            }
                            if zoomed {
                                status_parts.push("ZOOMED");
                            }
                            if go_to_active {
                                status_parts.push("GO_TO (press pane number)");
                            }
                            if !left_drawer_open {
                                status_parts.push("Ctrl+Shift+E: Spaces");
                            }
                            if !right_drawer_open {
                                status_parts.push("Ctrl+Shift+G: GitHub");
                            }
                            if !status_parts.is_empty() {
                                let label = status_parts.join(" | ");
                                let top_right = Pos2::new(area.x + area.width - 14.0, area.y + 10.0);
                                ui.painter().text(
                                    top_right,
                                    egui::Align2::RIGHT_TOP,
                                    label,
                                    FontId::new(12.0, FontFamily::Monospace),
                                    Color32::from_rgb(0x82, 0x8b, 0xb8),
                                );
                            }
                        });
                });

                self.cell_size = computed_cell_size;
                let egui_wants_repaint = full_output
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.repaint_delay.is_zero());

                // egui_winit が毎フレーム set_ime_allowed(false) を呼んで Windows IME (TSF) が
                // 「あ」固定になるのを防ぐため、端末用 IME 出力を常に Some に保つ
                let mut platform_output = full_output.platform_output.clone();
                if platform_output.ime.is_none() {
                    let cursor_rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(10.0, 20.0));
                    platform_output.ime = Some(egui::output::IMEOutput {
                        rect: cursor_rect,
                        cursor_rect,
                    });
                }
                egui_state.handle_platform_output(window, platform_output);

                let bg = tokyonight_moon_bg(self.config.appearance.background_opacity);
                match gpu.render_egui(window, &self.egui_ctx, full_output, bg) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        let size = window.inner_size();
                        gpu.resize(size.width, size.height);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                    Err(err) => eprintln!("wgpu render error: {err:?}"),
                }

                let mut need_redraw = egui_wants_repaint;
                self.rename_buf = rename_buf;
                if cancel_rename {
                    self.renaming_workspace_idx = None;
                    need_redraw = true;
                }
                if let Some((idx, new_name)) = commit_rename {
                    self.manager.rename_workspace(idx, new_name);
                    self.renaming_workspace_idx = None;
                    need_redraw = true;
                }
                if let Some(idx) = start_rename_idx {
                    if let Some(ws) = self.manager.workspaces.get(idx) {
                        self.rename_buf = ws.name.clone();
                        self.renaming_workspace_idx = Some(idx);
                        need_redraw = true;
                    }
                }
                if let Some(idx) = close_ws_idx {
                    let closed = self.manager.close_workspace(idx);
                    for pid in closed {
                        self.panes.remove(&pid);
                    }
                    if self.left_drawer_selected_idx >= self.manager.workspaces.len() {
                        self.left_drawer_selected_idx = self.manager.workspaces.len() - 1;
                    }
                    need_redraw = true;
                }
                if toggle_left_collapse {
                    self.left_drawer_open = !self.left_drawer_open;
                    self.router.mode = InputMode::Normal;
                    need_redraw = true;
                }
                if new_ws_requested {
                    self.handle_action(Action::NewWorkspace);
                    need_redraw = true;
                }
                if let Some(ws_idx) = switch_ws_target {
                    self.manager.switch_workspace(ws_idx);
                    self.left_drawer_selected_idx = ws_idx;
                    need_redraw = true;
                }
                if let Some((ws_idx, tab_idx)) = switch_tab_target {
                    self.manager.switch_workspace(ws_idx);
                    self.manager.switch_tab(tab_idx);
                    self.left_drawer_selected_idx = ws_idx;
                    need_redraw = true;
                }
                if new_tab_requested {
                    self.handle_action(Action::NewTab);
                    need_redraw = true;
                }
                if let Some(t_idx) = select_tab_idx {
                    self.manager.switch_tab(t_idx);
                    need_redraw = true;
                }
                if let Some(t_idx) = close_tab_idx {
                    let (closed_panes, keep_running) = self.manager.close_tab(t_idx);
                    for pid in closed_panes {
                        self.panes.remove(&pid);
                    }
                    if !keep_running {
                        self.save_session();
                        event_loop.exit();
                        return;
                    }
                    need_redraw = true;
                }
                if close_drawer_requested {
                    self.right_drawer_open = false;
                    if self.browser.take().is_some() {
                        if let Some(w) = &self.window {
                            w.focus_window();
                        }
                    }
                    self.router.mode = InputMode::Normal;
                    need_redraw = true;
                }
                if close_browser_requested {
                    if self.browser.take().is_some() {
                        if let Some(w) = &self.window {
                            w.focus_window();
                        }
                    }
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
                    let focused = self.manager.active_tab().focused_pane;
                    if let Some(pane) = self.panes.get_mut(&focused) {
                        let _ = pane.pty.write_bytes(text.as_bytes());
                    }
                }
                for (pane_id, cols, rows) in pending_resizes {
                    if let Some(runtime) = self.panes.get_mut(&pane_id) {
                        runtime.last_size = (cols, rows);
                        if let Ok(mut g) = runtime.grid.lock() {
                            g.resize(cols, rows);
                        }
                        let _ = runtime.pty.resize(cols as u16, rows as u16);
                        need_redraw = true;
                    }
                }
                if need_redraw {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

fn render_github_section(
    ui: &mut egui::Ui,
    heading: &str,
    accent: Color32,
    items: &[GitHubItem],
    kind: GitHubItemKind,
    global_idx: &mut usize,
    selected_idx: usize,
    scroll_to_selected: bool,
    open_url_target: &mut Option<String>,
    preview_url_target: &mut Option<String>,
    send_to_pane_text: &mut Option<String>,
) {
    let filtered: Vec<&GitHubItem> = items.iter().filter(|i| i.kind == kind).collect();
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(format!("{heading} ({})", filtered.len()))
            .strong()
            .color(accent),
    );
    if filtered.is_empty() {
        ui.label(
            egui::RichText::new("  None")
                .small()
                .color(Color32::from_rgb(0x63, 0x6d, 0xa6)),
        );
        return;
    }

    for item in filtered {
        let is_selected = *global_idx == selected_idx;
        *global_idx += 1;

        let fill = if is_selected {
            Color32::from_rgb(0x3b, 0x42, 0x61)
        } else {
            Color32::from_rgb(0x2f, 0x33, 0x4d)
        };
        let stroke = if is_selected {
            Stroke::new(1.5_f32, Color32::from_rgb(0x7a, 0xa2, 0xf7))
        } else {
            Stroke::NONE
        };

        let frame_resp = egui::Frame::new()
            .fill(fill)
            .stroke(stroke)
            .corner_radius(4.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("{} #{}", item.repo, item.number))
                        .small()
                        .color(accent),
                );
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&item.title).color(Color32::from_rgb(0xc8, 0xd3, 0xf5)),
                    )
                    .selectable(true),
                );
                ui.horizontal(|ui| {
                    if ui.small_button("Preview").clicked() {
                        *preview_url_target = Some(item.url.clone());
                    }
                    if ui.small_button("Open").clicked() {
                        *open_url_target = Some(item.url.clone());
                    }
                    if ui.small_button("Send to Pane").clicked() {
                        *send_to_pane_text = Some(format!(
                            "[{} #{}] {} ({})",
                            item.repo, item.number, item.title, item.url
                        ));
                    }
                });
            });
        if is_selected && scroll_to_selected {
            frame_resp.response.scroll_to_me(None);
        }
        ui.add_space(4.0);
    }
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
    let h_left = Rect::from_min_max(
        Pos2::new(rect.left(), cy - ht),
        Pos2::new(cx + ht, cy + ht),
    );
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
        return Some(PathBuf::from(appdata).join("conductty").join("session.json"));
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

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();

    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();

    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WSL_DISTRO_NAME").is_some() {
            let _ = Command::new("cmd.exe")
                .args(["/C", "start", "", url])
                .spawn();
        } else {
            let _ = Command::new("xdg-open").arg(url).spawn();
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
    for jp_name in ["Cica", "NotoSansCJK", "NotoSansJP", "BIZUDGothic", "Meiryo", "YuGothic", "Hiragino"] {
        let key = format!("conductty-jp-{jp_name}");
        if fonts.font_data.contains_key(&format!("conductty-{jp_name}")) {
            jp_fallbacks.push(format!("conductty-{jp_name}"));
            continue;
        }
        if let Some(bytes) = find_font_bytes(&search_dirs, jp_name) {
            fonts
                .font_data
                .insert(key.clone(), Arc::new(FontData::from_owned(bytes)));
            jp_fallbacks.push(key);
        }
    }

    if !loaded_names.is_empty() {
        if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
            for (idx, name) in loaded_names.into_iter().enumerate() {
                mono.insert(idx, name);
            }
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

fn find_font_bytes(dirs: &[PathBuf], family: &str) -> Option<Vec<u8>> {
    let normalized = family.replace(' ', "").to_lowercase();
    for dir in dirs {
        if let Some(bytes) = scan_dir_for_font(dir, &normalized, 0) {
            return Some(bytes);
        }
    }
    None
}

fn scan_dir_for_font(dir: &PathBuf, target: &str, depth: usize) -> Option<Vec<u8>> {
    if depth > 3 || !dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = scan_dir_for_font(&path, target, depth + 1) {
                return Some(found);
            }
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
                    if let Ok(bytes) = std::fs::read(&path) {
                        return Some(bytes);
                    }
                }
            }
        }
    }
    None
}

fn to_core_key_event(key: &Key, mods: ModifiersState) -> Option<CoreKeyEvent> {
    let code = match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let ch = chars.next()?;
            if ch == ' ' {
                CoreKeyCode::Space
            } else {
                CoreKeyCode::Char(ch)
            }
        }
        Key::Named(NamedKey::Enter) => CoreKeyCode::Enter,
        Key::Named(NamedKey::Escape) => CoreKeyCode::Escape,
        Key::Named(NamedKey::Backspace) => CoreKeyCode::Backspace,
        Key::Named(NamedKey::Tab) => CoreKeyCode::Tab,
        Key::Named(NamedKey::Space) => CoreKeyCode::Space,
        Key::Named(NamedKey::ArrowUp) => CoreKeyCode::Up,
        Key::Named(NamedKey::ArrowDown) => CoreKeyCode::Down,
        Key::Named(NamedKey::ArrowLeft) => CoreKeyCode::Left,
        Key::Named(NamedKey::ArrowRight) => CoreKeyCode::Right,
        _ => return None,
    };

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

fn main() {
    let config = AppConfig::default();
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();

    let mut app = App {
        config,
        rt,
        proxy,
        window: None,
        gpu: None,
        egui_ctx: egui::Context::default(),
        egui_state: None,
        manager: WorkspaceManager::new(1),
        panes: HashMap::new(),
        next_pane_id: 1,
        go_to_active: false,
        left_drawer_open: true,
        left_drawer_selected_idx: 0,
        renaming_workspace_idx: None,
        rename_buf: String::new(),
        right_drawer_open: false,
        drawer_snapshot: DrawerSnapshot::default(),
        drawer_selected_idx: 0,
        drawer_scroll_to_selected: false,
        cursor_pos: Pos2::ZERO,
        cell_size: (10.0, 22.0),
        selection: None,
        selecting: false,
        clipboard: arboard::Clipboard::new().ok(),
        browser: None,
        pending_browser_url: None,
        browser_error: None,
        kitty_textures: HashMap::new(),
        router: InputRouter::default(),
        modifiers: ModifiersState::empty(),
    };
    let _ = event_loop.run_app(&mut app);
}
