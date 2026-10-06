//! Mods page: the selected PC version's mod loader (nEVR runtime, the game's
//! `BugSplat64.dll`) and what it did at the last start, its plugins (on or off, their
//! arguments, the asset patches of NvrAssetPatches) and, in the right-hand card, the
//! additional plugins that can still be installed. Everything is read on a worker (`core::launcher::mods`); changes go into the
//! launcher's own files, never into the community update's.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use egui::Color32;

use super::{
    hero, panel, plugin_form, setup, versions, Dashboard, JobKind, JobResult, Msg, Page,
    SnapVariant,
};
use crate::core::launcher::catalog::Platform;
use crate::core::launcher::mods::{
    self, AssetPatch, Loader, ModCatalog, ModEntry, ModView, Plugin, PluginStatus, Source, Status,
};
use crate::core::launcher::plugin_settings;
use crate::core::launcher::store::{InstalledVersion, Target};
use crate::core::platform;
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::style::Icon;
use crate::ui::widgets::{MenuItem, Tone, BTN_H};

// Geometry in design pixels: under the header strip, as the EchoVRCE page's cards.
const LOADER: Dr = Dr::new(137.0, 156.0, 1146.0, 262.0);
const PLUGINS: Dr = Dr::new(137.0, 448.0, 1146.0, 598.0);
const CATALOGUE: Dr = Dr::new(1318.0, 156.0, 555.0, 890.0);
/// A plugin's row, an asset patch's, an argument's.
const ROW_H: f32 = 68.0;
/// Inside a plugin's tile, and between two tiles.
const TILE_PAD: f32 = 12.0;
const TILE_GAP: f32 = 12.0;
const SUB_H: f32 = 40.0;
const ARG_H: f32 = 46.0;
/// Where a row's text starts: right of its checkbox.
const INDENT: f32 = 39.0;
/// A plugin row's checkbox (its name line), and the line under the name.
const NAME_Y: f32 = 12.0;
const DETAIL_Y: f32 = 48.0;
/// The buttons on a row's name line, and the gap between its buttons and chips.
const ROW_BTN_H: f32 = 33.0;
const ACTION_GAP: f32 = 12.0;
/// A catalogue entry's gap to the next.
const ENTRY_GAP: f32 = 26.0;
/// How often the selected version's mods are read again while the page is open.
const READ_EVERY: Duration = Duration::from_secs(3);

const ADD_KEY: &str = "mods-add-dll";
const REMOVE_KEY: &str = "mods-remove";

/// The page's own state.
#[derive(Default)]
pub(super) struct Mods {
    /// The selected version's mods: whose, what, and when read (`None`: stale).
    view: Option<(String, ModView, Option<Instant>)>,
    reading: bool,
    /// Bumped by every change, so a read that started before it is dropped.
    gen: u64,
    catalog: Option<ModCatalog>,
    catalog_loading: bool,
    list_scroll: f32,
    catalog_scroll: f32,
    /// The plugin whose options are open, with its arguments as typed.
    editing: Option<Editing>,
    /// The plugin "Remove" asks about.
    removing: Option<String>,
}

struct Editing {
    file: String,
    rows: Vec<(String, String)>,
    /// The plugin's default arguments: a row that differs gets a reset.
    defaults: BTreeMap<String, String>,
}

impl Editing {
    /// `p`'s arguments as they are, to edit.
    fn of(p: &Plugin) -> Editing {
        Editing {
            file: p.file.clone(),
            rows: p.arg_strings().into_iter().collect(),
            defaults: p.default_strings(),
        }
    }
}

impl Mods {
    /// A read finished: kept unless something changed since it started.
    pub(super) fn read_done(&mut self, gen: u64, id: String, view: ModView) {
        self.reading = false;
        if gen == self.gen {
            self.view = Some((id, view, Some(Instant::now())));
        }
    }

    pub(super) fn catalog_done(&mut self, c: ModCatalog) {
        self.catalog_loading = false;
        self.catalog = Some(c);
    }

    /// Something changed the files: read them again.
    pub(super) fn changed(&mut self) {
        self.gen += 1;
        if let Some(v) = &mut self.view {
            v.2 = None;
        }
    }

    /// Changes the shown view right away (the files are read again after).
    fn edit(&mut self, f: impl FnOnce(&mut ModView)) {
        if let Some((_, v, _)) = &mut self.view {
            f(v);
        }
        self.changed();
    }
}

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    if d.demo && d.mods.view.is_none() {
        d.mods = demo(d.snap_variant);
        let open = match d.snap_variant {
            Some(SnapVariant::ModsSettings) => Some((crate::core::echoxr_hands::PLUGIN, false)),
            Some(SnapVariant::ModsSettingsAll) => Some(("CombatStats.dll", true)),
            _ => None,
        };
        if let Some((file, advanced)) = open {
            open_sheet(d, file);
            if let Some(setup::Overlay::PluginSettings(sheet)) = &mut d.overlay {
                sheet.form.advanced = advanced;
            }
        }
    }
    if !kit.ghost {
        answers(d, ctx);
    }
    let Some(v) = version(d) else {
        return not_available(d, kit, None);
    };
    if v.publisher_lock.is_some() || !v.bin_dir().ends_with("win10") {
        return not_available(d, kit, Some(&v));
    }
    if !kit.ghost && !d.demo {
        refresh(d, ctx, &v);
    }
    let view = d
        .mods
        .view
        .as_ref()
        .filter(|(id, ..)| *id == v.id)
        .map(|(_, view, _)| view.clone());
    let (half, dy) = (kit.dx(), kit.dy());
    loader_card(d, kit, &v, view.as_ref(), LOADER.wider(half));
    plugins_card(
        d,
        kit,
        ctx,
        &v,
        view.as_ref(),
        PLUGINS.wider(half).taller(dy),
    );
    panel::at_right(kit, |k| {
        catalogue_card(d, k, ctx, &v, view.as_ref(), CATALOGUE.taller(dy))
    });
}

/// The version mods are shown for: the one PLAY starts, when installed.
fn version(d: &Dashboard) -> Option<InstalledVersion> {
    match d.target() {
        Target::Installed(v) => Some(v),
        _ => None,
    }
}

/// Reads the version's mods again when due, and the catalogue once.
fn refresh(d: &mut Dashboard, ctx: &egui::Context, v: &InstalledVersion) {
    let m = &mut d.mods;
    let due = match &m.view {
        Some((id, _, Some(at))) => *id != v.id || at.elapsed() >= READ_EVERY,
        _ => true,
    };
    if due && !m.reading {
        m.reading = true;
        let (gen, v) = (m.gen, v.clone());
        d.worker.spawn(ctx, move |tx| {
            tx.send(Msg::ModView(gen, v.id.clone(), mods::read(&v)));
        });
        ctx.request_repaint_after(READ_EVERY);
    }
    if d.mods.catalog.is_none() && !d.mods.catalog_loading {
        d.mods.catalog_loading = true;
        d.worker
            .spawn(ctx, |tx| tx.send(Msg::ModCatalog(ModCatalog::load())));
    }
}

/// The answers to this page's dialogs.
fn answers(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(a) = d.dialogs.take(ADD_KEY) {
        if a.is_yes() {
            add_from_disk(d, ctx);
        }
    }
    if let Some(a) = d.dialogs.take(REMOVE_KEY) {
        let file = d.mods.removing.take();
        if let (Some(file), true, Some(v)) = (file, a.is_yes(), version(d)) {
            let title = format!("Removing {file}");
            run(d, ctx, &v, &title, move |v, _, _| {
                mods::remove(v, &file).map(|()| format!("{file} is removed"))
            });
        }
    }
}

/// Runs `f` on `v` as a job; its text goes into the status bar when it's done.
fn run(
    d: &mut Dashboard,
    ctx: &egui::Context,
    v: &InstalledVersion,
    title: &str,
    f: impl FnOnce(
            &InstalledVersion,
            &std::sync::atomic::AtomicBool,
            &mut dyn FnMut(crate::core::launcher::versions::Step),
        ) -> anyhow::Result<String>
        + Send
        + 'static,
) {
    let v = v.clone();
    d.start_job(
        ctx,
        JobKind::Mods,
        &job_id(&v),
        title,
        "Starting...",
        move |cancel, on| match f(&v, cancel, on) {
            Ok(notice) => JobResult::ModsChanged(notice),
            Err(e) => versions::job_err(e, "Mods"),
        },
    );
}

fn job_id(v: &InstalledVersion) -> String {
    format!("mods:{}", v.id)
}

/// Why `v`'s plugin files can't be changed now (`None`: they can).
fn busy(d: &Dashboard, v: &InstalledVersion) -> Option<&'static str> {
    if d.jobs.contains_key(&job_id(v)) || d.jobs.contains_key(&v.id) {
        return Some("Wait for the running job to finish");
    }
    d.files_in_use(v)
}

/// "Add DLL" was confirmed: pick the file, then copy it in as a job.
fn add_from_disk(d: &mut Dashboard, ctx: &egui::Context) {
    let Some(v) = version(d) else {
        return;
    };
    let Some(src) = rfd::FileDialog::new()
        .add_filter("Plugins", &["dll"])
        .pick_file()
    else {
        return;
    };
    run(d, ctx, &v, "Adding a plugin", move |v, _, _| {
        mods::add_local(v, &src).map(|f| format!("{f} is added: it loads at the next start"))
    });
}

/// A version where mods don't apply: none installed, or an event build.
fn not_available(d: &mut Dashboard, kit: &mut Kit, v: Option<&InstalledVersion>) {
    let r = LOADER.wider(kit.dx());
    let title = if v.is_some() {
        "Event build"
    } else {
        "No PC version yet"
    };
    let (x, y, w, bottom) = hero::card_frame(kit, r, title);
    let text = match v {
        Some(v) => format!(
            "{} is an event build: it runs EchoRelay's patch where the mod loader would be, so it has no mods.\n\nChoose the live build on the Play page to see its mods.",
            v.name
        ),
        None if d.platform == Platform::Quest => "Mods are for Echo VR on this PC, and PLAY starts the Quest's now. Switch to PCVR (next to PLAY) to see the PC version's mods.".into(),
        None => "Mods are for Echo VR on this PC: install the live build first, then choose its mods here.".into(),
    };
    kit.caps_text(x, y, w, &text, 16.0, design::BODY, dz(14.0));
    if v.is_none() {
        let bw = kit
            .button_width("Install", Some(Icon::Download), BTN_H)
            .max(dz(200.0));
        if kit
            .button(
                "mods-go-install",
                x,
                bottom - BTN_H,
                bw,
                BTN_H,
                Tone::Go,
                Some(Icon::Download),
                "Install",
                true,
                "Open the Install page",
            )
            .clicked
        {
            d.page = Page::Install;
        }
    }
}

// ---- the loader ----

