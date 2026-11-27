use crate::config::AppConfig;
use crate::layout::NavDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Prefix,
    PaneMove,
    PaneResize,
    LeftDrawer,
    RightDrawer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    Space,
    Backquote,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1〜F12
    F(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub mods: Modifiers,
    /// macOS の `macos-option-as-alt = left` 用: 左 Option が押されているか
    pub left_alt: bool,
    /// 修飾キーを押さずに押したときの文字（キティのキーボードプロトコルのキーの番号に使う）
    pub unshifted: Option<char>,
    /// US 配列でのキーの文字（キティのキーボードプロトコルの「US 配列のキー」に使う）
    pub base_layout: Option<char>,
    pub kind: KeyEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyEventKind {
    #[default]
    Press,
    /// 押し続けて繰り返したとき
    Repeat,
    Release,
}

/// キティのキーボードプロトコルのフラグ
pub mod kitty_flags {
    /// Esc や Ctrl・Alt との組み合わせを CSI u で送り、見分けられるようにする
    pub const DISAMBIGUATE: u8 = 1;
    /// 繰り返しと離したことも送る
    pub const EVENT_TYPES: u8 = 2;
    /// Shift を押したときの文字と US 配列のキーも送る
    pub const ALTERNATE_KEYS: u8 = 4;
    /// 文字のキーも含めて、すべてのキーを CSI u で送る
    pub const ALL_KEYS: u8 = 8;
    /// キーが出す文字も送る
    pub const TEXT: u8 = 16;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewTab,
    SplitRight,
    SplitDown,
    ToggleZoom,
    EnterPaneMoveMode,
    ExitPaneMoveMode,
    EnterPaneResizeMode,
    ExitPaneResizeMode,
    ResizeSplit(NavDirection),
    GotoSplit(NavDirection),
    ToggleLeftDrawer,
    ToggleLeftDrawerCollapse,
    ExitLeftDrawerMode,
    LeftDrawerNext,
    LeftDrawerPrev,
    LeftDrawerSelect,
    LeftDrawerRename,
    LeftDrawerCloseWorkspace,
    NewWorkspace,
    ToggleRightDrawer,
    CloseRightDrawer,
    DrawerNext,
    DrawerPrev,
    DrawerNextPlugin,
    DrawerPrevPlugin,
    DrawerOpen,
    DrawerPreview,
    DrawerSendToPane,
    DrawerRefresh,
    Copy,
    Paste,
    ReloadConfig,
    ToggleKeybindHints,
    OpenPaneMode,
    OpenGoTo,
    /// 前 (-1)・次 (1) のプロンプトへスクロールする
    JumpToPrompt(i8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoToCommand {
    Next,
    Prev,
    Select,
    Close,
    StartFilter,
}

/// 純粋な入力ルーティング結果（副作用なし）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputDecision {
    Execute(Action),
    SendToPty(Vec<u8>),
    Ignore,
}

/// 現在のレイアウトに対する実行可否コンテキスト（`performable:` 判定用）
pub trait PerformableContext {
    fn can_goto_split(&self, dir: NavDirection) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
    pub performable: bool,
    pub action: Action,
}

impl KeyBinding {
    pub fn parse(spec: &str, action: Action) -> Option<Self> {
        let trimmed = spec.trim();
        let (performable, rest) = if let Some(r) = trimmed.strip_prefix("performable:") {
            (true, r)
        } else {
            (false, trimmed)
        };

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut logo = false;
        let mut key_part: Option<&str> = None;

        for token in rest.split('+') {
            let t = token.trim().to_lowercase();
            match t.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "option" => alt = true,
                "shift" => shift = true,
                "cmd" | "super" | "logo" | "meta" => logo = true,
                _ => key_part = Some(token.trim()),
            }
        }

        let code = match key_part?.to_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "escape" | "esc" => KeyCode::Escape,
            "backspace" => KeyCode::Backspace,
            "tab" => KeyCode::Tab,
            "space" => KeyCode::Space,
            "backquote" | "`" => KeyCode::Backquote,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            s if s.chars().count() == 1 => KeyCode::Char(s.chars().next()?),
            _ => return None,
        };

        Some(Self {
            code,
            ctrl,
            alt,
            shift,
            logo,
            performable,
            action,
        })
    }

    fn matches(&self, event: &KeyEvent) -> bool {
        let code_matches = match (&self.code, &event.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => a.eq_ignore_ascii_case(b),
            _ => self.code == event.code,
        };
        let shift_matches = match self.code {
            KeyCode::Char(c) if !c.is_ascii_alphabetic() && !self.shift => true,
            _ => self.shift == event.mods.shift,
        };
        code_matches
            && self.ctrl == event.mods.ctrl
            && self.alt == event.mods.alt
            && shift_matches
            && self.logo == event.mods.logo
    }
}

/// 状態遷移型の純粋入力ルーター
#[derive(Debug, Clone)]
pub struct InputRouter {
    pub mode: InputMode,
    pub macos_left_option_as_alt: bool,
    pub normal_bindings: Vec<KeyBinding>,
    pub prefix_trigger: Option<KeyBinding>,
    pub prefix_bindings: Vec<KeyBinding>,
    pub go_to_bindings: Vec<(KeyBinding, GoToCommand)>,
    pub right_drawer_bindings: Vec<KeyBinding>,
    /// フォーカス中のペインがカーソルキーのアプリケーションモード (DECCKM) か。キーを送る前にアプリが入れる
    pub app_cursor_keys: bool,
    /// フォーカス中のペインのキティのキーボードプロトコルのフラグ。キーを送る前にアプリが入れる
    pub kitty_flags: u8,
}

impl Default for InputRouter {
    fn default() -> Self {
        Self::from_config(&AppConfig::default())
    }
}

