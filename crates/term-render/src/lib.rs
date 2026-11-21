use cosmic_text::{Attrs, Family, FontSystem};
use term_core::config::AppearanceConfig;

/// TokyoNight Moon の背景色 (RGBA 0.0..1.0)
pub fn tokyonight_moon_bg(opacity: f32) -> wgpu::Color {
    // #222436
    wgpu::Color {
        r: 0x22 as f64 / 255.0,
        g: 0x24 as f64 / 255.0,
        b: 0x36 as f64 / 255.0,
        a: opacity as f64,
    }
}

/// フォントフォールバック順（Cica -> FiraCode Nerd Font -> Fira Code -> システム）を組み立てる。
pub fn build_font_system() -> FontSystem {
    FontSystem::new()
}

/// 設定ファイルの `font_families` に沿って優先フォント属性を返す。
pub fn primary_font_attrs<'a>(config: &'a AppearanceConfig) -> Attrs<'a> {
    let primary = config
        .font_families
        .first()
        .map(|s| Family::Name(s.as_str()))
        .unwrap_or(Family::Monospace);
    Attrs::new().family(primary)
}
