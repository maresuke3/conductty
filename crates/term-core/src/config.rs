use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub appearance: AppearanceConfig,
    pub shell: ShellConfig,
    pub window: WindowConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct ShellConfig {
    pub windows_default_prog: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    pub save_state: String,
    pub split_preserve_zoom: String,
    pub macos_option_as_alt: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            appearance: AppearanceConfig {
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
            },
            shell: ShellConfig {
                windows_default_prog: vec!["wsl.exe".into(), "--cd".into(), "~".into()],
            },
            window: WindowConfig {
                save_state: "always".to_string(),
                split_preserve_zoom: "navigation".to_string(),
                macos_option_as_alt: "left".to_string(),
            },
        }
    }
}