impl InputRouter {
    pub fn from_config(config: &AppConfig) -> Self {
        let kb = &config.keybinds;
        let specs = [
            (&kb.pane_mode, Action::OpenPaneMode),
            (&kb.pane_move_mode, Action::EnterPaneMoveMode),
            (&kb.pane_resize_mode, Action::EnterPaneResizeMode),
            (&kb.toggle_zoom, Action::ToggleZoom),
            (&kb.toggle_left_drawer, Action::ToggleLeftDrawer),
            (&kb.toggle_right_drawer, Action::ToggleRightDrawer),
            (&kb.go_to, Action::OpenGoTo),
            (&kb.new_tab, Action::NewTab),
            (&kb.new_workspace, Action::NewWorkspace),
            (&kb.split_right, Action::SplitRight),
            (&kb.split_down, Action::SplitDown),
            (&kb.goto_left, Action::GotoSplit(NavDirection::Left)),
            (&kb.goto_down, Action::GotoSplit(NavDirection::Down)),
            (&kb.goto_up, Action::GotoSplit(NavDirection::Up)),
            (&kb.goto_right, Action::GotoSplit(NavDirection::Right)),
            (&kb.copy, Action::Copy),
            (&kb.paste, Action::Paste),
            (&kb.reload_config, Action::ReloadConfig),
            (&kb.keybind_hints, Action::ToggleKeybindHints),
            (&kb.jump_to_prev_prompt, Action::JumpToPrompt(-1)),
            (&kb.jump_to_next_prompt, Action::JumpToPrompt(1)),
        ];

        let prefix_trigger = if kb.prefix.trim().is_empty() {
            None
        } else {
            KeyBinding::parse(&kb.prefix, Action::NewTab)
        };

        let mut normal_bindings = Vec::new();
        let mut prefix_bindings = Vec::new();
        for (spec, action) in specs {
            for p in split_binding_list(spec) {
                if let Some(rest) = strip_leader_prefix(p) {
                    if let Some(b) = KeyBinding::parse(rest, action) {
                        prefix_bindings.push(b);
                    }
                } else if let Some(b) = KeyBinding::parse(p, action) {
                    normal_bindings.push(b);
                }
            }
        }

        // macOS では Cmd+C / Cmd+V も標準で使えるようにする
        if let Some(b) = KeyBinding::parse("cmd+c", Action::Copy) {
            normal_bindings.push(b);
        }
        if let Some(b) = KeyBinding::parse("cmd+v", Action::Paste) {
            normal_bindings.push(b);
        }

        let mut go_to_bindings = Vec::new();
        let go_to_specs = [
            (&kb.go_to_next, GoToCommand::Next),
            (&kb.go_to_prev, GoToCommand::Prev),
            (&kb.go_to_select, GoToCommand::Select),
            (&kb.go_to_close, GoToCommand::Close),
            (&kb.go_to_filter, GoToCommand::StartFilter),
        ];
        for (spec, cmd) in go_to_specs {
            for part in split_binding_list(spec) {
                if let Some(b) = KeyBinding::parse(part, Action::OpenGoTo) {
                    go_to_bindings.push((b, cmd));
                }
            }
        }

        let mut right_drawer_bindings = Vec::new();
        for p in split_binding_list(&kb.toggle_right_drawer) {
            if strip_leader_prefix(p).is_none()
                && let Some(b) = KeyBinding::parse(p, Action::CloseRightDrawer)
            {
                right_drawer_bindings.push(b);
            }
        }
        let right_drawer_specs = [
            (&kb.drawer_close, Action::CloseRightDrawer),
            (&kb.drawer_next, Action::DrawerNext),
            (&kb.drawer_prev, Action::DrawerPrev),
            (&kb.drawer_prev_plugin, Action::DrawerPrevPlugin),
            (&kb.drawer_next_plugin, Action::DrawerNextPlugin),
            (&kb.drawer_preview, Action::DrawerPreview),
            (&kb.drawer_open, Action::DrawerOpen),
            (&kb.drawer_send_to_pane, Action::DrawerSendToPane),
            (&kb.drawer_refresh, Action::DrawerRefresh),
        ];
        for (spec, action) in right_drawer_specs {
            for part in split_binding_list(spec) {
                if let Some(b) = KeyBinding::parse(part, action) {
                    right_drawer_bindings.push(b);
                }
            }
        }

        Self {
            mode: InputMode::Normal,
            macos_left_option_as_alt: config.window.macos_option_as_alt == "left",
            normal_bindings,
            prefix_trigger,
            prefix_bindings,
            go_to_bindings,
            right_drawer_bindings,
            app_cursor_keys: false,
            kitty_flags: 0,
        }
    }

    pub fn route_go_to(&self, event: &KeyEvent, filtering: bool) -> Option<GoToCommand> {
        for (binding, cmd) in &self.go_to_bindings {
            if !binding.matches(event) {
                continue;
            }
            if filtering {
                match cmd {
                    GoToCommand::Select => return Some(*cmd),
                    GoToCommand::Next | GoToCommand::Prev
                        if binding.ctrl || !matches!(binding.code, KeyCode::Char(_)) =>
                    {
                        return Some(*cmd);
                    }
                    GoToCommand::Close if binding.code == KeyCode::Escape => return Some(*cmd),
                    _ => {}
                }
            } else {
                return Some(*cmd);
            }
        }
        None
    }
}

fn split_binding_list(spec: &str) -> Vec<&str> {
    if spec.trim() == "," {
        vec![","]
    } else {
        spec.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }
}

fn strip_leader_prefix(spec: &str) -> Option<&str> {
    let lower = spec.to_ascii_lowercase();
    for pfx in ["prefix+", "leader+"] {
        if lower.starts_with(pfx) {
            return Some(&spec[pfx.len()..]);
        }
    }
    None
}

