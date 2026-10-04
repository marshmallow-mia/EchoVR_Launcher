//! Settings page, laid out like Play and Install: the game (how you play) and, under it,
//! launch options and storage as cards; the launcher itself (looks, updates, support and
//! About) in the right-hand panel.

use super::install::{myriad, text_link};
use super::{hero, panel, setup, Dashboard, LauncherUpdate, Msg, CREDITS};
use crate::core::launcher::relay;
use crate::core::launcher::store::{Runtime, SteamVrVia};
use crate::core::links::Handler;
use crate::core::{logs, paths, platform};
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::markdown;
use crate::ui::parts;
use crate::ui::style::Icon;
use crate::ui::widgets::{Tone, BTN_H};

// Geometry in design pixels: the cards where Play and Install have theirs.
const GAME: Dr = Dr::new(137.0, 80.0, 1146.0, 346.0);
const LOWER: [Dr; 2] = [
    Dr::new(137.0, 456.0, 555.0, 590.0),
    Dr::new(728.0, 456.0, 555.0, 590.0),
];
const TILE_H: f32 = 170.0;
/// The right-hand panel's content width, as YOUR LIBRARY's.
const PANEL_W: f32 = 492.0;

const CACHE_KEY: &str = "settings-delete-cache";
/// Asks before downloading ffmpeg for a background video.
pub(super) const FFMPEG_KEY: &str = "settings-download-ffmpeg";
pub(super) const BACKGROUND_JOB: &str = "background";
const UPLOAD_KEY: &str = "settings-upload-logs";
const UPLOAD_TEXT: &str = "This sends your logs to the developer, marshmallow-mia, to help with a problem: the launcher's, Echo VR's (from each installed version), EchoXR's, plugins' and the Quest logs you saved last. Only the developer can see them, and they're deleted after 30 days.\n\n\
They can contain your computer's user name (in folder paths), where Echo VR and the launcher are installed, your headset's model and serial number, your Echo VR account name and the matches you joined, the versions and options you use, and error messages. The server also sees your IP address.\n\n\
To have them deleted, message marshmallow-mia on Discord or email echo@mia-hentschel.de.";

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    if !kit.ghost {
        answers(d, ctx);
    }
    game(d, kit, ctx);
    // Side by side, sharing the extra width; taller in a taller window.
    let (half, dy) = (kit.dx() / 2.0, kit.dy());
    launch_options(d, kit, LOWER[0].wider(half).taller(dy));
    storage(d, kit, LOWER[1].moved(half).wider(half).taller(dy));
    panel::at_right(kit, |k| launcher(d, k, ctx));
}

/// The answers to this page's dialogs.
fn answers(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(a) = d.dialogs.take(FFMPEG_KEY) {
        if let Some(input) = d.background_pending.take().filter(|_| a.is_yes()) {
            d.set_background(ctx, input, true);
        }
    }
    if d.dialogs.take(CACHE_KEY).is_some_and(|a| a.is_yes()) {
        d.deleting_cache = true;
        let roots = d.cache_roots();
        d.worker.spawn(ctx, move |tx| {
            tx.send(Msg::CacheDeleted(crate::core::cache::delete_all(&roots)))
        });
    }
    if d.dialogs
        .take(UPLOAD_KEY)
        .is_some_and(|a| a == crate::ui::dialogs::Answer::Button(0))
    {
        upload(d, ctx);
    }
}

/// A checkbox with a grey note under its label; returns whether it flipped and the height
/// it took.
#[allow(clippy::too_many_arguments)]
fn option(
    kit: &mut Kit,
    key: &str,
    on: &mut bool,
    label: &str,
    note: &str,
    x: f32,
    y: f32,
    w: f32,
    enabled: bool,
    tip: &str,
) -> (bool, f32) {
    let flipped = kit.check(key, on, label, x, y, enabled, tip);
    let indent = dz(39.0);
    let h = kit.caps_text(
        x + indent,
        y + dz(38.0),
        w - indent,
        note,
        14.0,
        design::GREY,
        0.0,
    );
    (flipped, dz(38.0) + h + dz(26.0))
}