/// MOD LOADER: what is in the slot, what it did at the last start, and the switch for
/// starting without mods.
fn loader_card(
    d: &mut Dashboard,
    kit: &mut Kit,
    v: &InstalledVersion,
    view: Option<&ModView>,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(kit, r, "Mod loader");
    kit.caption(x, y, &format!("For {}", v.name));
    let Some(view) = view else {
        kit.caps_text(x, y + dz(34.0), w, "Reading…", 16.0, design::BODY, 0.0);
        return;
    };
    let (title, tag, color) = match &view.loader {
        Loader::Nevr { version } => (
            format!("nEVR runtime {}", short_version(version)),
            "Active",
            design::QUEST_ON,
        ),
        Loader::None => ("No mod loader".into(), "Not installed", design::GREY),
        Loader::Unknown => ("Unknown BugSplat64.dll".into(), "Not ours", design::DANGER),
    };
    let ty = y + dz(30.0);
    let g = kit.label_galley(&title, design::din(26.0), design::TEXT, w * 0.6);
    let tr = kit.put(x, ty, g);
    let tag_x = tr.max.x - kit.origin.x + dz(18.0);
    kit.dot_tag(tag_x, tr.center().y - kit.origin.y, tag, 14.0, color);
    // Left: the main action (or the switch); right: the folders.
    let by = bottom - BTN_H;
    // The text above them, a size smaller when it would reach them (a narrow card, with
    // the rail unfolded, and a long text: a path, a failed plugin, your own config).
    let text = loader_text(view);
    let (text_y, room) = (ty + dz(46.0), by - ty - dz(46.0) - dz(6.0));
    let height = |size: f32| -> f32 {
        let g = kit.caps_block(&text, size, design::BODY, w);
        g.iter().map(|g| g.size().y).sum::<f32>() + dz(8.0) * g.len().saturating_sub(1) as f32
    };
    let size = [15.5, 14.5, 13.5, 12.5]
        .into_iter()
        .find(|&s| height(s) <= room)
        .unwrap_or(12.5);
    kit.caps_text(x, text_y, w, &text, size, design::BODY, dz(8.0));
    match &view.loader {
        Loader::Nevr { .. } => {
            let mut off = !view.enabled;
            if kit.check(
                "mods-off",
                &mut off,
                "Start without mods",
                x,
                by + dz(5.0),
                true,
                "Echo VR starts with only the plugins it needs (the required ones, with their required patches). Turn it off again to have your mods back.",
            ) {
                write(d, v, |v| mods::set_enabled(v, !off), |m| m.enabled = !off);
            }
        }
        // The community update brings it: nothing to do here.
        Loader::None => {}
        Loader::Unknown => {}
    }
    let bin = v.bin_dir();
    let folders = [
        (
            "mods-logs",
            "Logs",
            mods::log_dir(&bin),
            "Open the plugins' logs (nEVR's own are in %LOCALAPPDATA%\\EchoVR\\logs)",
        ),
        (
            "mods-folder",
            "Mods folder",
            mods::plugins_dir(v),
            "Open the game's plugins folder, where the mods are",
        ),
    ];
    let mut right = x + w;
    for (key, label, dir, tip) in folders {
        let bw = kit
            .button_width(label, Some(Icon::Folder), BTN_H)
            .max(dz(150.0));
        right -= bw;
        if kit
            .button(
                key,
                right,
                by,
                bw,
                BTN_H,
                Tone::Dark,
                Some(Icon::Folder),
                label,
                true,
                tip,
            )
            .clicked
        {
            open_dir(d, &dir);
        }
        right -= dz(14.0);
    }
}

/// "4.0.0" of "4.0.0+182.e418eaa" (or of "4.0.0-182-ge418eaa-dirty").
fn short_version(v: &str) -> &str {
    v.split(['+', '-']).next().unwrap_or(v)
}

/// What the loader card says under its title.
fn loader_text(view: &ModView) -> String {
    let last = view.status.as_ref().map(last_start);
    match &view.loader {
        Loader::Nevr { .. } if view.stray_dbgcore => "There is a dbgcore.dll beside the game (the old loader's place): nEVR won't start with it, so PLAY takes it out first.".into(),
        Loader::Nevr { .. } if view.shadowed.is_some() => format!(
            "nEVR reads {} instead of the launcher's config.yaml, so the choices here don't apply: delete it to have them back.",
            view.shadowed.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
        ),
        Loader::Nevr { .. } if !view.enabled => {
            "Mods are off: Echo VR starts with only the required plugins.".into()
        }
        Loader::Nevr { version } => {
            let config = if view.game_config.is_some() {
                " The game reads your own _local/config.json."
            } else {
                ""
            };
            let about = format!("Discord sign-in, friends and parties in the game, and the mods below. Build {version}.{config}");
            match last {
                Some(l) => format!("{l} {about}"),
                None => format!("It hasn't started yet: what it loads shows here after you PLAY. {about}"),
            }
        }
        Loader::None => "Echo VR runs without plugins for now. nEVR runtime (Discord sign-in, friends and parties in the game, and plugins) comes with the community update, the next time this version updates.".into(),
        Loader::Unknown => "BugSplat64.dll here is neither the game's nor nEVR runtime. Verify on the Install page puts the right one back.".into(),
    }
}

/// "Last start 2026-10-04 18:04: 3 mods loaded, 1 failed."
fn last_start(s: &Status) -> String {
    // nEVR names its log by the local time it started: 2026-10-04T18-04-05.123.
    let when = s
        .started
        .split_once('T')
        .filter(|(day, time)| day.len() == 10 && time.len() >= 5)
        .map(|(day, time)| format!(" {day} {}", time[..5].replace('-', ":")))
        .unwrap_or_default();
    let count = |st: &str| s.plugins.iter().filter(|p| p.status == st).count();
    let (loaded, failed, skipped) = (count("loaded"), count("failed"), count("skipped"));
    if s.plugins.is_empty() && s.complete {
        return format!("Last start{when}: no mods.");
    }
    let mut parts = vec![format!(
        "{loaded} mod{} loaded",
        if loaded == 1 { "" } else { "s" }
    )];
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    format!("Last start{when}: {}.", parts.join(", "))
}

// ---- the plugins ----

/// PLUGINS: each plugin with its switch, where it's from, what happened at the last start,
/// its options, and NvrAssetPatches' asset patches under it.
fn plugins_card(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    v: &InstalledVersion,
    view: Option<&ModView>,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(kit, r, "Installed mods");
    let editable = view.is_some_and(|m| m.loader.editable());
    let busy = busy(d, v);
    // "Add DLL" on the title's row.
    let label = "Add DLL";
    let bw = kit
        .button_width(label, Some(Icon::Folder), BTN_H)
        .max(dz(160.0));
    // Plugins from disk aren't verified: only with local plugins on.
    let local = view.is_some_and(|v| v.local_plugins);
    let tip = match (editable, busy, local) {
        (false, ..) => "Needs the mod loader (nEVR)",
        (true, Some(why), _) => why,
        (true, None, false) => "Local plugins are off: x-local-plugins: true in this version's _local/config.yaml turns them on (see docs/plugins/local-plugins.md)",
        (true, None, true) => "Add a plugin DLL from your computer",
    };
    if kit
        .button(
            "mods-add",
            x + w - bw,
            dz(r.y + 16.0),
            bw,
            BTN_H,
            Tone::Dark,
            Some(Icon::Folder),
            label,
            editable && busy.is_none() && local,
            tip,
        )
        .clicked
    {
        d.dialogs.confirm(
            ADD_KEY,
            "Add a plugin",
            "A plugin is a program that runs inside Echo VR with the same rights as the game and you: add only DLLs from people you trust.\n\nIt is copied into the game's plugins folder and loads at the next start.",
            DlgIcon::Warning,
        );
    }
    let Some(view) = view else {
        return;
    };
    let rows = rows(view, d.mods.editing.as_ref(), vr_installed(d));
    if rows.is_empty() {
        kit.caps_text(
            x,
            y,
            w,
            "No mods here. The community update brings the asset patches and EchoRelay's patch; More mods has more, and Add DLL takes one from your computer.",
            15.5,
            design::BODY,
            0.0,
        );
        return;
    }
    let locked = !editable || !view.enabled;
    let current = matches!(view.loader, Loader::Nevr { .. });
    let names = names(d.mods.catalog.as_ref());
    // Each plugin (with its options and asset patches) in a tile of its own.
    let pad = dz(TILE_PAD);
    let (ix, iw) = (x + pad, w - 2.0 * pad);
    let heights: Vec<f32> = rows
        .iter()
        .map(|r| r.height() + extra_height(d, kit, r, iw))
        .collect();
    let (tops, tiles, content) = tile_layout(&rows, &heights);
    let list_h = bottom - y;
    kit.scroll_area(
        "mods-list",
        x,
        y,
        w + dz(16.0),
        list_h,
        content,
        &mut d.mods.list_scroll,
    );
    let scroll = d.mods.list_scroll;
    let top = y - scroll;
    kit.clipped(x - dz(12.0), y, w + dz(24.0), list_h, |k| {
        for &(ty, th) in &tiles {
            if top + ty + th >= y && top + ty <= y + list_h {
                group_tile(k, x, top + ty, w, th);
            }
        }
        let (x, w) = (ix, iw);
        for ((row, &h), &ry) in rows.iter().zip(&heights).zip(&tops) {
            let ry = top + ry;
            if ry + h >= y && ry <= y + list_h {
                match row {
                    Row::Plugin(p) => {
                        let look = Look {
                            name: display_name(p, &names),
                            author: d
                                .mods
                                .catalog
                                .as_ref()
                                .and_then(|c| c.entry_for(&p.file))
                                .map(|m| m.author.clone())
                                .filter(|a| !a.is_empty()),
                            update: update_for(p, d.mods.catalog.as_ref()),
                            editable,
                            locked,
                            current,
                            busy,
                            held: !p.verified && !view.local_plugins,
                        };
                        plugin_row(d, k, v, p, &look, x, ry, w)
                    }
                    Row::Vr(VrPart::EchoXr, _) => echoxr_row(d, k, v, x, ry, w),
                    Row::Vr(VrPart::Hands, p) => hands_row(d, k, v, *p, x, ry, w),
                    Row::Assets(on) => assets_row(d, k, v, *on, x, ry, editable && !locked),
                    Row::Asset(a, all_on) => {
                        asset_row(d, k, v, a, x, ry, editable && !locked && *all_on)
                    }
                    Row::Options(n) => options_rows(d, k, ctx, v, x, ry, w, *n),
                }
            }
        }
    });
}