impl InputRouter {
    pub fn route(&mut self, event: &KeyEvent, ctx: &impl PerformableContext) -> InputDecision {
        match self.mode {
            InputMode::Prefix => self.route_prefix_mode(event, ctx),
            InputMode::PaneMove => self.route_pane_move_mode(event),
            InputMode::PaneResize => self.route_pane_resize_mode(event),
            InputMode::LeftDrawer => self.route_left_drawer_mode(event),
            InputMode::RightDrawer => self.route_right_drawer_mode(event),
            InputMode::Normal => self.route_normal_mode(event, ctx),
        }
    }

    fn route_prefix_mode(
        &mut self,
        event: &KeyEvent,
        ctx: &impl PerformableContext,
    ) -> InputDecision {
        self.mode = InputMode::Normal;
        for binding in &self.prefix_bindings {
            if binding.matches(event) {
                if binding.performable
                    && let Action::GotoSplit(dir) = binding.action
                    && !ctx.can_goto_split(dir)
                {
                    continue;
                }
                match binding.action {
                    Action::EnterPaneMoveMode => self.mode = InputMode::PaneMove,
                    Action::EnterPaneResizeMode => self.mode = InputMode::PaneResize,
                    Action::ToggleLeftDrawer => self.mode = InputMode::LeftDrawer,
                    Action::ToggleRightDrawer => self.mode = InputMode::RightDrawer,
                    _ => {}
                }
                return InputDecision::Execute(binding.action);
            }
        }
        if self
            .prefix_trigger
            .as_ref()
            .is_some_and(|t| t.matches(event))
        {
            return match self.encode_vt_bytes(event) {
                Some(bytes) => InputDecision::SendToPty(bytes),
                None => InputDecision::Ignore,
            };
        }
        InputDecision::Ignore
    }

