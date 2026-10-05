//! What can be done to versions: install, update, verify, add or remove them, and the
//! MANAGE menu of an installed one. The Install page (`install.rs`) shows them.

use super::{setup, Dashboard, JobKind, JobResult};
use crate::core::error::UiError;
use crate::core::launcher::catalog::VersionEntry;
use crate::core::launcher::store::InstalledVersion;
use crate::core::launcher::versions;
use crate::core::{paths, platform};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::parts;
use crate::ui::widgets::MenuItem;

const REMOVE_KEY: &str = "remove-version";
const REPAIR_KEY: &str = "repair-version";

pub(super) fn ask_repair(d: &mut Dashboard, id: &str, msg: &str) {
    d.pending_repair = Some(id.to_string());
    d.dialogs
        .confirm(REPAIR_KEY, "Verify", msg, DlgIcon::Warning);
}

pub(super) fn job_err(e: anyhow::Error, title: &str) -> JobResult {
    if crate::core::http::is_cancelled(&e) {
        JobResult::Failed(None)
    } else {
        JobResult::Failed(Some(UiError::from_anyhow(&e, title)))
    }
}

pub(super) fn update(d: &mut Dashboard, ctx: &egui::Context, v: InstalledVersion) {
    let id = v.id.clone();
    d.start_job(
        ctx,
        JobKind::Update,
        &id,
        &format!("Updating {}", v.name),
        "Checking for updates...",
        move |cancel, on| match versions::update(&v, cancel, on) {
            Ok(()) => JobResult::Updated,
            Err(e) => {
                let title = crate::core::pc_update::error_title(&e);
                job_err(e, &title)
            }
        },
    );
}

/// Reinstalls `v` from `e`: checks its files against the build's checksums and fetches
/// only the broken ones again (`keep_patch`: a new player's licence patch stays).
pub(super) fn reinstall(
    d: &mut Dashboard,
    ctx: &egui::Context,
    e: VersionEntry,
    v: InstalledVersion,
    keep_patch: bool,
) {
    let id = v.id.clone();
    d.start_job(
        ctx,
        JobKind::Reinstall,
        &id,
        &format!("Reinstalling {}", v.name),
        "Reading the build's checksums...",
        move |cancel, on| match versions::reinstall(&e, &v, keep_patch, cancel, on) {
            Ok(r) => JobResult::Reinstalled(r),
            Err(err) => job_err(err, "Reinstall Failed"),
        },
    );
}

fn verify(d: &mut Dashboard, ctx: &egui::Context, v: InstalledVersion) {
    let id = v.id.clone();
    d.start_job(
        ctx,
        JobKind::Verify,
        &id,
        &format!("Verifying {}", v.name),
        "Verifying game files...",
        move |cancel, on| match versions::verify(&v, cancel, on) {
            Ok(bad) => JobResult::Verified(bad),
            Err(e) => job_err(e, "Verify Failed"),
        },
    );
}

pub(super) fn install(d: &mut Dashboard, ctx: &egui::Context, e: VersionEntry) {
    if !d.state.offers(&e) {
        let why = if e.downloadable() {
            "Event builds are coming soon."
        } else {
            "This build isn't on the download servers yet."
        };
        d.dialogs.error(
            &format!("Couldn't download {}", e.name),
            why,
            Default::default(),
        );
        return;
    }
    let library = d.state.library.clone();
    let id = e.id.clone();
    d.start_job(
        ctx,
        JobKind::Install,
        &id,
        &format!("Installing {}", e.name),
        "Preparing download...",
        move |cancel, on| match versions::install(&e, &library, cancel, on) {
            Ok(i) => JobResult::Installed(i.version, i.update_failed),
            Err(err) => job_err(err, "Install Failed"),
        },
    );
}

/// Takes the copy of `e` at `root` (installed some other way: the Meta app, an older
/// installer) into the library instead of downloading it: a job that checks its files
/// against the build's first. Returns the version's id.
pub(super) fn adopt(
    d: &mut Dashboard,
    ctx: &egui::Context,
    e: VersionEntry,
    root: String,
) -> String {
    // A folder added before (as "Existing install") becomes this version.
    let norm = paths::normalize(&root);
    let id = d
        .state
        .versions
        .iter()
        .find(|v| paths::normalize(&v.root).eq_ignore_ascii_case(&norm))
        .map_or_else(|| d.state.free_id(&e.id), |v| v.id.clone());
    d.state.selected = Some(id.clone());
    d.save();
    let job = id.clone();
    d.start_job(
        ctx,
        JobKind::Install,
        &job,
        &format!("Adding {}", e.name),
        "Checking the game files...",
        move |cancel, on| match versions::adopt(&e, &norm, &id, cancel, on) {
            Ok(i) => JobResult::Installed(i.version, i.update_failed),
            Err(err) => job_err(err, "Couldn't Add Echo VR"),
        },
    );
    job
}

