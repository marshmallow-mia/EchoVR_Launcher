//! The egui front end: the launcher in one window, with its dialogs as cards on top.
//! Installing, patching, SteamVR setup and the Quest all run inside it.

mod assets;
mod controls;
mod design;
mod dialogs;
#[cfg(test)]
mod headless;
mod kit;
mod launcher;
mod markdown;
mod parts;
mod snapshot;
mod style;
mod theme;
pub mod tray;
mod video;
mod web;
mod widgets;

use launcher::Dashboard;

pub fn run() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(crate::version::VERSION_TITLE)
            .with_inner_size([launcher::W, launcher::H])
            .with_min_inner_size([launcher::W * 0.75, launcher::H * 0.75])
            .with_icon(std::sync::Arc::new(assets::icon())),
        centered: true,
        // Reopens at the last size, position and full screen.
        persist_window: true,
        persistence_path: Some(crate::core::paths::data_dir().join("window.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Echo VR Launcher",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::install_style(&cc.egui_ctx);
            let snapshots = snapshot::Snapshotter::from_env();
            let mut app = App::default();
            app.menu.demo = snapshots.as_ref().is_some_and(|s| s.demo);
            app.snapshots = snapshots;
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[derive(Default)]
struct App {
    assets: assets::Assets,
    menu: Dashboard,
    snapshots: Option<snapshot::Snapshotter>,
    /// Tests: scale images in the background, as the real app does, despite `demo`.
    #[cfg(test)]
    async_assets: bool,
}

impl App {
    fn drive_snapshots(&mut self, ctx: &egui::Context) {
        let Some(snap) = self.snapshots.as_mut() else {
            return;
        };
        if let Some(shot) = snap.current() {
            self.menu.page = shot.page;
            self.menu.snap_variant = shot.variant;
        }
        if snap.tick(ctx, egui::ViewportId::ROOT) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        fit_zoom(&ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
        }
        snapshot::capture(ui);
        self.drive_snapshots(&ctx);
        let sync = self.menu.demo || self.snapshots.is_some();
        #[cfg(test)]
        let sync = sync && !self.async_assets;
        self.assets.sync.set(sync);
        let blocked = self.menu.dialogs.is_open();
        let mut kit = kit::Kit::new(ui, &self.assets, blocked);
        self.menu.show(&mut kit);
        self.menu.dialogs.show(&mut kit);
        // The embedded site goes where the page placed it, after everything else.
        self.menu.sync_web(&ctx, frame);
    }

    /// Only the window is remembered (`persist_window`), not the UI's state.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn on_exit(&mut self) {
        crate::core::elevation::shutdown();
        cleanup_staged_patches();
    }
}

/// Scales the launcher with its window: the design's 1280×720 fills it on one side, and
/// the room left on the other goes to the layout (`Kit::ex`, `Kit::ey`).
fn fit_zoom(ctx: &egui::Context) {
    let ppp = ctx.pixels_per_point();
    let native = ctx
        .native_pixels_per_point()
        .unwrap_or(ppp / ctx.zoom_factor());
    let px = ctx.viewport_rect().size() * ppp;
    let zoom = (px.x / (launcher::W * native)).min(px.y / (launcher::H * native));
    if zoom.is_finite() && zoom > 0.1 && (zoom - ctx.zoom_factor()).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
}

/// Personal patch files never outlive the session (the Java shutdown hook).
fn cleanup_staged_patches() {
    use crate::core::launcher::patch;
    let dir = crate::core::paths::downloads_dir();
    let _ = std::fs::remove_file(dir.join(patch::DLL));
    let _ = std::fs::remove_file(dir.join(patch::LINK_DLL));
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Personal patched APKs go; the stock APK stays for the next install.
            if name.ends_with(".patched.apk") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}