    fn route_left_drawer_mode(&mut self, event: &KeyEvent) -> InputDecision {
        let m = event.mods;
        if m.ctrl
            && m.shift
            && !m.alt
            && matches!(event.code, KeyCode::Char('e') | KeyCode::Char('E'))
        {
            self.mode = InputMode::Normal;
            return InputDecision::Execute(Action::ExitLeftDrawerMode);
        }

        match (&event.code, m.ctrl, m.shift, m.alt) {
            (KeyCode::Escape, _, _, _)
            | (KeyCode::Char('q') | KeyCode::Char('Q'), false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::ExitLeftDrawerMode)
            }
            (KeyCode::Char('c') | KeyCode::Char('C'), false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::ToggleLeftDrawerCollapse)
            }
            (KeyCode::Char('j') | KeyCode::Char('J'), false, false, false)
            | (KeyCode::Down, false, false, false) => {
                InputDecision::Execute(Action::LeftDrawerNext)
            }
            (KeyCode::Char('k') | KeyCode::Char('K'), false, false, false)
            | (KeyCode::Up, false, false, false) => InputDecision::Execute(Action::LeftDrawerPrev),
            (KeyCode::Enter, false, false, false) | (KeyCode::Space, false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::LeftDrawerSelect)
            }
            (KeyCode::Char('n') | KeyCode::Char('N'), false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::NewWorkspace)
            }
            (KeyCode::Char('r') | KeyCode::Char('R'), false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::LeftDrawerRename)
            }
            (KeyCode::Char('d') | KeyCode::Char('D'), false, false, false) => {
                InputDecision::Execute(Action::LeftDrawerCloseWorkspace)
            }
            _ => InputDecision::Ignore,
        }
    }

    fn route_right_drawer_mode(&mut self, event: &KeyEvent) -> InputDecision {
        for binding in &self.right_drawer_bindings {
            if binding.matches(event) {
                if binding.action == Action::CloseRightDrawer {
                    self.mode = InputMode::Normal;
                }
                return InputDecision::Execute(binding.action);
            }
        }
        InputDecision::Ignore
    }

    fn route_pane_move_mode(&mut self, event: &KeyEvent) -> InputDecision {
        match (&event.code, event.mods.ctrl, event.mods.shift) {
            (KeyCode::Enter, _, _) | (KeyCode::Escape, _, _) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::ExitPaneMoveMode)
            }
            (KeyCode::Char('h') | KeyCode::Char('H'), false, false) => {
                InputDecision::Execute(Action::GotoSplit(NavDirection::Left))
            }
            (KeyCode::Char('j') | KeyCode::Char('J'), false, false) => {
                InputDecision::Execute(Action::GotoSplit(NavDirection::Down))
            }
            (KeyCode::Char('k') | KeyCode::Char('K'), false, false) => {
                InputDecision::Execute(Action::GotoSplit(NavDirection::Up))
            }
            (KeyCode::Char('l') | KeyCode::Char('L'), false, false) => {
                InputDecision::Execute(Action::GotoSplit(NavDirection::Right))
            }
            (KeyCode::Char('z') | KeyCode::Char('Z'), false, false) => {
                InputDecision::Execute(Action::ToggleZoom)
            }
            // catch_all = ignore
            _ => InputDecision::Ignore,
        }
    }

    fn route_pane_resize_mode(&mut self, event: &KeyEvent) -> InputDecision {
        match (&event.code, event.mods.ctrl, event.mods.shift) {
            (KeyCode::Enter, _, _) | (KeyCode::Escape, _, _) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::ExitPaneResizeMode)
            }
            (KeyCode::Char('h') | KeyCode::Char('H'), false, false)
            | (KeyCode::Left, false, false) => {
                InputDecision::Execute(Action::ResizeSplit(NavDirection::Left))
            }
            (KeyCode::Char('j') | KeyCode::Char('J'), false, false)
            | (KeyCode::Down, false, false) => {
                InputDecision::Execute(Action::ResizeSplit(NavDirection::Down))
            }
            (KeyCode::Char('k') | KeyCode::Char('K'), false, false)
            | (KeyCode::Up, false, false) => {
                InputDecision::Execute(Action::ResizeSplit(NavDirection::Up))
            }
            (KeyCode::Char('l') | KeyCode::Char('L'), false, false)
            | (KeyCode::Right, false, false) => {
                InputDecision::Execute(Action::ResizeSplit(NavDirection::Right))
            }
            _ => InputDecision::Ignore,
        }
    }

    fn route_normal_mode(
        &mut self,
        event: &KeyEvent,
        ctx: &impl PerformableContext,
    ) -> InputDecision {
        for binding in &self.normal_bindings {
            if binding.matches(event) {
                if binding.performable
                    && let Action::GotoSplit(dir) = binding.action
                    && !ctx.can_goto_split(dir)
                {
                    continue;
                }
                match binding.action {
                    Action::EnterPaneMoveMode => self.mode = InputMode::PaneMove,
                    Action::EnterPaneResizeMode => self.mode = InputMode::PaneResize,
                    Action::ToggleLeftDrawer => self.mode = InputMode::LeftDrawer,
                    Action::ToggleRightDrawer => self.mode = InputMode::RightDrawer,
                    _ => {}
                }
                return InputDecision::Execute(binding.action);
            }
        }

        if self
            .prefix_trigger
            .as_ref()
            .is_some_and(|t| t.matches(event))
        {
            self.mode = InputMode::Prefix;
            return InputDecision::Ignore;
        }

        // 通常キーは VT バイト列へエンコード
        match self.encode_vt_bytes(event) {
            Some(bytes) => InputDecision::SendToPty(bytes),
            None => InputDecision::Ignore,
        }
    }

    /// キーを PTY に送るバイト列にする（xterm / Ghostty と同じ形）
    pub fn encode_vt_bytes(&self, event: &KeyEvent) -> Option<Vec<u8>> {
        let treat_as_alt = event.mods.alt && (!self.macos_left_option_as_alt || event.left_alt);
        if self.kitty_flags != 0 {
            return encode_kitty_key(event, self.kitty_flags, treat_as_alt);
        }
        // 離したことはキティのキーボードプロトコルでしか送らない
        if event.kind == KeyEventKind::Release {
            return None;
        }
        // xterm の修飾キーの番号: 1 + Shift(1) + Alt(2) + Ctrl(4)
        let m = 1 + event.mods.shift as u8 + 2 * treat_as_alt as u8 + 4 * event.mods.ctrl as u8;
        let seq = |s: String| Some(s.into_bytes());
        match event.code {
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Right
            | KeyCode::Left
            | KeyCode::Home
            | KeyCode::End => {
                let c = match event.code {
                    KeyCode::Up => 'A',
                    KeyCode::Down => 'B',
                    KeyCode::Right => 'C',
                    KeyCode::Left => 'D',
                    KeyCode::Home => 'H',
                    _ => 'F',
                };
                return if m > 1 {
                    seq(format!("\x1b[1;{m}{c}"))
                } else if self.app_cursor_keys {
                    seq(format!("\x1bO{c}"))
                } else {
                    seq(format!("\x1b[{c}"))
                };
            }
            KeyCode::F(n @ 1..=4) => {
                let c = ['P', 'Q', 'R', 'S'][n as usize - 1];
                return if m > 1 {
                    seq(format!("\x1b[1;{m}{c}"))
                } else {
                    seq(format!("\x1bO{c}"))
                };
            }
            KeyCode::Insert
            | KeyCode::Delete
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::F(_) => {
                let n = match event.code {
                    KeyCode::Insert => 2,
                    KeyCode::Delete => 3,
                    KeyCode::PageUp => 5,
                    KeyCode::PageDown => 6,
                    KeyCode::F(5) => 15,
                    KeyCode::F(f @ 6..=10) => f + 11,
                    KeyCode::F(f @ 11..=12) => f + 12,
                    _ => return None,
                };
                return if m > 1 {
                    seq(format!("\x1b[{n};{m}~"))
                } else {
                    seq(format!("\x1b[{n}~"))
                };
            }
            KeyCode::Tab if event.mods.shift => return seq("\x1b[Z".to_string()),
            _ => {}
        }

        let mut out = Vec::new();
        if treat_as_alt {
            out.push(0x1b);
        }
        match event.code {
            KeyCode::Char(c) => match ctrl_byte(c).filter(|_| event.mods.ctrl) {
                Some(b) => out.push(b),
                None => {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            },
            KeyCode::Enter => out.push(b'\r'),
            KeyCode::Escape => out.push(0x1b),
            KeyCode::Backspace => out.push(if event.mods.ctrl { 0x08 } else { 0x7f }),
            KeyCode::Tab => out.push(b'\t'),
            KeyCode::Space => out.push(if event.mods.ctrl { 0x00 } else { b' ' }),
            KeyCode::Backquote => out.push(b'`'),
            _ => return None,
        }
        Some(out)
    }
}

