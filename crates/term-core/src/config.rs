use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub include: Option<String>,
    pub features: FeaturesConfig,
    pub appearance: AppearanceConfig,
    pub theme: ThemeColorsConfig,
    pub keybinds: KeybindsConfig,
    pub shell: ShellConfig,
    pub window: WindowConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeftDrawerMode {
    Always,
    Collapsed,
    Hidden,
}

impl Default for LeftDrawerMode {
    fn default() -> Self {
        Self::Always
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeaturesConfig {
    pub multiplexer_mode: bool,
    pub left_drawer_mode: LeftDrawerMode,
    pub right_drawer: bool,
    pub right_drawer_default_open: bool,
    pub embedded_browser: bool,
    pub kitty_graphics: bool,
    pub copy_on_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub font_families: Vec<String>,
    pub font_size: f32,
    pub font_thicken: bool,
    pub adjust_cell_height: f32,
    pub theme: String,
    pub background_opacity: f32,
    pub window_padding_x: f32,
    pub window_padding_y: f32,
    pub window_padding_balance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeColorsConfig {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub accent: Option<String>,
    pub drawer_background: Option<String>,
    pub card_background: Option<String>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindsConfig {
    pub prefix: String,
    pub new_tab: String,
    pub split_right: String,
    pub split_down: String,
    pub toggle_zoom: String,
    pub pane_mode: String,
    pub goto_left: String,
    pub goto_down: String,
    pub goto_up: String,
    pub goto_right: String,
    pub toggle_left_drawer: String,
    pub toggle_right_drawer: String,
    pub select_pane: String,
    pub go_to: String,
    pub go_to_next: String,
    pub go_to_prev: String,
    pub go_to_select: String,
    pub go_to_close: String,
    pub go_to_filter: String,
    pub copy: String,
    pub paste: String,
    pub reload_config: String,
    pub keybind_hints: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub windows_default_prog: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub save_state: String,
    pub split_preserve_zoom: String,
    pub macos_option_as_alt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    pub accent: [u8; 3],
    pub drawer_bg: [u8; 3],
    pub card_bg: [u8; 3],
    pub cursor: [u8; 3],
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            multiplexer_mode: false,
            left_drawer_mode: LeftDrawerMode::Always,
            right_drawer: true,
            right_drawer_default_open: false,
            embedded_browser: true,
            kitty_graphics: true,
            copy_on_select: true,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            font_families: vec![
                "Cica".to_string(),
                "FiraCode Nerd Font".to_string(),
                "Fira Code".to_string(),
            ],
            font_size: 18.0,
            font_thicken: true,
            adjust_cell_height: 2.0,
            theme: "TokyoNight Moon".to_string(),
            background_opacity: 1.0,
            window_padding_x: 10.0,
            window_padding_y: 10.0,
            window_padding_balance: true,
        }
    }
}

impl Default for ThemeColorsConfig {
    fn default() -> Self {
        Self {
            background: None,
            foreground: None,
            accent: None,
            drawer_background: None,
            card_background: None,
            cursor: None,
        }
    }
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            prefix: "ctrl+b".to_string(),
            new_tab: "ctrl+t".to_string(),
            split_right: "alt+d".to_string(),
            split_down: "alt+shift+d".to_string(),
            toggle_zoom: "ctrl+shift+z".to_string(),
            pane_mode: "ctrl+shift+h".to_string(),
            goto_left: "performable:ctrl+h".to_string(),
            goto_down: "performable:ctrl+j".to_string(),
            goto_up: "performable:ctrl+k".to_string(),
            goto_right: "performable:ctrl+l".to_string(),
            toggle_left_drawer: "ctrl+shift+e".to_string(),
            toggle_right_drawer: "ctrl+shift+g".to_string(),
            select_pane: "ctrl+shift+p".to_string(),
            go_to: "ctrl+shift+space".to_string(),
            go_to_next: "j, down, ctrl+j, ctrl+n".to_string(),
            go_to_prev: "k, up, ctrl+k, ctrl+p".to_string(),
            go_to_select: "enter".to_string(),
            go_to_close: "esc, q".to_string(),
            go_to_filter: "/".to_string(),
            copy: "ctrl+shift+c".to_string(),
            paste: "ctrl+v".to_string(),
            reload_config: "ctrl+shift+r".to_string(),
            keybind_hints: "prefix+?".to_string(),
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            windows_default_prog: vec!["wsl.exe".into(), "--cd".into(), "~".into()],
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            save_state: "always".to_string(),
            split_preserve_zoom: "navigation".to_string(),
            macos_option_as_alt: "left".to_string(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            include: None,
            features: FeaturesConfig::default(),
            appearance: AppearanceConfig::default(),
            theme: ThemeColorsConfig::default(),
            keybinds: KeybindsConfig::default(),
            shell: ShellConfig::default(),
            window: WindowConfig::default(),
        }
    }
}

/// 設定ファイルの探索優先順位:
/// 1. `~/.config/conductty/config.conductty`
/// 2. `~/.config/config.conductty`
/// 3. `~/.conductty/config.conductty`
/// 4. `~/.config.conductty`
pub fn config_search_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".config").join("conductty").join("config.conductty"),
        home.join(".config").join("config.conductty"),
        home.join(".conductty").join("config.conductty"),
        home.join(".config.conductty"),
    ]
}