/// Pure: where each row goes (its top, from the list's), the tiles around each plugin
/// with what belongs to it (top, height), and the list's height. A plugin or a VR part
/// starts a tile; its options and asset patches go in the same one.
fn tile_layout(rows: &[Row], heights: &[f32]) -> (Vec<f32>, Vec<(f32, f32)>, f32) {
    let (pad, gap) = (dz(TILE_PAD), dz(TILE_GAP));
    let mut tops = Vec::with_capacity(rows.len());
    let mut tiles: Vec<(f32, f32)> = Vec::new();
    let mut y = 0.0;
    for (row, &h) in rows.iter().zip(heights) {
        if matches!(row, Row::Plugin(_) | Row::Vr(..)) || tiles.is_empty() {
            if let Some(last) = tiles.last_mut() {
                last.1 = y + pad - last.0;
                y += pad + gap;
            }
            tiles.push((y, 0.0));
            y += pad;
        }
        tops.push(y);
        y += h;
    }
    if let Some(last) = tiles.last_mut() {
        last.1 = y + pad - last.0;
        y += pad;
    }
    (tops, tiles, y)
}

/// A plugin's tile: the cards' faint tile with a thin rim, so plugins read apart.
fn group_tile(k: &Kit, x: f32, y: f32, w: f32, h: f32) {
    hero::tile(k, x, y, w, h, false);
    k.ui.painter().rect_stroke(
        k.rect(x, y, w, h),
        dz(6.0),
        egui::Stroke::new(1.0, Color32::from_white_alpha(22)),
        egui::StrokeKind::Inside,
    );
}

/// The catalogue's names of the plugin files it lists (lowercase file name to name).
fn names(c: Option<&ModCatalog>) -> std::collections::HashMap<String, String> {
    c.into_iter()
        .flat_map(|c| &c.mods)
        .map(|m| (m.file.to_ascii_lowercase(), m.name.clone()))
        .collect()
}

/// A plugin's name as the page shows it: the catalogue's, else its own (words, not
/// `snake_case`).
fn display_name(p: &Plugin, names: &std::collections::HashMap<String, String>) -> String {
    names
        .get(&p.file.to_ascii_lowercase())
        .cloned()
        .unwrap_or_else(|| p.name.replace('_', " "))
}

/// The catalogue's newer version of a plugin it installed (`None`: none, or not from it).
fn update_for(p: &Plugin, c: Option<&ModCatalog>) -> Option<ModEntry> {
    let Source::Catalog { version, .. } = &p.source else {
        return None;
    };
    c?.entry_for(&p.file)
        .filter(|m| m.downloadable() && m.version != *version)
        .cloned()
}

/// Which VR parts are installed, so listed with the plugins: EchoXR while it's how VR
/// plays (always on Linux; on Windows instead of Revive), EchoXR Hands while it's on.
fn vr_installed(d: &Dashboard) -> VrInstalled {
    use crate::core::launcher::store::SteamVrVia;
    if !(cfg!(any(windows, target_os = "linux")) || d.demo) {
        return VrInstalled::default();
    }
    VrInstalled {
        echoxr: (cfg!(target_os = "linux") && !d.demo)
            || d.state.profile.steamvr_via == SteamVrVia::EchoXr,
        // On, and really there (a failed download once marked it on).
        hands: d.state.echoxr_hands && (d.demo || crate::core::echoxr_hands::is_fetched()),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct VrInstalled {
    echoxr: bool,
    hands: bool,
}

/// How a plugin's row is drawn.
struct Look {
    name: String,
    /// Who made it (the catalogue's word).
    author: Option<String>,
    /// The catalogue's newer version of it.
    update: Option<ModEntry>,
    /// The launcher can change the mods (nEVR is there).
    editable: bool,
    /// Mods are off, or can't be changed: the switches don't work.
    locked: bool,
    /// nEVR (its log tells what happened at the last start).
    current: bool,
    busy: Option<&'static str>,
    /// Not verified, and local plugins are off: it isn't loaded.
    held: bool,
}

/// Why a plugin that isn't verified doesn't load, and how to change that.
const HELD_TIP: &str = "Not from the community update or the catalogue: it loads only with local plugins on (x-local-plugins: true in this version's _local/config.yaml, see docs/plugins/local-plugins.md)";

/// The VR parts above the plugins: not nEVR's, the launcher puts them in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VrPart {
    /// EchoXR's OpenXR layer: Linux's VR (always), Windows' SteamVR instead of Revive.
    EchoXr,
    /// EchoXR Hands: its plugin (nEVR's view of it, once there) and finger bridge.
    Hands,
}

enum Row<'a> {
    Vr(VrPart, Option<&'a Plugin>),
    Plugin(&'a Plugin),
    /// The asset patches' own switch (on or off).
    Assets(bool),
    /// An asset patch, and whether they're on at all.
    Asset(&'a AssetPatch, bool),
    /// The options of the plugin above, with this many arguments.
    Options(usize),
}

impl Row<'_> {
    fn height(&self) -> f32 {
        match self {
            Row::Plugin(_) | Row::Vr(..) => dz(ROW_H),
            Row::Assets(_) | Row::Asset(..) => dz(SUB_H),
            Row::Options(n) => dz(ARG_H) * (*n as f32 + 1.0) + dz(12.0),
        }
    }
}

/// The list's rows, only what is installed: the VR parts in use, then each plugin, its
/// options when open, and the asset patches under NvrAssetPatches.
fn rows<'a>(view: &'a ModView, editing: Option<&Editing>, vr: VrInstalled) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    let hands = |p: &&Plugin| {
        p.file
            .eq_ignore_ascii_case(crate::core::echoxr_hands::PLUGIN)
    };
    if vr.echoxr {
        out.push(Row::Vr(VrPart::EchoXr, None));
    }
    if vr.hands {
        out.push(Row::Vr(VrPart::Hands, view.plugins.iter().find(hands)));
    }
    // Hand tracking's plugin is its row above, not one of the plugins.
    for p in view.plugins.iter().filter(|p| !hands(p)) {
        out.push(Row::Plugin(p));
        if let Some(e) = editing.filter(|e| e.file.eq_ignore_ascii_case(&p.file)) {
            out.push(Row::Options(e.rows.len()));
        }
        let assets = p.file.eq_ignore_ascii_case(mods::ASSET_PLUGIN)
            && p.present
            && !view.asset_patches.is_empty();
        if assets {
            out.push(Row::Assets(view.assets_enabled));
            out.extend(
                view.asset_patches
                    .iter()
                    .map(|a| Row::Asset(a, view.assets_enabled)),
            );
        }
    }
    out
}

/// A plugin's chip: only what needs looking at (`None`: all is well, or nothing to say
/// yet). Loaded, off (its check shows that) and not started yet say nothing; the loader's
/// card sums up the last start.
fn state(p: &Plugin) -> Option<(&'static str, Color32)> {
    if !p.present {
        return Some(("Missing", design::RED));
    }
    if !p.enabled {
        return None;
    }
    match p.status.as_ref()?.status.as_str() {
        "loaded" => None,
        "skipped" => Some(("Skipped", design::QUEST_WARN)),
        _ => Some(("Failed", design::RED)),
    }
}

