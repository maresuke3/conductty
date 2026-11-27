use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::grid::{Cell, Color, DEFAULT_PALETTE};

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
    #[serde(default = "default_plugins")]
    pub plugins: Vec<DrawerPluginConfig>,
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
            plugins: default_plugins(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DrawerPluginConfig {
    pub id: String,
    pub name: String,
    pub category: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub show_send_to_pane: bool,
    pub config: toml::Value,
}

impl Default for DrawerPluginConfig {
    fn default() -> Self {
        Self {
            id: "github".to_string(),
            name: "GitHub".to_string(),
            category: "tasks".to_string(),
            command: "conductty-plugin-github".to_string(),
            args: Vec::new(),
            enabled: true,
            show_send_to_pane: true,
            config: toml::Value::Table(toml::map::Map::new()),
        }
    }
}

pub fn default_plugins() -> Vec<DrawerPluginConfig> {
    vec![
        DrawerPluginConfig::default(),
        DrawerPluginConfig {
            id: "docs".to_string(),
            name: "Docs".to_string(),
            category: "docs".to_string(),
            command: "conductty-plugin-docs".to_string(),
            args: Vec::new(),
            enabled: true,
            show_send_to_pane: true,
            config: toml::Value::Table(toml::map::Map::new()),
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum LeftDrawerMode {
    #[default]
    Always,
    Collapsed,
    Hidden,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeaturesConfig {
    pub multiplexer_mode: bool,
    pub left_drawer_mode: LeftDrawerMode,
    pub left_drawer_toggle_button: bool,
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
#[derive(Default)]
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
    pub new_workspace: String,
    pub split_right: String,
    pub split_down: String,
    pub toggle_zoom: String,
    pub pane_mode: String,
    pub pane_move_mode: String,
    pub pane_resize_mode: String,
    pub goto_left: String,
    pub goto_down: String,
    pub goto_up: String,
    pub goto_right: String,
    pub toggle_left_drawer: String,
    pub toggle_right_drawer: String,
    pub drawer_next: String,
    pub drawer_prev: String,
    pub drawer_next_plugin: String,
    pub drawer_prev_plugin: String,
    pub drawer_preview: String,
    pub drawer_open: String,
    pub drawer_send_to_pane: String,
    pub drawer_refresh: String,
    pub drawer_close: String,
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
    /// 256色のパレット（ANSI の 0〜15 と、256色の 16〜255）
    pub palette: [[u8; 3]; 256],
}

/// プリセットが見つからないときの TokyoNight Moon
impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            bg: [0x22, 0x24, 0x36],
            fg: [0xc8, 0xd3, 0xf5],
            accent: [0x7a, 0xa2, 0xf7],
            drawer_bg: [0x1e, 0x20, 0x30],
            card_bg: [0x2f, 0x33, 0x4d],
            cursor: [0x82, 0xaa, 0xff],
            palette: DEFAULT_PALETTE,
        }
    }
}

impl ResolvedTheme {
    fn color_rgb(&self, color: Color) -> Option<[u8; 3]> {
        match color {
            Color::Default => None,
            Color::Indexed(i) => Some(self.palette[i as usize]),
            Color::Rgb(c) => Some([c.r, c.g, c.b]),
        }
    }

    /// セルを描く前景色と背景色。背景が None なら塗らない
    pub fn cell_colors(&self, cell: &Cell) -> ([u8; 3], Option<[u8; 3]>) {
        let fg = self.color_rgb(cell.fg).unwrap_or(self.fg);
        let bg = self.color_rgb(cell.bg);
        if cell.reverse {
            (bg.unwrap_or(self.bg), Some(fg))
        } else {
            (fg, bg)
        }
    }
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            multiplexer_mode: false,
            left_drawer_mode: LeftDrawerMode::Always,
            left_drawer_toggle_button: true,
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

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            prefix: "ctrl+b".to_string(),
            new_tab: "ctrl+t".to_string(),
            new_workspace: "ctrl+shift+t".to_string(),
            split_right: "alt+d".to_string(),
            split_down: "alt+shift+d".to_string(),
            toggle_zoom: "ctrl+shift+z".to_string(),
            pane_mode: "prefix+h".to_string(),
            pane_move_mode: "ctrl+shift+h".to_string(),
            pane_resize_mode: "prefix+r".to_string(),
            goto_left: "performable:ctrl+h".to_string(),
            goto_down: "performable:ctrl+j".to_string(),
            goto_up: "performable:ctrl+k".to_string(),
            goto_right: "performable:ctrl+l".to_string(),
            toggle_left_drawer: "ctrl+shift+e".to_string(),
            toggle_right_drawer: "ctrl+shift+g".to_string(),
            drawer_next: "j, down".to_string(),
            drawer_prev: "k, up".to_string(),
            drawer_next_plugin: "l, right, tab".to_string(),
            drawer_prev_plugin: "h, left, shift+tab".to_string(),
            drawer_preview: "enter, b".to_string(),
            drawer_open: "ctrl+enter, cmd+enter, o".to_string(),
            drawer_send_to_pane: "s".to_string(),
            drawer_refresh: "r".to_string(),
            drawer_close: "esc, q".to_string(),
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
            windows_default_prog: vec!["powershell.exe".into()],
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

/// 実行環境 OS の種別（OS 別設定ファイルおよび `[os.<name>]` 上書きの判定用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    MacOs,
    Windows,
    Linux,
    Wsl,
}

impl TargetOs {
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WSL_DISTRO_NAME").is_some() {
                Self::Wsl
            } else {
                Self::Linux
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Self::Linux
        }
    }

    /// `[os.<key>]` セクションで適用するキー順（WSL は `linux` の後に `wsl` を重ねる）
    pub fn override_keys(self) -> &'static [&'static str] {
        match self {
            Self::MacOs => &["macos"],
            Self::Windows => &["windows"],
            Self::Linux => &["linux"],
            Self::Wsl => &["linux", "wsl"],
        }
    }

    /// 優先して探索する OS 専用設定ファイル名（先頭ほど高優先度）
    pub fn file_names(self) -> &'static [&'static str] {
        match self {
            Self::MacOs => &["macos.conductty"],
            Self::Windows => &["windows.conductty"],
            Self::Linux => &["linux.conductty"],
            Self::Wsl => &["wsl.conductty", "linux.conductty"],
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
        home.join(".config")
            .join("conductty")
            .join("config.conductty"),
        home.join(".config").join("config.conductty"),
        home.join(".conductty").join("config.conductty"),
        home.join(".config.conductty"),
    ]
}