/// BACKGROUND: the launcher's own, or a video or picture of your own (converted to the
/// launcher's format; `core::launcher::background`). Returns its height.
fn background_row(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    x: f32,
    y: f32,
    w: f32,
) -> f32 {
    kit.caption(x, y, "Background");
    let mut ty = y + dz(28.0);
    let job = hero::job_view(d, BACKGROUND_JOB);
    let text = match (&job, &d.custom_bg) {
        (Some(j), _) => j.label.clone(),
        (None, Some(c)) if c.video.is_some() => format!("Your video: {}", c.name),
        (None, Some(c)) => format!("Your picture: {}", c.name),
        (None, None) => "The launcher's own".into(),
    };
    let g = myriad(kit, &text, design::myriad(20.0), design::BODY, w, true);
    ty += kit.put(x, ty, g).height() + dz(12.0);
    let half = (w - dz(14.0)) / 2.0;
    if let Some(j) = &job {
        let label = if j.cancelling {
            "Stopping…"
        } else {
            "Cancel"
        };
        if kit
            .button(
                "bg-cancel",
                x,
                ty,
                half,
                BTN_H,
                Tone::Dark,
                Some(Icon::Close),
                label,
                !j.cancelling,
                "Stop converting",
            )
            .clicked
        {
            d.cancel_job(BACKGROUND_JOB);
        }
    } else if kit
        .button(
            "bg-choose",
            x,
            ty,
            half,
            BTN_H,
            Tone::Panel,
            Some(Icon::Folder),
            "Choose file",
            !d.any_job(),
            "A video (MP4, MOV, WebM, MKV, GIF) or a picture (PNG, JPEG)",
        )
        .clicked
    {
        if let Some(input) = pick_background() {
            d.set_background(ctx, input, false);
        }
    }
    let custom = d.custom_bg.is_some() && job.is_none();
    if kit
        .button(
            "bg-default",
            x + half + dz(14.0),
            ty,
            half,
            BTN_H,
            Tone::Dark,
            None,
            "Default",
            custom,
            "Back to the launcher's own background",
        )
        .clicked
    {
        match crate::core::launcher::background::reset() {
            Ok(()) => {
                d.load_custom_background();
                d.notify("The launcher's own background is back");
            }
            Err(e) => d.dialogs.error(
                "Couldn't reset the background",
                &format!("{e:#}"),
                Default::default(),
            ),
        }
    }
    ty += BTN_H + dz(12.0);
    let note = format!(
        "Videos are cut to {} seconds and cropped to fill the window.",
        crate::core::launcher::background::MAX_SECONDS
    );
    ty += kit.caps_text(x, ty, w, &note, 14.0, design::GREY, 0.0);
    ty - y
}

/// A video or picture for the background.
fn pick_background() -> Option<std::path::PathBuf> {
    use crate::core::launcher::background::{PICTURE_EXTENSIONS, VIDEO_EXTENSIONS};
    let all: Vec<&str> = VIDEO_EXTENSIONS
        .iter()
        .chain(PICTURE_EXTENSIONS.iter())
        .copied()
        .collect();
    rfd::FileDialog::new()
        .add_filter("Videos and pictures", &all)
        .add_filter("Videos", &VIDEO_EXTENSIONS)
        .add_filter("Pictures", &PICTURE_EXTENSIONS)
        .pick_file()
}

/// The background video's speeds, in percent.
const SPEEDS: [u32; 5] = [50, 100, 150, 200, 300];

/// The background video's speed: one button per step, the current one blue. Returns
/// its height.
fn speed_row(d: &mut Dashboard, kit: &mut Kit, x: f32, y: f32, w: f32) -> f32 {
    let indent = dz(39.0);
    kit.caption(x + indent, y, "Speed");
    let by = y + dz(26.0);
    let h = dz(40.0);
    let gap = dz(8.0);
    let n = SPEEDS.len() as f32;
    let bw = (w - indent - (n - 1.0) * gap) / n;
    let on = d.state.animated_background;
    let current = d.state.background_speed;
    for (i, speed) in SPEEDS.into_iter().enumerate() {
        let selected = current == speed;
        let label = format!("{speed}%");
        let tone = if selected { Tone::Blue } else { Tone::Dark };
        let tip = if on {
            "How fast the background video plays"
        } else {
            "Turn on the animated background first"
        };
        let bx = x + indent + i as f32 * (bw + gap);
        if kit
            .button(
                &format!("bg-speed-{i}"),
                bx,
                by,
                bw,
                h,
                tone,
                None,
                &label,
                on,
                tip,
            )
            .clicked
            && !selected
        {
            d.state.background_speed = speed;
            d.save();
        }
    }
    dz(26.0) + h
}

