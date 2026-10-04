//! A website inside the window, over part of a page: echovrce.com on the EchoVRCE page, in
//! the OS's own web view (WebView2 on Windows, WebKit on macOS; not on Linux, where it would
//! make WebKitGTK a dependency of the portable build). A page says every frame where the
//! site goes (`place`); after the frame `sync` puts it there, or hides it when the page
//! isn't shown or a dialog is open (a native view always covers what egui draws).

/// Whether this build can show websites inside the window.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// The web view and where it should be.
#[derive(Default)]
pub struct WebPane {
    #[cfg(any(windows, target_os = "macos"))]
    view: Option<wry::WebView>,
    #[cfg(any(windows, target_os = "macos"))]
    context: Option<wry::WebContext>,
    /// Where it goes this frame (egui points); `None` hides it.
    wanted: Option<egui::Rect>,
    /// Where it went last (window logical pixels), so unchanged bounds aren't set again.
    #[cfg(any(windows, target_os = "macos"))]
    placed: Option<[f32; 4]>,
    /// What to open with the next view: the URL, and a script that runs before each page.
    pending: Option<(String, String)>,
    /// The web view couldn't be made: why.
    pub error: Option<String>,
}

impl WebPane {
    /// Shows the site over `r` (egui points) this frame.
    pub fn place(&mut self, r: egui::Rect) {
        self.wanted = Some(r);
    }

    /// Whether a site is open (or about to be).
    pub fn is_open(&self) -> bool {
        #[cfg(any(windows, target_os = "macos"))]
        if self.view.is_some() {
            return true;
        }
        self.pending.is_some()
    }

    /// Opens `url` in a new view, with `init` run before every page's own scripts.
    pub fn open(&mut self, url: &str, init: &str) {
        self.close();
        self.error = None;
        self.pending = Some((url.to_string(), init.to_string()));
    }

    /// Runs `js` in the open page.
    pub fn run(&self, js: &str) {
        #[cfg(any(windows, target_os = "macos"))]
        if let Some(v) = &self.view {
            let _ = v.evaluate_script(js);
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = js;
    }

    pub fn reload(&self) {
        #[cfg(any(windows, target_os = "macos"))]
        if let Some(v) = &self.view {
            let _ = v.reload();
        }
    }

    pub fn close(&mut self) {
        #[cfg(any(windows, target_os = "macos"))]
        {
            self.view = None;
            self.placed = None;
        }
        self.pending = None;
    }

    /// After the frame: makes the view when one is waiting and wanted, and puts it where
    /// the page placed it (hidden when it wasn't placed, or `blocked` by a dialog).
    pub fn sync(&mut self, ctx: &egui::Context, frame: &eframe::Frame, blocked: bool) {
        let wanted = self.wanted.take().filter(|_| !blocked);
        #[cfg(any(windows, target_os = "macos"))]
        {
            // Made once the page places it (the pending site waits until then).
            if let Some(r) = wanted.filter(|_| self.view.is_none()) {
                if let Some((url, init)) = self.pending.take() {
                    match self.make(frame, &url, &init, bounds(ctx, r)) {
                        Ok(v) => {
                            tracing::info!("web view: opened {url}");
                            self.view = Some(v);
                        }
                        Err(e) => {
                            tracing::warn!("web view: {e}");
                            self.error = Some(e.to_string());
                        }
                    }
                }
            }
            let Some(v) = &self.view else {
                return;
            };
            match wanted {
                Some(r) => {
                    let b = bounds(ctx, r);
                    let key = rect_key(&b);
                    if self.placed != Some(key) {
                        if let Err(e) = v.set_bounds(b).and_then(|()| v.set_visible(true)) {
                            tracing::warn!("web view: placing it failed: {e}");
                        }
                        tracing::debug!("web view: at {key:?}");
                        self.placed = Some(key);
                    }
                }
                None if self.placed.is_some() => {
                    let _ = v.set_visible(false);
                    self.placed = None;
                }
                None => {}
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = (ctx, frame, wanted);
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn make(
        &mut self,
        frame: &eframe::Frame,
        url: &str,
        init: &str,
        bounds: wry::Rect,
    ) -> wry::Result<wry::WebView> {
        let context = self.context.get_or_insert_with(|| {
            wry::WebContext::new(Some(crate::core::paths::data_dir().join("webview")))
        });
        let home = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        wry::WebViewBuilder::new_with_web_context(context)
            .with_url(url)
            .with_initialization_script(init)
            .with_bounds(bounds)
            // The site's own pages stay inside; links elsewhere open in the browser.
            .with_navigation_handler(move |to| {
                let host = url::Url::parse(&to)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string));
                let inside = host.is_none()
                    || host == home
                    || host
                        .as_deref()
                        .is_some_and(|h| h == "discord.com" || h.ends_with(".discord.com"));
                if !inside {
                    crate::core::platform::open_url(&to);
                }
                inside
            })
            .with_on_page_load_handler(|event, url| {
                let what = match event {
                    wry::PageLoadEvent::Started => "loading",
                    wry::PageLoadEvent::Finished => "loaded",
                };
                tracing::info!(
                    "web view: {what} {}",
                    url.split(['?', '#']).next().unwrap_or("")
                );
            })
            // Right-click → Inspect Element in debug builds.
            .with_devtools(cfg!(debug_assertions))
            .with_new_window_req_handler(|to, _| {
                crate::core::platform::open_url(&to);
                wry::NewWindowResponse::Deny
            })
            .build_as_child(frame)
    }
}

/// `r` (egui points) in the window's logical pixels.
#[cfg(any(windows, target_os = "macos"))]
fn bounds(ctx: &egui::Context, r: egui::Rect) -> wry::Rect {
    let z = ctx.zoom_factor();
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(r.min.x * z, r.min.y * z).into(),
        size: wry::dpi::LogicalSize::new(r.width() * z, r.height() * z).into(),
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn rect_key(r: &wry::Rect) -> [f32; 4] {
    let p = r.position.to_logical::<f32>(1.0);
    let s = r.size.to_logical::<f32>(1.0);
    [p.x, p.y, s.width, s.height]
}
