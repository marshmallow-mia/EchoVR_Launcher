//! "Go back": when an update breaks something, the version before, part by part
//! ([`crate::core::launcher::goback`]). One card lists everything that can go back (the
//! launcher, each version's game update, each mod, each plugin) and the versions skipped;
//! a plugin's menu and a version's MANAGE menu go back from where the update was. A
//! version release.echovr.de pulled is told at once and offered the way back (or, when it
//! says so, taken back without asking).

use super::{setup, Dashboard, JobKind, JobResult, Msg, Res};
use crate::core::launcher::goback::{self as core, Affected, Part, WayBack};
use crate::ui::design::{self, dz};
use crate::ui::dialogs::{Answer, Icon as DlgIcon};
use crate::ui::kit::Kit;
use crate::ui::widgets::{Tone, BTN_H};

const ASK_KEY: &str = "goback-ask";

/// What the card and the menus know.
#[derive(Default)]
pub(super) struct GoBack {
    /// Every way back there is, as last read.
    pub ways: Vec<WayBack>,
    /// The ones release.echovr.de pulled (the last full update check's).
    pub pulled: Vec<Affected>,
    /// The pulled ones told about already (by key and version).
    pub told: std::collections::BTreeSet<String>,
    /// The way back asked about, until the answer.
    pub asked: Option<WayBack>,
    /// The launcher's update running is a way back (its version isn't noted as the one
    /// to go back to).
    pub launcher_back: bool,
}

/// Reads the ways back again (on a worker).
pub(super) fn refresh(d: &mut Dashboard, ctx: &egui::Context) {
    if d.demo {
        return;
    }
    let state = d.state.clone();
    d.worker.spawn(ctx, move |tx| {
        tx.send(Msg::WaysBack(core::ways_back(&state)))
    });
}

/// Opens the card.
pub(super) fn open(d: &mut Dashboard, ctx: &egui::Context) {
    refresh(d, ctx);
    d.overlay = Some(setup::Overlay::GoBack { scroll: 0.0 });
}

/// The way back of `part`, if it has one.
pub(super) fn way_of<'a>(d: &'a Dashboard, part: &Part) -> Option<&'a WayBack> {
    d.goback.ways.iter().find(|w| w.part == *part)
}

/// Asks before going back.
pub(super) fn ask(d: &mut Dashboard, w: WayBack) {
    let pulled = d
        .goback
        .pulled
        .iter()
        .find(|a| a.way.part == w.part)
        .map(|a| format!("{}\n\n", a.note))
        .unwrap_or_default();
    let restart = if w.part == Part::Launcher {
        " The launcher restarts into it."
    } else {
        ""
    };
    let msg = format!(
        "{pulled}{} goes back from {} to {}.{restart}\n\n{} isn't offered again; a newer version is. \
         \"Use newest\" (Settings → Go back) gets it anyway.",
        w.name, w.now, w.before, w.now
    );
    d.goback.asked = Some(w);
    d.dialogs.options(
        ASK_KEY,
        "Go back to the version before?",
        &msg,
        DlgIcon::Warning,
        &["Go back", "Cancel"],
    );
}

/// Every frame: the answer to [`ask`], and pulled versions not told about yet.
pub(super) fn answers(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(answer) = d.dialogs.take(ASK_KEY) {
        if let (Answer::Button(0), Some(w)) = (answer, d.goback.asked.take()) {
            run(d, ctx, w);
        }
    }
    let new: Vec<Affected> = d
        .goback
        .pulled
        .iter()
        .filter(|a| !d.goback.told.contains(&told_key(a)))
        .cloned()
        .collect();
    for a in new {
        d.goback.told.insert(told_key(&a));
        let line = format!("{} {} has a serious problem", a.way.name, a.way.now);
        if a.auto && d.busy_any().is_none() && !d.game().is_running() {
            d.notify(&format!("{line}: going back to {}", a.way.before));
            run(d, ctx, a.way.clone());
        } else {
            d.notify(&format!("{line}: Settings → Go back"));
            if d.overlay.is_none() {
                open(d, ctx);
            }
        }
        if d.state.desktop_notifications {
            crate::core::desktop_notify::show(&line, &a.note);
        }
    }
}