// ---- the cards ----

/// GAME: how you play, as the Install card asks it (a tile each), and the SteamVR
/// artwork.
fn game(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let (x, y, w, _) = hero::card_frame(kit, GAME.wider(kit.dx()), "Game");
    kit.caption(x, y, "How you play");
    let top = y + dz(30.0);
    let gap = dz(16.0);
    let offered = setup::runtimes(d);
    let n = offered.len() as f32;
    let tw = (w - (n - 1.0) * gap) / n;
    for (i, rt) in offered.into_iter().enumerate() {
        let tx = x + i as f32 * (tw + gap);
        let on = d.state.profile.runtime == rt;
        if kit
            .tile(
                &format!("settings-runtime-{i}"),
                tx,
                top,
                tw,
                dz(TILE_H),
                setup::runtime_label(rt),
                setup::runtime_note(rt),
                on,
            )
            .clicked
            && !on
        {
            d.state.profile.runtime = rt;
            d.save();
        }
    }
    if d.state.profile.runtime == Runtime::Revive {
        let ay = top + dz(TILE_H) + dz(24.0);
        // SteamVR through Revive's injector, or EchoXR's OpenXR runtime in the game's
        // folder; Revive's own options beside them.
        let cap = kit.caption(x, ay + dz(9.0), "SteamVR through");
        let mut bx = x + cap.width() + dz(16.0);
        // Lower than a card's buttons: the row is the card's last.
        let bh = dz(42.0);
        let choices = [
            (
                SteamVrVia::Revive,
                "Revive",
                "Revive injects itself into Echo VR; the launcher installs it (asks for administrator rights)",
            ),
            (
                SteamVrVia::EchoXr,
                "EchoXR",
                "EchoXR's OpenXR runtime in the game's folder: no injection and no administrator rights. Live build only.",
            ),
        ];
        for (via, label, tip) in choices {
            let on = d.state.profile.steamvr_via == via;
            let bw = kit.button_width(label, None, bh).max(dz(130.0));
            let tone = if on { Tone::Blue } else { Tone::Dark };
            let key = format!("steamvr-via-{label}");
            if kit
                .button(&key, bx, ay - dz(4.0), bw, bh, tone, None, label, true, tip)
                .clicked
                && !on
            {
                d.state.profile.steamvr_via = via;
                d.save();
            }
            bx += bw + dz(10.0);
        }
        if d.state.profile.steamvr_via != SteamVrVia::Revive {
            return;
        }
        let cx = bx + dz(30.0);
        let cy = ay;
        if kit.check(
            "artwork",
            &mut d.state.revive_artwork,
            "Game artwork",
            cx,
            cy,
            true,
            "When setting up SteamVR, also install Echo VR's artwork for the SteamVR library",
        ) {
            d.save();
        }
        // Beside the artwork's, on the same row.
        let idle = !d.any_job();
        if kit.check(
            "steamvr-library",
            &mut d.state.revive_library,
            "Echo VR in SteamVR's library",
            cx + (x + w - cx) * 0.4,
            cy,
            idle,
            "Echo VR in SteamVR's library starts the version PLAY starts, with your launch options. After switching versions, tick this off and on to update it.",
        ) {
            d.save();
            // Once SteamVR is set up, the entry follows the box (setup adds it otherwise).
            if cfg!(windows) && !d.revive_missing() {
                setup::steamvr_library(d, ctx, d.state.revive_library);
            }
        }
    }
}

