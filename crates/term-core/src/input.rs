use crate::config::AppConfig;
use crate::layout::NavDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Pane,
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
    EnterPaneMode,
    ExitPaneMode,
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
    DrawerOpen,
    DrawerPreview,
    DrawerSendToPane,
    DrawerRefresh,
    Copy,
    Paste,
    ReloadConfig,
    OpenGoTo,
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
        code_matches
            && self.ctrl == event.mods.ctrl
            && self.alt == event.mods.alt
            && self.shift == event.mods.shift
            && self.logo == event.mods.logo
    }
}

/// 状態遷移型の純粋入力ルーター
#[derive(Debug, Clone)]
pub struct InputRouter {
    pub mode: InputMode,
    pub macos_left_option_as_alt: bool,
    pub normal_bindings: Vec<KeyBinding>,
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
            (&kb.pane_mode, Action::EnterPaneMode),
            (&kb.toggle_zoom, Action::ToggleZoom),
            (&kb.toggle_left_drawer, Action::ToggleLeftDrawer),
            (&kb.toggle_right_drawer, Action::ToggleRightDrawer),
            (&kb.go_to, Action::OpenGoTo),
            (&kb.new_tab, Action::NewTab),
            (&kb.split_right, Action::SplitRight),
            (&kb.split_down, Action::SplitDown),
            (&kb.goto_left, Action::GotoSplit(NavDirection::Left)),
            (&kb.goto_down, Action::GotoSplit(NavDirection::Down)),
            (&kb.goto_up, Action::GotoSplit(NavDirection::Up)),
            (&kb.goto_right, Action::GotoSplit(NavDirection::Right)),
            (&kb.copy, Action::Copy),
            (&kb.paste, Action::Paste),
            (&kb.reload_config, Action::ReloadConfig),
        ];

        let mut normal_bindings = Vec::new();
        for (spec, action) in specs {
            if let Some(b) = KeyBinding::parse(spec, action) {
                normal_bindings.push(b);
            }
        }

        // macOS では Cmd+C / Cmd+V も標準で使えるようにする
        if let Some(b) = KeyBinding::parse("cmd+c", Action::Copy) {
            normal_bindings.push(b);
        }
        if let Some(b) = KeyBinding::parse("cmd+v", Action::Paste) {
            normal_bindings.push(b);
        }

        Self {
            mode: InputMode::Normal,
            macos_left_option_as_alt: config.window.macos_option_as_alt == "left",
            normal_bindings,
        }
    }
}

impl InputRouter {
    pub fn route(&mut self, event: &KeyEvent, ctx: &impl PerformableContext) -> InputDecision {
        match self.mode {
            InputMode::Pane => self.route_pane_mode(event),
            InputMode::LeftDrawer => self.route_left_drawer_mode(event),
            InputMode::RightDrawer => self.route_right_drawer_mode(event),
            InputMode::Normal => self.route_normal_mode(event, ctx),
        }
    }

    fn route_left_drawer_mode(&mut self, event: &KeyEvent) -> InputDecision {
        let m = event.mods;
        if m.ctrl && m.shift && !m.alt && matches!(event.code, KeyCode::Char('e') | KeyCode::Char('E')) {
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
            | (KeyCode::Up, false, false, false) => {
                InputDecision::Execute(Action::LeftDrawerPrev)
            }
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
        if m.ctrl && m.shift && !m.alt && matches!(event.code, KeyCode::Char('g') | KeyCode::Char('G')) {
            self.mode = InputMode::Normal;
            return InputDecision::Execute(Action::CloseRightDrawer);
        }

        match (&event.code, m.ctrl, m.shift, m.alt) {
            (KeyCode::Escape, _, _, _) | (KeyCode::Char('q') | KeyCode::Char('Q'), false, false, false) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::CloseRightDrawer)
            }
            (KeyCode::Char('j') | KeyCode::Char('J'), false, false, false) | (KeyCode::Down, false, false, false) => {
                InputDecision::Execute(Action::DrawerNext)
            }
            (KeyCode::Char('k') | KeyCode::Char('K'), false, false, false) | (KeyCode::Up, false, false, false) => {
                InputDecision::Execute(Action::DrawerPrev)
            }
            (KeyCode::Enter, false, false, false) | (KeyCode::Char('o') | KeyCode::Char('O'), false, false, false) => {
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

    fn route_pane_mode(&mut self, event: &KeyEvent) -> InputDecision {
        match (&event.code, event.mods.ctrl, event.mods.shift) {
            (KeyCode::Enter, _, _) | (KeyCode::Escape, _, _) => {
                self.mode = InputMode::Normal;
                InputDecision::Execute(Action::ExitPaneMode)
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

    fn route_normal_mode(
        &mut self,
        event: &KeyEvent,
        ctx: &impl PerformableContext,
    ) -> InputDecision {
        for binding in &self.normal_bindings {
            if binding.matches(event) {
                if binding.performable {
                    if let Action::GotoSplit(dir) = binding.action {
                        if !ctx.can_goto_split(dir) {
                            continue;
                        }
                    }
                }
                match binding.action {
                    Action::EnterPaneMode => self.mode = InputMode::Pane,
                    Action::ToggleLeftDrawer => self.mode = InputMode::LeftDrawer,
                    Action::ToggleRightDrawer => self.mode = InputMode::RightDrawer,
                    _ => {}
                }
                return InputDecision::Execute(binding.action);
            }
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
    fn pane_mode_captures_and_ignores_unknown_keys() {
        let mut router = InputRouter::default();
        let ctx = MockCtx { has_right: false };

        let enter_mode = KeyEvent {
            code: KeyCode::Char('h'),
            mods: mods(true, false, true),
            left_alt: false,
        };
        assert_eq!(
            router.route(&enter_mode, &ctx),
            InputDecision::Execute(Action::EnterPaneMode)
        );
        assert_eq!(router.mode, InputMode::Pane);

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
            InputDecision::Execute(Action::ExitPaneMode)
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
        assert_eq!(router.route(&ev, &ctx), InputDecision::Execute(Action::NewTab));
    }
}