/// The grey line under a plugin's name.
fn detail(p: &Plugin) -> String {
    let error = p
        .status
        .as_ref()
        .filter(|s| !s.loaded() && p.enabled && !s.error.is_empty())
        .map(|s: &PluginStatus| s.error.clone());
    if let Some(e) = error {
        return format!("{}  ·  {e}", p.file);
    }
    let version = (!p.version.is_empty()).then(|| format!("v{}", p.version));
    [version, Some(p.file.clone())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

#[allow(clippy::too_many_arguments)]
fn plugin_row(
    d: &mut Dashboard,
    k: &mut Kit,
    v: &InstalledVersion,
    p: &Plugin,
    look: &Look,
    x: f32,
    y: f32,
    w: f32,
) {
    let Look {
        editable,
        locked,
        busy,
        ..
    } = *look;
    let key = |what: &str| format!("mods-{what}-{}", p.file);
    // Right to left: Update, Remove, Options, the state chip.
    let mut a = Actions::new(x, y, w);
    if let Some(m) = &look.update {
        let tip = match (editable, busy) {
            (false, _) => "Needs the mod loader (nEVR) in this version",
            (true, Some(why)) => why,
            (true, None) => "Download its new version (it loads at the next start)",
        };
        if a.button(
            k,
            &key("update"),
            Tone::Go,
            Some(Icon::Download),
            "Update",
            editable && busy.is_none(),
            tip,
        ) {
            let ctx = k.ui.ctx().clone();
            get(d, &ctx, v, m);
        }
    }
    if p.removable() {
        let tip = busy.unwrap_or("Delete this plugin from the game's folder");
        if a.button(
            k,
            &key("remove"),
            Tone::Dark,
            None,
            "Remove",
            editable && busy.is_none(),
            tip,
        ) {
            d.mods.removing = Some(p.file.clone());
            d.dialogs.confirm_danger(
                REMOVE_KEY,
                "Remove plugin",
                &format!("Delete {} ({}) from {}?", look.name, p.file, v.name),
                "Remove",
            );
        }
    }
    if p.present && p.settings.is_some() {
        if a.button(
            k,
            &key("settings"),
            Tone::Dark,
            None,
            "Settings",
            true,
            "What can be set for this plugin",
        ) {
            open_sheet(d, &p.file);
        }
        row_settings(d, k, v, p, &mut a, editable && busy.is_none());
    } else if p.present {
        let open = d
            .mods
            .editing
            .as_ref()
            .is_some_and(|e| e.file.eq_ignore_ascii_case(&p.file));
        let tone = if open { Tone::Blue } else { Tone::Dark };
        let tip = if editable {
            "The arguments nEVR hands this plugin"
        } else {
            "Needs the mod loader (nEVR)"
        };
        if a.button(k, &key("options"), tone, None, "Arguments", editable, tip) {
            d.mods.editing = if open { None } else { Some(Editing::of(p)) };
        }
    }
    // What needs looking at: only nEVR says what it loaded.
    if look.held {
        a.chip(k, "Not loaded", design::QUEST_WARN);
    } else if let Some((chip, color)) = state(p).filter(|_| look.current) {
        a.chip(k, chip, color);
    }

    // Left: the switch with the name, its source tag; the details under it.
    let mut on = p.enabled;
    let can = editable && !locked && p.present && !p.required && !look.held;
    let tip = if !editable {
        "Needs the mod loader (nEVR)"
    } else if look.held {
        HELD_TIP
    } else if p.required {
        "The game needs it: always on, also without mods"
    } else if locked {
        "Mods are off"
    } else if p.enabled {
        "Turn it off: it isn't loaded at the next start"
    } else {
        "Turn it on: it loads at the next start"
    };
    if k.check(&key("on"), &mut on, &look.name, x, y + dz(NAME_Y), can, tip) {
        let file = p.file.clone();
        write(
            d,
            v,
            |v| mods::set_plugin_enabled(v, &file, on),
            |m| {
                if let Some(q) = m.plugins.iter_mut().find(|q| q.file == file) {
                    q.enabled = on;
                }
            },
        );
    }
    let source = match &p.source {
        Source::Shipped if !p.verified => ("Unverified", design::QUEST_WARN),
        Source::Shipped => ("Community update", design::SUBTLE),
        Source::Catalog { .. } => ("Catalogue", design::BLUE),
        Source::Local => ("Your DLL", design::QUEST_WARN),
    };
    let tags = [
        Some(source),
        p.required.then_some(("Required", design::QUEST_ON)),
        p.changed().then_some(("Changed", design::QUEST_WARN)),
    ];
    name_tags(k, x, &a, &look.name, tags.into_iter().flatten());
    let line = plugin_detail(p, look.author.as_deref());
    let g = detail_lines(k, &line, w);
    k.put(x + dz(INDENT), y + dz(DETAIL_Y), g);
}

/// The middle of a row's name line (its checkbox is 24 high).
fn name_cy(y: f32) -> f32 {
    y + dz(NAME_Y) + 12.0
}

/// A row's buttons, chips and switches on its name line, right to left from its right
/// edge, each centred on the line.
struct Actions {
    /// Where the next one ends (the gap to the last one taken off).
    right: f32,
    cy: f32,
}

impl Actions {
    fn new(x: f32, y: f32, w: f32) -> Actions {
        Actions {
            right: x + w,
            cy: name_cy(y),
        }
    }

    /// A button as wide as its label (at least 110); returns the click.
    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        k: &mut Kit,
        key: &str,
        tone: Tone,
        icon: Option<Icon>,
        label: &str,
        enabled: bool,
        tip: &str,
    ) -> bool {
        let bh = dz(ROW_BTN_H);
        let bw = k.button_width(label, icon, bh).max(dz(110.0));
        self.right -= bw;
        let clicked = k
            .button(
                key,
                self.right,
                self.cy - bh / 2.0,
                bw,
                bh,
                tone,
                icon,
                label,
                enabled,
                tip,
            )
            .clicked;
        self.right -= dz(ACTION_GAP);
        clicked
    }

    fn chip(&mut self, k: &Kit, text: &str, color: Color32) {
        self.right -= k.chip_width(text);
        k.chip(self.right, self.cy - dz(13.5), text, color);
        self.right -= dz(ACTION_GAP);
    }

    /// A dropdown showing the current choice; returns the one picked.
    fn select(
        &mut self,
        k: &mut Kit,
        key: &str,
        choices: &[plugin_settings::Choice],
        value: &str,
        enabled: bool,
        tip: &str,
    ) -> Option<String> {
        let bh = dz(ROW_BTN_H);
        let label = choices
            .iter()
            .find(|c| c.value == value)
            .map_or(value, |c| c.label.as_str());
        let bw = (k.button_width(label, None, bh) + bh).max(dz(140.0));
        self.right -= bw;
        let picked = if enabled {
            let items: Vec<MenuItem> = choices
                .iter()
                .map(|c| MenuItem::Pick {
                    label: c.label.clone(),
                    detail: c.help.clone(),
                    detail_color: design::GREY,
                    checked: c.value == value,
                    tip: c.tooltip.clone(),
                })
                .collect();
            k.menu_button(
                key,
                label,
                &items,
                self.right,
                self.cy - bh / 2.0,
                bw,
                bh,
                tip,
            )
            .map(|i| choices[i].value.clone())
        } else {
            k.button(
                key,
                self.right,
                self.cy - bh / 2.0,
                bw,
                bh,
                Tone::Dark,
                None,
                label,
                false,
                tip,
            );
            None
        };
        self.right -= dz(ACTION_GAP);
        picked
    }

    /// A checkbox with its label; returns true when it was flipped.
    fn check(
        &mut self,
        k: &mut Kit,
        key: &str,
        on: &mut bool,
        label: &str,
        enabled: bool,
        tip: &str,
    ) -> bool {
        self.right -= k.check_width(label);
        let flipped = k.check(key, on, label, self.right, self.cy - 12.0, enabled, tip);
        self.right -= dz(ACTION_GAP);
        flipped
    }
}

/// The tags right of a row's name, as many as fit before its buttons.
fn name_tags<'a>(
    k: &Kit,
    x: f32,
    a: &Actions,
    name: &str,
    tags: impl IntoIterator<Item = (&'a str, Color32)>,
) {
    let name_w = k
        .label_galley(name, design::din(18.0), design::TEXT, f32::INFINITY)
        .size()
        .x;
    let mut tag_x = x + dz(INDENT) + name_w + dz(16.0);
    for (tag, color) in tags {
        let tw = k.dot_tag_width(tag, 12.0);
        if tag_x + tw >= a.right {
            break;
        }
        k.dot_tag(tag_x, a.cy, tag, 12.0, color);
        tag_x += tw + dz(14.0);
    }
}

/// A plugin's line under its name: its version and file, and who made it.
fn plugin_detail(p: &Plugin, author: Option<&str>) -> String {
    match author {
        Some(a) => format!("{}  ·  by {a}", detail(p)),
        None => detail(p),
    }
}

/// A VR part's row, as a plugin's: its name with tags, what it does under it. It is
/// listed while it is in use: on Linux EchoXR is how VR plays (always, required); on
/// Windows SteamVR plays through it instead of Revive, and Remove goes back to Revive (it
/// is in More mods again).
fn echoxr_row(d: &mut Dashboard, k: &mut Kit, v: &InstalledVersion, x: f32, y: f32, w: f32) {
    use crate::core::launcher::store::{Runtime, SteamVrVia};
    let linux = cfg!(target_os = "linux") && !d.demo;
    let name = "EchoXR";
    let steamvr = d.state.profile.runtime == Runtime::Revive;
    let busy = d.any_job();
    let tip = if linux {
        "Linux plays VR through it: always on"
    } else {
        "SteamVR plays through it instead of Revive (Remove goes back to Revive)"
    };
    let key = |what: &str| format!("mods-vr-{what}-{name}");
    let mut a = Actions::new(x, y, w);
    if !linux {
        let tip = if busy {
            "Wait until the job is done"
        } else {
            "SteamVR plays through Revive again (hand tracking, which needs EchoXR, goes too)"
        };
        if a.button(k, &key("remove"), Tone::Dark, None, "Remove", !busy, tip) {
            d.state.profile.steamvr_via = SteamVrVia::Revive;
            let hands_off = d.state.echoxr_hands;
            if hands_off {
                d.state.echoxr_hands = false;
                if let Err(e) = crate::core::echoxr_hands::remove_from(&v.bin_dir()) {
                    tracing::warn!("hand tracking: {e:#}");
                }
                d.mods.changed();
            }
            d.save();
            d.notify(if hands_off {
                "SteamVR plays through Revive now, without hand tracking (it needs EchoXR)"
            } else {
                "SteamVR plays through Revive now"
            });
        }
    }
    // No chip: PLAY prepares it whenever it isn't ready, nothing to look at.
    k.check(&key("on"), &mut true, name, x, y + dz(NAME_Y), false, tip);
    let tags = [
        Some(("VR", design::BLUE)),
        linux.then_some(("Required", design::QUEST_ON)),
        (!linux && !steamvr).then_some(("Used with SteamVR", design::QUEST_WARN)),
    ];
    name_tags(k, x, &a, name, tags.into_iter().flatten());
    let g = detail_lines(k, &echoxr_detail(linux), w);
    k.put(x + dz(INDENT), y + dz(DETAIL_Y), g);
}

/// EchoXR's line under its name.
fn echoxr_detail(linux: bool) -> String {
    use crate::core::echoxr;
    if linux {
        format!(
            "v{}  ·  by {}  ·  Echo VR in your headset on SteamVR or WiVRn",
            echoxr::VERSION,
            echoxr::AUTHORS
        )
    } else {
        format!(
            "v{}  ·  by {}  ·  SteamVR without Revive  ·  live build only",
            echoxr::VERSION,
            echoxr::AUTHORS
        )
    }
}

/// EchoXR Hands' line under its name.
fn hands_detail() -> String {
    use crate::core::echoxr_hands as hands;
    format!(
        "v{}  ·  by {}  ·  your own fingers, from OpenXR hand tracking (SteamVR, WiVRn)",
        hands::VERSION,
        hands::AUTHORS
    )
}

/// EchoXR Hands' row, while it is on (installed): what nEVR did with its plugin, finger
/// sharing, its settings and Remove (it goes back to More mods); what it
/// does under it.
#[allow(clippy::too_many_arguments)]
fn hands_row(
    d: &mut Dashboard,
    k: &mut Kit,
    v: &InstalledVersion,
    plugin: Option<&Plugin>,
    x: f32,
    y: f32,
    w: f32,
) {
    use crate::core::echoxr_hands as hands;
    let linux = cfg!(target_os = "linux") && !d.demo;
    let bin = v.bin_dir();
    let name = "EchoXR Hands";
    let busy = busy(d, v);
    let tip = "Your own fingers on Echo VR's hands, from your headset's hand tracking (Remove takes it out)";
    let key = |what: &str| format!("mods-vr-{what}-hands");
    let mut a = Actions::new(x, y, w);
    let installed = d.demo || hands::installed_in(&bin);
    let tip_remove = busy.unwrap_or("Take its plugin out of the game's plugins folder");
    if a.button(
        k,
        &key("remove"),
        Tone::Dark,
        None,
        "Remove",
        busy.is_none(),
        tip_remove,
    ) {
        d.state.echoxr_hands = false;
        d.save();
        match hands::remove_from(&bin) {
            Ok(()) => d.notify("EchoXR Hands is removed"),
            Err(e) => d.dialogs.error(
                "Couldn't remove EchoXR Hands",
                &format!("{e:#}"),
                Default::default(),
            ),
        }
        d.mods.changed();
    }
    // Its settings (from its description), finger sharing among them on the row.
    if let Some(p) = plugin.filter(|p| p.settings.is_some()) {
        if a.button(
            k,
            &key("settings"),
            Tone::Dark,
            None,
            "Settings",
            true,
            "EchoXR Hands' settings: they show in the game straight away",
        ) {
            open_sheet(d, &p.file);
        }
        row_settings(d, k, v, p, &mut a, busy.is_none());
    }
    // What needs looking at. Not in plugins/ yet, but downloaded: PLAY puts it there.
    let chip = match plugin {
        Some(p) if d.mods.view.is_some() => state(p),
        _ if installed || d.demo || hands::is_fetched() => None,
        _ => Some(("Not downloaded", design::QUEST_WARN)),
    };
    if let Some((chip, color)) = chip {
        a.chip(k, chip, color);
    }
    k.check(&key("on"), &mut true, name, x, y + dz(NAME_Y), false, tip);
    let tags = [
        Some(("VR", design::BLUE)),
        Some(("Needs EchoXR", design::SUBTLE)),
        linux.then_some(("Experimental", design::QUEST_WARN)),
    ];
    name_tags(k, x, &a, name, tags.into_iter().flatten());
    let g = detail_lines(k, &hands_detail(), w);
    k.put(x + dz(INDENT), y + dz(DETAIL_Y), g);
}

/// A row's line under its name in DMCAPS (file names keep their case), on as many lines
/// as a row `w` wide needs.
fn detail_lines(k: &Kit, text: &str, w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    design::caps_append(&mut job, text, 14.0, design::GREY, false);
    job.wrap.max_width = w - dz(INDENT);
    k.ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// How much taller than a one-line row `row` is, its line under the name wrapping.
fn extra_height(d: &Dashboard, k: &Kit, row: &Row, w: f32) -> f32 {
    let text = match row {
        Row::Plugin(p) => {
            let author = d
                .mods
                .catalog
                .as_ref()
                .and_then(|c| c.entry_for(&p.file))
                .map(|m| m.author.clone())
                .filter(|a| !a.is_empty());
            plugin_detail(p, author.as_deref())
        }
        Row::Vr(VrPart::EchoXr, _) => echoxr_detail(cfg!(target_os = "linux") && !d.demo),
        Row::Vr(VrPart::Hands, _) => hands_detail(),
        _ => return 0.0,
    };
    let g = detail_lines(k, &text, w);
    let one = g.rows.first().map_or(0.0, |r| r.height());
    (g.size().y - one).max(0.0)
}

/// The asset patches' own switch, under NvrAssetPatches.
fn assets_row(
    d: &mut Dashboard,
    k: &mut Kit,
    v: &InstalledVersion,
    on: bool,
    x: f32,
    y: f32,
    can: bool,
) {
    let mut on_now = on;
    let tip = if can {
        "Every optional asset patch on or off (the required ones stay on)"
    } else {
        "Turn NvrAssetPatches on first"
    };
    if k.check(
        "mods-assets",
        &mut on_now,
        "All optional asset patches",
        x + dz(INDENT),
        y + dz(8.0),
        can,
        tip,
    ) {
        write(
            d,
            v,
            |v| mods::set_asset_patch(v, None, on_now),
            |m| m.assets_enabled = on_now,
        );
    }
}

fn asset_row(
    d: &mut Dashboard,
    k: &mut Kit,
    v: &InstalledVersion,
    a: &AssetPatch,
    x: f32,
    y: f32,
    can: bool,
) {
    let mut on = a.enabled;
    let label = a.label.replace('_', " ");
    let key = format!("mods-asset-{}", a.label);
    let tip = if a.required {
        "The game needs it: always on"
    } else if can {
        "Takes effect at the next start"
    } else {
        ""
    };
    let lx = x + dz(INDENT * 2.0);
    let switched = k.check(
        &key,
        &mut on,
        &label,
        lx,
        y + dz(8.0),
        can && !a.required,
        tip,
    );
    if a.required {
        // Right of the check's label (box 16, gap 10, then the label in DIN 18).
        let g = k.label_galley(&label, design::din(18.0), design::TEXT, f32::INFINITY);
        let tx = lx + 26.0 + g.size().x + dz(14.0);
        k.dot_tag(tx, y + dz(8.0) + 12.0, "Required", 12.0, design::QUEST_ON);
    }
    if switched {
        let l = a.label.clone();
        write(
            d,
            v,
            |v| mods::set_asset_patch(v, Some(&l), on),
            |m| {
                if let Some(p) = m.asset_patches.iter_mut().find(|p| p.label == l) {
                    p.enabled = on;
                }
            },
        );
    }
}

/// A plugin's arguments: a key and a value field each, Add, Save and Cancel.
#[allow(clippy::too_many_arguments)]
fn options_rows(
    d: &mut Dashboard,
    k: &mut Kit,
    _ctx: &egui::Context,
    v: &InstalledVersion,
    x: f32,
    y: f32,
    w: f32,
    n: usize,
) {
    let Some(e) = &mut d.mods.editing else {
        return;
    };
    let fx = x + dz(INDENT);
    let fw = w - dz(INDENT);
    let fh = dz(36.0);
    let kw = fw * 0.35;
    let close = dz(40.0);
    let vw = fw - kw - 2.0 * close - dz(30.0);
    let mut gone = None;
    let mut reset = None;
    let defaults = e.defaults.clone();
    for (i, (key, value)) in e.rows.iter_mut().enumerate() {
        let ry = y + i as f32 * dz(ARG_H);
        k.field(
            &format!("mods-arg-k-{i}"),
            key,
            fx,
            ry,
            kw,
            fh,
            "name",
            false,
            "The argument's name, e.g. logging",
        );
        k.field(
            &format!("mods-arg-v-{i}"),
            value,
            fx + kw + dz(10.0),
            ry,
            vw,
            fh,
            defaults.get(key.trim()).map_or("value", String::as_str),
            false,
            "Its value (handed on as text)",
        );
        // Differs from its default (or has none): a reset.
        let default = defaults.get(key.trim());
        if default != Some(&*value)
            && k.button(
                &format!("mods-arg-r-{i}"),
                fx + fw - 2.0 * close - dz(10.0),
                ry,
                close,
                fh,
                Tone::Dark,
                Some(Icon::Refresh),
                "",
                true,
                match default {
                    Some(_) => "Back to its default",
                    None => "Not a default argument: take it out",
                },
            )
            .clicked
        {
            reset = Some(i);
        }
        if k.button(
            &format!("mods-arg-x-{i}"),
            fx + fw - close,
            ry,
            close,
            fh,
            Tone::Dark,
            Some(Icon::Close),
            "",
            true,
            "Take this argument out",
        )
        .clicked
        {
            gone = Some(i);
        }
    }
    if let Some(i) = reset {
        match defaults.get(e.rows[i].0.trim()) {
            Some(dv) => e.rows[i].1 = dv.clone(),
            None => {
                e.rows.remove(i);
            }
        }
    } else if let Some(i) = gone {
        e.rows.remove(i);
    }
    let by = y + n as f32 * dz(ARG_H);
    let bh = dz(32.0);
    let add_w = k.button_width("Add argument", None, bh).max(dz(170.0));
    if k.button(
        "mods-arg-add",
        fx,
        by,
        add_w,
        bh,
        Tone::Dark,
        None,
        "Add argument",
        true,
        "",
    )
    .clicked
    {
        e.rows.push((String::new(), String::new()));
    }
    let labels = ["Save", "Cancel"];
    let mut right = x + w;
    for (i, label) in labels.into_iter().enumerate().rev() {
        let bw = k.button_width(label, None, bh).max(dz(110.0));
        right -= bw;
        let tone = if i == 0 { Tone::Blue } else { Tone::Dark };
        if k.button(
            &format!("mods-arg-{label}"),
            right,
            by,
            bw,
            bh,
            tone,
            None,
            label,
            true,
            "",
        )
        .clicked
        {
            if i == 0 {
                let file = e.file.clone();
                let args: BTreeMap<String, String> = e
                    .rows
                    .iter()
                    .filter(|(key, _)| !key.trim().is_empty())
                    .map(|(key, value)| (key.trim().to_string(), value.clone()))
                    .collect();
                // nEVR reads ${…} as an environment variable: an unset one and no plugin
                // loads. The options stay open to change it.
                let env = args.iter().find(|(k, v)| {
                    crate::core::launcher::nevr::expands_env(k)
                        || crate::core::launcher::nevr::expands_env(v)
                });
                if let Some((key, _)) = env {
                    d.dialogs.error(
                        "Couldn't save the arguments",
                        &format!("{key}: nEVR reads ${{…}} as an environment variable, and when that isn't set it loads no plugins at all. Leave it out."),
                        Default::default(),
                    );
                    return;
                }
                d.mods.editing = None;
                let shown = args.clone();
                write(
                    d,
                    v,
                    |v| mods::set_args(v, &file, &args),
                    |m| {
                        if let Some(p) = m.plugins.iter_mut().find(|p| p.file == file) {
                            p.args = shown
                                .into_iter()
                                .map(|(k, v)| (k, serde_json::Value::String(v)))
                                .collect();
                        }
                    },
                );
                d.notify("Saved: the plugin gets its arguments at the next start");
            } else {
                d.mods.editing = None;
            }
            return;
        }
        right -= dz(10.0);
    }
    // RESET TO DEFAULT: every argument back to the plugin's defaults, saved.
    let typed: BTreeMap<String, String> = e
        .rows
        .iter()
        .filter(|(key, _)| !key.trim().is_empty())
        .map(|(key, value)| (key.trim().to_string(), value.clone()))
        .collect();
    let label = "Reset to default";
    let bw = k
        .button_width(label, Some(Icon::Refresh), bh)
        .max(dz(190.0));
    if k.button(
        "mods-arg-reset",
        right - bw,
        by,
        bw,
        bh,
        Tone::Dark,
        Some(Icon::Refresh),
        label,
        typed != defaults,
        "Every argument back to the plugin's defaults",
    )
    .clicked
    {
        let file = e.file.clone();
        d.mods.editing = None;
        write(
            d,
            v,
            |v| mods::reset_args(v, &file),
            |m| {
                if let Some(p) = m.plugins.iter_mut().find(|p| p.file == file) {
                    p.args = p.defaults.clone();
                }
            },
        );
        d.notify("Reset: the plugin gets its default arguments at the next start");
    }
}

// ---- a plugin's settings (from its description) ----

/// The settings sheet of plugin `file`.
#[derive(Clone)]
pub(super) struct SettingsSheet {
    file: String,
    form: plugin_form::FormState,
}

const ACTION_KEY: &str = "mods-settings-action";

fn open_sheet(d: &mut Dashboard, file: &str) {
    d.overlay = Some(setup::Overlay::PluginSettings(Box::new(SettingsSheet {
        file: file.to_string(),
        form: Default::default(),
    })));
}

/// The shown plugin `file` of `v`.
fn shown_plugin(d: &Dashboard, v: &InstalledVersion, file: &str) -> Option<Plugin> {
    d.mods
        .view
        .as_ref()
        .filter(|(id, ..)| *id == v.id)
        .and_then(|(_, view, _)| {
            view.plugins
                .iter()
                .find(|p| p.file.eq_ignore_ascii_case(file))
        })
        .cloned()
}

/// Writes `changes` to plugin `p`'s settings, and shows them right away.
fn apply_settings(
    d: &mut Dashboard,
    v: &InstalledVersion,
    p: &Plugin,
    changes: Vec<(String, String)>,
) {
    let Some(sp) = &p.settings else {
        return;
    };
    if changes.is_empty() {
        return;
    }
    let settings = sp.settings.clone();
    let file = p.file.clone();
    write(
        d,
        v,
        |v| plugin_settings::write(v, &file, &settings, &changes),
        |m| {
            let Some(q) = m.plugins.iter_mut().find(|q| q.file == file) else {
                return;
            };
            for (key, value) in &changes {
                let args = settings
                    .field(key)
                    .map_or(&settings.store, |f| f.store(&settings))
                    == &plugin_settings::Store::Args;
                if args {
                    q.args
                        .insert(key.clone(), serde_json::Value::String(value.clone()));
                }
                if let Some(sp) = &mut q.settings {
                    if sp.values.contains_key(key) {
                        sp.values.insert(key.clone(), value.clone());
                    }
                }
            }
        },
    );
}

/// The settings a plugin shows on its row (right to left): its bools as checks, its
/// choices as dropdowns.
fn row_settings(
    d: &mut Dashboard,
    k: &mut Kit,
    v: &InstalledVersion,
    p: &Plugin,
    a: &mut Actions,
    enabled: bool,
) {
    let Some(sp) = &p.settings else {
        return;
    };
    let mut changes = Vec::new();
    for key in sp.settings.row.iter().rev() {
        let Some(f) = sp.settings.field(key) else {
            continue;
        };
        let value = sp.values.get(key).cloned().unwrap_or_default();
        let id = format!("mods-row-{}-{key}", p.file);
        let on = enabled && plugin_settings::holds(&f.enabled_if, &sp.values);
        match &f.kind {
            plugin_settings::Kind::Bool { on: yes, off } => {
                let mut b = plugin_form::is_on(f, &value);
                if a.check(k, &id, &mut b, &f.label, on, f.hover()) {
                    changes.push((key.clone(), if b { yes.clone() } else { off.clone() }));
                }
            }
            plugin_settings::Kind::Choice { choices } => {
                if let Some(c) = a.select(k, &id, choices, &value, on, f.hover()) {
                    changes.push((key.clone(), c));
                }
            }
            _ => {}
        }
    }
    apply_settings(d, v, p, changes);
}

/// A plugin's settings over the page: its description's form, whether the changes apply
/// now or at the next start, Show advanced, Reset all and Done.
pub(super) fn settings_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let Some(setup::Overlay::PluginSettings(sheet)) = &d.overlay else {
        return;
    };
    let file = sheet.file.clone();
    let p = version(d).and_then(|v| shown_plugin(d, &v, &file).map(|p| (v, p)));
    let Some((v, p, sp)) = p.and_then(|(v, p)| {
        let sp = p.settings.clone()?;
        Some((v, p, sp))
    }) else {
        d.overlay = None;
        return;
    };
    let s = sp.settings.clone();
    let title = if p
        .file
        .eq_ignore_ascii_case(crate::core::echoxr_hands::PLUGIN)
    {
        "EchoXR Hands".to_string()
    } else {
        display_name(&p, &names(d.mods.catalog.as_ref()))
    };
    // Arguments need the mod loader; a file of its own is the plugin's.
    let editable = d
        .mods
        .view
        .as_ref()
        .is_some_and(|(_, m, _)| m.loader.editable());
    let needs_loader = s
        .fields()
        .any(|f| f.has_value() && *f.store(&s) == plugin_settings::Store::Args);
    let why_not =
        busy(d, &v).or((needs_loader && !editable).then_some("Needs the mod loader (nEVR)"));
    let enabled = why_not.is_none() && !d.demo;

    let (w, h) = (dz(1180.0), dz(900.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, &format!("{title} settings"));
    let foot = bottom - BTN_H;
    let (top, list_bottom) = (y, foot - dz(20.0));
    let Some(setup::Overlay::PluginSettings(sheet)) = &mut d.overlay else {
        return;
    };
    let st = &mut sheet.form;
    let content = plugin_form::height(k, &s, &sp.values, st, cw);
    k.scroll_area(
        "mods-settings-scroll",
        x,
        top,
        cw + dz(18.0),
        list_bottom - top,
        content,
        &mut st.scroll,
    );
    let scroll = st.scroll;
    let out = k.clipped(x - dz(6.0), top, cw + dz(12.0), list_bottom - top, |k| {
        plugin_form::form(
            k,
            "mods-settings",
            &s,
            &sp.values,
            st,
            x,
            top - scroll,
            cw,
            (top, list_bottom),
            enabled,
        )
    });
    let mut changes = out.changes;
    if let Some((label, question)) = out.confirm {
        d.dialogs
            .confirm(ACTION_KEY, &label, &question, DlgIcon::Warning);
    }

    // The foot, under a line: Show advanced, when changes apply; Reset all and Done.
    k.ui.painter().hline(
        (k.origin.x + x)..=(k.origin.x + x + cw),
        k.origin.y + foot - dz(12.0),
        egui::Stroke::new(1.0, Color32::from_white_alpha(28)),
    );
    let Some(setup::Overlay::PluginSettings(sheet)) = &mut d.overlay else {
        return;
    };
    if s.has_advanced() {
        k.check(
            "mods-settings-advanced",
            &mut sheet.form.advanced,
            "Show advanced",
            x,
            foot + (BTN_H - 24.0) / 2.0,
            true,
            "Every setting, also the fine-tuning ones",
        );
    }
    let applies = match why_not {
        Some(why) => why.to_string(),
        None if s.all_live() => "Changes apply right away, also while you play.".into(),
        None if s.store.live() => {
            "Changes apply right away, those marked at the next start.".into()
        }
        None => "Changes apply at the next start.".into(),
    };
    let note_x = x + if s.has_advanced() {
        k.check_width("Show advanced") + dz(30.0)
    } else {
        0.0
    };
    let done_w = k.button_width("Done", None, BTN_H).max(dz(150.0));
    let reset_w = k.button_width("Reset all", None, BTN_H).max(dz(150.0));
    let right = x + cw;
    let g = k.label_galley(
        &applies,
        design::din(14.0),
        design::GREY,
        right - done_w - reset_w - dz(40.0) - note_x,
    );
    k.put(note_x, foot + (BTN_H - g.size().y) / 2.0, g);
    let typing = ctx.memory(|m| m.focused().is_some());
    if k.button(
        "mods-settings-done",
        right - done_w,
        foot,
        done_w,
        BTN_H,
        Tone::Blue,
        None,
        "Done",
        true,
        "",
    )
    .clicked
        || (!typing && ctx.input(|i| i.key_pressed(egui::Key::Escape)))
    {
        d.overlay = None;
    }
    let defaults = s.defaults();
    let differs = defaults
        .iter()
        .any(|(key, dv)| sp.values.get(key).is_some_and(|cur| cur != dv));
    if k.button(
        "mods-settings-reset",
        right - done_w - dz(12.0) - reset_w,
        foot,
        reset_w,
        BTN_H,
        Tone::Dark,
        None,
        "Reset all",
        enabled && differs,
        "Every setting back to its default",
    )
    .clicked
    {
        changes.extend(
            defaults
                .into_iter()
                .filter(|(key, dv)| sp.values.get(key).is_some_and(|cur| cur != dv)),
        );
    }
    if let Some(a) = d.dialogs.take(ACTION_KEY) {
        let pending = match &mut d.overlay {
            Some(setup::Overlay::PluginSettings(sheet)) => sheet.form.pending.take(),
            _ => None,
        };
        if a.is_yes() {
            changes.extend(pending.unwrap_or_default());
        }
    }
    apply_settings(d, &v, &p, changes);
}

/// Writes a change (`f`, small files: on this thread) and shows it at once (`show`), or
/// says why it couldn't be made.
fn write(
    d: &mut Dashboard,
    v: &InstalledVersion,
    f: impl FnOnce(&InstalledVersion) -> anyhow::Result<()>,
    show: impl FnOnce(&mut ModView),
) {
    if !d.demo {
        if let Err(e) = f(v) {
            d.dialogs.error(
                "Couldn't change the mods",
                &format!("{e:#}"),
                Default::default(),
            );
            return;
        }
    }
    d.mods.edit(show);
}

fn open_dir(d: &mut Dashboard, dir: &std::path::Path) {
    if d.demo {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = platform::open_folder(dir) {
        d.dialogs.error(
            "Couldn't open the folder",
            &format!("{}\n\n{e:#}", dir.display()),
            Default::default(),
        );
    }
}

// ---- more mods (the catalogue's, and the VR parts not in use) ----

/// What MORE MODS offers: the catalogue's plugins that aren't here (and aren't
/// required: those come with the update), and the VR parts not in use.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Extra<'a> {
    Mod(&'a ModEntry),
    /// EchoXR, on Windows while SteamVR plays through Revive.
    EchoXr,
    /// EchoXR Hands, while it's off.
    Hands,
}

impl Extra<'_> {
    /// How it is shown: as a catalogue entry.
    fn info(&self) -> ModEntry {
        match self {
            Extra::Mod(m) => (*m).clone(),
            Extra::EchoXr => ModEntry {
                id: "echoxr".into(),
                name: "EchoXR".into(),
                summary: "SteamVR plays Echo VR through EchoXR's OpenXR layer instead of Revive: no injection, no administrator rights. Live build only.".into(),
                author: crate::core::echoxr::AUTHORS.into(),
                version: crate::core::echoxr::VERSION.into(),
                homepage: "https://github.com/EchoTools/EchoXR".into(),
                capabilities: vec!["vr".into()],
                ..Default::default()
            },
            Extra::Hands => ModEntry {
                id: "echoxr-hands".into(),
                name: "EchoXR Hands".into(),
                summary: "Your own fingers on Echo VR's hands, from your headset's hand tracking (SteamVR, WiVRn). It needs EchoXR: getting it turns EchoXR on.".into(),
                author: crate::core::echoxr_hands::AUTHORS.into(),
                version: crate::core::echoxr_hands::VERSION.into(),
                homepage: "https://github.com/EchoTools/EchoXR-Hands".into(),
                capabilities: vec!["vr".into(), "network".into()],
                ..Default::default()
            },
        }
    }
}

/// The additional plugins for `view`: the VR parts not installed first, then the
/// catalogue's optional plugins that aren't in the game's folder.
fn extras<'a>(
    view: Option<&ModView>,
    catalog: &'a ModCatalog,
    vr: Option<VrInstalled>,
) -> Vec<Extra<'a>> {
    let mut out = Vec::new();
    if let Some(vr) = vr {
        if !vr.echoxr {
            out.push(Extra::EchoXr);
        }
        if !vr.hands {
            out.push(Extra::Hands);
        }
    }
    let here = |m: &ModEntry| {
        view.is_some_and(|view| {
            view.plugins
                .iter()
                .any(|p| p.present && p.file.eq_ignore_ascii_case(&m.file))
        })
    };
    out.extend(
        catalog
            .mods
            .iter()
            .filter(|m| !m.required && !m.shipped && !here(m))
            .map(Extra::Mod),
    );
    out
}