/// Asks for the executable of an Echo VR copy already on this PC: its install root, or
/// `None` (cancelled, or not Echo VR: said so).
pub(super) fn choose_copy(d: &mut Dashboard) -> Option<String> {
    let exe = parts::choose_exe()?;
    let root = paths::resolve_install_root(&exe);
    if paths::has_echo_install(&root) {
        return Some(root);
    }
    d.dialogs.error(
        "Echo VR not found",
        "That isn't Echo VR: choose echovr.exe in its ready-at-dawn-echo-arena\\bin\\win10 folder.",
        Default::default(),
    );
    None
}

pub(super) fn add_existing(d: &mut Dashboard) {
    let Some(p) = parts::choose_folder() else {
        return;
    };
    let root = paths::resolve_install_root(&p);
    if !paths::has_echo_install(&root) {
        d.dialogs.error(
            "Echo VR not found",
            "That folder doesn't contain Echo VR (ready-at-dawn-echo-arena\\bin\\win10\\echovr.exe).",
            Default::default(),
        );
    } else if let Some(id) = d.state.add_external(&root, None) {
        d.state.selected = Some(id);
        d.save();
    } else {
        d.dialogs
            .info("Already added", "This install is already in the list.");
    }
}

/// Adds the Echo VR install in the Meta (Oculus) library, if there is one.
pub(super) fn find_meta(d: &mut Dashboard) {
    let Some(base) = crate::core::platform::oculus_base_path() else {
        d.dialogs.error(
            "Meta install not found",
            "Could not find the Meta Quest (Oculus) app on this PC.",
            Default::default(),
        );
        return;
    };
    let sep = if base.ends_with(['\\', '/']) { "" } else { "/" };
    let root = paths::normalize(&format!("{base}{sep}Software/Software"));
    if !paths::has_echo_install(&root) {
        d.dialogs.info(
            "Echo VR not in your Meta library",
            &format!(
                "Echo VR is not installed in your Meta library ({root}).\n\n\
                 If you own it, install it from the Meta Store and launch it once. \
                 Otherwise install it on this page."
            ),
        );
        return;
    }
    match d
        .state
        .add_external(&root, Some("Echo VR (Meta library)".into()))
    {
        Some(id) => {
            d.state.selected = Some(id);
            d.save();
        }
        None => d
            .dialogs
            .info("Already added", "Your Meta install is already in the list."),
    }
}

/// Removes (deletes or forgets) version `id`, with what the launcher made for it: its
/// desktop shortcuts, SteamVR's library entry when that starts it, and, in a folder it
/// only forgets, the files the launcher put there.
fn remove(d: &mut Dashboard, ctx: &egui::Context, id: &str) {
    use crate::core::uninstall;
    let Some(v) = d.state.version(id).cloned() else {
        return;
    };
    if v.external {
        let failed = uninstall::clean_version(&v);
        if !failed.is_empty() {
            d.dialogs.info(
                "Forgotten",
                &format!(
                    "Some of what the launcher put into {} couldn't be removed:\n\n{}",
                    v.root,
                    failed.join("\n")
                ),
            );
        }
    } else if let Err(e) = versions::remove(&v, &d.state.library) {
        d.dialogs
            .error("Remove Failed", &format!("{e:#}"), Default::default());
        return;
    }
    // The live build's shortcuts ("Echo VR") only when no other live build is left.
    let another_live = d
        .state
        .versions
        .iter()
        .any(|x| x.id != id && x.publisher_lock.is_none());
    if v.publisher_lock.is_some() || !another_live {
        for name in uninstall::shortcut_names(&v) {
            if let Err(e) = platform::remove_shortcut(&name) {
                tracing::warn!("shortcut {name}: {e:#}");
            }
        }
    }
    if cfg!(windows) && crate::core::revive::library_points_into(&v.root) {
        super::setup::steamvr_library(d, ctx, false);
    }
    d.state.versions.retain(|x| x.id != id);
    if d.state.selected.as_deref() == Some(id) {
        d.state.selected = d.state.versions.first().map(|v| v.id.clone());
    }
    d.save();
}

/// The answers to Remove and Repair, whichever page is showing.
pub(super) fn handle_answers(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(a) = d.dialogs.take(REMOVE_KEY) {
        if let (true, Some(id)) = (a.is_yes(), d.pending_remove.take()) {
            remove(d, ctx, &id);
        }
    }
    if let Some(a) = d.dialogs.take(REPAIR_KEY) {
        let v = d
            .pending_repair
            .take()
            .and_then(|id| d.state.version(&id).cloned());
        if let (true, Some(v)) = (a.is_yes(), v) {
            update(d, ctx, v);
        }
    }
}