/// LAUNCH OPTIONS: what Echo VR is started with (desktop shortcuts too).
fn launch_options(d: &mut Dashboard, kit: &mut Kit, r: Dr) {
    let (x, mut y, w, _) = hero::card_frame(kit, r, "Launch options");
    let (flipped, h) = option(
        kit,
        "opt-windowed",
        &mut d.state.profile.windowed,
        "Windowed",
        "Echo VR's window on the desktop isn't full screen.",
        x,
        y,
        w,
        true,
        "Starts Echo VR with -windowed",
    );
    let mut changed = flipped;
    y += h;
    let flat = d.state.profile.runtime == Runtime::Flat;
    let tip = if flat {
        "Starts Echo VR with -spectatorstream"
    } else {
        "Choose Flat under How you play"
    };
    let (flipped, h) = option(
        kit,
        "opt-spectator",
        &mut d.state.profile.spectator,
        "Spectator stream",
        "Flat only: join matches as a spectator, for streams and casting.",
        x,
        y,
        w,
        flat,
        tip,
    );
    changed |= flipped;
    y += h;
    kit.caption(x, y, "Extra arguments");
    y += dz(30.0);
    changed |= kit.field(
        "opt-args",
        &mut d.state.profile.extra_args,
        x,
        y,
        w,
        BTN_H,
        "e.g. -mp -http",
        false,
        "Passed to echovr.exe after the options above",
    );
    y += BTN_H + dz(12.0);
    y += kit.caps_text(
        x,
        y,
        w,
        "Added to every start as typed; quotes group words. Desktop shortcuts get them too.",
        14.0,
        design::GREY,
        0.0,
    );
    if changed {
        d.save();
    }
    classic_lobbies(d, kit, x, y + dz(26.0), w);
}

/// The classic lobbies server the event builds play on, and your account there.
fn classic_lobbies(d: &mut Dashboard, kit: &mut Kit, x: f32, y: f32, w: f32) {
    kit.caption(x, y, "Classic lobbies server (event builds)");
    let fy = y + dz(30.0);
    let bw = kit.button_width("Account", None, BTN_H).max(dz(150.0));
    let invalid = !relay::valid_server(&d.relay_server_field);
    let done = kit.field(
        "relay-server",
        &mut d.relay_server_field,
        x,
        fy,
        w - bw - dz(14.0),
        BTN_H,
        relay::DEFAULT_SERVER,
        invalid,
        "The EchoRelay server's address and port, e.g. 168.119.2.92:6800",
    );
    if done {
        let typed = d.relay_server_field.trim().to_string();
        if typed.is_empty() || invalid {
            // Back to what it was (empty: the community's server).
            if typed.is_empty() {
                d.state.relay_server = relay::DEFAULT_SERVER.into();
            }
            d.relay_server_field = d.state.relay_server.clone();
        } else if typed != d.state.relay_server {
            d.state.relay_server = typed;
            d.save();
            setup::write_relay_configs(d);
        }
    }
    if kit
        .button(
            "relay-account",
            x + w - bw,
            fy,
            bw,
            BTN_H,
            Tone::Dark,
            None,
            "Account",
            true,
            "Your display name and password on that server",
        )
        .clicked
    {
        d.overlay = Some(setup::relay_account(d, false));
    }
    let note = match &d.state.relay_account {
        Some(a) => format!("You play there as {}.", a.name),
        None => "An event build asks for your account there when you first play it.".into(),
    };
    kit.caps_text(x, fy + BTN_H + dz(12.0), w, &note, 14.0, design::GREY, 0.0);
}

