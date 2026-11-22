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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub mods: Modifiers,
    /// macOS の `macos-option-as-alt = left` 用: 左 Option が押されているか
    pub left_alt: bool,
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

        Self {
            mode: InputMode::Normal,
            macos_left_option_as_alt: config.window.macos_option_as_alt == "left",
            normal_bindings,
            prefix_trigger,
            prefix_bindings,
            go_to_bindings,
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
        let m = event.mods;
        if m.ctrl
            && m.shift
            && !m.alt
            && matches!(event.code, KeyCode::Char('g') | KeyCode::Char('G'))
        {
            self.mode = InputMode::Normal;
            return InputDecision::Execute(Action::CloseRightDrawer);
        }

        match (&event.code, m.ctrl, m.shift, m.alt) {
            (KeyCode::Escape, _, _, _)
            | (KeyCode::Char('q') | KeyCode::Char('Q'), false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::CloseRightDrawer)
            }
            (KeyCode::Char('j') | KeyCode::Char('J'), false, false, false)
            | (KeyCode::Down, false, false, false) => InputDecision::Execute(Action::DrawerNext),
            (KeyCode::Char('k') | KeyCode::Char('K'), false, false, false)
            | (KeyCode::Up, false, false, false) => InputDecision::Execute(Action::DrawerPrev),
            (KeyCode::Char('h') | KeyCode::Char('H'), false, false, false)
            | (KeyCode::Left, false, false, false)
            | (KeyCode::Tab, false, true, false) => {
                InputDecision::Execute(Action::DrawerPrevPlugin)
            }
            (KeyCode::Char('l') | KeyCode::Char('L'), false, false, false)
            | (KeyCode::Right, false, false, false)
            | (KeyCode::Tab, false, false, false) => {
                InputDecision::Execute(Action::DrawerNextPlugin)
            }
            (KeyCode::Enter, false, false, false)
            | (KeyCode::Char('o') | KeyCode::Char('O'), false, false, false) => {
                InputDecision::Execute(Action::DrawerOpen)
            }
            (KeyCode::Char('b') | KeyCode::Char('B'), false, false, false) => {
                InputDecision::Execute(Action::DrawerPreview)
            }
            (KeyCode::Char('s') | KeyCode::Char('S'), false, false, false) => {
                InputDecision::Execute(Action::DrawerSendToPane)
            }
            (KeyCode::Char('r') | KeyCode::Char('R'), false, false, false) => {
                InputDecision::Execute(Action::DrawerRefresh)
            }
            _ => InputDecision::Ignore,
        }
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

    fn encode_vt_bytes(&self, event: &KeyEvent) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let treat_as_alt = event.mods.alt && (!self.macos_left_option_as_alt || event.left_alt);
        if treat_as_alt {
            out.push(0x1b);
        }

        match event.code {
            KeyCode::Char(c) => {
                if event.mods.ctrl && c.is_ascii_alphabetic() {
                    let ctrl_byte = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                    out.push(ctrl_byte);
                } else {
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    out.extend_from_slice(s.as_bytes());
                }
            }
            KeyCode::Enter => out.push(b'\r'),
            KeyCode::Escape => out.push(0x1b),
            KeyCode::Backspace => out.push(0x7f),
            KeyCode::Tab => out.push(b'\t'),
            KeyCode::Space => out.push(b' '),
            KeyCode::Backquote => out.push(b'`'),
            KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
            KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
            KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
            KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        }
        Some(out)
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
    fn performable_goto_split_passes_through_when_unavailable() {
        let mut router = InputRouter::default();
        let ev = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(true, false, false),
            left_alt: false,
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
        };
        assert_eq!(router.route(&random, &ctx), InputDecision::Ignore);

        // Escape で Normal に戻る
        let esc = KeyEvent {
            code: KeyCode::Escape,
            mods: mods(false, false, false),
            left_alt: false,
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
        };
        let key_h = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(false, false, false),
            left_alt: false,
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
        };
        let key_r = KeyEvent {
            code: KeyCode::Char('r'),
            mods: mods(false, false, false),
            left_alt: false,
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
        };
        assert_eq!(
            router.route(&key_l, &ctx),
            InputDecision::Execute(Action::ResizeSplit(NavDirection::Right))
        );

        let enter = KeyEvent {
            code: KeyCode::Enter,
            mods: mods(false, false, false),
            left_alt: false,
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
        };
        assert_eq!(
            router.route(&j, &ctx),
            InputDecision::Execute(Action::DrawerNext)
        );

        let l = KeyEvent {
            code: KeyCode::Char('l'),
            mods: mods(false, false, false),
            left_alt: false,
        };
        assert_eq!(
            router.route(&l, &ctx),
            InputDecision::Execute(Action::DrawerNextPlugin)
        );

        let h = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(false, false, false),
            left_alt: false,
        };
        assert_eq!(
            router.route(&h, &ctx),
            InputDecision::Execute(Action::DrawerPrevPlugin)
        );

        let q = KeyEvent {
            code: KeyCode::Char('q'),
            mods: mods(false, false, false),
            left_alt: false,
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
        };
        assert_eq!(router.route(&ctrl_a, &ctx), InputDecision::Ignore);
        assert_eq!(router.mode, InputMode::Prefix);

        let key_c = KeyEvent {
            code: KeyCode::Char('c'),
            mods: mods(false, false, false),
            left_alt: false,
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
        };
        let k = KeyEvent {
            code: KeyCode::Char('k'),
            mods: mods(false, false, false),
            left_alt: false,
        };
        let slash = KeyEvent {
            code: KeyCode::Char('/'),
            mods: mods(false, false, false),
            left_alt: false,
        };
        let q = KeyEvent {
            code: KeyCode::Char('q'),
            mods: mods(false, false, false),
            left_alt: false,
        };
        let ctrl_j = KeyEvent {
            code: KeyCode::Char('j'),
            mods: mods(true, false, false),
            left_alt: false,
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
        };
        assert_eq!(router.route(&ctrl_b, &ctx), InputDecision::Ignore);
        assert_eq!(router.mode, InputMode::Prefix);

        let qmark = KeyEvent {
            code: KeyCode::Char('?'),
            mods: mods(false, false, true),
            left_alt: false,
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
        };
        let key_w = KeyEvent {
            code: KeyCode::Char('w'),
            mods: mods(false, false, false),
            left_alt: false,
        };
        assert_eq!(router.route(&ctrl_b, &ctx), InputDecision::Ignore);
        assert_eq!(
            router.route(&key_w, &ctx),
            InputDecision::Execute(Action::NewWorkspace)
        );
    }
}