/// MORE MODS: what can still be installed, each with what it does and GET.
fn catalogue_card(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    v: &InstalledVersion,
    view: Option<&ModView>,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(kit, r, "More mods");
    let Some(catalog) = d.mods.catalog.clone() else {
        kit.caps_text(
            x,
            y,
            w,
            "Loading the mods catalogue…",
            15.5,
            design::BODY,
            0.0,
        );
        return;
    };
    let note = if catalog.builtin {
        "The built-in list: release.echovr.de couldn't be reached."
    } else {
        "From release.echovr.de. Every download is checked against its checksum, and again before every start."
    };
    let note_h = {
        let gs = kit.caps_block(note, 13.5, design::GREY, w);
        gs.iter().map(|g| g.size().y).sum::<f32>()
    };
    let list_h = bottom - y - note_h - dz(16.0);
    kit.caps_text(x, bottom - note_h, w, note, 13.5, design::GREY, 0.0);
    let vr = (cfg!(any(windows, target_os = "linux")) || d.demo).then(|| vr_installed(d));
    let extras = extras(view, &catalog, vr);
    if extras.is_empty() {
        kit.caps_text(
            x,
            y,
            w,
            "Everything available is installed: it's under Installed mods.",
            15.5,
            design::BODY,
            0.0,
        );
        return;
    }
    let editable = view.is_some_and(|m| m.loader.editable());
    let busy = busy(d, v);
    let infos: Vec<ModEntry> = extras.iter().map(Extra::info).collect();
    let heights: Vec<f32> = infos
        .iter()
        .map(|m| entry_height(kit, m, w) + dz(ENTRY_GAP))
        .collect();
    let content: f32 = heights.iter().sum();
    kit.scroll_area(
        "mods-catalogue",
        x,
        y,
        w + dz(16.0),
        list_h,
        content,
        &mut d.mods.catalog_scroll,
    );
    let mut ey = y - d.mods.catalog_scroll;
    kit.clipped(x - dz(4.0), y, w + dz(8.0), list_h, |k| {
        for ((e, m), h) in extras.iter().zip(&infos).zip(&heights) {
            if ey + h >= y && ey <= y + list_h {
                entry(d, k, ctx, v, *e, m, x, ey, w, editable, busy);
            }
            ey += h;
        }
    });
}