/// キティのキーボードプロトコルでキーを送るバイト列にする
/// shortcut: Caps Lock / Num Lock の状態と、修飾キーだけを押したことは送らない（winit から取れないため）。
///           これらを使うアプリが出てきたら、OS から状態を取って足す
fn encode_kitty_key(event: &KeyEvent, flags: u8, alt: bool) -> Option<Vec<u8>> {
    use kitty_flags::*;
    let all_keys = flags & ALL_KEYS != 0;
    let report_events = flags & EVENT_TYPES != 0;
    if event.kind == KeyEventKind::Release && !report_events {
        return None;
    }
    let m = &event.mods;
    let mod_bits = m.shift as u8 + 2 * alt as u8 + 4 * m.ctrl as u8 + 8 * m.logo as u8;
    let event_type = match event.kind {
        KeyEventKind::Press => None,
        _ if !report_events => None,
        KeyEventKind::Repeat => Some(2),
        KeyEventKind::Release => Some(3),
    };
    // 「;修飾:種類」の部分。どちらも既定なら省く
    let mods_part = match (mod_bits, event_type) {
        (0, None) => String::new(),
        (_, None) => format!(";{}", mod_bits + 1),
        (_, Some(t)) => format!(";{}:{t}", mod_bits + 1),
    };
    let csi = |body: String| Some(format!("\x1b[{body}").into_bytes());
    let legacy_ok = !all_keys && event.kind != KeyEventKind::Release;

    match event.code {
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Right
        | KeyCode::Left
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::F(1 | 2 | 4) => {
            let c = match event.code {
                KeyCode::Up => 'A',
                KeyCode::Down => 'B',
                KeyCode::Right => 'C',
                KeyCode::Left => 'D',
                KeyCode::Home => 'H',
                KeyCode::End => 'F',
                KeyCode::F(1) => 'P',
                KeyCode::F(2) => 'Q',
                _ => 'S',
            };
            let one = if mods_part.is_empty() { "" } else { "1" };
            return csi(format!("{one}{mods_part}{c}"));
        }
        KeyCode::Insert | KeyCode::Delete | KeyCode::PageUp | KeyCode::PageDown | KeyCode::F(_) => {
            let n = match event.code {
                KeyCode::Insert => 2,
                KeyCode::Delete => 3,
                KeyCode::PageUp => 5,
                KeyCode::PageDown => 6,
                // F3 は CSI R（カーソル位置の返答）とぶつかるので 13~
                KeyCode::F(3) => 13,
                KeyCode::F(5) => 15,
                KeyCode::F(f @ 6..=10) => f + 11,
                KeyCode::F(f @ 11..=12) => f + 12,
                _ => return None,
            };
            return csi(format!("{n}{mods_part}~"));
        }
        // Enter・Tab・BS は、修飾キーなしならアプリが壊れたあとも打てるよう昔のまま送る
        KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace => {
            let (code, legacy) = match event.code {
                KeyCode::Enter => (13, b'\r'),
                KeyCode::Tab => (9, b'\t'),
                _ => (127, 0x7f),
            };
            if legacy_ok && mod_bits == 0 {
                return Some(vec![legacy]);
            }
            if !all_keys && event.kind == KeyEventKind::Release && mod_bits == 0 {
                return None;
            }
            return csi(format!("{code}{mods_part}u"));
        }
        KeyCode::Escape => return csi(format!("27{mods_part}u")),
        _ => {}
    }

    // 文字を出すキー
    let c = match event.code {
        KeyCode::Char(c) => c,
        KeyCode::Space => ' ',
        KeyCode::Backquote => '`',
        _ => return None,
    };
    let produces_text = !m.ctrl && !alt && !m.logo;
    // Shift だけなら、見分ける必要が無いので文字のまま送る（離したことは文字では表せないので送らない）
    if !all_keys && produces_text {
        if event.kind == KeyEventKind::Release {
            return None;
        }
        let mut buf = [0u8; 4];
        return Some(c.encode_utf8(&mut buf).as_bytes().to_vec());
    }
    // キーの番号は修飾キーを押さないときの文字（Shift+a でも a）
    let key = event
        .unshifted
        .unwrap_or_else(|| c.to_lowercase().next().unwrap_or(c));
    let mut key_part = (key as u32).to_string();
    if flags & ALTERNATE_KEYS != 0 {
        let shifted = (m.shift && c != key).then_some(c);
        let base = event.base_layout.filter(|&b| b != key);
        match (shifted, base) {
            (Some(s), Some(b)) => key_part += &format!(":{}:{}", s as u32, b as u32),
            (Some(s), None) => key_part += &format!(":{}", s as u32),
            (None, Some(b)) => key_part += &format!("::{}", b as u32),
            (None, None) => {}
        }
    }
    let text =
        (flags & TEXT != 0 && all_keys && produces_text && event.kind != KeyEventKind::Release)
            .then(|| (c as u32).to_string());
    match text {
        Some(text) if mods_part.is_empty() => csi(format!("{key_part};;{text}u")),
        Some(text) => csi(format!("{key_part}{mods_part};{text}u")),
        None => csi(format!("{key_part}{mods_part}u")),
    }
}

