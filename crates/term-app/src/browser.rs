#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// HTML プレビュー表示中の `EmbeddedBrowser::url`（http(s) ではないので External ボタンは出さない）
pub const HTML_URL: &str = "about:blank";

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use std::sync::Arc;

    use super::{BrowserBounds, HTML_URL};
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
            let safe_url = term_drawer::validate_http_url(url)?.to_string();
            let builder = WebViewBuilder::new().with_url(&safe_url);
            Self::create(event_loop, parent, bounds, builder, safe_url)
        }

        pub fn open_html(
            event_loop: &ActiveEventLoop,
            parent: &Window,
            html: &str,
            bounds: BrowserBounds,
        ) -> Result<Self, String> {
            let builder = WebViewBuilder::new().with_html(html);
            Self::create(event_loop, parent, bounds, builder, HTML_URL.to_string())
        }

        fn create(
            event_loop: &ActiveEventLoop,
            parent: &Window,
            bounds: BrowserBounds,
            builder: WebViewBuilder<'_>,
            url: String,
        ) -> Result<Self, String> {
            let scale = parent.scale_factor();
            let base = parent
                .inner_position()
                .unwrap_or(PhysicalPosition::new(0, 0));
            let pos = PhysicalPosition::new(
                base.x + (bounds.x * scale).round() as i32,
                base.y + (bounds.y * scale).round() as i32,
            );

            let width = bounds.width.max(100.0);
            let height = bounds.height.max(100.0);
            let attrs = WindowAttributes::default()
                .with_title("conductty preview")
                .with_decorations(false)
                .with_inner_size(LogicalSize::new(width, height))
                .with_position(pos);

            let window = Arc::new(
                event_loop
                    .create_window(attrs)
                    .map_err(|e| format!("Preview ウィンドウ作成失敗: {e}"))?,
            );

            let webview = builder
                .with_focused(false)
                .with_bounds(child_rect(bounds))
                .with_navigation_handler(|nav_url| allow_navigation(&nav_url))
                .build_as_child(&window)
                .map_err(|e| format!("WebView 起動失敗: {e}"))?;

            Ok(Self {
                window,
                webview,
                url,
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
            let _ = self.webview.set_bounds(child_rect(bounds));
        }

        pub fn load_url(&mut self, url: &str) {
            if let Ok(safe_url) = term_drawer::validate_http_url(url) {
                self.url = safe_url.to_string();
                let _ = self.webview.load_url(safe_url);
            }
        }

        pub fn load_html(&mut self, html: &str) {
            self.url = HTML_URL.to_string();
            let _ = self.webview.load_html(html);
        }
    }

    /// HTML 文字列のロードは macOS で about:blank、Windows(WebView2) で data:text/html として通知される
    fn allow_navigation(url: &str) -> bool {
        url == HTML_URL
            || url.starts_with("data:text/html")
            || term_drawer::validate_http_url(url).is_ok()
    }

    fn child_rect(bounds: BrowserBounds) -> wry::Rect {
        wry::Rect {
            position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
            size: wry::dpi::LogicalSize::new(bounds.width.max(100.0), bounds.height.max(100.0))
                .into(),
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

        pub fn open_html(
            _event_loop: &ActiveEventLoop,
            _parent: &Window,
            _html: &str,
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

        pub fn load_html(&mut self, _html: &str) {
            self.url = super::HTML_URL.to_string();
        }
    }
}

pub use imp::EmbeddedBrowser;
