//! A website inside the window, over part of a page: echovrce.com on the EchoVRCE page, in
//! the OS's own web view (WebView2 on Windows, WebKit on macOS; not on Linux, where it would
//! make WebKitGTK a dependency of the portable build). A page says every frame where the
//! site goes (`place`); after the frame `sync` puts it there, or hides it when the page
//! isn't shown or a dialog is open (a native view always covers what egui draws).
//!
//! The site never signs in with Discord itself: its login asks the launcher instead
//! ([`Event::SignIn`]), which hands it a session. Only a view opened to approve a sign-in
//! code ([`WebPane::open_sign_in`]) lets Discord's login in.

/// Whether this build can show websites inside the window.
pub fn supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

/// The web view and where it should be.
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
    /// What to open with the next view.
    pending: Option<Pending>,
    /// The open view approves a sign-in code (Discord's login may load in it).
    sign_in: bool,
    /// The web view couldn't be made: why.
    pub error: Option<String>,
    /// The page's zoom (1 = its own size).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    zoom: f64,
    /// What the page asked of the launcher, not yet taken.
    events: std::sync::Arc<std::sync::Mutex<Vec<Event>>>,
}

/// A view to make: the URL, a script that runs before each page, and whether it
/// approves a sign-in code.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
struct Pending {
    url: String,
    init: String,
    sign_in: bool,
}

/// What the page asked of the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A zoom step, with the browser's keys or Ctrl+wheel.
    Zoom(Zoom),
    /// The site wants to sign in (its Discord login was started): it needs a session.
    SignIn,
}

/// A zoom step asked for with the browser's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// Runs before every page: the browser's zoom keys (Ctrl or Cmd with +, - and 0) and
/// Ctrl+wheel, told to the launcher, which zooms the view (the keys never reach egui
/// while the site has the keyboard).
const ZOOM_KEYS: &str = r#"(() => {
  const send = (m) => window.ipc && window.ipc.postMessage("zoom:" + m);
  addEventListener("keydown", (e) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const m = { "+": "in", "=": "in", "-": "out", "_": "out", "0": "reset" }[e.key];
    if (!m) return;
    e.preventDefault();
    send(m);
  }, true);
  let wheel = 0;
  addEventListener("wheel", (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    wheel += e.deltaY;
    if (Math.abs(wheel) >= 50) {
      send(wheel < 0 ? "in" : "out");
      wheel = 0;
    }
  }, { passive: false, capture: true });
})();"#;

impl Default for WebPane {
    fn default() -> Self {
        Self {
            #[cfg(any(windows, target_os = "macos"))]
            view: None,
            #[cfg(any(windows, target_os = "macos"))]
            context: None,
            wanted: None,
            #[cfg(any(windows, target_os = "macos"))]
            placed: None,
            pending: None,
            sign_in: false,
            error: None,
            zoom: 1.0,
            events: Default::default(),
        }
    }
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

    /// Whether the open view approves a sign-in code ([`WebPane::open_sign_in`]).
    pub fn is_sign_in(&self) -> bool {
        self.is_open() && self.sign_in
    }

    /// Opens `url` in a new view, with `init` run before every page's own scripts.
    pub fn open(&mut self, url: &str, init: &str) {
        self.open_view(url, init, false);
    }

    /// Opens `url`, the page approving a sign-in code, in a new view where signing in
    /// to echovrce.com with Discord works.
    pub fn open_sign_in(&mut self, url: &str) {
        self.open_view(url, "", true);
    }

    fn open_view(&mut self, url: &str, init: &str, sign_in: bool) {
        self.close();
        self.error = None;
        self.sign_in = sign_in;
        self.pending = Some(Pending {
            url: url.to_string(),
            init: format!("{init}\n{ZOOM_KEYS}"),
            sign_in,
        });
    }

    /// Zooms the page to `z` (1 = its own size), now and in views opened later.
    pub fn set_zoom(&mut self, z: f64) {
        if (z - self.zoom).abs() < 1e-3 {
            return;
        }
        self.zoom = z;
        #[cfg(any(windows, target_os = "macos"))]
        if let Some(v) = &self.view {
            if let Err(e) = v.zoom(z) {
                tracing::warn!("web view: zooming failed: {e}");
            }
        }
    }