/// STORAGE: where versions go, and the cache.
fn storage(d: &mut Dashboard, kit: &mut Kit, r: Dr) {
    let (x, mut y, w, _) = hero::card_frame(kit, r, "Storage");
    kit.caption(x, y, "Library folder");
    y += dz(30.0);
    let tip = format!("Where new versions are installed:\n{}", d.library_field);
    let bw = kit.button_width("Browse", Some(Icon::Folder), BTN_H);
    if kit.field(
        "library",
        &mut d.library_field,
        x,
        y,
        w - bw - dz(14.0),
        BTN_H,
        "",
        false,
        &tip,
    ) {
        set_library(d);
    }
    if kit
        .button(
            "lib-browse",
            x + w - bw,
            y,
            bw,
            BTN_H,
            Tone::Dark,
            Some(Icon::Folder),
            "Browse",
            !d.any_job(),
            "Pick the library folder",
        )
        .clicked
    {
        if let Some(p) = parts::choose_folder() {
            d.library_field = p;
            set_library(d);
        }
    }
    y += BTN_H + dz(12.0);
    let free = d
        .free_bytes()
        .map(|b| format!("{} free there. ", super::play::gb(b)))
        .unwrap_or_default();
    let hint = format!(
        "{free}New versions get their own folder here; existing installs stay where they are."
    );
    y += kit.caps_text(x, y, w, &hint, 14.0, design::GREY, 0.0) + dz(34.0);

    kit.caption(x, y, "Cache");
    y += dz(30.0);
    let size = d.cache_bytes();
    let size_text = size.map_or_else(|| "…".to_string(), super::play::gb);
    let g = kit.spaced_galley(&size_text, design::din(24.0), design::TEXT, dz(0.5), false);
    y += kit.put(x, y, g).height() + dz(8.0);
    y += kit.caps_text(
        x,
        y,
        w,
        "Downloaded installers, patches and the video converter, temporary files, and game zips left by cancelled installs.",
        14.0,
        design::GREY,
        0.0,
    ) + dz(20.0);
    let half = (w - dz(14.0)) / 2.0;
    let empty = size == Some(0);
    let can = !d.deleting_cache && !d.any_job() && !empty;
    let tip = if empty {
        "Nothing to delete"
    } else {
        "Delete the cached files (asks first)"
    };
    if kit
        .button(
            "del-cache",
            x,
            y,
            half,
            BTN_H,
            Tone::Dark,
            Some(Icon::Refresh),
            "Delete cache",
            can,
            tip,
        )
        .clicked
    {
        ask_delete_cache(d);
    }
    if kit
        .button(
            "data",
            x + half + dz(14.0),
            y,
            half,
            BTN_H,
            Tone::Dark,
            Some(Icon::Folder),
            "Data folder",
            true,
            "Open the folder with the settings (launcher.json) and logs",
        )
        .clicked
    {
        open_dir(d, &paths::data_dir());
    }
}

// ---- the right-hand panel ----

/// LAUNCHER: how it looks and behaves, updates, support, and About at the bottom.
fn launcher(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    panel::frame(kit, "Launcher");
    let look = panel::look();
    let (x, w) = (dz(panel::X), dz(PANEL_W));
    let mut y = dz(panel::BODY_Y);

    let (flipped, h) = option(
        kit,
        "animated-background",
        &mut d.state.animated_background,
        "Animated background",
        "The background video. It pauses while the launcher isn't in front.",
        x,
        y,
        w,
        true,
        "",
    );
    if flipped {
        d.save();
    }
    y += h - dz(10.0);
    y += speed_row(d, kit, x, y, w) + dz(26.0);
    y += background_row(d, kit, ctx, x, y, w) + dz(26.0);
    let (flipped, h) = option(
        kit,
        "minimize",
        &mut d.state.minimize_on_launch,
        "Minimize when Echo VR starts",
        "Keeps the launcher out of the way while you play.",
        x,
        y,
        w,
        true,
        "",
    );
    if flipped {
        d.save();
    }
    y += h + dz(6.0);
    // spark:// links (Windows, Linux).
    if let Some(handler) = d
        .link_handler
        .clone()
        .filter(|h| *h != Handler::Unsupported)
    {
        let mut on = handler == Handler::Ours;
        let note = match handler {
            Handler::Other(_) => "Spark opens them now. Tick to have the launcher open them instead.",
            _ => "Match links from Discord, echo.taxi or the browser open the launcher, ready to join.",
        };
        let (flipped, h) = option(
            kit,
            "spark-links",
            &mut on,
            "Open spark:// links",
            note,
            x,
            y,
            w,
            true,
            "",
        );
        if flipped {
            d.set_spark_links(ctx, on);
        }
        y += h + dz(6.0);
    }

    // Support.
    y += markdown::draw(kit, x, y, w, "__**Support**__", &look) + dz(10.0);
    let (label, tip) = if d.uploading_logs {
        ("Uploading logs…", "Your logs are on their way")
    } else {
        (
            "Upload logs",
            "Asks first, and says which logs go and what they contain",
        )
    };
    if kit
        .button(
            "upload-logs",
            x,
            y,
            w,
            BTN_H,
            Tone::Panel,
            Some(Icon::Info),
            label,
            !d.uploading_logs,
            tip,
        )
        .clicked
    {
        ask_upload(d);
    }

    about(d, kit, ctx, x, w);
    let footer = myriad(
        kit,
        "Not affiliated with Meta or Ready at Dawn",
        design::myriad(18.0),
        design::HEADING,
        w,
        true,
    );
    kit.put(x, dz(panel::footer_y(kit)), footer);
}