/// Downloads the catalogue's `m` into `v` as a job (new, or its new version).
fn get(d: &mut Dashboard, ctx: &egui::Context, v: &InstalledVersion, m: &ModEntry) {
    let entry = m.clone();
    let title = format!("Installing {}", m.name);
    run(d, ctx, v, &title, move |v, cancel, on| {
        mods::install(v, &entry, cancel, on)
            .map(|()| format!("{} is installed: it loads at the next start", entry.name))
    });
}

/// GET on an additional plugin.
fn get_extra(d: &mut Dashboard, ctx: &egui::Context, v: &InstalledVersion, e: Extra) {
    use crate::core::echoxr_hands as hands;
    use crate::core::launcher::store::SteamVrVia;
    match e {
        Extra::Mod(m) => get(d, ctx, v, m),
        Extra::EchoXr => {
            d.state.profile.steamvr_via = SteamVrVia::EchoXr;
            d.save();
            d.notify("SteamVR plays through EchoXR now");
        }
        Extra::Hands => {
            // On (and EchoXR with it) only once it is installed: a failed download leaves
            // it in More mods.
            let v = v.clone();
            d.start_job(
                ctx,
                JobKind::Mods,
                &job_id(&v),
                "Installing EchoXR Hands",
                "Starting...",
                move |cancel, on| match hands::fetch(cancel, on)
                    .and_then(|()| hands::install_into(&v.bin_dir()))
                {
                    Ok(()) => JobResult::HandsInstalled,
                    Err(e) => versions::job_err(e, "Mods"),
                },
            );
        }
    }
}