    /// What the page asked for since the last call.
    pub fn take_events(&self) -> Vec<Event> {
        self.events
            .lock()
            .map(|mut z| std::mem::take(&mut *z))
            .unwrap_or_default()
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
            // The keyboard back to the launcher first: gone with the view, it would
            // reach nothing.
            if let Some(v) = &self.view {
                let _ = v.focus_parent();
            }
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
                if let Some(Pending { url, init, sign_in }) = self.pending.take() {
                    match self.make(ctx, frame, &url, &init, sign_in, bounds(ctx, r)) {
                        Ok(v) => {
                            tracing::info!("web view: opened {url}");
                            if (self.zoom - 1.0).abs() > 1e-3 {
                                let _ = v.zoom(self.zoom);
                            }
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
                    // A hidden view keeps the keyboard (WebView2 does): the launcher's
                    // own fields then take clicks but no typing.
                    let _ = v.focus_parent();
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
        ctx: &egui::Context,
        frame: &eframe::Frame,
        url: &str,
        init: &str,
        sign_in: bool,
        bounds: wry::Rect,
    ) -> wry::Result<wry::WebView> {
        let context = self.context.get_or_insert_with(|| {
            wry::WebContext::new(Some(crate::core::paths::data_dir().join("webview")))
        });
        let home = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        let (events, ctx) = (self.events.clone(), ctx.clone());
        let send = move |e: Event| {
            if let Ok(mut q) = events.lock() {
                q.push(e);
            }
            ctx.request_repaint();
        };
        let (send_nav, send_window) = (send.clone(), send.clone());
        let home_nav = home.clone();
        wry::WebViewBuilder::new_with_web_context(context)
            .with_url(url)
            .with_initialization_script(init)
            .with_bounds(bounds)
            // The site's own pages stay inside; links elsewhere open in the browser.
            // Discord's login stays inside only while approving a code; the site's own
            // asks the launcher for a session instead.
            .with_navigation_handler(move |to| match place(&to, home_nav.as_deref(), sign_in) {
                Goes::Inside => true,
                Goes::SignIn => {
                    send_nav(Event::SignIn);
                    false
                }
                Goes::Browser => {
                    crate::core::platform::open_url(&to);
                    false
                }
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
            .with_ipc_handler(move |req| {
                let z = match req.body().as_str() {
                    "zoom:in" => Zoom::In,
                    "zoom:out" => Zoom::Out,
                    "zoom:reset" => Zoom::Reset,
                    _ => return,
                };
                send(Event::Zoom(z));
            })
            // Right-click → Inspect Element in debug builds.
            .with_devtools(cfg!(debug_assertions))
            // No windows of its own: the site then opens Discord's login in the view
            // itself (approving a code), and other links go to the browser.
            .with_new_window_req_handler(move |to, _| {
                match place(&to, home.as_deref(), sign_in) {
                    Goes::Inside if sign_in => {}
                    Goes::SignIn => send_window(Event::SignIn),
                    _ => crate::core::platform::open_url(&to),
                }
                wry::NewWindowResponse::Deny
            })
            .build_as_child(frame)
    }
}

/// Where a page goes from the view.
#[cfg(any(windows, target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum Goes {
    Inside,
    /// Discord's login, from the site: the launcher hands the site a session instead.
    SignIn,
    Browser,
}

#[cfg(any(windows, target_os = "macos", test))]
/// Where `to` goes from a view showing `home`'s site (`sign_in`: approving a code).
fn place(to: &str, home: Option<&str>, sign_in: bool) -> Goes {
    let Ok(u) = url::Url::parse(to) else {
        return Goes::Inside;
    };
    let Some(host) = u.host_str() else {
        return Goes::Inside;
    };
    if Some(host) == home {
        return Goes::Inside;
    }
    let discord = host == "discord.com" || host.ends_with(".discord.com");
    let login = discord && u.path().contains("oauth2");
    match (login, sign_in) {
        (true, false) => Goes::SignIn,
        (_, true) if discord => Goes::Inside,
        _ => Goes::Browser,
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: Option<&str> = Some("echovrce.com");
    const LOGIN: &str = "https://discord.com/api/oauth2/authorize?client_id=1&redirect_uri=x";

    #[test]
    fn the_sites_discord_login_asks_the_launcher() {
        assert_eq!(place(LOGIN, HOME, false), Goes::SignIn);
        assert_eq!(
            place("https://discord.com/oauth2/authorize?a=1", HOME, false),
            Goes::SignIn
        );
    }

    #[test]
    fn approving_a_code_keeps_discord_inside() {
        assert_eq!(place(LOGIN, HOME, true), Goes::Inside);
        assert_eq!(place("https://discord.com/login", HOME, true), Goes::Inside);
    }

    #[test]
    fn other_links_open_in_the_browser() {
        assert_eq!(
            place("https://echovrce.com/home", HOME, false),
            Goes::Inside
        );
        assert_eq!(place("https://discord.gg/abc", HOME, false), Goes::Browser);
        assert_eq!(
            place("https://discord.com/invite/abc", HOME, false),
            Goes::Browser
        );
        assert_eq!(place("https://github.com/x", HOME, true), Goes::Browser);
    }
}