/// OS 専用設定ファイル（`<os>.conductty`）を優先しつつ探索するパス一覧を返す。
/// 各配置ディレクトリで `<os>.conductty` が見つかればそれを採用し、`config.conductty` は読まない。
pub fn config_search_paths_for_os(home: &Path, target_os: TargetOs) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let dirs = [
        home.join(".config").join("conductty"),
        home.join(".config"),
        home.join(".conductty"),
    ];
    for dir in dirs {
        for name in target_os.file_names() {
            paths.push(dir.join(name));
        }
        paths.push(dir.join("config.conductty"));
    }
    for name in target_os.file_names() {
        paths.push(home.join(format!(".{name}")));
    }
    paths.push(home.join(".config.conductty"));
    paths
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

const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

fn is_wsl_unc_path(inc: &str) -> bool {
    let norm: String = inc
        .chars()
        .map(|c| {
            if c == '/' {
                '\\'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    norm.starts_with("\\\\wsl.localhost\\") || norm.starts_with("\\\\wsl$\\")
}

fn resolve_include_path(base_dir: &Path, inc: &str) -> Result<PathBuf, String> {
    if matches!(inc.as_bytes(), [b'\\' | b'/', b'\\' | b'/', ..]) && !is_wsl_unc_path(inc) {
        return Err(format!("UNC include path is not allowed: {inc}"));
    }
    let inc_path = Path::new(inc);
    if inc_path.is_absolute() {
        Ok(inc_path.to_path_buf())
    } else {
        Ok(base_dir.join(inc_path))
    }
}

fn apply_os_overrides(
    val: &mut toml::Value,
    target_os: TargetOs,
    base_dir: Option<&Path>,
    depth: u8,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let Some(root_map) = val.as_table_mut() else {
        return Ok(());
    };
    let Some(toml::Value::Table(mut os_map)) = root_map.remove("os") else {
        return Ok(());
    };

    for key in target_os.override_keys() {
        let Some(mut section) = os_map.remove(*key) else {
            continue;
        };
        if depth < 5
            && let (Some(dir), Some(inc)) = (
                base_dir,
                section
                    .get("include")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            )
        {
            let resolved = resolve_include_path(dir, &inc)?;
            let mut included =
                load_toml_with_include_inner(&resolved, depth + 1, target_os, visited)?;
            merge_toml(&mut included, section);
            section = included;
        }
        merge_toml(val, section);
    }
    Ok(())
}

fn update_theme_in_toml_text(raw: &str, theme_name: &str) -> String {
    let escaped = theme_name.replace('\\', "\\\\").replace('"', "\\\"");
    let new_line = format!("theme = \"{escaped}\"");
    let mut out = Vec::new();
    let mut in_appearance = false;
    let mut saw_appearance = false;
    let mut replaced = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_appearance && !replaced {
                out.push(new_line.clone());
                replaced = true;
            }
            in_appearance = trimmed.split('#').next().unwrap_or("").trim() == "[appearance]";
            if in_appearance {
                saw_appearance = true;
            }
            out.push(line.to_string());
            continue;
        }
        if in_appearance
            && !trimmed.starts_with('#')
            && trimmed
                .strip_prefix("theme")
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        {
            if !replaced {
                out.push(new_line.clone());
                replaced = true;
            }
            continue;
        }
        out.push(line.to_string());
    }

    if in_appearance && !replaced {
        out.push(new_line.clone());
        replaced = true;
    }
    if !replaced && !saw_appearance {
        if !out.is_empty() && !out.last().is_some_and(|l| l.is_empty()) {
            out.push(String::new());
        }
        out.push("[appearance]".to_string());
        out.push(new_line);
    }

    let mut result = out.join("\n");
    if raw.ends_with('\n') || !result.is_empty() {
        result.push('\n');
    }
    result
}

#[cfg(test)]
fn load_toml_with_include(path: &Path, depth: u8) -> Result<toml::Value, String> {
    load_toml_with_include_for_os(path, depth, TargetOs::current())
}

fn load_toml_with_include_for_os(
    path: &Path,
    depth: u8,
    target_os: TargetOs,
) -> Result<toml::Value, String> {
    let mut visited = HashSet::new();
    load_toml_with_include_inner(path, depth, target_os, &mut visited)
}

fn load_toml_with_include_inner(
    path: &Path,
    depth: u8,
    target_os: TargetOs,
    visited: &mut HashSet<PathBuf>,
) -> Result<toml::Value, String> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical.clone()) {
        return Err(format!(
            "cyclic config include detected: {}",
            path.display()
        ));
    }
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("config metadata error ({}): {e}", path.display()))?;
    if !meta.is_file() {
        visited.remove(&canonical);
        return Err(format!("config is not a regular file: {}", path.display()));
    }
    if meta.len() > MAX_CONFIG_FILE_BYTES {
        visited.remove(&canonical);
        return Err(format!(
            "config file too large ({} bytes): {}",
            meta.len(),
            path.display()
        ));
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("config read error ({}): {e}", path.display()))?;
    let mut val: toml::Value = raw
        .parse()
        .map_err(|e| format!("config parse error ({}): {e}", path.display()))?;

    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    if depth < 5
        && let Some(inc) = val.get("include").and_then(|v| v.as_str())
    {
        let resolved = resolve_include_path(base_dir, inc)?;
        let mut base = load_toml_with_include_inner(&resolved, depth + 1, target_os, visited)?;
        merge_toml(&mut base, val);
        val = base;
    }
    apply_os_overrides(&mut val, target_os, Some(base_dir), depth + 1, visited)?;
    visited.remove(&canonical);
    Ok(val)
}

