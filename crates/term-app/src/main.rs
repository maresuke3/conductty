use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Pos2, Rect, Stroke, Vec2};
use term_core::config::AppConfig;
use term_core::grid::TerminalGrid;
use term_core::input::{
    Action, InputDecision, InputMode, InputRouter, KeyCode as CoreKeyCode, KeyEvent as CoreKeyEvent,
    Modifiers, PerformableContext,
};
use term_core::layout::{NavDirection, PaneId, Rect as CoreRect, SplitDirection, Tab};
use term_core::pty::PtySession;
use term_drawer::{DrawerSnapshot, GitHubItem, GitHubItemKind, fetch_github_snapshot};
use term_render::tokyonight_moon_bg;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const RIGHT_DRAWER_WIDTH: f32 = 340.0;

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
        eprintln!(
            "[wgpu] backend={:?} format={:?} alpha={:?}",
            adapter.get_info().backend,
            format,
            alpha_mode
        );

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
    tab: &'a Tab,
    area: CoreRect,
}

impl PerformableContext for TabNavCtx<'_> {
    fn can_goto_split(&self, dir: NavDirection) -> bool {
        self.tab.can_goto_split(dir, self.area)
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
    tab: Tab,
    panes: HashMap<PaneId, PaneRuntime>,
    next_pane_id: PaneId,
    go_to_active: bool,
    right_drawer_open: bool,
    drawer_snapshot: DrawerSnapshot,
    drawer_selected_idx: usize,
    drawer_scroll_to_selected: bool,
    cursor_pos: Pos2,
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

        let drawer_w = if self.right_drawer_open {
            RIGHT_DRAWER_WIDTH.min(w * 0.45)
        } else {
            0.0
        };

        CoreRect {
            x: 0.0,
            y: 0.0,
            width: (w - drawer_w).max(100.0),
            height: h,
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

    fn handle_action(&mut self, action: Action) {
        let area = self.terminal_area();
        match action {
            Action::SplitRight => {
                if let Some(new_id) = self.spawn_pane() {
                    self.tab.split_focused(new_id, SplitDirection::Horizontal);
                }
            }
            Action::SplitDown => {
                if let Some(new_id) = self.spawn_pane() {
                    self.tab.split_focused(new_id, SplitDirection::Vertical);
                }
            }
            Action::GotoSplit(dir) => {
                self.tab.goto_split(dir, area);
            }
            Action::ToggleZoom => {
                self.tab.toggle_zoom();
            }
            Action::OpenGoTo => {
                self.go_to_active = !self.go_to_active;
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
            Action::DrawerSendToPane => {
                if let Some(item) = self.selected_drawer_item() {
                    let text = format!("[{} #{}] {} ({})", item.repo, item.number, item.title, item.url);
                    if let Some(pane) = self.panes.get_mut(&self.tab.focused_pane) {
                        let _ = pane.pty.write_bytes(text.as_bytes());
                    }
                    self.right_drawer_open = false;
                    self.router.mode = InputMode::Normal;
                }
            }
            Action::DrawerRefresh => {
                self.trigger_drawer_refresh();
            }
            Action::EnterPaneMode | Action::ExitPaneMode => {}
            Action::NewTab => {
                eprintln!("NewTab will be wired in Step 4");
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // macOS (Metal) は標準で透過が安定。Windows (DWM) / WSLg はリサイズ時の白抜け・EGLクラッシュを防ぐため明示オプトイン時のみ透過にする
            let transparent = cfg!(target_os = "macos")
                || std::env::var_os("CONDUCTTY_TRANSPARENT").is_some();
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
                self.tab = Tab::new(1, first_id);
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
                if !self.tab.close_pane(id) {
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(window), Some(egui_state)) = (&self.window, &mut self.egui_state) {
            let response = egui_state.on_window_event(window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let area = self.terminal_area();
                if self.cursor_pos.x < area.width {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y.round() as i32 * 3,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / 16.0).round() as i32,
                    };
                    if lines != 0 {
                        if let Some(pane) = self.panes.get(&self.tab.focused_pane) {
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
                    if let Some(pane) = self.panes.get_mut(&self.tab.focused_pane) {
                        let _ = pane.pty.write_bytes(text.as_bytes());
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.go_to_active {
                    if let Key::Character(s) = &event.logical_key {
                        if let Ok(num) = s.parse::<PaneId>() {
                            if self.panes.contains_key(&num) {
                                self.tab.focused_pane = num;
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
                            tab: &self.tab,
                            area,
                        };
                        self.router.route(&core_ev, &ctx)
                    };

                    match decision {
                        InputDecision::SendToPty(bytes) => {
                            if let Some(pane) = self.panes.get_mut(&self.tab.focused_pane) {
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
                let visible_panes = self.tab.visible_rects(area);
                let focused_pane = self.tab.focused_pane;
                let pane_mode = self.router.mode == InputMode::Pane;
                let zoomed = self.tab.zoomed;
                let go_to_active = self.go_to_active;
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
                let mut close_drawer_requested = false;
                let mut open_url_target: Option<String> = None;
                let mut send_to_pane_text: Option<String> = None;
                let scroll_to_selected = self.drawer_scroll_to_selected;
                self.drawer_scroll_to_selected = false;
                let mut pending_resizes: Vec<(PaneId, usize, usize)> = Vec::new();

                let full_output = self.egui_ctx.run(raw_input, |ctx| {
                    if right_drawer_open {
                        egui::SidePanel::right("conductty_right_drawer")
                            .exact_width(RIGHT_DRAWER_WIDTH)
                            .resizable(false)
                            .frame(
                                egui::Frame::new()
                                    .fill(Color32::from_rgb(0x1e, 0x20, 0x30))
                                    .inner_margin(12.0),
                            )
                            .show(ctx, |ui| {
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
                                            &mut send_to_pane_text,
                                        );
                                    });

                                ui.separator();
                                ui.label(
                                    egui::RichText::new(
                                        "j/k: select | o/Enter: open | s: send | r: refresh | q/Esc: close",
                                    )
                                    .small()
                                    .color(Color32::from_rgb(0x82, 0x8b, 0xb8)),
                                );
                            });
                    }

                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            let font_id = FontId::new(font_size, FontFamily::Monospace);
                            let char_w = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
                            let row_h = font_size + cell_extra_h + 2.0;

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
                                                let start_col = col;
                                                while col < g.cols {
                                                    if let Some(cell) = g.visible_cell(col, row) {
                                                        if cell.fg == fg && cell.bg == bg {
                                                            run.push(cell.ch);
                                                            col += 1;
                                                            continue;
                                                        }
                                                    }
                                                    break;
                                                }

                                                if let Some(bg_rgb) = bg {
                                                    let bg_rect = Rect::from_min_size(
                                                        Pos2::new(
                                                            rect.x + pad_x + start_col as f32 * char_w,
                                                            y,
                                                        ),
                                                        Vec2::new(
                                                            (col - start_col) as f32 * char_w,
                                                            row_h,
                                                        ),
                                                    );
                                                    painter.rect_filled(
                                                        bg_rect,
                                                        0.0,
                                                        Color32::from_rgb(
                                                            bg_rgb.r, bg_rgb.g, bg_rgb.b,
                                                        ),
                                                    );
                                                }

                                                let trimmed = run.trim_end();
                                                if !trimmed.is_empty() {
                                                    let pos = Pos2::new(
                                                        rect.x + pad_x + start_col as f32 * char_w,
                                                        y,
                                                    );
                                                    painter.text(
                                                        pos,
                                                        egui::Align2::LEFT_TOP,
                                                        trimmed,
                                                        font_id.clone(),
                                                        Color32::from_rgb(fg.r, fg.g, fg.b),
                                                    );
                                                }
                                            }
                                        }

                                        if is_focused && g.scroll_offset == 0 {
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
                            if !right_drawer_open {
                                status_parts.push("Ctrl+Shift+G: GitHub");
                            }
                            if !status_parts.is_empty() {
                                let label = status_parts.join(" | ");
                                let top_right = Pos2::new(area.width - 14.0, 10.0);
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

                let egui_wants_repaint = full_output
                    .viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .is_some_and(|v| v.repaint_delay.is_zero());

                egui_state.handle_platform_output(window, full_output.platform_output.clone());
                // egui_winit は TextEdit 非フォーカス時に IME を無効化するため、端末用に常に有効化し直す
                window.set_ime_allowed(true);

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
                if close_drawer_requested {
                    self.right_drawer_open = false;
                    self.router.mode = InputMode::Normal;
                    need_redraw = true;
                }
                if refresh_requested {
                    self.trigger_drawer_refresh();
                    need_redraw = true;
                }
                if let Some(url) = open_url_target {
                    open_url(&url);
                }
                if let Some(text) = send_to_pane_text {
                    if let Some(pane) = self.panes.get_mut(&self.tab.focused_pane) {
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
        tab: Tab::new(1, 1),
        panes: HashMap::new(),
        next_pane_id: 1,
        go_to_active: false,
        right_drawer_open: false,
        drawer_snapshot: DrawerSnapshot::default(),
        drawer_selected_idx: 0,
        drawer_scroll_to_selected: false,
        cursor_pos: Pos2::ZERO,
        router: InputRouter::default(),
        modifiers: ModifiersState::empty(),
    };
    let _ = event_loop.run_app(&mut app);
}
