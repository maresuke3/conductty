#[cfg(target_os = "windows")]
mod imp {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            dw_attribute: u32,
            pv_attribute: *const core::ffi::c_void,
            cb_attribute: u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lp_module_name: *const u16) -> isize;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn LoadImageW(
            h_inst: isize,
            name: *const u16,
            r#type: u32,
            cx: i32,
            cy: i32,
            fu_load: u32,
        ) -> isize;
        fn SetClassLongPtrW(hwnd: isize, n_index: i32, dw_new_long: isize) -> isize;
        fn SendMessageW(hwnd: isize, msg: u32, w_param: usize, l_param: isize) -> isize;
    }

    pub fn apply(window: &Window, _opacity: f32) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = win32.hwnd.get();

        unsafe {
            // タイトルバーをダークモードにする (DWMWA_USE_IMMERSIVE_DARK_MODE = 20)
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                20,
                (&dark as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            );

            // .rsrc のアイコンリソース (ID=1) をロードし、クラスアイコンとウィンドウアイコンの両方へ設定する
            let h_inst = GetModuleHandleW(core::ptr::null());
            let big_icon = LoadImageW(h_inst, 1usize as *const u16, 1, 0, 0, 0x0040 | 0x8000);
            let small_icon = LoadImageW(h_inst, 1usize as *const u16, 1, 32, 32, 0x8000);
            if big_icon != 0 {
                SetClassLongPtrW(hwnd, -14, big_icon); // GCLP_HICON
                SendMessageW(hwnd, 0x0080, 1, big_icon); // WM_SETICON, ICON_BIG
            }
            if small_icon != 0 {
                SetClassLongPtrW(hwnd, -34, small_icon); // GCLP_HICONSM
                SendMessageW(hwnd, 0x0080, 0, small_icon); // WM_SETICON, ICON_SMALL
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