/// 利用可能なビルトインテーマ名一覧（正規名称）
pub const THEME_PRESETS: &[&str] = &[
    "Solarized Dark",
    "Solarized Light",
    "Dracula",
    "GitHub Dark",
    "GitHub Light",
    "Gruvbox Dark",
    "Gruvbox Light",
    "Gruvbox Material",
    "Tokyo Night",
    "Tokyo Night Day",
    "Tokyo Night Moon",
    "Tokyo Night Night",
    "Tokyo Night Storm",
];

fn normalize_theme_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// テーマ名から正規名称と ResolvedTheme を解決する（大文字小文字・空白・ハイフン無視）
pub fn resolve_theme_preset(name: &str) -> Option<(&'static str, ResolvedTheme)> {
    match normalize_theme_key(name).as_str() {
        "solarizeddark" => Some((
            "Solarized Dark",
            ResolvedTheme {
                bg: [0x00, 0x2b, 0x36],
                fg: [0x83, 0x94, 0x96],
                accent: [0x26, 0x8b, 0xd2],
                drawer_bg: [0x00, 0x21, 0x2b],
                card_bg: [0x07, 0x36, 0x42],
                cursor: [0x93, 0xa1, 0xa1],
                ..ResolvedTheme::default()
            },
        )),
        "solarizedlight" => Some((
            "Solarized Light",
            ResolvedTheme {
                bg: [0xfd, 0xf6, 0xe3],
                fg: [0x58, 0x6e, 0x75],
                accent: [0x26, 0x8b, 0xd2],
                drawer_bg: [0xee, 0xe8, 0xd5],
                card_bg: [0xe4, 0xdd, 0xc8],
                cursor: [0x65, 0x7b, 0x83],
                ..ResolvedTheme::default()
            },
        )),
        "dracula" => Some((
            "Dracula",
            ResolvedTheme {
                bg: [0x28, 0x2a, 0x36],
                fg: [0xf8, 0xf8, 0xf2],
                accent: [0xbd, 0x93, 0xf9],
                drawer_bg: [0x21, 0x22, 0x2c],
                card_bg: [0x34, 0x37, 0x46],
                cursor: [0xff, 0x79, 0xc6],
                ..ResolvedTheme::default()
            },
        )),
        "githubdark" => Some((
            "GitHub Dark",
            ResolvedTheme {
                bg: [0x0d, 0x11, 0x17],
                fg: [0xe6, 0xed, 0xf3],
                accent: [0x58, 0xa6, 0xff],
                drawer_bg: [0x01, 0x04, 0x09],
                card_bg: [0x16, 0x1b, 0x22],
                cursor: [0x58, 0xa6, 0xff],
                ..ResolvedTheme::default()
            },
        )),
        "githublight" => Some((
            "GitHub Light",
            ResolvedTheme {
                bg: [0xff, 0xff, 0xff],
                fg: [0x1f, 0x23, 0x28],
                accent: [0x09, 0x69, 0xda],
                drawer_bg: [0xf6, 0xf8, 0xfa],
                card_bg: [0xea, 0xee, 0xf2],
                cursor: [0x09, 0x69, 0xda],
                ..ResolvedTheme::default()
            },
        )),
        "gruvboxdark" | "gruvbox" => Some((
            "Gruvbox Dark",
            ResolvedTheme {
                bg: [0x28, 0x28, 0x28],
                fg: [0xeb, 0xdb, 0xb2],
                accent: [0xfe, 0x80, 0x19],
                drawer_bg: [0x1d, 0x20, 0x21],
                card_bg: [0x3c, 0x38, 0x36],
                cursor: [0xfb, 0xf1, 0xc7],
                ..ResolvedTheme::default()
            },
        )),
        "gruvboxlight" => Some((
            "Gruvbox Light",
            ResolvedTheme {
                bg: [0xfb, 0xf1, 0xc7],
                fg: [0x3c, 0x38, 0x36],
                accent: [0xd6, 0x5d, 0x0e],
                drawer_bg: [0xf2, 0xe5, 0xbc],
                card_bg: [0xeb, 0xdb, 0xb2],
                cursor: [0x3c, 0x38, 0x36],
                ..ResolvedTheme::default()
            },
        )),
        "gruvboxmaterial" => Some((
            "Gruvbox Material",
            ResolvedTheme {
                bg: [0x29, 0x28, 0x28],
                fg: [0xd4, 0xbe, 0x98],
                accent: [0xe7, 0x8a, 0x4e],
                drawer_bg: [0x1d, 0x20, 0x21],
                card_bg: [0x32, 0x30, 0x2f],
                cursor: [0xd4, 0xbe, 0x98],
                ..ResolvedTheme::default()
            },
        )),
        "tokyonight" | "tokyonightnight" => Some((
            if normalize_theme_key(name) == "tokyonightnight" {
                "Tokyo Night Night"
            } else {
                "Tokyo Night"
            },
            ResolvedTheme {
                bg: [0x1a, 0x1b, 0x26],
                fg: [0xc0, 0xca, 0xf5],
                accent: [0x7a, 0xa2, 0xf7],
                drawer_bg: [0x16, 0x16, 0x1e],
                card_bg: [0x24, 0x28, 0x3b],
                cursor: [0xc0, 0xca, 0xf5],
                ..ResolvedTheme::default()
            },
        )),
        "tokyonightday" => Some((
            "Tokyo Night Day",
            ResolvedTheme {
                bg: [0xe1, 0xe2, 0xe7],
                fg: [0x37, 0x60, 0xbf],
                accent: [0x2e, 0x7d, 0xe9],
                drawer_bg: [0xd0, 0xd5, 0xe3],
                card_bg: [0xc4, 0xc8, 0xda],
                cursor: [0x37, 0x60, 0xbf],
                ..ResolvedTheme::default()
            },
        )),
        "tokyonightstorm" => Some((
            "Tokyo Night Storm",
            ResolvedTheme {
                bg: [0x24, 0x28, 0x3b],
                fg: [0xc0, 0xca, 0xf5],
                accent: [0x7a, 0xa2, 0xf7],
                drawer_bg: [0x1f, 0x23, 0x35],
                card_bg: [0x29, 0x2e, 0x42],
                cursor: [0xc0, 0xca, 0xf5],
                ..ResolvedTheme::default()
            },
        )),
        "tokyonightmoon" => Some((
            "Tokyo Night Moon",
            ResolvedTheme {
                bg: [0x22, 0x24, 0x36],
                fg: [0xc8, 0xd3, 0xf5],
                accent: [0x7a, 0xa2, 0xf7],
                drawer_bg: [0x1e, 0x20, 0x30],
                card_bg: [0x2f, 0x33, 0x4d],
                cursor: [0x82, 0xaa, 0xff],
                ..ResolvedTheme::default()
            },
        )),
        _ => None,
    }
}

