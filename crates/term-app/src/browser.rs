#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use std::sync::Arc;

    use super::BrowserBounds;
    use winit::dpi::{LogicalSize, PhysicalPosition};
    use winit::event_loop::ActiveEventLoop;
    use winit::window::{Window, WindowAttributes, WindowId};
    use wry::{WebView, WebViewBuilder};

    pub struct EmbeddedBrowser {
        // WebView を先に drop してからホスト Window を破棄する
        webview: WebView,
        window: Arc<Window>,
        pub url: String,
    }

    impl EmbeddedBrowser {
        pub fn open(
            event_loop: &ActiveEventLoop,
            parent: &Window,
            url: &str,
            bounds: BrowserBounds,
        ) -> Result<Self, String> {
            let scale = parent.scale_factor();
            let base = parent
                .inner_position()
                .unwrap_or(PhysicalPosition::new(0, 0));
            let pos = PhysicalPosition::new(
                base.x + (bounds.x * scale).round() as i32,
                base.y + (bounds.y * scale).round() as i32,
            );

            let attrs = WindowAttributes::default()
                .with_title("conductty preview")
                .with_decorations(false)
                .with_inner_size(LogicalSize::new(
                    bounds.width.max(100.0),
                    bounds.height.max(100.0),
                ))
                .with_position(pos);

            let window = Arc::new(
                event_loop
                    .create_window(attrs)
                    .map_err(|e| format!("Preview ウィンドウ作成失敗: {e}"))?,
            );

            let safe_url = term_drawer::validate_http_url(url)?.to_string();
            let webview = WebViewBuilder::new()
                .with_url(&safe_url)
                .with_navigation_handler(|nav_url| term_drawer::validate_http_url(&nav_url).is_ok())
                .build(&window)
                .map_err(|e| format!("WebView 起動失敗: {e}"))?;

            Ok(Self {
                window,
                webview,
                url: safe_url,
            })
        }

        pub fn window_id(&self) -> WindowId {
            self.window.id()
        }

        pub fn sync_to_parent(&self, parent: &Window, bounds: BrowserBounds) {
            let scale = parent.scale_factor();
            if let Ok(base) = parent.inner_position() {
                let pos = PhysicalPosition::new(
                    base.x + (bounds.x * scale).round() as i32,
                    base.y + (bounds.y * scale).round() as i32,
                );
                self.window.set_outer_position(pos);
            }
            let _ = self.window.request_inner_size(LogicalSize::new(
                bounds.width.max(100.0),
                bounds.height.max(100.0),
            ));
        }

        pub fn load_url(&mut self, url: &str) {
            if let Ok(safe_url) = term_drawer::validate_http_url(url) {
                self.url = safe_url.to_string();
                let _ = self.webview.load_url(safe_url);
            }
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use super::BrowserBounds;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::{Window, WindowId};

    pub struct EmbeddedBrowser {
        pub url: String,
    }

    impl EmbeddedBrowser {
        pub fn open(
            _event_loop: &ActiveEventLoop,
            _parent: &Window,
            _url: &str,
            _bounds: BrowserBounds,
        ) -> Result<Self, String> {
            Err("Embedded browser is not supported on this OS".to_string())
        }

        pub fn window_id(&self) -> WindowId {
            unreachable!()
        }

        pub fn sync_to_parent(&self, _parent: &Window, _bounds: BrowserBounds) {}

        pub fn load_url(&mut self, url: &str) {
            self.url = url.to_string();
        }
    }
}

pub use imp::EmbeddedBrowser;