/// Ctrl+キーで送る制御文字（xterm と同じ。Ctrl+2〜8 は Ctrl+@ / [ / \\ / ] / ^ / _ / ? と同じ）
fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockCtx {
        has_right: bool,
    }
    impl PerformableContext for MockCtx {
        fn can_goto_split(&self, dir: NavDirection) -> bool {
            dir == NavDirection::Right && self.has_right
        }
    }

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            logo: false,
        }
    }

    #[test]
    fn encodes_special_keys_like_xterm() {
        let router = InputRouter::default();
        let enc = |code, m: Modifiers| {
            let ev = KeyEvent {
                code,
                mods: m,
                left_alt: true,
                unshifted: None,
                base_layout: None,
                kind: KeyEventKind::Press,
            };
            String::from_utf8(router.encode_vt_bytes(&ev).unwrap()).unwrap()
        };
        let none = mods(false, false, false);
        let cases = [
            (KeyCode::Up, none, "\x1b[A"),
            (KeyCode::Up, mods(true, false, false), "\x1b[1;5A"),
            (KeyCode::Right, mods(false, true, true), "\x1b[1;4C"),
            (KeyCode::Home, none, "\x1b[H"),
            (KeyCode::End, mods(false, false, true), "\x1b[1;2F"),
            (KeyCode::Insert, none, "\x1b[2~"),
            (KeyCode::Delete, none, "\x1b[3~"),
            (KeyCode::PageUp, none, "\x1b[5~"),
            (KeyCode::PageDown, mods(false, false, true), "\x1b[6;2~"),
            (KeyCode::F(1), none, "\x1bOP"),
            (KeyCode::F(4), mods(false, false, true), "\x1b[1;2S"),
            (KeyCode::F(5), none, "\x1b[15~"),
            (KeyCode::F(6), none, "\x1b[17~"),
            (KeyCode::F(10), none, "\x1b[21~"),
            (KeyCode::F(11), none, "\x1b[23~"),
            (KeyCode::F(12), mods(true, false, false), "\x1b[24;5~"),
            (KeyCode::Tab, mods(false, false, true), "\x1b[Z"),
            (KeyCode::Space, mods(true, false, false), "\x00"),
            (KeyCode::Backspace, mods(true, false, false), "\x08"),
            (KeyCode::Char('['), mods(true, false, false), "\x1b"),
            (KeyCode::Char('/'), mods(true, false, false), "\x1f"),
            (KeyCode::Char('a'), mods(true, false, false), "\x01"),
            (KeyCode::Char('x'), mods(false, true, false), "\x1bx"),
        ];
        for (code, m, expected) in cases {
            assert_eq!(enc(code.clone(), m), expected, "{code:?} {m:?}");
        }
    }

    #[test]
    fn encodes_kitty_keyboard_protocol() {
        use kitty_flags::*;
        let key = |code, m: Modifiers, kind, unshifted, base_layout| KeyEvent {
            code,
            mods: m,
            left_alt: true,
            unshifted,
            base_layout,
            kind,
        };
        let enc = |flags: u8, ev: KeyEvent| {
            let router = InputRouter {
                kitty_flags: flags,
                ..InputRouter::default()
            };
            router
                .encode_vt_bytes(&ev)
                .map(|b| String::from_utf8(b).unwrap())
        };
        let none = mods(false, false, false);
        let ctrl = mods(true, false, false);
        let alt = mods(false, true, false);
        let shift = mods(false, false, true);
        let press = KeyEventKind::Press;
        let release = KeyEventKind::Release;
        let repeat = KeyEventKind::Repeat;
        let ch = |c: char| KeyCode::Char(c);
        let d = DISAMBIGUATE;
        let cases: [(u8, KeyEvent, Option<&str>); 22] = [
            // 見分けるだけ: Esc・Ctrl・Alt は CSI u、文字と Shift+文字はそのまま
            (
                d,
                key(KeyCode::Escape, none, press, None, None),
                Some("\x1b[27u"),
            ),
            (
                d,
                key(ch('a'), ctrl, press, Some('a'), Some('a')),
                Some("\x1b[97;5u"),
            ),
            (
                d,
                key(ch('a'), alt, press, Some('a'), Some('a')),
                Some("\x1b[97;3u"),
            ),
            (
                d,
                key(ch('a'), none, press, Some('a'), Some('a')),
                Some("a"),
            ),
            (
                d,
                key(ch('A'), shift, press, Some('a'), Some('a')),
                Some("A"),
            ),
            (d, key(KeyCode::Enter, none, press, None, None), Some("\r")),
            (
                d,
                key(KeyCode::Enter, shift, press, None, None),
                Some("\x1b[13;2u"),
            ),
            (
                d,
                key(KeyCode::Tab, shift, press, None, None),
                Some("\x1b[9;2u"),
            ),
            (d, key(KeyCode::Up, none, press, None, None), Some("\x1b[A")),
            (
                d,
                key(KeyCode::Up, ctrl, press, None, None),
                Some("\x1b[1;5A"),
            ),
            (
                d,
                key(KeyCode::F(3), none, press, None, None),
                Some("\x1b[13~"),
            ),
            (
                d,
                key(KeyCode::F(1), none, press, None, None),
                Some("\x1b[P"),
            ),
            // 離したことは、種類を送るフラグがないと送らない
            (d, key(KeyCode::Escape, none, release, None, None), None),
            (
                d | EVENT_TYPES,
                key(KeyCode::Escape, none, release, None, None),
                Some("\x1b[27;1:3u"),
            ),
            (
                d | EVENT_TYPES,
                key(ch('a'), ctrl, repeat, Some('a'), Some('a')),
                Some("\x1b[97;5:2u"),
            ),
            // 文字のまま送るキーは、離したことを表せないので送らない
            (
                d | EVENT_TYPES,
                key(ch('a'), none, release, Some('a'), Some('a')),
                None,
            ),
            (
                d | EVENT_TYPES,
                key(KeyCode::Enter, none, release, None, None),
                None,
            ),
            // すべてのキーを CSI u で送る
            (
                ALL_KEYS,
                key(ch('a'), none, press, Some('a'), Some('a')),
                Some("\x1b[97u"),
            ),
            (
                ALL_KEYS,
                key(KeyCode::Enter, none, press, None, None),
                Some("\x1b[13u"),
            ),
            (
                ALL_KEYS | TEXT,
                key(ch('A'), shift, press, Some('a'), Some('a')),
                Some("\x1b[97;2;65u"),
            ),
            // Shift を押したときの文字と、US 配列のキー（ここでは ф が US 配列の a の位置）
            (
                ALL_KEYS | ALTERNATE_KEYS,
                key(ch('A'), shift, press, Some('a'), Some('a')),
                Some("\x1b[97:65;2u"),
            ),
            (
                ALL_KEYS | ALTERNATE_KEYS,
                key(ch('ф'), none, press, Some('ф'), Some('a')),
                Some("\x1b[1092::97u"),
            ),
        ];
        for (flags, ev, expected) in cases {
            let label = format!("{flags} {ev:?}");
            assert_eq!(enc(flags, ev).as_deref(), expected, "{label}");
        }
    }

    #[test]
    fn app_cursor_keys_send_ss3_without_modifiers() {
        let router = InputRouter {
            app_cursor_keys: true,
            ..InputRouter::default()
        };
        let enc = |code, m| {
            let ev = KeyEvent {
                code,
                mods: m,
                left_alt: false,
                unshifted: None,
                base_layout: None,
                kind: KeyEventKind::Press,
            };
            router.encode_vt_bytes(&ev).unwrap()
        };
        assert_eq!(enc(KeyCode::Up, mods(false, false, false)), b"\x1bOA");
        assert_eq!(enc(KeyCode::End, mods(false, false, false)), b"\x1bOF");
        // 修飾キーがあれば CSI のまま
        assert_eq!(enc(KeyCode::Left, mods(true, false, false)), b"\x1b[1;5D");
    }

    #[test]
    fn performable_goto_split_passes_through_when_unavailable() {
        let mut router = InputRouter::default();
        let ev = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        // 左にペインがないので PTY へ Ctrl+H (0x08) を透過
        assert_eq!(
            router.route(&ev, &MockCtx { has_right: true }),
            InputDecision::SendToPty(vec![0x08])
        );
    }

    #[test]
    fn pane_move_mode_captures_and_ignores_unknown_keys() {
        let mut router = InputRouter::default();
        let ctx = MockCtx { has_right: false };

        let enter_mode = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(true, false, true),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&enter_mode, &ctx),
            InputDecision::Execute(Action::EnterPaneMoveMode)
        );
        assert_eq!(router.mode, InputMode::PaneMove);

        // catch_all = ignore
        let random = KeyEvent {
            code: KeyCode::Char('x'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&random, &ctx), InputDecision::Ignore);

        // Escape で Normal に戻る
        let esc = KeyEvent {
            code: KeyCode::Escape,
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&esc, &ctx),
            InputDecision::Execute(Action::ExitPaneMoveMode)
        );
        assert_eq!(router.mode, InputMode::Normal);
    }

    #[test]
    fn pane_mode_prefix_h_opens_number_picker() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.keybinds.pane_mode, "prefix+h");
        let mut router = InputRouter::from_config(&cfg);
        let ctx = MockCtx { has_right: false };

        let ctrl_b = KeyEvent {
            code: KeyCode::Char('b'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let key_h = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&ctrl_b, &ctx), InputDecision::Ignore);
        assert_eq!(
            router.route(&key_h, &ctx),
            InputDecision::Execute(Action::OpenPaneMode)
        );
    }

    #[test]
    fn pane_resize_mode_prefix_r_resizes_and_exits() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.keybinds.pane_resize_mode, "prefix+r");
        let mut router = InputRouter::from_config(&cfg);
        let ctx = MockCtx { has_right: false };

        let ctrl_b = KeyEvent {
            code: KeyCode::Char('b'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let key_r = KeyEvent {
            code: KeyCode::Char('r'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&ctrl_b, &ctx), InputDecision::Ignore);
        assert_eq!(
            router.route(&key_r, &ctx),
            InputDecision::Execute(Action::EnterPaneResizeMode)
        );
        assert_eq!(router.mode, InputMode::PaneResize);

        let key_l = KeyEvent {
            code: KeyCode::Char('l'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&key_l, &ctx),
            InputDecision::Execute(Action::ResizeSplit(NavDirection::Right))
        );

        let enter = KeyEvent {
            code: KeyCode::Enter,
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&enter, &ctx),
            InputDecision::Execute(Action::ExitPaneResizeMode)
        );
        assert_eq!(router.mode, InputMode::Normal);
    }

    #[test]
    fn right_drawer_mode_navigation_and_exit() {
        let mut router = InputRouter::default();
        let ctx = MockCtx { has_right: false };

        let toggle = KeyEvent {
            code: KeyCode::Char('g'),
            mods: mods(true, false, true),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&toggle, &ctx),
            InputDecision::Execute(Action::ToggleRightDrawer)
        );
        assert_eq!(router.mode, InputMode::RightDrawer);

        let j = KeyEvent {
            code: KeyCode::Char('j'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&j, &ctx),
            InputDecision::Execute(Action::DrawerNext)
        );

        let l = KeyEvent {
            code: KeyCode::Char('l'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&l, &ctx),
            InputDecision::Execute(Action::DrawerNextPlugin)
        );

        let h = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&h, &ctx),
            InputDecision::Execute(Action::DrawerPrevPlugin)
        );

        let q = KeyEvent {
            code: KeyCode::Char('q'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&q, &ctx),
            InputDecision::Execute(Action::CloseRightDrawer)
        );
        assert_eq!(router.mode, InputMode::Normal);
    }

    #[test]
    fn custom_keybind_overrides_default_from_config() {
        let mut cfg = AppConfig::default();
        cfg.keybinds.new_tab = "ctrl+shift+t".to_string();
        let mut router = InputRouter::from_config(&cfg);
        let ctx = MockCtx { has_right: false };

        let ev = KeyEvent {
            code: KeyCode::Char('t'),
            mods: mods(true, false, true),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&ev, &ctx),
            InputDecision::Execute(Action::NewTab)
        );
    }

    #[test]
    fn prefix_and_direct_bindings_work_together() {
        let mut cfg = AppConfig::default();
        cfg.keybinds.prefix = "ctrl+a".to_string();
        cfg.keybinds.new_tab = "ctrl+t, prefix+c".to_string();
        cfg.keybinds.split_right = "leader+%".to_string();
        let mut router = InputRouter::from_config(&cfg);
        let ctx = MockCtx { has_right: false };

        // ダイレクトバインド (ctrl+t)
        let ctrl_t = KeyEvent {
            code: KeyCode::Char('t'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&ctrl_t, &ctx),
            InputDecision::Execute(Action::NewTab)
        );

        // prefix 起動 (ctrl+a) -> c で NewTab
        let ctrl_a = KeyEvent {
            code: KeyCode::Char('a'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&ctrl_a, &ctx), InputDecision::Ignore);
        assert_eq!(router.mode, InputMode::Prefix);

        let key_c = KeyEvent {
            code: KeyCode::Char('c'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&key_c, &ctx),
            InputDecision::Execute(Action::NewTab)
        );
        assert_eq!(router.mode, InputMode::Normal);

        // prefix 起動 -> % (Shift 込み) で SplitRight
        assert_eq!(router.route(&ctrl_a, &ctx), InputDecision::Ignore);
        let pct = KeyEvent {
            code: KeyCode::Char('%'),
            mods: mods(false, false, true),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&pct, &ctx),
            InputDecision::Execute(Action::SplitRight)
        );
        assert_eq!(router.mode, InputMode::Normal);

        // prefix 2 回押しで PTY へ 0x01 (Ctrl+A) パススルー
        assert_eq!(router.route(&ctrl_a, &ctx), InputDecision::Ignore);
        assert_eq!(
            router.route(&ctrl_a, &ctx),
            InputDecision::SendToPty(vec![0x01])
        );
        assert_eq!(router.mode, InputMode::Normal);

        // 未割り当てキー (Esc) でキャンセル
        assert_eq!(router.route(&ctrl_a, &ctx), InputDecision::Ignore);
        let esc = KeyEvent {
            code: KeyCode::Escape,
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&esc, &ctx), InputDecision::Ignore);
        assert_eq!(router.mode, InputMode::Normal);
    }

    #[test]
    fn go_to_vim_and_filter_bindings_work() {
        let cfg = AppConfig::default();
        let router = InputRouter::from_config(&cfg);

        let j = KeyEvent {
            code: KeyCode::Char('j'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let k = KeyEvent {
            code: KeyCode::Char('k'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let slash = KeyEvent {
            code: KeyCode::Char('/'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let q = KeyEvent {
            code: KeyCode::Char('q'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let ctrl_j = KeyEvent {
            code: KeyCode::Char('j'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };

        assert_eq!(router.route_go_to(&j, false), Some(GoToCommand::Next));
        assert_eq!(router.route_go_to(&k, false), Some(GoToCommand::Prev));
        assert_eq!(
            router.route_go_to(&slash, false),
            Some(GoToCommand::StartFilter)
        );
        assert_eq!(router.route_go_to(&q, false), Some(GoToCommand::Close));

        // フィルタ入力中は j / q は文字入力扱いになり、Ctrl+j は移動として効く
        assert_eq!(router.route_go_to(&j, true), None);
        assert_eq!(router.route_go_to(&q, true), None);
        assert_eq!(router.route_go_to(&ctrl_j, true), Some(GoToCommand::Next));
    }

    #[test]
    fn default_prefix_ctrl_b_opens_keybind_hints() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.keybinds.prefix, "ctrl+b");
        let mut router = InputRouter::from_config(&cfg);
        let ctx = MockCtx { has_right: false };

        let ctrl_b = KeyEvent {
            code: KeyCode::Char('b'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&ctrl_b, &ctx), InputDecision::Ignore);
        assert_eq!(router.mode, InputMode::Prefix);

        let qmark = KeyEvent {
            code: KeyCode::Char('?'),
            mods: mods(false, false, true),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&qmark, &ctx),
            InputDecision::Execute(Action::ToggleKeybindHints)
        );
        assert_eq!(router.mode, InputMode::Normal);
    }

    #[test]
    fn new_workspace_default_and_prefix_override_work() {
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.keybinds.new_workspace, "ctrl+shift+t");
        let mut router = InputRouter::from_config(&cfg);
        let ctx = MockCtx { has_right: false };

        let ctrl_shift_t = KeyEvent {
            code: KeyCode::Char('t'),
            mods: mods(true, false, true),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&ctrl_shift_t, &ctx),
            InputDecision::Execute(Action::NewWorkspace)
        );

        cfg.keybinds.new_workspace = "prefix+w".to_string();
        let mut router = InputRouter::from_config(&cfg);
        let ctrl_b = KeyEvent {
            code: KeyCode::Char('b'),
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        let key_w = KeyEvent {
            code: KeyCode::Char('w'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(router.route(&ctrl_b, &ctx), InputDecision::Ignore);
        assert_eq!(
            router.route(&key_w, &ctx),
            InputDecision::Execute(Action::NewWorkspace)
        );
    }

    #[test]
    fn right_drawer_preview_open_and_custom_keybinds() {
        let mut router = InputRouter::default();
        let ctx = MockCtx { has_right: false };
        router.mode = InputMode::RightDrawer;

        let enter = KeyEvent {
            code: KeyCode::Enter,
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&enter, &ctx),
            InputDecision::Execute(Action::DrawerPreview)
        );

        let ctrl_enter = KeyEvent {
            code: KeyCode::Enter,
            mods: mods(true, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&ctrl_enter, &ctx),
            InputDecision::Execute(Action::DrawerOpen)
        );

        let cmd_enter = KeyEvent {
            code: KeyCode::Enter,
            mods: Modifiers {
                ctrl: false,
                alt: false,
                shift: false,
                logo: true,
            },
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            router.route(&cmd_enter, &ctx),
            InputDecision::Execute(Action::DrawerOpen)
        );

        // config によるカスタマイズ上書き
        let mut cfg = AppConfig::default();
        cfg.keybinds.drawer_preview = "p".to_string();
        cfg.keybinds.drawer_open = "enter".to_string();
        let mut custom_router = InputRouter::from_config(&cfg);
        custom_router.mode = InputMode::RightDrawer;

        let key_p = KeyEvent {
            code: KeyCode::Char('p'),
            mods: mods(false, false, false),
            left_alt: false,
            unshifted: None,
            base_layout: None,
            kind: KeyEventKind::Press,
        };
        assert_eq!(
            custom_router.route(&key_p, &ctx),
            InputDecision::Execute(Action::DrawerPreview)
        );
        assert_eq!(
            custom_router.route(&enter, &ctx),
            InputDecision::Execute(Action::DrawerOpen)
        );
    }
}
