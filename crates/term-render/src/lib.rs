use cosmic_text::{Attrs, Family, FontSystem};
use term_core::config::AppearanceConfig;

fn srgb_to_linear(c: u8) -> f64 {
    let s = c as f64 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// 任意の sRGB `[r, g, b]` を wgpu の Linear 色空間へ変換して返す（透過時はプリマルチプライ）
pub fn bg_from_rgb(rgb: [u8; 3], opacity: f32) -> wgpu::Color {
    let a = opacity.clamp(0.0, 1.0) as f64;
    wgpu::Color {
        r: srgb_to_linear(rgb[0]) * a,
        g: srgb_to_linear(rgb[1]) * a,
        b: srgb_to_linear(rgb[2]) * a,
        a,
    }
}

/// TokyoNight Moon の背景色 (#222436) を wgpu の Linear 色空間へ正確に変換して返す
pub fn tokyonight_moon_bg(opacity: f32) -> wgpu::Color {
    bg_from_rgb([0x22, 0x24, 0x36], opacity)
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
