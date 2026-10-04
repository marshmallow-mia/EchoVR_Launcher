//! Embedded images, uploaded to the GPU at the exact size they are drawn (pre-scaled
//! with a Lanczos filter, like Swing's `SCALE_SMOOTH`). Scaling runs on background
//! threads, and the launcher asks for every page's images right after it opens (see
//! `Dashboard::prewarm`), so a page opened for the first time doesn't wait for them.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use egui::{ColorImage, Context, TextureFilter, TextureHandle, TextureOptions};

const IMAGES: &[(&str, &[u8])] = &[
    ("icon.png", include_bytes!("../../assets/img/icon.png")),
    (
        "main_background.jpg",
        include_bytes!("../../assets/img/main_background.jpg"),
    ),
    (
        "left_sidebar.jpg",
        include_bytes!("../../assets/img/left_sidebar.jpg"),
    ),
    (
        "sidebar_selected.png",
        include_bytes!("../../assets/img/sidebar_selected.png"),
    ),
    (
        "sidebar_hover.png",
        include_bytes!("../../assets/img/sidebar_hover.png"),
    ),
    (
        "logo_echovr.png",
        include_bytes!("../../assets/img/logo_echovr.png"),
    ),
    (
        "play_button_blank.png",
        include_bytes!("../../assets/img/play_button_blank.png"),
    ),
    (
        "update_button.png",
        include_bytes!("../../assets/img/update_button.png"),
    ),
    (
        "update_button_alert.png",
        include_bytes!("../../assets/img/update_button_alert.png"),
    ),
    (
        "hardware_pc.png",
        include_bytes!("../../assets/img/hardware_pc.png"),
    ),
    (
        "hardware_quest.png",
        include_bytes!("../../assets/img/hardware_quest.png"),
    ),
    (
        "news_header.png",
        include_bytes!("../../assets/img/news_header.png"),
    ),
    (
        "header_strip.png",
        include_bytes!("../../assets/img/header_strip.png"),
    ),
    (
        "news_fallback_bg.jpg",
        include_bytes!("../../assets/img/news_fallback_bg.jpg"),
    ),
    (
        "news_fallback_text.png",
        include_bytes!("../../assets/img/news_fallback_text.png"),
    ),
    (
        "card_bg.png",
        include_bytes!("../../assets/img/card_bg.png"),
    ),
    (
        "panel_bg.png",
        include_bytes!("../../assets/img/panel_bg.png"),
    ),
    (
        "icon_play.png",
        include_bytes!("../../assets/img/icon_play.png"),
    ),
    (
        "icon_spark.png",
        include_bytes!("../../assets/img/icon_spark.png"),
    ),
    (
        "icon_echovrce.png",
        include_bytes!("../../assets/img/icon_echovrce.png"),
    ),
    (
        "play_button_running.png",
        include_bytes!("../../assets/img/play_button_running.png"),
    ),
    (
        "play_button_blank_hover.png",
        include_bytes!("../../assets/img/play_button_blank_hover.png"),
    ),
    (
        "play_button_blank_grey.png",
        include_bytes!("../../assets/img/play_button_blank_grey.png"),
    ),
    (
        "update_button_hover.png",
        include_bytes!("../../assets/img/update_button_hover.png"),
    ),
    (
        "update_button_grey.png",
        include_bytes!("../../assets/img/update_button_grey.png"),
    ),
    (
        "update_button_grey_hover.png",
        include_bytes!("../../assets/img/update_button_grey_hover.png"),
    ),
    (
        "update_button_alert_hover.png",
        include_bytes!("../../assets/img/update_button_alert_hover.png"),
    ),
    (
        "update_button_alert_grey.png",
        include_bytes!("../../assets/img/update_button_alert_grey.png"),
    ),
    (
        "update_button_alert_grey_hover.png",
        include_bytes!("../../assets/img/update_button_alert_grey_hover.png"),
    ),
    (
        "hardware_pc_hover.png",
        include_bytes!("../../assets/img/hardware_pc_hover.png"),
    ),
    (
        "hardware_quest_hover.png",
        include_bytes!("../../assets/img/hardware_quest_hover.png"),
    ),
    (
        "sidebar_selected_hover.png",
        include_bytes!("../../assets/img/sidebar_selected_hover.png"),
    ),
    (
        "sidebar_pressed.png",
        include_bytes!("../../assets/img/sidebar_pressed.png"),
    ),
    (
        "icon_community.png",
        include_bytes!("../../assets/img/icon_community.png"),
    ),
    (
        "play_button_blank_grey_hover.png",
        include_bytes!("../../assets/img/play_button_blank_grey_hover.png"),
    ),
    (
        "blue_button.png",
        include_bytes!("../../assets/img/blue_button.png"),
    ),
    (
        "blue_button_hover.png",
        include_bytes!("../../assets/img/blue_button_hover.png"),
    ),
    (
        "blue_button_grey.png",
        include_bytes!("../../assets/img/blue_button_grey.png"),
    ),
    (
        "blue_button_grey_hover.png",
        include_bytes!("../../assets/img/blue_button_grey_hover.png"),
    ),
    (
        "header_strip_small.png",
        include_bytes!("../../assets/img/header_strip_small.png"),
    ),
];

