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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_from_rgb_premultiplies_and_clamps_opacity() {
        let opaque = bg_from_rgb([255, 0, 0], 1.5);
        assert!((opaque.a - 1.0).abs() < 1e-6);
        assert!((opaque.r - 1.0).abs() < 1e-6);
        assert!(opaque.g.abs() < 1e-6);

        let half = bg_from_rgb([255, 255, 255], 0.5);
        assert!((half.a - 0.5).abs() < 1e-6);
        assert!((half.r - 0.5).abs() < 1e-6);
    }
}