fn told_key(a: &Affected) -> String {
    format!("{}={}", a.way.part.key(), a.way.now)
}

/// Goes back.
fn run(d: &mut Dashboard, ctx: &egui::Context, w: WayBack) {
    if w.part == Part::Launcher {
        // Held first: the launcher restarts at the end.
        d.state.holds.insert(w.part.key(), w.now.clone());
        d.state.launcher_previous = None;
        d.save();
        d.goback.launcher_back = true;
        d.install_launcher(ctx, w.before.clone(), "Going back to launcher");
        return;
    }
    let res = match &w.part {
        Part::Game { version } | Part::Mod { version, .. } => vec![Res::Version(version.clone())],
        _ => vec![Res::Mods],
    };
    let state = d.state.clone();
    let title = format!("Going back: {}", w.name);
    let job = format!("goback-{}", w.part.key());
    d.start_job(
        ctx,
        JobKind::GoBack,
        &job,
        res,
        &title,
        "Putting the version before back...",
        move |cancel, on| match core::go_back(&state, &w.part, cancel, on) {
            Ok((key, from)) => JobResult::WentBack {
                key,
                from,
                text: format!("{} is back at {}", w.name, w.before),
            },
            Err(e) => super::versions::job_err(e, "Couldn't go back"),
        },
    );
}

/// A way back is done: what it went back from is held.
pub(super) fn went_back(
    d: &mut Dashboard,
    ctx: &egui::Context,
    key: String,
    from: String,
    text: &str,
) {
    // A version's game files are the update's before it now: "Use newest" offers it again.
    if let Some(id) = key.strip_prefix("game:") {
        if let Some(v) = d.state.versions.iter_mut().find(|v| v.id == id) {
            crate::core::updates::after_going_back(v);
        }
    }
    d.state.holds.insert(key, from);
    d.save();
    d.goback
        .pulled
        .retain(|a| !d.state.holds.contains_key(&a.way.part.key()));
    d.notify(text);
    d.mods.changed();
    d.plugins.reload();
    d.updates.check_soon();
    refresh(d, ctx);
}

/// Lets go of a hold: the newest version is offered again.
fn use_newest(d: &mut Dashboard, ctx: &egui::Context, key: &str) {
    d.state.holds.remove(key);
    d.save();
    if key == Part::Launcher.key() {
        d.check_launcher_update(ctx);
    }
    d.updates.check_soon();
    d.notify("The newest version is offered again");
}

/// What a hold is about, for the card.
fn hold_label(d: &Dashboard, key: &str, value: &str) -> String {
    let version_name = |id: &str| {
        d.state
            .versions
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.name.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let mut parts = key.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("launcher"), ..) => format!("Echo VR Launcher {value}"),
        (Some("game"), Some(id), _) => {
            format!("The game update you went back from ({})", version_name(id))
        }
        (Some("mod"), Some(id), Some(file)) => format!("{file} {value} ({})", version_name(id)),
        (Some("plugin"), Some(id), _) => {
            let name = d
                .plugins
                .list
                .iter()
                .find(|p| p.page.id == id)
                .map(|p| p.page.name.clone())
                .unwrap_or_else(|| id.to_string());
            format!("The {name} plugin {value}")
        }
        _ => key.to_string(),
    }
}