/// An embedded image's name (as `'static`) and bytes.
fn entry(name: &str) -> (&'static str, &'static [u8]) {
    *IMAGES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("unknown image {name}"))
}

fn decode(name: &str) -> image::RgbaImage {
    image::load_from_memory(entry(name).1)
        .expect("embedded image")
        .to_rgba8()
}

fn to_color_image(img: &image::RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Native pixel size of an embedded image.
pub fn native_size(name: &str) -> (u32, u32) {
    thread_local! {
        static SIZES: RefCell<HashMap<String, (u32, u32)>> = RefCell::new(HashMap::new());
    }
    SIZES.with(|s| {
        *s.borrow_mut().entry(name.to_string()).or_insert_with(|| {
            image::ImageReader::new(std::io::Cursor::new(entry(name).1))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .unwrap_or((0, 0))
        })
    })
}

/// Window icon data.
pub fn icon() -> egui::IconData {
    let img = decode("icon.png");
    egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    }
}

/// A texture's cache key: the image and its size in physical pixels.
type Key = (&'static str, u32, u32);

#[derive(Default)]
pub struct Assets {
    shared: Arc<Shared>,
    /// Scale on the UI thread, as before: snapshots need every frame to be final.
    pub sync: Cell<bool>,
    /// A page nobody sees is being drawn to warm its images: queue them, draw nothing.
    pub ghost: Cell<bool>,
    started: Cell<bool>,
    /// Drawn in place of an image while `ghost` is set.
    blank: RefCell<Option<TextureHandle>>,
    /// The window's scale and size (physical pixels) the textures are made for.
    window: Cell<(f32, u32, u32)>,
    /// Being resized since then: images are drawn from their full-size textures.
    resizing: Cell<Option<Instant>>,
}

/// How long the window has to keep its size before images are scaled for it.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Default)]
struct Shared {
    /// Decoded images, so a new size doesn't decode the PNG again.
    decoded: Mutex<HashMap<&'static str, Arc<image::RgbaImage>>>,
    textures: Mutex<HashMap<Key, TextureHandle>>,
    /// Full-size textures (mipmapped): drawn until the scaled one is ready.
    native: Mutex<HashMap<&'static str, TextureHandle>>,
    queue: Mutex<Queue>,
    wake: Condvar,
}

/// Textures waiting for a scaling thread: the ones on screen first.
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Key>,
    queued: HashSet<Key>,
    /// Drawn from the full-size texture meanwhile: repaint once scaled.
    urgent: HashSet<Key>,
    closed: bool,
}

impl Shared {
    fn decoded(&self, name: &'static str) -> Arc<image::RgbaImage> {
        if let Some(img) = lock(&self.decoded).get(name) {
            return img.clone();
        }
        let img = Arc::new(decode(name));
        lock(&self.decoded).entry(name).or_insert(img).clone()
    }

