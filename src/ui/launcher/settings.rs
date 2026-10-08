//! Settings page, laid out like Play and Install: the game (how you play) and, under it,
//! launch options and storage as cards; the launcher itself (looks, updates, support and
//! About) in the right-hand panel.

use super::install::{myriad, text_link};
use super::{hero, panel, setup, Dashboard, JobKind, JobResult, LauncherUpdate, Msg};
use crate::core::launcher::relay;
use crate::core::launcher::store::{Runtime, SteamVrVia, VdVia};
use crate::core::launcher::update_check::Channel;
use crate::core::links::Handler;
use crate::core::{logs, paths, platform, revive};
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
const UPLOAD_TEXT: &str = "This sends the logs checked above to the developer, marshmallow-mia, and selected Echo VR Lounge moderators, to help with a problem. Only they can see them, and they're deleted after 30 days. Your EchoVRCE account's ID, its Discord ID and this PC's Echo VR account (OVR-ORG) go with them, so they know whose logs they are.\n\n\
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
    if d.dialogs.take(CACHE_KEY).is_some_and(|a| a.is_yes()) {
        d.deleting_cache = true;
        let roots = d.cache_roots();
        d.worker.spawn(ctx, move |tx| {
            tx.send(Msg::CacheDeleted(crate::core::cache::delete_all(&roots)))
        });
    }
}