/// The card: every way back, and the versions skipped.
pub(super) fn card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let (w, h) = (dz(1100.0), dz(760.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Go back to an earlier version");
    let mut top = y;
    top += k.caps_text(
        x,
        top,
        cw,
        "When an update breaks something, put back the version you had before: each part on its own. \
         The version you went back from isn't offered again until a newer one is out.",
        16.0,
        design::BODY,
        0.0,
    ) + dz(18.0);
    let by = bottom - BTN_H;
    let list_h = by - dz(16.0) - top;
    let busy = d.busy_any();
    let ways = d.goback.ways.clone();
    let holds: Vec<(String, String)> = d
        .state
        .holds
        .iter()
        .map(|(key, v)| (key.clone(), hold_label(d, key, v)))
        .collect();
    let btn_w = k.button_width("Use newest", None, BTN_H).max(150.0);
    let text_w = cw - btn_w - dz(20.0);
    let row_h = dz(84.0);
    let content = row_h * (ways.len() + holds.len()) as f32
        + if holds.is_empty() { 0.0 } else { dz(44.0) }
        + if ways.is_empty() { dz(60.0) } else { 0.0 };
    let Some(setup::Overlay::GoBack { scroll }) = &mut d.overlay else {
        return;
    };
    k.scroll_area("goback", x, top, cw + dz(16.0), list_h, content, scroll);
    let mut ry = top - *scroll;
    let mut clicked: Option<WayBack> = None;
    let mut newest: Option<String> = None;
    let pulled = d.goback.pulled.clone();
    k.clipped(x - dz(4.0), top, cw + dz(8.0), list_h, |k| {
        if ways.is_empty() {
            k.caps_text(
                x,
                ry,
                cw,
                "Nothing to go back to: no part has an earlier version (or release.echovr.de can't be reached now).",
                15.0,
                design::GREY,
                0.0,
            );
            ry += dz(60.0);
        }
        for way in &ways {
            let name = k.label_galley(&way.name, design::din(19.0), design::TEXT, text_w);
            k.put(x, ry, name);
            let note = pulled.iter().find(|a| a.way.part == way.part);
            let mut sub = format!("Now {} · goes back to {}", way.now, way.before);
            // A kept copy goes back at once (since the update of that day); one from the
            // history downloads.
            if way.server && way.part != Part::Launcher {
                sub.push_str(&match way.size {
                    Some(n) if n >= 1_000_000 => format!(" · downloads {:.1} MB", n as f64 / 1e6),
                    Some(n) => format!(" · downloads {} KB", n.div_ceil(1000)),
                    None => " · downloads it again".into(),
                });
            } else if let Some(at) = way.at.as_deref().and_then(|a| a.get(..10)) {
                sub.push_str(&format!(" · updated {at}"));
            }
            let (sub, color) = match note {
                Some(a) => (format!("{sub}\nPulled: {}", a.note), design::DANGER),
                None => (sub, design::GREY),
            };
            k.caps_text(x, ry + dz(30.0), text_w, &sub, 14.0, color, 0.0);
            let tone = if note.is_some() {
                Tone::Danger
            } else {
                Tone::Blue
            };
            if k.button(
                &format!("goback-{}", way.part.key()),
                x + cw - btn_w,
                ry + dz(6.0),
                btn_w,
                BTN_H,
                tone,
                None,
                "Go back",
                busy.is_none(),
                busy.as_deref().unwrap_or(""),
            )
            .clicked
            {
                clicked = Some(way.clone());
            }
            ry += row_h;
        }
        if !holds.is_empty() {
            k.caption(x, ry + dz(8.0), "VERSIONS SKIPPED");
            ry += dz(44.0);
        }
        for (key, label) in &holds {
            let g = k.label_galley(label, design::din(17.0), design::TEXT, text_w);
            k.put(x, ry, g);
            k.caps_text(
                x,
                ry + dz(28.0),
                text_w,
                "Not offered as an update. A newer version is.",
                14.0,
                design::GREY,
                0.0,
            );
            if k.button(
                &format!("goback-newest-{key}"),
                x + cw - btn_w,
                ry + dz(4.0),
                btn_w,
                BTN_H,
                Tone::Dark,
                None,
                "Use newest",
                true,
                "Offer this version as an update again",
            )
            .clicked
            {
                newest = Some(key.clone());
            }
            ry += row_h;
        }
    });
    if let Some(w) = clicked {
        ask(d, w);
    }
    if let Some(key) = newest {
        use_newest(d, ctx, &key);
    }
    let close_w = k.button_width("Close", None, BTN_H).max(110.0);
    if k.button(
        "goback-close",
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
        || k.key(egui::Key::Escape)
    {
        d.overlay = None;
    }
}
