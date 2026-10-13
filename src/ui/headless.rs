//! Headless UI snapshots: the same shots as snapshot mode (`snapshot.rs`), rendered
//! offscreen with wgpu instead of screenshotting a window, so they also work with the
//! screen locked or on a machine without a desktop session.
//!
//! `ECHOVR_SNAPSHOTS=<dir> cargo test headless -- --ignored` (`ECHOVR_SNAPSHOTS_ONLY`
//! filters as in snapshot mode; `ECHOVR_SNAPSHOTS_SCALE=2` renders at 2x;
//! `ECHOVR_SNAPSHOTS_SIZE=1680x720` in a window of that size (logical pixels);
//! `ECHOVR_SNAPSHOTS_FEED=live` shows the real RIGHT NOW numbers and news instead of made-up ones;
//! `ECHOVR_SNAPSHOTS_RAIL=1` unfolds the rail). Every text cut with "…" goes to `cuts.txt`.

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
        super::cuts::set_shot(&shot.name);
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
    // Every text cut with "…", by shot: labels that don't fit.
    let cuts: String = super::cuts::take()
        .into_iter()
        .map(|(shot, text)| format!("{shot}\t{}\n", text.replace('\n', " ")))
        .collect();
    std::fs::write(dir.join("cuts.txt"), cuts).unwrap();
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

/// The rail with ten pinned plugins: the wheel scrolls their tabs, the last one opens,
/// a right click on it opens its menu, and Move up moves it.
/// `ECHOVR_SNAPSHOTS=<dir> cargo test rail_scroll -- --ignored` (pictures of each step).
#[test]
#[ignore = "needs a GPU; run with ECHOVR_SNAPSHOTS=<dir> cargo test rail_scroll -- --ignored"]
fn rail_scroll() {
    use egui::{pos2, vec2, Event, PointerButton};
    let dir = std::env::var_os("ECHOVR_SNAPSHOTS").map(std::path::PathBuf::from);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(launcher::W, launcher::H))
        .wgpu()
        .build_eframe(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::install_style(&cc.egui_ctx);
            let mut app = App::default();
            app.menu.demo = true;
            app.menu.page = launcher::Page::Play;
            app.menu.snap_variant = Some(launcher::SnapVariant::ManyPluginsOpen);
            app
        });
    harness.run_steps(6);
    let click = |h: &mut egui_kittest::Harness<App>, x: f32, y: f32, button: PointerButton| {
        let pos = pos2(x, y);
        h.hover_at(pos);
        h.step();
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Default::default(),
            });
        }
        h.run_steps(3);
    };
    let open = |h: &egui_kittest::Harness<App>, key: &str| {
        let id = crate::ui::widgets::menu_id(key);
        h.ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false))
    };
    let save = |h: &mut egui_kittest::Harness<App>, name: &str| {
        if let Some(dir) = &dir {
            let img = h.render().expect("render");
            img.save(dir.join(format!("rail_scroll_{name}.png")))
                .unwrap();
        }
    };
    // (Logical pixels in the 1280×720 window.) The wheel over the plugin tabs: down to
    // the end, where the last one sits right over the Plugins page.
    harness.hover_at(pos2(60.0, 500.0));
    harness.step();
    harness.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -600.0),
        phase: egui::TouchPhase::Move,
        modifiers: Default::default(),
    });
    harness.run_steps(40);
    save(&mut harness, "1_scrolled");
    // The last one, Clip recorder (the 11th installed: the 3rd is unpinned).
    click(&mut harness, 60.0, 615.0, PointerButton::Primary);
    assert_eq!(harness.state().menu.page, launcher::Page::Plugin(10));
    save(&mut harness, "2_opened");
    // Its menu, above it (no room below), then Move up.
    click(&mut harness, 60.0, 615.0, PointerButton::Secondary);
    assert!(open(&harness, "rail-tab"), "the tab's menu opened");
    save(&mut harness, "3_tab_menu");
    click(&mut harness, 260.0, 485.0, PointerButton::Primary);
    assert!(!open(&harness, "rail-tab"));
    save(&mut harness, "4_moved");
    // Stats tracker is the last tab now.
    click(&mut harness, 60.0, 615.0, PointerButton::Primary);
    assert_eq!(harness.state().menu.page, launcher::Page::Plugin(9));
}

/// The uwu easter egg: "uwu" typed into a field (and nothing else) turns it on, Unuwufy
/// turns it off, and the field still saying "uwu" doesn't turn it on again.
/// `ECHOVR_SNAPSHOTS=<dir> cargo test uwu_trigger -- --ignored` (pictures of each step).
#[test]
#[ignore = "needs a GPU; run with cargo test uwu_trigger -- --ignored"]
fn uwu_trigger() {
    use egui::{pos2, Event, Key, PointerButton};
    let dir = std::env::var_os("ECHOVR_SNAPSHOTS").map(std::path::PathBuf::from);
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(launcher::W, launcher::H))
        .wgpu()
        .build_eframe(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::install_style(&cc.egui_ctx);
            let mut app = App::default();
            app.menu.demo = true;
            app.menu.page = launcher::Page::Settings;
            app
        });
    harness.run_steps(6);
    let click = |h: &mut egui_kittest::Harness<App>, x: f32, y: f32| {
        let pos = pos2(x, y);
        h.hover_at(pos);
        h.step();
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        h.run_steps(3);
    };
    let save = |h: &mut egui_kittest::Harness<App>, name: &str| {
        if let Some(dir) = &dir {
            let img = h.render().expect("render");
            img.save(dir.join(format!("uwu_trigger_{name}.png")))
                .unwrap();
        }
    };
    // (Logical pixels in the 1280×720 window.) Settings' Extra arguments field.
    click(&mut harness, 276.0, 489.0);
    harness.event(Event::Text("uwux".into()));
    harness.run_steps(3);
    assert!(!super::uwu::on(), "\"uwux\" isn't the word");
    harness.event(Event::Key {
        key: Key::Backspace,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    });
    harness.run_steps(6);
    save(&mut harness, "1_on");
    assert!(super::uwu::on(), "\"uwu\" turns it on");
    // Unuwufy, at the status bar's right end.
    click(&mut harness, 814.0, 24.0);
    harness.run_steps(6);
    save(&mut harness, "2_off");
    assert!(!super::uwu::on(), "Unuwufy turns it off");
    harness.run_steps(6);
    assert!(
        !super::uwu::on(),
        "the field still saying uwu doesn't turn it on again"
    );
}