impl AppConfig {
    pub fn load() -> Self {
        Self::load_for_os(TargetOs::current())
    }

    pub fn load_for_os(target_os: TargetOs) -> Self {
        if let Some(home) = resolve_home_dir() {
            let paths = config_search_paths_for_os(&home, target_os);
            for path in &paths {
                if path.is_file() {
                    match load_toml_with_include_for_os(path, 0, target_os)
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
        Self::from_toml_str_for_os(raw, TargetOs::current())
    }

    pub fn from_toml_str_for_os(raw: &str, target_os: TargetOs) -> Result<Self, toml::de::Error> {
        let mut val: toml::Value = toml::from_str(raw)?;
        let mut visited = HashSet::new();
        let _ = apply_os_overrides(&mut val, target_os, None, 0, &mut visited);
        val.try_into()
    }

    pub fn active_config_path() -> Option<PathBuf> {
        let home = resolve_home_dir()?;
        let paths = config_search_paths_for_os(&home, TargetOs::current());
        paths.iter().find(|p| p.is_file()).cloned().or_else(|| {
            Some(
                home.join(".config")
                    .join("conductty")
                    .join("config.conductty"),
            )
        })
    }

    pub fn save_theme_to_config(theme_name: &str) -> Result<PathBuf, String> {
        let path = Self::active_config_path()
            .ok_or_else(|| "設定ファイルの保存先ホームディレクトリが見つかりません".to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("設定ディレクトリ作成失敗: {e}"))?;
        }
        let existing = if path.is_file() {
            std::fs::read_to_string(&path).map_err(|e| format!("設定ファイル読み込み失敗: {e}"))?
        } else {
            String::new()
        };
        let updated = update_theme_in_toml_text(&existing, theme_name);
        std::fs::write(&path, updated).map_err(|e| format!("設定ファイル書き込み失敗: {e}"))?;
        Ok(path)
    }

    pub fn resolve_theme(&self) -> ResolvedTheme {
        let mut base = resolve_theme_preset(&self.appearance.theme)
            .map(|(_, t)| t)
            .unwrap_or_default();

        if let Some(hex) = self.theme.background.as_deref().and_then(parse_hex_rgb) {
            base.bg = hex;
        }
        if let Some(hex) = self.theme.foreground.as_deref().and_then(parse_hex_rgb) {
            base.fg = hex;
        }
        if let Some(hex) = self.theme.accent.as_deref().and_then(parse_hex_rgb) {
            base.accent = hex;
        }
        if let Some(hex) = self
            .theme
            .drawer_background
            .as_deref()
            .and_then(parse_hex_rgb)
        {
            base.drawer_bg = hex;
        }
        if let Some(hex) = self
            .theme
            .card_background
            .as_deref()
            .and_then(parse_hex_rgb)
        {
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
    fn cell_colors_resolve_with_theme_palette_and_reverse() {
        use crate::grid::Rgb;
        let mut theme = ResolvedTheme::default();
        theme.palette[1] = [0xff, 0x00, 0x55];
        let cell = |fg, bg, reverse| Cell {
            fg,
            bg,
            reverse,
            ..Cell::default()
        };

        assert_eq!(theme.cell_colors(&Cell::default()), (theme.fg, None));
        assert_eq!(
            theme.cell_colors(&cell(
                Color::Indexed(1),
                Color::Rgb(Rgb { r: 1, g: 2, b: 3 }),
                false
            )),
            ([0xff, 0x00, 0x55], Some([1, 2, 3]))
        );
        // 反転で背景が既定色なら、前景にはテーマの背景色を使う
        assert_eq!(
            theme.cell_colors(&cell(Color::Indexed(1), Color::Default, true)),
            (theme.bg, Some([0xff, 0x00, 0x55]))
        );
    }

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

    #[test]
    fn os_search_paths_prioritize_os_specific_file_before_config() {
        let home = Path::new("/home/tester");

        let mac_paths = config_search_paths_for_os(home, TargetOs::MacOs);
        assert_eq!(
            mac_paths[0],
            PathBuf::from("/home/tester/.config/conductty/macos.conductty")
        );
        assert_eq!(
            mac_paths[1],
            PathBuf::from("/home/tester/.config/conductty/config.conductty")
        );

        let wsl_paths = config_search_paths_for_os(home, TargetOs::Wsl);
        assert_eq!(
            wsl_paths[0],
            PathBuf::from("/home/tester/.config/conductty/wsl.conductty")
        );
        assert_eq!(
            wsl_paths[1],
            PathBuf::from("/home/tester/.config/conductty/linux.conductty")
        );
        assert_eq!(
            wsl_paths[2],
            PathBuf::from("/home/tester/.config/conductty/config.conductty")
        );
    }

    #[test]
    fn os_section_overrides_apply_only_for_matching_os_and_wsl_layers_over_linux() {
        let raw = r##"
[appearance]
font_size = 18.0
background_opacity = 0.85

[keybinds]
split_right = "alt+d"

[os.macos.appearance]
font_size = 16.0

[os.macos.keybinds]
split_right = "cmd+d"

[os.windows.appearance]
font_size = 20.0

[os.linux.features]
embedded_browser = false

[os.linux.appearance]
font_size = 17.0

[os.wsl.appearance]
background_opacity = 1.0
"##;

        let mac = AppConfig::from_toml_str_for_os(raw, TargetOs::MacOs).unwrap();
        assert!((mac.appearance.font_size - 16.0).abs() < f32::EPSILON);
        assert!((mac.appearance.background_opacity - 0.85).abs() < f32::EPSILON);
        assert_eq!(mac.keybinds.split_right, "cmd+d");
        assert!(mac.features.embedded_browser);

        let win = AppConfig::from_toml_str_for_os(raw, TargetOs::Windows).unwrap();
        assert!((win.appearance.font_size - 20.0).abs() < f32::EPSILON);
        assert_eq!(win.keybinds.split_right, "alt+d");

        let linux = AppConfig::from_toml_str_for_os(raw, TargetOs::Linux).unwrap();
        assert!((linux.appearance.font_size - 17.0).abs() < f32::EPSILON);
        assert!((linux.appearance.background_opacity - 0.85).abs() < f32::EPSILON);
        assert!(!linux.features.embedded_browser);

        // WSL は [os.linux.*] の後に [os.wsl.*] が重なる
        let wsl = AppConfig::from_toml_str_for_os(raw, TargetOs::Wsl).unwrap();
        assert!((wsl.appearance.font_size - 17.0).abs() < f32::EPSILON);
        assert!((wsl.appearance.background_opacity - 1.0).abs() < f32::EPSILON);
        assert!(!wsl.features.embedded_browser);
    }

    #[test]
    fn windows_default_shell_is_powershell_and_os_windows_shell_overrides_it() {
        assert_eq!(
            AppConfig::default().shell.windows_default_prog,
            vec!["powershell.exe"]
        );

        let raw = r#"
[os.windows.shell]
windows_default_prog = ["wsl.exe", "--cd", "~"]
"#;
        let win = AppConfig::from_toml_str_for_os(raw, TargetOs::Windows).unwrap();
        assert_eq!(win.shell.windows_default_prog, vec!["wsl.exe", "--cd", "~"]);

        let mac = AppConfig::from_toml_str_for_os(raw, TargetOs::MacOs).unwrap();
        assert_eq!(mac.shell.windows_default_prog, vec!["powershell.exe"]);
    }

    #[test]
    fn include_rejects_unc_paths_and_cycles() {
        for bad in [
            r"\\evil-server\share\a.toml",
            "//evil-server/share/a.toml",
            r"\/evil-server/share/a.toml",
            r"/\evil-server\share\a.toml",
        ] {
            let err = resolve_include_path(Path::new("/tmp"), bad).unwrap_err();
            assert!(err.contains("UNC"), "expected UNC rejection for {bad}");
        }

        // Windows から WSL 上の設定を include するローカル WSL パスは許可する
        for wsl in [
            r"\\wsl.localhost\Ubuntu\home\user\config.conductty",
            r"\\WSL$\Ubuntu\home\user\config.conductty",
            "//wsl.localhost/Ubuntu/home/user/config.conductty",
        ] {
            assert!(
                resolve_include_path(Path::new("/tmp"), wsl).is_ok(),
                "expected WSL UNC path to be allowed: {wsl}"
            );
        }

        let dir = std::env::temp_dir().join("conductty_test_cycle");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a_path = dir.join("a.toml");
        let b_path = dir.join("b.toml");
        let big_path = dir.join("big.toml");
        std::fs::write(&a_path, "include = \"b.toml\"\n").unwrap();
        std::fs::write(&b_path, "include = \"a.toml\"\n").unwrap();
        std::fs::write(&big_path, vec![b'#'; (MAX_CONFIG_FILE_BYTES as usize) + 16]).unwrap();

        let cycle_err = load_toml_with_include(&a_path, 0).unwrap_err();
        assert!(cycle_err.contains("cyclic"));
        let big_err = load_toml_with_include(&big_path, 0).unwrap_err();
        assert!(big_err.contains("too large"));
        let dir_err = load_toml_with_include(&dir, 0).unwrap_err();
        assert!(dir_err.contains("not a regular file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_theme_presets_resolve_and_update_toml() {
        for preset in THEME_PRESETS {
            let (canonical, _) = resolve_theme_preset(preset).expect("preset should resolve");
            assert_eq!(canonical, *preset);
        }
        assert_eq!(
            resolve_theme_preset("tokyo-night-storm").map(|(n, _)| n),
            Some("Tokyo Night Storm")
        );
        assert_eq!(
            resolve_theme_preset("solarized light").map(|(n, _)| n),
            Some("Solarized Light")
        );
        assert_eq!(
            resolve_theme_preset("gruvbox-material").map(|(n, _)| n),
            Some("Gruvbox Material")
        );

        let raw = "[features]\nright_drawer = true\n\n[appearance]\ntheme = \"TokyoNight Moon\"\nfont_size = 18.0\n";
        let updated = update_theme_in_toml_text(raw, "Dracula");
        assert!(updated.contains("theme = \"Dracula\""));
        assert!(!updated.contains("TokyoNight Moon"));
        assert!(updated.contains("font_size = 18.0"));

        // [[plugins]] 内の theme キーは触らず、[appearance] 側に追記する
        let raw = "[appearance]\nfont_size = 18.0\n\n[[plugins]]\ntheme = \"keep\"\n";
        let updated = update_theme_in_toml_text(raw, "GitHub Light");
        let v: toml::Value = toml::from_str(&updated).expect("valid toml");
        assert_eq!(v["appearance"]["theme"].as_str(), Some("GitHub Light"));
        assert_eq!(v["plugins"][0]["theme"].as_str(), Some("keep"));

        // [appearance] が無ければ新設する
        let updated = update_theme_in_toml_text("[features]\nright_drawer = true\n", "Dracula");
        let v: toml::Value = toml::from_str(&updated).expect("valid toml");
        assert_eq!(v["appearance"]["theme"].as_str(), Some("Dracula"));
        assert_eq!(v["features"]["right_drawer"].as_bool(), Some(true));
    }
}