    /// Scales `key`'s image to its size (Lanczos) and uploads it.
    fn scale(&self, ctx: &Context, key: Key) -> TextureHandle {
        let (name, tw, th) = key;
        let started = Instant::now();
        let src = self.decoded(name);
        let img = if (src.width(), src.height()) != (tw, th) {
            to_color_image(&image::imageops::resize(
                &*src,
                tw,
                th,
                image::imageops::FilterType::Lanczos3,
            ))
        } else {
            to_color_image(&src)
        };
        let t = ctx.load_texture(format!("{name}@{tw}x{th}"), img, TextureOptions::LINEAR);
        tracing::debug!(
            "{name} scaled to {tw}x{th} in {:.0} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        lock(&self.textures).entry(key).or_insert(t).clone()
    }

    /// The image at its own size, mipmapped so it also looks right drawn smaller.
    fn native(&self, ctx: &Context, name: &'static str) -> TextureHandle {
        if let Some(t) = lock(&self.native).get(name) {
            return t.clone();
        }
        let img = to_color_image(&self.decoded(name));
        let options = TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));
        let t = ctx.load_texture(format!("{name}@native"), img, options);
        lock(&self.native).entry(name).or_insert(t).clone()
    }

    /// Queues `key` for a scaling thread; `urgent` ones (on screen) go first.
    fn queue(&self, key: Key, urgent: bool) {
        let mut q = lock(&self.queue);
        if urgent {
            q.urgent.insert(key);
        }
        if q.queued.insert(key) {
            if urgent {
                q.jobs.push_front(key);
            } else {
                q.jobs.push_back(key);
            }
            self.wake.notify_one();
        } else if urgent {
            if let Some(i) = q.jobs.iter().position(|k| *k == key) {
                q.jobs.remove(i);
                q.jobs.push_front(key);
            }
        }
    }

    /// A scaling thread: takes the next queued texture until the assets are dropped.
    fn work(&self, ctx: &Context) {
        loop {
            let key = {
                let mut q = lock(&self.queue);
                loop {
                    if q.closed {
                        return;
                    }
                    if let Some(k) = q.jobs.pop_front() {
                        break k;
                    }
                    q = self.wake.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            if !lock(&self.textures).contains_key(&key) {
                let _ = self.scale(ctx, key);
            }
            let mut q = lock(&self.queue);
            q.queued.remove(&key);
            if q.urgent.remove(&key) {
                ctx.request_repaint();
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Drop for Assets {
    fn drop(&mut self) {
        lock(&self.shared.queue).closed = true;
        self.shared.wake.notify_all();
    }
}

impl Assets {
    /// The texture's key: `name` at the physical size of a `w`x`h` (logical) rect, or at
    /// its own size when that is smaller (upscales are left to linear filtering).
    fn key(ctx: &Context, name: &str, w: u32, h: u32) -> Key {
        let name = entry(name).0;
        let ppp = ctx.pixels_per_point();
        let (nw, nh) = native_size(name);
        let (tw, th) = (
            (w as f32 * ppp).round() as u32,
            (h as f32 * ppp).round() as u32,
        );
        if tw > 0 && th > 0 && (tw < nw || th < nh) {
            (name, tw, th)
        } else {
            (name, nw, nh)
        }
    }

    /// The scaling threads, on first need.
    fn start(&self, ctx: &Context) {
        if self.started.replace(true) {
            return;
        }
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1))
            .clamp(1, 3);
        for i in 0..threads {
            let (shared, ctx) = (self.shared.clone(), ctx.clone());
            let _ = std::thread::Builder::new()
                .name(format!("assets-{i}"))
                .spawn(move || shared.work(&ctx));
        }
    }

    /// Once per frame. While the window is being resized, images are drawn from their
    /// full-size textures; once it keeps its size, the old sizes are dropped and true is
    /// returned, so that the pages are warmed again for the new one.
    pub fn track_window(&self, ctx: &Context) -> bool {
        if self.sync.get() {
            return false;
        }
        let px = ctx.viewport_rect().size() * ctx.pixels_per_point();
        let now = (
            ctx.pixels_per_point(),
            px.x.round() as u32,
            px.y.round() as u32,
        );
        let was = self.window.replace(now);
        if was != now && was.1 > 0 {
            self.resizing.set(Some(Instant::now()));
        }
        match self.resizing.get() {
            Some(at) if at.elapsed() < SETTLE => {
                ctx.request_repaint_after(SETTLE - at.elapsed());
                false
            }
            Some(_) => {
                self.resizing.set(None);
                lock(&self.shared.textures).clear();
                let mut q = lock(&self.shared.queue);
                q.jobs.clear();
                q.queued.clear();
                q.urgent.clear();
                true
            }
            None => false,
        }
    }

    /// A texture for drawing `name` into a `w`x`h` (logical) rect. Downscales are done
    /// once on the CPU at the physical target size (a GPU minification of a 802px image
    /// into 300px aliases badly), on a background thread: until that is done, the image
    /// is drawn from its mipmapped full-size texture.
    pub fn tex(&self, ctx: &Context, name: &str, w: u32, h: u32) -> TextureHandle {
        let key = Self::key(ctx, name, w, h);
        if let Some(t) = lock(&self.shared.textures).get(&key) {
            return t.clone();
        }
        if self.ghost.get() {
            if self.resizing.get().is_none() {
                self.start(ctx);
                self.shared.queue(key, false);
            }
            return self.blank(ctx);
        }
        // Snapshots, and the very first frame: the launcher opens with its final look.
        if self.sync.get() || ctx.cumulative_frame_nr() == 0 {
            return self.shared.scale(ctx, key);
        }
        let native = self.shared.native(ctx, key.0);
        if (key.1, key.2) == native_size(key.0) {
            lock(&self.shared.textures).insert(key, native.clone());
            return native;
        }
        if self.resizing.get().is_some() {
            return native;
        }
        self.start(ctx);
        self.shared.queue(key, true);
        native
    }

    /// Scales `name` for a `w`x`h` rect in the background (a hover image, before the first
    /// hover).
    pub fn prefetch(&self, ctx: &Context, name: &str, w: u32, h: u32) {
        if self.sync.get() || self.resizing.get().is_some() {
            return;
        }
        let key = Self::key(ctx, name, w, h);
        if lock(&self.shared.textures).contains_key(&key) {
            return;
        }
        self.start(ctx);
        self.shared.queue(key, false);
    }

    /// Nothing is queued or being scaled.
    #[cfg(test)]
    pub fn idle(&self) -> bool {
        lock(&self.shared.queue).queued.is_empty()
    }

    fn blank(&self, ctx: &Context) -> TextureHandle {
        self.blank
            .borrow_mut()
            .get_or_insert_with(|| {
                ctx.load_texture(
                    "blank",
                    ColorImage::filled([1, 1], egui::Color32::TRANSPARENT),
                    TextureOptions::LINEAR,
                )
            })
            .clone()
    }
}