/// ABOUT, above the panel's footer: the icon, the name and version, and the credits and
/// links.
fn about(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, x: f32, w: f32) {
    let links_y = dz(panel::footer_y(kit) - 62.0);
    let s = dz(60.0);
    let top = links_y - dz(28.0) - s;
    let line = top - dz(24.0);
    kit.ui.painter().hline(
        kit.rect(x, line, w, 0.0).x_range(),
        kit.rect(x, line, w, 0.0).min.y,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)),
    );
    kit.image("icon.png", x, top, s, s);
    kit.title(x + s + dz(20.0), top + dz(4.0), "Echo VR Launcher", 22.0);
    let version = env!("CARGO_PKG_VERSION");
    let status = match &d.launcher_update {
        LauncherUpdate::Checking => format!("Version {version} · checking for updates"),
        LauncherUpdate::Latest => format!("Version {version} · the latest"),
        LauncherUpdate::Available(r) => format!("Version {version} · {} is out", r.version),
        LauncherUpdate::Failed => format!("Version {version} · couldn't check for updates"),
    };
    let g = kit.label_galley(&status, design::din(15.0), design::GREY, w - s - dz(20.0));
    kit.put(x + s + dz(20.0), top + dz(40.0), g);

    let update = match &d.launcher_update {
        LauncherUpdate::Available(_) => {
            Some(("about-update", "Download update", "Open the release page"))
        }
        LauncherUpdate::Checking => None,
        _ => Some((
            "about-update",
            "Check for updates",
            "Look for a newer launcher",
        )),
    };
    let links = [
        Some(("about-credits", "Credits", "Who made this possible")),
        Some(("about-discord", "Discord", "The Echo VR community")),
        Some((
            "about-source",
            "Source code",
            "The launcher on GitHub (GPL-3.0)",
        )),
        update,
    ];
    let mut lx = x;
    for (key, label, tip) in links.into_iter().flatten() {
        let clicked = text_link(kit, key, lx, links_y, label, 20.0, true, tip);
        let lw = myriad(
            kit,
            label,
            design::myriad(20.0),
            design::LINK,
            f32::INFINITY,
            true,
        )
        .size()
        .x;
        lx += lw + dz(28.0);
        if clicked {
            match (key, &d.launcher_update) {
                ("about-credits", _) => d.dialogs.info("Credits", CREDITS),
                ("about-discord", _) => platform::open_url(crate::core::LOUNGE_INVITE),
                ("about-source", _) => platform::open_url(env!("CARGO_PKG_REPOSITORY")),
                (_, LauncherUpdate::Available(r)) => platform::open_url(&r.url),
                _ => d.check_launcher_update(ctx),
            }
        }
    }
}

/// Asks before deleting the cache, saying how much goes.
pub(super) fn ask_delete_cache(d: &mut Dashboard) {
    let amount = d
        .cache_bytes()
        .map_or_else(|| "the cached files".into(), super::play::gb);
    d.dialogs.confirm_danger(
        CACHE_KEY,
        "Delete cached files?",
        &format!(
            "This deletes {amount}: downloaded installers and patches, temporary files, and game zips left by cancelled installs.\n\nYour installed versions, settings and logs stay."
        ),
        "Delete",
    );
}