/// A checkbox with a grey note under its label (none: a compact row); returns whether it
/// flipped and the height it took.
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
    if note.is_empty() {
        return (flipped, dz(50.0));
    }
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
    // Virtual Desktop (on Windows): what its streamer runs Echo VR through.
    if (cfg!(windows) || d.demo) && d.state.profile.runtime == Runtime::VirtualDesktop {
        let ry = top + dz(TILE_H) + dz(24.0);
        let cap = kit.caption(x, ry + dz(8.0), "Through");
        let mut rx = cap.max.x + dz(28.0);
        for via in VdVia::ALL {
            let (label, tip) = vd_route(via);
            let on = d.state.profile.vd_via == via;
            if kit.radio(
                &format!("settings-vd-{via:?}"),
                on,
                label,
                rx,
                ry,
                true,
                tip,
            ) {
                d.state.profile.vd_via = via;
                d.save();
            }
            let g = kit.label_galley(label, design::din(16.0), design::BODY, f32::INFINITY);
            rx += 26.0 + g.size().x + dz(44.0);
        }
        return;
    }
    // SteamVR through Revive (on Windows; EchoXR instead is its switch on the Mods
    // page): Revive's own options.
    if cfg!(windows) || d.demo {
        if d.state.profile.runtime != Runtime::Revive
            || d.state.profile.steamvr_via != SteamVrVia::Revive
        {
            return;
        }
        let ay = top + dz(TILE_H) + dz(24.0);
        let cx = x;
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
        // Revive was there before (PLAY never prepared it): the artwork from here.
        let present = cfg!(windows) && !d.demo && !d.revive_missing();
        if present && d.state.revive_artwork && !d.any_job() && !revive::artwork_installed() {
            let lx = cx + dz(250.0);
            if kit
                .link(
                    "artwork-install",
                    lx,
                    cy + dz(2.0),
                    "Install it",
                    14.0,
                    "The artwork isn't installed yet (asks for administrator rights)",
                )
                .clicked
            {
                setup::revive_artwork(d, ctx);
            }
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

/// A Virtual Desktop route's name and what it needs.
fn vd_route(via: VdVia) -> (&'static str, &'static str) {
    match via {
        VdVia::Meta => (
            "Oculus mode",
            "Virtual Desktop's streamer starts Echo VR and gives it the headset, as its Games list does",
        ),
        VdVia::SteamVr => (
            "SteamVR (EchoXR)",
            "Through EchoXR on SteamVR, which Virtual Desktop streams: SteamVR has to be installed",
        ),
        VdVia::VdXr => (
            "VD's OpenXR (EchoXR)",
            "Through EchoXR on Virtual Desktop's own OpenXR runtime: neither SteamVR nor Meta's runtime",
        ),
    }
}

/// LAUNCH OPTIONS: what Echo VR is started with (desktop shortcuts too).
fn launch_options(d: &mut Dashboard, kit: &mut Kit, r: Dr) {
    let (x, mut y, w, _) = hero::card_frame(kit, r, "Launch options");
    // With the classic lobbies' server too, the card is full: the notes go into the
    // checks' tooltips.
    let compact = d.state.has_event_builds();
    let say = |note: &'static str, tip: &str| -> (&'static str, String) {
        if compact {
            ("", format!("{note} {tip}"))
        } else {
            (note, tip.to_string())
        }
    };
    let flat = d.state.profile.runtime == Runtime::Flat;
    // nEVR takes -windowed for "no headset": in VR it would start the game flat.
    let (note, tip) = say(
        "Flat only: Echo VR's window on the desktop isn't full screen.",
        if flat {
            "Starts Echo VR with -windowed"
        } else {
            "Choose Flat under How you play"
        },
    );
    let (flipped, h) = option(
        kit,
        "opt-windowed",
        &mut d.state.profile.windowed,
        "Windowed",
        note,
        x,
        y,
        w,
        flat,
        &tip,
    );
    let mut changed = flipped;
    y += h;
    let (note, tip) = say(
        "Flat only: join matches as a spectator, for streams and casting.",
        if flat {
            "Starts Echo VR with -spectatorstream"
        } else {
            "Choose Flat under How you play"
        },
    );
    let (flipped, h) = option(
        kit,
        "opt-spectator",
        &mut d.state.profile.spectator,
        "Spectator stream",
        note,
        x,
        y,
        w,
        flat,
        &tip,
    );
    changed |= flipped;
    y += h;
    // Your own _local/config.json (another server) instead of nEVR's built-in one.
    let (note, tip) = say(
        "Only with the mod loader: Echo VR keeps using your own _local/config.json (e.g. for another server).",
        "Off: an EchoVRCE-era _local/config.json is set aside before each start, and nEVR's built-in config, with friends and parties, applies",
    );
    let (flipped, h) = option(
        kit,
        "opt-own-config",
        &mut d.state.own_game_config,
        "Use my own config.json",
        note,
        x,
        y,
        w,
        true,
        &tip,
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
    if d.state.has_event_builds() {
        classic_lobbies(d, kit, x, y + dz(26.0), w);
    }
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
        "Downloaded installers and patches, temporary files, and game zips left by cancelled installs.",
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
            Some(Icon::Trash),
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
    let uy = y + BTN_H + dz(14.0);
    if kit
        .button(
            "uninstall",
            x,
            uy,
            w,
            BTN_H,
            Tone::Danger,
            Some(Icon::Trash),
            "Uninstall…",
            !d.any_job(),
            "Remove Echo VR, its mods and VR set-up, and the launcher's data (choose what)",
        )
        .clicked
    {
        ask_uninstall(d);
    }
}

/// The update announcements' switches; returns where the next part starts.
fn update_options(d: &mut Dashboard, kit: &mut Kit, x: f32, y: f32, w: f32) -> f32 {
    use crate::core::tray;
    let mut y = y;
    let (flipped, h) = option(
        kit,
        "desktop-notifications",
        &mut d.state.desktop_notifications,
        "Update notifications",
        "A desktop notification when the launcher, Echo VR or a plugin has an update (the launcher looks every 15 minutes).",
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
    let (flipped, h) = option(
        kit,
        "tray",
        &mut d.state.tray,
        "Keep looking in the tray",
        "While the launcher is closed, its icon in the tray looks for updates and opens it.",
        x,
        y,
        w,
        true,
        "",
    );
    if flipped {
        // Off, it doesn't start at login either.
        if !d.state.tray && d.state.tray_at_login && !d.demo {
            let _ = tray::set_autostart(false);
            d.state.tray_at_login = false;
        }
        d.save();
        if !d.demo {
            let on = d.state.tray;
            std::thread::spawn(move || if on { tray::start() } else { tray::quit() });
        }
    }
    y += h + dz(6.0);
    let mut login = d.state.tray_at_login;
    let (flipped, h) = option(
        kit,
        "tray-login",
        &mut login,
        "Start the tray at login",
        "Updates are found from the moment you log in, without opening the launcher.",
        x,
        y,
        w,
        d.state.tray,
        "Needs \"Keep looking in the tray\"",
    );
    if flipped && !d.demo {
        match tray::set_autostart(login) {
            Ok(()) => {
                d.state.tray_at_login = login;
                d.save();
            }
            Err(e) => d
                .dialogs
                .error("Couldn't change it", &format!("{e:#}"), Default::default()),
        }
    }
    y += h + dz(12.0);
    // A test channel for the PC update (launcher.json's update_manifest, or the
    // environment's ECHOVR_UPDATE_MANIFEST): said here while it applies.
    if let Some((url, from)) = crate::core::pc_update::channel_override() {
        let channel = url
            .trim_end_matches("/update.manifest")
            .rsplit('/')
            .next()
            .unwrap_or(&url)
            .to_string();
        y += kit.caps_text(
            x,
            y,
            w,
            &format!("Echo VR updates from the test channel {channel} ({from}), not the live one."),
            14.0,
            design::QUEST_WARN,
            0.0,
        ) + dz(8.0);
        if from == "launcher.json" {
            if kit
                .link(
                    "update-channel-live",
                    x,
                    y,
                    "Back to the live channel",
                    14.0,
                    &url,
                )
                .clicked
            {
                d.state.update_manifest = None;
                d.save();
            }
            y += dz(30.0);
        }
        y += dz(8.0);
    }
    y
}

// ---- uninstalling ----

const UNINSTALL_KEY: &str = "uninstall";
const UNINSTALLED_KEY: &str = "uninstalled";

/// Opens the uninstall card with every part ticked.
pub(super) fn ask_uninstall(d: &mut Dashboard) {
    d.overlay = Some(setup::Overlay::Uninstall {
        picked: crate::core::uninstall::Part::all(),
    });
}

/// UNINSTALL: each part with what it removes, ticked or not; then a confirmation.
pub(super) fn uninstall_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    use crate::core::uninstall::Part;
    let parts = Part::all();
    let Some(setup::Overlay::Uninstall { picked }) = &mut d.overlay else {
        return;
    };
    let (w, h) = (dz(1100.0), dz(900.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Uninstall");
    let mut ry = y;
    ry += k.caps_text(
        x,
        ry,
        cw,
        "Choose what goes. The launcher's program file stays: delete it yourself afterwards.",
        16.0,
        design::BODY,
        0.0,
    ) + dz(16.0);
    for part in &parts {
        let mut on = picked.contains(part);
        let (flipped, used) = option(
            k,
            &format!("uninstall-{part:?}"),
            &mut on,
            part.title(),
            part.detail(),
            x,
            ry,
            cw,
            true,
            "",
        );
        if flipped {
            picked.retain(|p| p != part);
            if on {
                picked.push(*part);
            }
        }
        ry += used + dz(10.0);
    }
    let chosen: Vec<Part> = parts
        .iter()
        .copied()
        .filter(|p| picked.contains(p))
        .collect();
    let by = bottom - BTN_H;
    let cancel_w = k.button_width("Cancel", None, BTN_H).max(110.0);
    let go_w = k.button_width("Uninstall", None, BTN_H).max(150.0);
    let all_w = k.button_width("Everything", None, BTN_H).max(140.0);
    if k.button(
        "uninstall-all",
        x,
        by,
        all_w,
        BTN_H,
        Tone::Dark,
        None,
        "Everything",
        true,
        "Tick every part",
    )
    .clicked
    {
        *picked = parts.clone();
    }
    let right = x + cw;
    if k.button(
        "uninstall-cancel",
        right - cancel_w,
        by,
        cancel_w,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        d.overlay = None;
        return;
    }
    if k.button(
        "uninstall-go",
        right - cancel_w - 8.0 - go_w,
        by,
        go_w,
        BTN_H,
        Tone::Danger,
        Some(Icon::Trash),
        "Uninstall",
        !chosen.is_empty(),
        "Remove the ticked parts (asks once more)",
    )
    .clicked
    {
        let list = chosen
            .iter()
            .map(|p| format!("• {}", p.title()))
            .collect::<Vec<_>>()
            .join("\n");
        d.uninstall_parts = chosen;
        d.overlay = None;
        d.dialogs.confirm_danger(
            UNINSTALL_KEY,
            "Uninstall",
            &format!("This removes:\n\n{list}\n\nIt can't be undone."),
            "Uninstall",
        );
    }
}

/// The answers to the uninstall's confirmation and to its last word.
pub(super) fn uninstall_answers(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(a) = d.dialogs.take(UNINSTALL_KEY) {
        let parts = std::mem::take(&mut d.uninstall_parts);
        if a.is_yes() && !parts.is_empty() {
            let state = d.state.clone();
            let mut consent = setup::consent_asker(d.worker.tx(ctx));
            d.start_job(
                ctx,
                JobKind::Uninstall,
                "uninstall",
                "Uninstalling",
                "Starting...",
                move |_, on| {
                    JobResult::Uninstalled(crate::core::uninstall::run(
                        &parts,
                        &state,
                        &mut consent,
                        on,
                    ))
                },
            );
        }
    }
    if d.dialogs.take(UNINSTALLED_KEY).is_some() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// The uninstall is done: the versions it removed go from the list; without its data the
/// launcher says where its program file is and closes.
pub(super) fn uninstalled(d: &mut Dashboard, o: crate::core::uninstall::Outcome) {
    d.state.versions.retain(|v| !o.removed.contains(&v.id));
    if d.state
        .selected
        .as_ref()
        .is_some_and(|s| o.removed.contains(s))
    {
        d.state.selected = d.state.versions.first().map(|v| v.id.clone());
    }
    let notes = if o.notes.is_empty() {
        String::new()
    } else {
        format!("\n\nNot everything could go:\n{}", o.notes.join("\n"))
    };
    if o.data_removed {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "the launcher's program file".into());
        d.dialogs.options(
            UNINSTALLED_KEY,
            "Uninstalled",
            &format!("The launcher's things are gone. It closes now; delete its program file yourself:\n\n{exe}{notes}"),
            DlgIcon::Info,
            &["Close the launcher"],
        );
    } else {
        d.save();
        d.mods.changed();
        if notes.is_empty() {
            d.notify("Uninstalled");
        } else {
            d.dialogs.info("Uninstalled", notes.trim_start());
        }
    }
}

// ---- the right-hand panel ----

/// LAUNCHER: how it looks and behaves, updates, support, and About at the bottom.
fn launcher(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    panel::frame(kit, "Launcher");
    update_button(d, kit, ctx);
    let look = panel::look();
    let (x, w) = (dz(panel::X), dz(PANEL_W));
    let mut y = dz(panel::BODY_Y);

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

    // Updates: found every 15 minutes; how they are announced.
    if cfg!(any(windows, target_os = "linux")) || d.demo {
        y = update_options(d, kit, x, y, w);
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
    y += BTN_H + dz(14.0);
    if kit
        .button(
            "advanced-settings",
            x,
            y,
            w,
            BTN_H,
            Tone::Panel,
            Some(Icon::Gear),
            "Advanced settings",
            true,
            "The launcher's update channel: main, beta or alpha",
        )
        .clicked
    {
        d.overlay = Some(setup::Overlay::Advanced);
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
    let (status, action) = update_status(d);
    let g = kit.label_galley(&status, design::din(15.0), design::GREY, w - s - dz(20.0));
    kit.put(x + s + dz(20.0), top + dz(40.0), g);

    // An update that's out has its button at the panel's top instead.
    let update = action
        .as_ref()
        .filter(|_| !matches!(d.launcher_update, LauncherUpdate::Available(_)))
        .map(|(label, tip)| ("about-update", *label, tip.as_str()));
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
    let mut picked = None;
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
            picked = Some(key);
        }
    }
    match picked {
        Some("about-credits") => d.overlay = Some(setup::Overlay::Credits { scroll: 0.0 }),
        Some("about-discord") => platform::open_url(crate::core::LOUNGE_INVITE),
        Some("about-source") => platform::open_url(env!("CARGO_PKG_REPOSITORY")),
        Some(_) => update_action(d, ctx),
        None => {}
    }
}

/// The launcher's version and what its update check says, with what can be done about
/// it (a link's or button's label and tip): About's line and Advanced settings'.
fn update_status(d: &Dashboard) -> (String, Option<(&'static str, String)>) {
    let version = env!("CARGO_PKG_VERSION");
    let channel = d.state.launcher_channel.name();
    let updating = super::hero::job_view(d, super::LAUNCHER_JOB);
    let status = match &d.launcher_update {
        LauncherUpdate::Available(r) if updating.is_some() => format!(
            "updating to {}: {}",
            r.version,
            updating.as_ref().map(|j| j.step()).unwrap_or_default()
        ),
        LauncherUpdate::Checking => "checking for updates".into(),
        LauncherUpdate::Latest if d.state.launcher_channel == Channel::Main => "the latest".into(),
        LauncherUpdate::Latest => format!("the latest on {channel}"),
        // Back from a beta: the channel's release is older.
        LauncherUpdate::Available(r) if r.back => format!("{channel} is at {}", r.version),
        LauncherUpdate::Available(r) => format!("{} is out", r.version),
        LauncherUpdate::Failed => "couldn't check for updates".into(),
    };
    let self_update = crate::core::launcher::self_update::supported();
    let action = match &d.launcher_update {
        _ if updating.is_some() => None,
        LauncherUpdate::Available(r) if self_update && r.back => Some((
            "Switch now",
            format!(
                "Download {channel}'s launcher {}, check it and restart into it",
                r.version
            ),
        )),
        LauncherUpdate::Available(_) if self_update => Some((
            "Update now",
            "Download the new launcher, check it and restart into it".into(),
        )),
        LauncherUpdate::Available(_) => Some(("Download update", "Open the release page".into())),
        LauncherUpdate::Checking => None,
        _ => Some(("Check for updates", "Look for a newer launcher".into())),
    };
    (format!("Version {version} · {status}"), action)
}

/// A launcher update that's out: a button beside the panel's title, where it's seen
/// (About's line says what it is).
fn update_button(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let LauncherUpdate::Available(r) = &d.launcher_update else {
        return;
    };
    let (_, Some((short, tip))) = update_status(d) else {
        return;
    };
    let (tone, label) = match short {
        "Update now" => (Tone::Go, format!("Update to {}", r.version)),
        "Switch now" => (Tone::Blue, format!("Switch to {}", r.version)),
        other => (Tone::Blue, other.to_string()),
    };
    // Beside the title: as much room as it leaves (the short label when that's too little).
    let right = dz(panel::X + PANEL_W);
    let room = dz(PANEL_W - 190.0);
    let icon = Some(Icon::Download);
    let label = if kit.button_width(&label, icon, BTN_H) <= room {
        label
    } else {
        short.to_string()
    };
    let w = kit.button_width(&label, icon, BTN_H).max(150.0);
    if kit
        .button(
            "launcher-update",
            right - w,
            dz(37.0),
            w,
            BTN_H,
            tone,
            icon,
            &label,
            true,
            &tip,
        )
        .clicked
    {
        update_action(d, ctx);
    }
}

/// What [`update_status`]'s action does.
fn update_action(d: &mut Dashboard, ctx: &egui::Context) {
    match &d.launcher_update {
        LauncherUpdate::Available(_) if crate::core::launcher::self_update::supported() => {
            d.update_launcher(ctx)
        }
        LauncherUpdate::Available(r) => platform::open_url(&r.url),
        _ => d.check_launcher_update(ctx),
    }
}

/// ADVANCED SETTINGS: the launcher's update channel (main, beta or alpha), with what the
/// update check says on it and its update.
pub(super) fn advanced_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let (w, h) = (dz(1000.0), dz(570.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Advanced settings");
    let updating = super::hero::job_view(d, super::LAUNCHER_JOB).is_some();
    let mut ry = y;
    k.caption(x, ry, "LAUNCHER CHANNEL");
    ry += dz(34.0);
    ry += k.caps_text(
        x,
        ry,
        cw,
        "Which releases the launcher updates to. Beta and alpha get main's releases too; \
         back on main, main's newest launcher is offered again.",
        16.0,
        design::BODY,
        0.0,
    ) + dz(18.0);
    let indent = dz(39.0);
    for c in Channel::ALL {
        let on = d.state.launcher_channel == c;
        if k.radio(
            &format!("channel-{}", c.name()),
            on,
            c.label(),
            x,
            ry,
            !updating,
            "",
        ) {
            d.state.launcher_channel = c;
            d.save();
            d.check_launcher_update(ctx);
        }
        let nh = k.caps_text(
            x + indent,
            ry + dz(38.0),
            cw - indent,
            c.note(),
            14.0,
            design::GREY,
            0.0,
        );
        ry += dz(38.0) + nh + dz(22.0);
    }
    let (status, action) = update_status(d);
    let g = k.label_galley(&status, design::din(15.0), design::GREY, cw);
    k.put(x, ry + dz(6.0), g);

    let by = bottom - BTN_H;
    let close_w = k.button_width("Close", None, BTN_H).max(110.0);
    let right = x + cw;
    if k.button(
        "advanced-close",
        right - close_w,
        by,
        close_w,
        BTN_H,
        Tone::Dark,
        None,
        "Close",
        true,
        "",
    )
    .clicked
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        d.overlay = None;
        return;
    }
    if let Some((label, tip)) = action {
        let icon =
            matches!(d.launcher_update, LauncherUpdate::Available(_)).then_some(Icon::Download);
        let aw = k.button_width(label, icon, BTN_H).max(150.0);
        if k.button(
            "advanced-update",
            right - close_w - 8.0 - aw,
            by,
            aw,
            BTN_H,
            Tone::Blue,
            icon,
            label,
            true,
            &tip,
        )
        .clicked
        {
            update_action(d, ctx);
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

/// Finds the logs and asks which go (`upload_card`), saying what that shares and with whom.
pub(super) fn ask_upload(d: &mut Dashboard) {
    // Uploads are for players signed in with EchoVRCE (the service checks it).
    if !d.demo && upload_token(d).is_none() {
        d.page = super::Page::EchoVrce;
        d.dialogs.info(
            "Sign in with EchoVRCE first",
            "Logs can be uploaded by players signed in with EchoVRCE: sign in here, then upload them again from Settings.",
        );
        return;
    }
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
    d.overlay = Some(setup::Overlay::UploadLogs { off: Vec::new() });
}

/// The order of the upload card's rows.
const UPLOAD_ROWS: [logs::Kind; 5] = [
    logs::Kind::Echo,
    logs::Kind::Launcher,
    logs::Kind::Plugin,
    logs::Kind::EchoXr,
    logs::Kind::Quest,
];

/// UPLOAD LOGS: a check for each kind of log found (all on), what goes, and what that
/// shares.
pub(super) fn upload_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let sources = &d.upload_sources;
    let Some(setup::Overlay::UploadLogs { off }) = &mut d.overlay else {
        return;
    };
    let (w, h) = (dz(1000.0), dz(580.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Upload logs");
    let mut ry = y;
    ry += k.caps_text(
        x,
        ry,
        cw,
        "Choose which logs go. All of them help the most.",
        17.0,
        design::BODY,
        0.0,
    ) + dz(18.0);
    for kind in UPLOAD_ROWS {
        let of_kind: Vec<&logs::Source> = sources.iter().filter(|s| s.kind == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        let mut on = !off.contains(&kind);
        let names: Vec<&str> = of_kind.iter().map(|s| s.name.as_str()).collect();
        let bytes: u64 = of_kind.iter().map(|s| s.bytes).sum();
        let size = if bytes < 100_000 {
            format!("{} KB", bytes.div_ceil(1000))
        } else {
            format!("{:.1} MB", bytes as f64 / 1_000_000.0)
        };
        let label = format!(
            "{} ({} {}, {size})",
            kind.title(),
            of_kind.len(),
            if of_kind.len() == 1 { "file" } else { "files" },
        );
        if k.check(
            &format!("upload-{}", kind.title()),
            &mut on,
            &label,
            x,
            ry,
            true,
            &names.join("\n"),
        ) {
            off.retain(|o| *o != kind);
            if !on {
                off.push(kind);
            }
        }
        ry += dz(36.0);
    }
    let chosen: Vec<logs::Source> = sources
        .iter()
        .filter(|s| !off.contains(&s.kind))
        .cloned()
        .collect();
    ry += dz(8.0);
    let goes = if chosen.is_empty() {
        "Nothing is checked.".to_string()
    } else {
        what_goes(&chosen)
    };
    ry += k.caps_text(x, ry, cw, &goes, 16.0, design::TEXT, 0.0) + dz(18.0);
    k.caps_text(x, ry, cw, UPLOAD_TEXT, 14.0, design::GREY, 0.0);

    let by = bottom - BTN_H;
    let cancel_w = k.button_width("Cancel", None, BTN_H).max(110.0);
    let up_w = k.button_width("Upload", None, BTN_H).max(140.0);
    let right = x + cw;
    if k.button(
        "upload-cancel",
        right - cancel_w,
        by,
        cancel_w,
        BTN_H,
        Tone::Dark,
        None,
        "Cancel",
        true,
        "",
    )
    .clicked
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        d.overlay = None;
        d.upload_sources.clear();
        return;
    }
    if k.button(
        "upload-go",
        right - cancel_w - 8.0 - up_w,
        by,
        up_w,
        BTN_H,
        Tone::Go,
        None,
        "Upload",
        !chosen.is_empty(),
        "Send the checked logs",
    )
    .clicked
    {
        d.overlay = None;
        d.upload_sources = chosen;
        upload(d, ctx);
    }
}

/// CREDITS: each part of the launcher and what it brings along, with who made it, its
/// licence and a link; Echo VR's owners under it.
pub(super) fn credits_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let (w, h) = (dz(1100.0), dz(860.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Credits");
    let note_h: f32 = k
        .caps_block(super::CREDITS_NOTE, 14.0, design::GREY, cw)
        .iter()
        .map(|g| g.size().y)
        .sum();
    let by = bottom - BTN_H;
    let list_h = by - dz(16.0) - note_h - dz(12.0) - y;
    let link_w = k
        .link_width("GitHub", 13.5)
        .max(k.link_width("Website", 13.5))
        .max(k.link_width("Credits", 13.5));
    // Each entry: its name and licence, who made it, what it is.
    let heights: Vec<f32> = super::CREDITS
        .iter()
        .map(|c| {
            dz(30.0)
                + k.caps_block(
                    &format!("by {}", c.by),
                    15.0,
                    design::TEXT,
                    cw - link_w - dz(16.0),
                )
                .iter()
                .map(|g| g.size().y)
                .sum::<f32>()
                + dz(4.0)
                + k.caps_block(c.what, 14.0, design::BODY, cw)
                    .iter()
                    .map(|g| g.size().y)
                    .sum::<f32>()
                + dz(22.0)
        })
        .collect();
    let content: f32 = heights.iter().sum();
    let Some(setup::Overlay::Credits { scroll }) = &mut d.overlay else {
        return;
    };
    k.scroll_area("credits", x, y, cw + dz(16.0), list_h, content, scroll);
    let mut cy = y - *scroll;
    let mut open = None;
    k.clipped(x - dz(4.0), y, cw + dz(8.0), list_h, |k| {
        for (c, eh) in super::CREDITS.iter().zip(&heights) {
            if cy + eh >= y && cy <= y + list_h {
                let name = k.label_galley(c.name, design::din(19.0), design::TEXT, cw);
                let nr = k.put(x, cy, name);
                if !c.licence.is_empty() {
                    let lic = k.label_galley(c.licence, design::din(13.0), design::GREY, cw);
                    k.put(
                        nr.max.x - k.origin.x + dz(12.0),
                        nr.center().y - k.origin.y - lic.size().y / 2.0,
                        lic,
                    );
                }
                let link = if c.url.contains("github.com") {
                    "GitHub"
                } else {
                    "Website"
                };
                if k.link(
                    &format!("credit-{}", c.name),
                    x + cw - k.link_width(link, 13.5),
                    cy,
                    link,
                    13.5,
                    c.url,
                )
                .clicked
                {
                    open = Some(c.url);
                }
                if !c.credits.is_empty()
                    && k.link(
                        &format!("credits-{}", c.name),
                        x + cw - k.link_width("Credits", 13.5),
                        cy + dz(30.0),
                        "Credits",
                        13.5,
                        c.credits,
                    )
                    .clicked
                {
                    open = Some(c.credits);
                }
                let mut ty = cy + dz(30.0);
                ty += k.caps_text(
                    x,
                    ty,
                    cw - link_w - dz(16.0),
                    &format!("by {}", c.by),
                    15.0,
                    design::TEXT,
                    0.0,
                ) + dz(4.0);
                k.caps_text(x, ty, cw, c.what, 14.0, design::BODY, 0.0);
            }
            cy += eh;
        }
    });
    if let Some(url) = open {
        platform::open_url(url);
    }
    k.caps_text(
        x,
        by - dz(12.0) - note_h,
        cw,
        super::CREDITS_NOTE,
        14.0,
        design::GREY,
        0.0,
    );
    let notices_w = k.button_width("Licences", None, BTN_H).max(140.0);
    if k.button(
        "credits-notices",
        x,
        by,
        notices_w,
        BTN_H,
        Tone::Dark,
        None,
        "Licences",
        true,
        "The third-party licences of what the launcher is built with",
    )
    .clicked
    {
        platform::open_url(&format!(
            "{}/blob/main/THIRD_PARTY_NOTICES.txt",
            env!("CARGO_PKG_REPOSITORY")
        ));
    }
    let close_w = k.button_width("Close", None, BTN_H).max(110.0);
    if k.button(
        "credits-close",
        x + cw - close_w,
        by,
        close_w,
        BTN_H,
        Tone::Dark,
        None,
        "Close",
        true,
        "",
    )
    .clicked
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        d.overlay = None;
    }
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
        s(logs::Kind::Plugin, "nevr-20261004-171702.jsonl", 820_000),
        s(logs::Kind::Plugin, "NvrAssetPatches.log", 6_000),
    ]
}

/// Sends the logs the card left checked, in the background.
fn upload(d: &mut Dashboard, ctx: &egui::Context) {
    if d.demo || d.uploading_logs {
        return;
    }
    let Some(token) = upload_token(d) else {
        ask_upload(d);
        return;
    };
    d.uploading_logs = true;
    d.notify("Uploading your logs…");
    let sources = std::mem::take(&mut d.upload_sources);
    let versions = d.state.versions.clone();
    d.worker.spawn(ctx, move |tx| {
        let who = logs::Uploader {
            token,
            oid: logs::find_oid(&versions),
        };
        let r = logs::bundle(&sources)
            .and_then(|b| logs::upload(b, &who))
            .map_err(|e| format!("{e:#}"));
        tx.send(Msg::LogsUploaded(r));
    });
}

/// The EchoVRCE session's token while signed in (kept renewed by the EchoVRCE page).
fn upload_token(d: &Dashboard) -> Option<String> {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    d.vrce.account.as_ref()?;
    d.vrce
        .tokens
        .as_ref()
        .filter(|t| t.expires().is_none_or(|e| e > now + 30))
        .map(|t| t.token.clone())
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
            "It sends 8 log files (7.1 MB): from the launcher (1), EchoXR (2), Echo VR (3) and nEVR and its plugins (2)."
        );
        assert_eq!(
            what_goes(&demo_sources()[..1]),
            "It sends 1 log file (0.4 MB): from the launcher (1)."
        );
    }
}