/// The MANAGE ▾ button of an installed version (logical pixels), and what its rows do.
#[allow(clippy::too_many_arguments)]
pub(super) fn manage_menu(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    v: &InstalledVersion,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) {
    let ok = d.demo || v.present();
    enum Act {
        Update,
        Verify,
        Open,
        Shortcut,
        Patch,
        Unpatch,
        Account,
        Remove,
        Uninstall,
    }
    let event = v.publisher_lock.is_some();
    let mut menu = Vec::new();
    // Event builds get no updates (REINSTALL checks their files).
    if versions::has_updates(v) {
        menu.push((
            MenuItem::row("Update", "Download any changed game files"),
            Some(Act::Update),
        ));
        menu.push((
            MenuItem::row(
                "Verify files",
                "Check every game file against the update manifest",
            ),
            Some(Act::Verify),
        ));
    }
    menu.push((MenuItem::row("Open folder", ""), Some(Act::Open)));
    // Shortcuts start the game directly, which only works on Windows for now.
    if cfg!(windows) || d.demo {
        menu.push((
            MenuItem::row(
                "Desktop shortcut",
                "A shortcut that starts this version directly",
            ),
            Some(Act::Shortcut),
        ));
    }
    // The licence patch, for owners too (it stays optional for them). Event builds play
    // with EchoRelay's patch instead, and an account on the classic lobbies server.
    menu.push((MenuItem::Divider, None));
    if event {
        menu.push((
            MenuItem::row(
                "Classic lobbies account…",
                "Your name and password on the classic lobbies server",
            ),
            Some(Act::Account),
        ));
    } else if v.patched {
        menu.push((
            MenuItem::row("Remove licence patch", "Put the original pnsovr.dll back"),
            Some(Act::Unpatch),
        ));
    } else {
        menu.push((
            MenuItem::row(
                "Licence patch…",
                "Your personal patch, through Discord or from a link",
            ),
            Some(Act::Patch),
        ));
    }
    menu.push((MenuItem::Divider, None));
    menu.push((
        MenuItem::row(
            if v.external { "Forget" } else { "Remove" },
            if v.external {
                "Remove from the launcher; the folder stays on disk"
            } else {
                "Delete this version from disk"
            },
        ),
        Some(Act::Remove),
    ));
    menu.push((
        MenuItem::row(
            "Uninstall…",
            "Echo VR, its mods and VR set-up, and the launcher's data: choose what",
        ),
        Some(Act::Uninstall),
    ));
    let (items, acts): (Vec<MenuItem>, Vec<Option<Act>>) = menu.into_iter().unzip();
    let picked = k
        .menu_button(
            &format!("menu-{}", v.id),
            "Manage",
            &items,
            x,
            y,
            w,
            h,
            "Update, verify, patch, open or remove",
        )
        .and_then(|i| acts.into_iter().nth(i).flatten());
    let busy = d.any_job();
    match picked {
        Some(Act::Update) if ok && !busy => update(d, ctx, v.clone()),
        Some(Act::Verify) if ok && !busy => verify(d, ctx, v.clone()),
        Some(Act::Shortcut) if ok => setup::shortcut(d, &v.id),
        Some(Act::Patch) if ok && !busy => d.overlay = Some(setup::licence(&v.id)),
        Some(Act::Unpatch) if ok && !busy => setup::unpatch(d, ctx, &v.id),
        Some(Act::Account) => d.overlay = Some(setup::relay_account(d, false)),
        Some(Act::Open) => {
            if let Err(e) = platform::open_folder(&v.bin_dir()) {
                d.dialogs.error(
                    "Couldn't open folder",
                    &format!("{e:#}"),
                    Default::default(),
                );
            }
        }
        Some(Act::Uninstall) if !busy => super::settings::ask_uninstall(d),
        Some(Act::Remove) => {
            d.pending_remove = Some(v.id.clone());
            if v.external {
                let msg = format!(
                    "Remove {} from the launcher?\n\nThe folder {} stays on disk; what the launcher put into it (EchoXR, the plugins it added, its sign-in) is taken out.",
                    v.name, v.root
                );
                d.dialogs
                    .confirm_danger(REMOVE_KEY, "Forget", &msg, "Forget");
            } else {
                let msg = format!("Delete {}?\n\nThis removes {} from disk.", v.name, v.root);
                d.dialogs
                    .confirm_danger(REMOVE_KEY, "Remove", &msg, "Delete");
            }
        }
        Some(_) if busy => d.dialogs.info(
            "Busy",
            "Another job is running. Wait until it's done, then try again.",
        ),
        Some(_) => d.dialogs.info(
            "Game files missing",
            "This version's folder is gone. Install it again, or forget it.",
        ),
        None => {}
    }
}