fn resolve_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    }
}

fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_map), toml::Value::Table(overlay_map)) => {
            for (k, v) in overlay_map {
                if k == "include" {
                    continue;
                }
                if let Some(existing) = base_map.get_mut(&k) {
                    merge_toml(existing, v);
                } else {
                    base_map.insert(k, v);
                }
            }
        }
        (b, o) => *b = o,
    }
}

fn load_toml_with_include(path: &Path, depth: u8) -> Result<toml::Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("config read error ({}): {e}", path.display()))?;
    let val: toml::Value = raw
        .parse()
        .map_err(|e| format!("config parse error ({}): {e}", path.display()))?;

    if depth < 5 {
        if let Some(inc) = val.get("include").and_then(|v| v.as_str()) {
            let inc_path = Path::new(inc);
            let resolved = if inc_path.is_absolute() || inc.starts_with("\\\\") {
                inc_path.to_path_buf()
            } else {
                path.parent().unwrap_or_else(|| Path::new(".")).join(inc_path)
            };
            let mut base = load_toml_with_include(&resolved, depth + 1)?;
            merge_toml(&mut base, val);
            return Ok(base);
        }
    }
    Ok(val)
}

impl AppConfig {
    pub fn load() -> Self {
        if let Some(home) = resolve_home_dir() {
            let paths = config_search_paths(&home);
            for path in &paths {
                if path.is_file() {
                    match load_toml_with_include(path, 0)
                        .and_then(|v| v.try_into::<AppConfig>().map_err(|e| e.to_string()))
                    {
                        Ok(cfg) => {
                            eprintln!("[conductty] loaded config: {}", path.display());
                            return cfg;
                        }
                        Err(err) => {
                            eprintln!("[conductty] {err}");
                            return Self::default();
                        }
                    }
                }
            }
            eprintln!(
                "[conductty] config not found (checked home: {}), using default",
                home.display()
            );
        }
        Self::default()
    }

    pub fn from_toml_str(raw: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(raw)
    }