/// What uploading the logs shares, and with whom; asks before it happens.
pub(super) fn ask_upload(d: &mut Dashboard) {
    d.upload_sources = if d.demo {
        demo_sources()
    } else {
        logs::collect(&d.state.versions)
    };
    if d.upload_sources.is_empty() {
        d.dialogs.info(
            "No logs yet",
            "There are no logs to upload yet: start Echo VR once, then try again.",
        );
        return;
    }
    let text = format!("{}\n\n{UPLOAD_TEXT}", what_goes(&d.upload_sources));
    d.dialogs.options(
        UPLOAD_KEY,
        "Upload your logs?",
        &text,
        DlgIcon::Info,
        &["Upload", "Cancel"],
    );
}

/// Pure: "It sends 8 log files (6.3 MB): from the launcher (2), Echo VR (5) and EchoXR (1)."
fn what_goes(sources: &[logs::Source]) -> String {
    let bytes: u64 = sources.iter().map(|s| s.bytes).sum();
    let parts: Vec<String> = logs::Kind::ALL
        .iter()
        .filter_map(|k| {
            let n = sources.iter().filter(|s| s.kind == *k).count();
            (n > 0).then(|| format!("{} ({n})", k.label()))
        })
        .collect();
    let from = match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let files = if sources.len() == 1 { "file" } else { "files" };
    format!(
        "It sends {} log {files} ({:.1} MB): from {from}.",
        sources.len(),
        bytes as f64 / 1_000_000.0
    )
}

/// Snapshots: logs as a player's PC has them.
fn demo_sources() -> Vec<logs::Source> {
    let s = |kind, name: &str, bytes| logs::Source {
        kind,
        name: name.into(),
        path: Default::default(),
        bytes,
    };
    vec![
        s(logs::Kind::Launcher, "EchoVR_Launcher.log", 412_000),
        s(logs::Kind::EchoXr, "pc-latest.launcher.log", 9_000),
        s(logs::Kind::EchoXr, "pc-latest.runtime.log", 31_000),
        s(logs::Kind::Echo, "pc-latest.r14-1.log", 2_400_000),
        s(logs::Kind::Echo, "pc-latest.r14-2.log", 1_900_000),
        s(logs::Kind::Echo, "pc-latest.r14-3.log", 1_550_000),
    ]
}

/// Sends the logs the dialog listed, in the background.
fn upload(d: &mut Dashboard, ctx: &egui::Context) {
    if d.demo || d.uploading_logs {
        return;
    }
    d.uploading_logs = true;
    d.notify("Uploading your logs…");
    let sources = std::mem::take(&mut d.upload_sources);
    d.worker.spawn(ctx, move |tx| {
        let r = logs::bundle(&sources)
            .and_then(logs::upload)
            .map_err(|e| format!("{e:#}"));
        tx.send(Msg::LogsUploaded(r));
    });
}

/// The logs arrived: their reference, copied, to give the developer.
pub(super) fn logs_sent(d: &mut Dashboard, ctx: &egui::Context, code: &str) {
    ctx.copy_text(code.to_string());
    d.dialogs.info(
        "Your logs are uploaded",
        &format!(
            "Your reference is {code} (it's copied). Send it to marshmallow-mia on Discord, so the right logs are looked at.\n\nThey're deleted after 30 days."
        ),
    );
}

fn open_dir(d: &mut Dashboard, dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = platform::open_folder(dir) {
        d.dialogs.error(
            "Couldn't open folder",
            &format!("{e:#}"),
            Default::default(),
        );
    }
}

pub(super) fn set_library(d: &mut Dashboard) {
    let lib = paths::normalize(&d.library_field);
    if lib.is_empty() {
        d.library_field = d.state.library.clone();
        return;
    }
    d.library_field = lib.clone();
    if lib != d.state.library {
        d.state.library = lib;
        d.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_goes() {
        assert_eq!(
            what_goes(&demo_sources()),
            "It sends 6 log files (6.3 MB): from the launcher (1), EchoXR (2) and Echo VR (3)."
        );
        assert_eq!(
            what_goes(&demo_sources()[..1]),
            "It sends 1 log file (0.4 MB): from the launcher (1)."
        );
    }
}