/// The meta line of a catalogue entry: author, size, what it does.
fn meta(m: &ModEntry) -> String {
    let author = (!m.author.is_empty()).then(|| format!("by {}", m.author));
    let size = m.size.map(|b| {
        if b >= 1_000_000 {
            format!("{:.1} MB", b as f64 / 1e6)
        } else {
            format!("{} KB", b.div_ceil(1000))
        }
    });
    let caps = (!m.capabilities.is_empty()).then(|| m.capabilities.join(", ").replace('-', " "));
    [author, size, caps]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

fn entry_height(k: &Kit, m: &ModEntry, w: f32) -> f32 {
    let block = |text: &str, size: f32, w: f32| -> f32 {
        k.caps_block(text, size, design::BODY, w)
            .iter()
            .map(|g| g.size().y)
            .sum()
    };
    dz(40.0)
        + block(&m.summary, 14.5, w)
        + dz(8.0)
        + block(&meta(m), 13.0, meta_room(k, m, w))
        + dz(8.0)
}

/// How wide the line with who made it may be: beside Source, when it has one.
fn meta_room(k: &Kit, m: &ModEntry, w: f32) -> f32 {
    if m.homepage.is_empty() {
        w
    } else {
        w - k.link_width("Source", 13.5) - dz(16.0)
    }
}

#[allow(clippy::too_many_arguments)]
fn entry(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    v: &InstalledVersion,
    e: Extra,
    m: &ModEntry,
    x: f32,
    y: f32,
    w: f32,
    editable: bool,
    busy: Option<&'static str>,
) {
    // Right: GET, or that it isn't out yet.
    let bh = dz(ROW_BTN_H);
    let mut right = x + w;
    let gettable = match e {
        Extra::Mod(m) => m.downloadable(),
        Extra::EchoXr | Extra::Hands => true,
    };
    if gettable {
        let label = "Get";
        let bw = k
            .button_width(label, Some(Icon::Download), bh)
            .max(dz(110.0));
        right -= bw;
        let why = match e {
            Extra::EchoXr if d.any_job() => Some("Wait until the job is done"),
            Extra::EchoXr => None,
            _ if !editable => Some("Needs the mod loader (nEVR) in this version"),
            _ => busy,
        };
        let tip = why.unwrap_or(match e {
            Extra::EchoXr => "SteamVR plays through EchoXR instead of Revive (PLAY prepares it)",
            Extra::Hands => "Download it into this version (it turns EchoXR on too)",
            Extra::Mod(_) => "Download it into this version (it loads at the next start)",
        });
        if k.button(
            &format!("mods-get-{}", m.id),
            right,
            y,
            bw,
            bh,
            Tone::Go,
            Some(Icon::Download),
            label,
            why.is_none(),
            tip,
        )
        .clicked
        {
            get_extra(d, ctx, v, e);
        }
    } else {
        let chip = "Coming soon";
        let cw = k.chip_width(chip);
        right -= cw;
        k.chip(right, y + (bh - dz(27.0)) / 2.0, chip, design::QUEST_OFF);
    }
    // Left: name and version; the summary; who made it and what it does.
    let name = k.label_galley(
        &m.name,
        design::din(19.0),
        design::TEXT,
        right - dz(14.0) - x,
    );
    let nr = k.put(x, y + dz(2.0), name);
    if !m.version.is_empty() {
        let vx = nr.max.x - k.origin.x + dz(10.0);
        let g = k.label_galley(
            &format!("v{}", m.version),
            design::din(14.0),
            design::GREY,
            f32::INFINITY,
        );
        if vx + g.size().x < right - dz(14.0) {
            k.put(vx, nr.center().y - k.origin.y - g.size().y / 2.0, g);
        }
    }
    let mut ty = y + dz(40.0);
    ty += k.caps_text(x, ty, w, &m.summary, 14.5, design::BODY, 0.0) + dz(8.0);
    // Who made it and what it does: on as many lines as it takes (a narrow card).
    k.caps_text(x, ty, meta_room(k, m, w), &meta(m), 13.0, design::GREY, 0.0);
    if !m.homepage.is_empty() {
        let lx = x + w - k.link_width("Source", 13.5);
        if k.link(
            &format!("mods-src-{}", m.id),
            lx,
            ty - dz(1.0),
            "Source",
            13.5,
            &m.homepage,
        )
        .clicked
            && !d.demo
        {
            platform::open_url(&m.homepage);
        }
    }
}

// ---- snapshots ----

/// A made-up description with every kind of control, for snapshots.
fn showcase() -> serde_json::Value {
    serde_json::json!({
        "store": { "kind": "args" },
        "row": ["panel"],
        "sections": [
            { "title": "Panel", "help": "What the wrist panel shows after a round.", "fields": [
                { "key": "panel", "type": "bool", "label": "Wrist panel", "default": true, "style": "switch",
                  "help": "Shows your round on your left wrist." },
                { "key": "compact", "type": "bool", "label": "Compact", "default": false },
                { "key": "show", "type": "multi", "label": "Show", "style": "chips",
                  "choices": ["damage", "eliminations", "accuracy"], "default": ["damage"] },
                { "key": "corner", "type": "choice", "label": "Corner", "style": "segmented", "default": "left",
                  "choices": [ { "value": "left", "label": "Left" }, { "value": "right", "label": "Right" } ] },
                { "key": "theme", "type": "choice", "label": "Theme", "default": "team",
                  "choices": [ { "value": "team", "label": "Team colours", "help": "Orange or blue" }, { "value": "mono", "label": "White" } ] },
                { "key": "tint", "type": "color", "label": "Accent", "default": "#ff8800" },
                { "key": "opacity", "type": "number", "label": "Opacity", "min": 0, "max": 100, "step": 5, "default": 80, "unit": "%", "style": "slider" },
                { "key": "rounds", "type": "number", "label": "Rounds kept", "integer": true, "min": 1, "max": 50, "default": 10, "style": "stepper" },
                { "key": "delay", "type": "number", "label": "Delay", "min": 0, "max": 10, "default": 2, "unit": "s",
                  "help": "Seconds after the round ends." }
            ]},
            { "title": "Upload", "fields": [
                { "key": "upload", "type": "choice", "label": "Upload rounds", "style": "radio", "default": "never",
                  "choices": [ { "value": "never", "label": "Never" }, { "value": "ranked", "label": "Ranked matches" }, { "value": "all", "label": "Every match" } ] },
                { "key": "server", "type": "text", "label": "Server", "format": "url", "placeholder": "https://stats.example",
                  "enabled_if": { "upload": "all" } },
                { "key": "token", "type": "secret", "label": "Token", "advanced": true },
                { "key": "hotkey", "type": "key", "label": "Show the panel", "default": "Ctrl+Shift+S" },
                { "type": "action", "label": "Clear saved rounds", "tone": "danger", "sets": { "clear": "1" },
                  "confirm": "Delete every round kept on this PC?" },
                { "type": "note", "text": "Rounds are kept in plugin_logs/combat_stats.", "advanced": true },
                { "type": "link", "label": "How it counts", "url": "https://example.com/combat-stats" }
            ]}
        ]
    })
}

/// Made-up mods for snapshots (`variant`: no loader yet, or a plugin's options open).
fn demo(variant: Option<SnapVariant>) -> Mods {
    let status = |file: &str, name: &str, version: &str, st: &str, error: &str| PluginStatus {
        file: file.into(),
        name: name.into(),
        version: version.into(),
        api: 5,
        status: st.into(),
        error: error.into(),
        ..Default::default()
    };
    let plugin = |file: &str, name: &str, source: Source, st: Option<PluginStatus>| Plugin {
        file: file.into(),
        name: name.into(),
        version: st.as_ref().map(|s| s.version.clone()).unwrap_or_default(),
        added: source != Source::Shipped,
        required: source == Source::Shipped,
        verified: source != Source::Local,
        source,
        enabled: true,
        args: Default::default(),
        defaults: Default::default(),
        present: true,
        status: st,
        settings: None,
    };
    let bare = variant == Some(SnapVariant::ModsNoLoader);
    let long = variant == Some(SnapVariant::ModsLongText);
    let mut plugins = vec![
        plugin(
            "NvrAssetPatches.dll",
            "NvrAssetPatches",
            Source::Shipped,
            Some(status(
                "NvrAssetPatches.dll",
                "NvrAssetPatches",
                "1.1.0",
                "loaded",
                "",
            )),
        ),
        plugin(
            "CombatStats.dll",
            "combat_stats",
            Source::Catalog {
                id: "combat-stats".into(),
                version: "0.3.0".into(),
            },
            Some(status(
                "CombatStats.dll",
                "combat_stats",
                "0.3.0",
                "loaded",
                "",
            )),
        ),
        plugin(
            "MyTweak.dll",
            "MyTweak",
            Source::Local,
            Some(status(
                "MyTweak.dll",
                "MyTweak",
                "",
                "failed",
                "missing NvrPluginGetInfo export",
            )),
        ),
    ];
    let hands = matches!(
        variant,
        Some(SnapVariant::ModsHands | SnapVariant::ModsSettings | SnapVariant::ModsSettingsAll)
    );
    if hands {
        let file = crate::core::echoxr_hands::PLUGIN;
        let mut p = plugin(
            file,
            "EchoXRHands",
            Source::Shipped,
            Some(status(file, "EchoXRHands", "0.4.0", "loaded", "")),
        );
        p.required = false;
        let (settings, origin) = plugin_settings::describe(file, None, None).expect("built in");
        let text = include_str!("../../core/launcher/testdata/EchoXRHands.txt");
        let files = BTreeMap::from([(
            "EchoXRHands.txt".to_string(),
            text.replace("Network = 0", "Network = 1"),
        )]);
        let values = plugin_settings::values_from(&settings, &Default::default(), &files);
        p.settings = Some(plugin_settings::PluginSettings {
            settings,
            origin,
            values,
        });
        plugins.push(p);
    }
    if variant == Some(SnapVariant::ModsSettingsAll) {
        let (settings, _) = plugin_settings::parse_settings(&showcase());
        let settings = std::sync::Arc::new(settings.expect("showcase"));
        let mut values = settings.defaults();
        values.insert("rounds".into(), "12".into());
        values.insert("tint".into(), "#3a7bff".into());
        values.insert("show".into(), "damage,accuracy".into());
        plugins[1].settings = Some(plugin_settings::PluginSettings {
            settings,
            origin: plugin_settings::Origin::Catalogue,
            values,
        });
    }
    plugins[0].args = serde_json::json!({ "logging": "normal" })
        .as_object()
        .cloned()
        .unwrap_or_default();
    plugins[0].defaults = plugins[0].args.clone();
    if long {
        plugins[1].status = Some(status(
            "CombatStats.dll",
            "combat_stats",
            "0.3.0",
            "skipped",
            "",
        ));
    }
    if bare {
        plugins.truncate(1);
        for p in &mut plugins {
            p.status = None;
            p.version.clear();
        }
    }
    let asset = |label: &str, enabled| AssetPatch {
        required: label.starts_with("netgun"),
        label: label.into(),
        enabled,
    };
    let view = ModView {
        local_plugins: variant != Some(SnapVariant::ModsLocked),
        loader: if bare {
            Loader::None
        } else {
            Loader::Nevr {
                version: "4.0.0+182.e418eaa".into(),
            }
        },
        enabled: true,
        status: (!bare).then(|| Status {
            started: "2026-10-04T18-04-05.123".into(),
            complete: true,
            plugins: plugins.iter().filter_map(|p| p.status.clone()).collect(),
        }),
        plugins,
        shadowed: None,
        game_config: long.then(|| "_local/config.json".into()),
        stray_dbgcore: false,
        assets_enabled: true,
        asset_patches: vec![
            asset("netgun_base", true),
            asset("netgun_combustion", true),
            asset("poster_a_tex", true),
            asset("poster_c_tex", false),
        ],
    };
    let mut catalog = ModCatalog::builtin();
    catalog.builtin = false;
    catalog.mods.push(ModEntry {
        id: "combat-stats".into(),
        name: "Combat Stats".into(),
        summary: "Your damage, eliminations and accuracy after every combat round, in a panel on your wrist.".into(),
        author: "community".into(),
        version: "0.3.1".into(),
        file: "CombatStats.dll".into(),
        url: "mods/CombatStats-0.3.1.dll".into(),
        sha256: Some("00".repeat(32)),
        size: Some(412_000),
        capabilities: vec!["observes-only".into()],
        ..Default::default()
    });
    catalog.mods.push(ModEntry {
        id: "replay-recorder".into(),
        name: "Replay Recorder".into(),
        summary: "Records your matches for the replay viewer.".into(),
        author: "community".into(),
        version: "1.0.0".into(),
        file: "ReplayRecorder.dll".into(),
        url: "mods/ReplayRecorder-1.0.0.dll".into(),
        sha256: Some("11".repeat(32)),
        size: Some(1_830_000),
        capabilities: vec!["observes-only".into(), "network".into()],
        ..Default::default()
    });
    let editing = (variant == Some(SnapVariant::ModsOptions)).then(|| Editing {
        defaults: BTreeMap::from([("logging".into(), "normal".into())]),
        file: "NvrAssetPatches.dll".into(),
        rows: vec![
            ("logging".into(), "verbose".into()),
            ("log_dir".into(), "plugin_logs/assets".into()),
        ],
    });
    Mods {
        view: Some(("pc-latest".into(), view, Some(Instant::now()))),
        catalog: Some(catalog),
        editing,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_the_last_start_did() {
        let p = |st: &str| PluginStatus {
            status: st.into(),
            ..Default::default()
        };
        let mut s = Status {
            started: "2026-10-04T18-04-05.123".into(),
            plugins: vec![p("loaded"), p("failed"), p("failed")],
            complete: true,
        };
        assert_eq!(
            last_start(&s),
            "Last start 2026-10-04 18:04: 1 mod loaded, 2 failed."
        );
        s.started = "?".into();
        s.plugins = vec![p("loaded"), p("loaded"), p("skipped")];
        assert_eq!(last_start(&s), "Last start: 2 mods loaded, 1 skipped.");
        s.plugins.clear();
        assert_eq!(last_start(&s), "Last start: no mods.");
        assert_eq!(short_version("4.0.0+182.e418eaa"), "4.0.0");
        assert_eq!(short_version("4.0.0-182-ge418eaa-dirty"), "4.0.0");
    }

    /// A plugin's asset patches and options share its tile; the next plugin starts one.
    #[test]
    fn plugins_get_a_tile_each() {
        let patch = AssetPatch {
            label: "a".into(),
            enabled: true,
            required: false,
        };
        let p = Plugin {
            file: "a.dll".into(),
            name: "a".into(),
            version: String::new(),
            source: Source::Shipped,
            enabled: true,
            added: false,
            required: false,
            verified: true,
            args: Default::default(),
            defaults: Default::default(),
            present: true,
            status: None,
            settings: None,
        };
        let rows = [
            Row::Vr(VrPart::EchoXr, None),
            Row::Plugin(&p),
            Row::Assets(true),
            Row::Asset(&patch, true),
            Row::Plugin(&p),
        ];
        let heights = [50.0, 50.0, 20.0, 20.0, 50.0];
        let (tops, tiles, content) = tile_layout(&rows, &heights);
        let (pad, gap) = (dz(TILE_PAD), dz(TILE_GAP));
        assert_eq!(tiles.len(), 3);
        // EchoXR alone, then a plugin with its two asset patch rows, then the last plugin.
        assert_eq!(tiles[0], (0.0, 50.0 + 2.0 * pad));
        assert_eq!(tiles[1], (tiles[0].1 + gap, 90.0 + 2.0 * pad));
        assert_eq!(tops[1], tiles[1].0 + pad);
        assert_eq!(tops[3], tops[2] + 20.0);
        assert_eq!(tops[4], tiles[2].0 + pad);
        assert_eq!(content, tiles[2].0 + tiles[2].1);
    }

    #[test]
    fn plugin_states() {
        let mut p = Plugin {
            file: "a.dll".into(),
            name: "a".into(),
            version: String::new(),
            source: Source::Shipped,
            enabled: true,
            added: false,
            required: false,
            verified: true,
            args: Default::default(),
            defaults: Default::default(),
            present: true,
            status: None,
            settings: None,
        };
        // Not started yet: nothing to say.
        assert_eq!(state(&p), None);
        p.status = Some(PluginStatus {
            status: "failed".into(),
            error: "missing NvrPluginGetInfo export".into(),
            ..Default::default()
        });
        assert_eq!(state(&p).map(|s| s.0), Some("Failed"));
        assert_eq!(detail(&p), "a.dll  ·  missing NvrPluginGetInfo export");
        p.enabled = false;
        assert_eq!(state(&p), None);
        p.present = false;
        assert_eq!(state(&p).map(|s| s.0), Some("Missing"));
        p.present = true;
        p.enabled = true;
        p.status.as_mut().unwrap().status = "loaded".into();
        assert_eq!(state(&p), None);
    }

    #[test]
    fn plugins_lists_only_whats_installed() {
        let m = demo(None);
        let (_, view, _) = m.view.as_ref().unwrap();
        let vr = |list: &[Row]| {
            list.iter()
                .filter_map(|r| match r {
                    Row::Vr(part, _) => Some(*part),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let none = rows(view, None, VrInstalled::default());
        assert!(vr(&none).is_empty());
        assert_eq!(
            none.iter().filter(|r| matches!(r, Row::Plugin(_))).count(),
            view.plugins.len()
        );
        let both = VrInstalled {
            echoxr: true,
            hands: true,
        };
        assert_eq!(vr(&rows(view, None, both)), [VrPart::EchoXr, VrPart::Hands]);
    }

    #[test]
    fn additional_plugins_are_the_ones_not_here() {
        let m = demo(None);
        let (_, view, _) = m.view.as_ref().unwrap();
        let catalog = m.catalog.as_ref().unwrap();
        let ids = |vr| {
            extras(Some(view), catalog, vr)
                .iter()
                .map(|e| e.info().id)
                .collect::<Vec<_>>()
        };
        // Combat Stats is here, the required, shipped ones come with the update.
        assert_eq!(ids(None), ["replay-recorder"]);
        assert_eq!(
            ids(Some(VrInstalled::default())),
            ["echoxr", "echoxr-hands", "replay-recorder"]
        );
        let echoxr = VrInstalled {
            echoxr: true,
            hands: false,
        };
        assert_eq!(ids(Some(echoxr)), ["echoxr-hands", "replay-recorder"]);
        // An update is offered on the plugin's row instead.
        let combat = view.plugins.iter().find(|p| p.file == "CombatStats.dll");
        assert_eq!(
            update_for(combat.unwrap(), Some(catalog)).map(|m| m.version),
            Some("0.3.1".to_string())
        );
    }
}
