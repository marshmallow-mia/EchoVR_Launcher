//! Headless UI snapshots: the same shots as snapshot mode (`snapshot.rs`), rendered
//! offscreen with wgpu instead of screenshotting a window, so they also work with the
//! screen locked or on a machine without a desktop session.
//!
//! `ECHOVR_SNAPSHOTS=<dir> cargo test headless -- --ignored` (`ECHOVR_SNAPSHOTS_ONLY`
//! filters as in snapshot mode; `ECHOVR_SNAPSHOTS_SCALE=2` renders at 2x;
//! `ECHOVR_SNAPSHOTS_SIZE=1680x720` in a window of that size (logical pixels);
//! `ECHOVR_SNAPSHOTS_FEED=live` shows the real RIGHT NOW numbers and news instead of made-up ones).

use super::{launcher, snapshot, theme, App};

#[test]
#[ignore = "needs a GPU; run with ECHOVR_SNAPSHOTS=<dir> cargo test headless -- --ignored"]
fn headless_snapshots() {
    let Some(dir) = std::env::var_os("ECHOVR_SNAPSHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let scale = std::env::var("ECHOVR_SNAPSHOTS_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let live = std::env::var("ECHOVR_SNAPSHOTS_FEED").is_ok_and(|v| v == "live");
    let size = std::env::var("ECHOVR_SNAPSHOTS_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(egui::vec2(w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or(egui::vec2(launcher::W, launcher::H));
    for shot in snapshot::shots() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(scale)
            .wgpu()
            .build_eframe(|cc| {
                theme::install_fonts(&cc.egui_ctx);
                theme::install_style(&cc.egui_ctx);
                let mut app = App::default();
                app.menu.demo = true;
                app.menu.feed_live = live;
                app.menu.page = shot.page;
                app.menu.snap_variant = shot.variant;
                app
            });
        harness.run_steps(4);
        let pointer = shot
            .hover
            .map(|(x, y)| egui::pos2(x * 2.0 / 3.0, y * 2.0 / 3.0));
        if let Some(pos) = pointer {
            harness.hover_at(pos);
        }
        harness.run_steps(8);
        // The live feed downloads in the background: wait for it (at most 20 s).
        let started = std::time::Instant::now();
        while live
            && !harness.state().menu.feed_settled()
            && started.elapsed() < std::time::Duration::from_secs(20)
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
            harness.step();
        }
        harness.run_steps(4);
        // Held down: render the frame right after the press (held for long, egui turns it
        // into a long press, which lets go of the button).
        if let (Some(pos), true) = (pointer, shot.press) {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            harness.step();
        }
        let img = harness.render().expect("render");
        img.save(dir.join(format!("{}.png", shot.name))).unwrap();
    }
}

/// Opening each page for the first time, as in the real app (images scaled in the
/// background, every page warmed up after the first frame, and again after a resize):
/// how long its first frame takes. `cargo test tab_switch -- --ignored --nocapture`
#[test]
#[ignore = "needs a GPU; run with cargo test tab_switch -- --ignored --nocapture"]
fn tab_switch_timing() {
    use std::time::{Duration, Instant};
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(launcher::W, launcher::H))
        .wgpu()
        .build_eframe(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::install_style(&cc.egui_ctx);
            let mut app = App::default();
            app.menu.demo = true;
            app.async_assets = true;
            app
        });
    // Then again after the window is resized: it settles, and every page is warmed for
    // the new size.
    for size in [None, Some(egui::vec2(1680.0, 720.0))] {
        if let Some(size) = size {
            harness.set_size(size);
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(30));
                harness.step();
                assert!(harness.state().assets.idle(), "scaled while resizing");
            }
            std::thread::sleep(Duration::from_millis(350));
        }
        let started = Instant::now();
        harness.run_steps(launcher::Page::ALL.len() + 2);
        while !harness.state().assets.idle() && started.elapsed() < Duration::from_secs(120) {
            std::thread::sleep(Duration::from_millis(20));
            harness.step();
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        eprintln!("{size:?} warm-up: {ms:.0} ms");
        let mut slowest = 0.0f64;
        for page in launcher::Page::ALL {
            harness.state_mut().menu.page = page;
            let t = Instant::now();
            harness.step();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            slowest = slowest.max(ms);
            eprintln!("{page:?}: first frame {ms:.1} ms");
        }
        assert!(harness.state().assets.idle(), "a page asked for new images");
        eprintln!("slowest: {slowest:.1} ms");
    }
}
