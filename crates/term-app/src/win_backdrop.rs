#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::core::{IUnknown, Interface};
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

    pub struct DirectCompState {
        device: IDCompositionDevice,
        _target: IDCompositionTarget,
        visual: IDCompositionVisual,
    }

    impl DirectCompState {
        pub fn new(window: &Window) -> Option<Self> {
            let Ok(handle) = window.window_handle() else {
                return None;
            };
            let RawWindowHandle::Win32(win32) = handle.as_raw() else {
                return None;
            };
            let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
            unsafe {
                let device: IDCompositionDevice =
                    DCompositionCreateDevice2::<Option<&IUnknown>, IDCompositionDevice>(None).ok()?;
                let target = device.CreateTargetForHwnd(hwnd, true).ok()?;
                let visual = device.CreateVisual().ok()?;
                target.SetRoot(&visual).ok()?;
                device.Commit().ok()?;
                Some(Self {
                    device,
                    _target: target,
                    visual,
                })
            }
        }

        pub fn visual_ptr(&self) -> *mut std::ffi::c_void {
            self.visual.as_raw()
        }

        pub fn commit(&self) {
            unsafe {
                let _ = self.device.Commit();
            }
        }
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
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use winit::window::Window;

    pub struct DirectCompState;

    impl DirectCompState {
        pub fn new(_window: &Window) -> Option<Self> {
            None
        }

        #[allow(dead_code)]
        pub fn visual_ptr(&self) -> *mut std::ffi::c_void {
            std::ptr::null_mut()
        }

        pub fn commit(&self) {}
    }

    pub fn apply(_window: &Window, _opacity: f32) {}
}

pub use imp::{DirectCompState, apply};
