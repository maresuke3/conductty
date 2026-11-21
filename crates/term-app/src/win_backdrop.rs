#[cfg(target_os = "windows")]
mod imp {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const LWA_ALPHA: u32 = 0x0000_0002;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            dw_attribute: u32,
            pv_attribute: *const core::ffi::c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, n_index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, n_index: i32, dw_new_long: isize) -> isize;
        fn SetLayeredWindowAttributes(hwnd: isize, cr_key: u32, b_alpha: u8, dw_flags: u32) -> i32;
    }

    pub fn apply(window: &Window, opacity: f32) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = win32.hwnd.get();

        unsafe {
            // 1. タイトルバーをダークモードにする (DWMWA_USE_IMMERSIVE_DARK_MODE = 20)
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                20,
                (&dark as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );

            // 2. Windows 透過: Vulkan + winit の白いすりガラスを避け、WS_EX_LAYERED + LWA_ALPHA で純粋に半透明化する
            if opacity < 0.999 {
                let alpha = (opacity.clamp(0.1, 1.0) * 255.0).round() as u8;
                let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED);
                let _ = SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use winit::window::Window;
    pub fn apply(_window: &Window, _opacity: f32) {}
}

pub use imp::apply;