    pub fn resolve_theme(&self) -> ResolvedTheme {
        let mut base = match self.appearance.theme.to_lowercase().as_str() {
            "tokyonight" | "tokyo night" => ResolvedTheme {
                bg: [0x1a, 0x1b, 0x26],
                fg: [0xc0, 0xca, 0xf5],
                accent: [0x7a, 0xa2, 0xf7],
                drawer_bg: [0x16, 0x16, 0x1e],
                card_bg: [0x24, 0x28, 0x3b],
                cursor: [0x7a, 0xa2, 0xf7],
            },
            "solarized dark" => ResolvedTheme {
                bg: [0x00, 0x2b, 0x36],
                fg: [0x83, 0x94, 0x96],
                accent: [0x26, 0x8b, 0xd2],
                drawer_bg: [0x00, 0x21, 0x2b],
                card_bg: [0x07, 0x36, 0x42],
                cursor: [0x93, 0xa1, 0xa1],
            },
            // Default: TokyoNight Moon
            _ => ResolvedTheme {
                bg: [0x22, 0x24, 0x36],
                fg: [0xc8, 0xd3, 0xf5],
                accent: [0x7a, 0xa2, 0xf7],
                drawer_bg: [0x1e, 0x20, 0x30],
                card_bg: [0x2f, 0x33, 0x4d],
                cursor: [0x82, 0xaa, 0xff],
            },
        };

        if let Some(hex) = self.theme.background.as_deref().and_then(parse_hex_rgb) {
            base.bg = hex;
        }
        if let Some(hex) = self.theme.foreground.as_deref().and_then(parse_hex_rgb) {
            base.fg = hex;
        }
        if let Some(hex) = self.theme.accent.as_deref().and_then(parse_hex_rgb) {
            base.accent = hex;
        }
        if let Some(hex) = self.theme.drawer_background.as_deref().and_then(parse_hex_rgb) {
            base.drawer_bg = hex;
        }
        if let Some(hex) = self.theme.card_background.as_deref().and_then(parse_hex_rgb) {
            base.card_bg = hex;
        }
        if let Some(hex) = self.theme.cursor.as_deref().and_then(parse_hex_rgb) {
            base.cursor = hex;
        }
        base
    }
}

pub fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_paths_follow_exact_priority_order() {
        let home = Path::new("/home/tester");
        let paths = config_search_paths(home);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/tester/.config/conductty/config.conductty"),
                PathBuf::from("/home/tester/.config/config.conductty"),
                PathBuf::from("/home/tester/.conductty/config.conductty"),
                PathBuf::from("/home/tester/.config.conductty"),
            ]
        );
    }

    #[test]
    fn partial_config_preserves_defaults_and_overrides_fields() {
        let raw = r##"
[features]
multiplexer_mode = true
left_drawer_mode = "collapsed"
right_drawer = false
embedded_browser = false

[appearance]
theme = "TokyoNight"
background_opacity = 0.85

[theme]
accent = "#ff9e64"

[keybinds]
new_tab = "ctrl+shift+t"
"##;
        let cfg = AppConfig::from_toml_str(raw).unwrap();
        assert!(cfg.features.multiplexer_mode);
        assert_eq!(cfg.features.left_drawer_mode, LeftDrawerMode::Collapsed);
        assert!(!cfg.features.right_drawer);
        assert!(!cfg.features.embedded_browser);
        assert!(cfg.features.kitty_graphics); // 未指定はデフォルト維持
        assert!((cfg.appearance.background_opacity - 0.85).abs() < f32::EPSILON);
        assert_eq!(cfg.keybinds.new_tab, "ctrl+shift+t");
        assert_eq!(cfg.keybinds.split_right, "alt+d"); // 未指定はデフォルト維持

        let resolved = cfg.resolve_theme();
        assert_eq!(resolved.bg, [0x1a, 0x1b, 0x26]); // TokyoNight preset
        assert_eq!(resolved.accent, [0xff, 0x9e, 0x64]); // custom hex override
    }

    #[test]
    fn include_loads_target_file_and_merges_overrides() {
        let dir = std::env::temp_dir().join("conductty_include_test");
        let _ = std::fs::create_dir_all(&dir);
        let base_path = dir.join("base.conductty");
        let entry_path = dir.join("entry.conductty");
        std::fs::write(
            &base_path,
            "[features]\nleft_drawer_mode = \"hidden\"\n[appearance]\nfont_size = 20.0\n",
        )
        .unwrap();
        std::fs::write(
            &entry_path,
            format!(
                "include = \"{}\"\n[appearance]\nfont_size = 16.0\n",
                base_path.display().to_string().replace('\\', "\\\\")
            ),
        )
        .unwrap();

        let val = load_toml_with_include(&entry_path, 0).unwrap();
        let cfg: AppConfig = val.try_into().unwrap();
        assert_eq!(cfg.features.left_drawer_mode, LeftDrawerMode::Hidden);
        assert!((cfg.appearance.font_size - 16.0).abs() < f32::EPSILON);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
