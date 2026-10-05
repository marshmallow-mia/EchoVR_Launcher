//! Development aid: `ECHOVR_SNAPSHOTS=<dir>` walks the launcher's pages and states,
//! saves a PNG of each window into `<dir>`, and exits. Used to compare the port against
//! the Java UI without screen-recording permissions.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{ColorImage, ViewportId};

use super::launcher::{Page, SnapVariant};

static LAST: Mutex<Option<(ViewportId, Arc<ColorImage>)>> = Mutex::new(None);

/// Called by every viewport each frame: keeps a screenshot reply if one arrived.
pub fn capture(ui: &egui::Ui) {
    ui.input(|i| {
        for e in &i.raw.events {
            if let egui::Event::Screenshot {
                viewport_id, image, ..
            } = e
            {
                *LAST.lock().unwrap() = Some((*viewport_id, image.clone()));
            }
        }
    });
}

#[derive(Debug, Clone)]
pub struct Shot {
    pub name: String,
    pub page: Page,
    pub variant: Option<SnapVariant>,
    /// Where the pointer rests (design pixels), for hover states. Headless only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub hover: Option<(f32, f32)>,
    /// ...with the mouse button held down there.
    #[cfg_attr(not(test), allow(dead_code))]
    pub press: bool,
}

/// Every page, then extra states (mostly of the Play page). `ECHOVR_SNAPSHOTS_ONLY=play,setup`
/// keeps only shots whose name contains one of these.
pub fn shots() -> Vec<Shot> {
    let mut shots: Vec<Shot> = [
        ("play", Page::Play),
        ("install", Page::Install),
        ("mods", Page::Mods),
        ("servers", Page::Servers),
        ("echovrce", Page::EchoVrce),
        ("friends", Page::Friends),
        ("plugins", Page::Plugins),
        ("settings", Page::Settings),
    ]
    .into_iter()
    .map(|(n, page)| Shot {
        name: format!("launcher_{n}"),
        page,
        variant: None,
        hover: None,
        press: false,
    })
    .collect();
    for (n, page, v) in [
        ("play_not_installed", Page::Play, SnapVariant::Fresh),
        ("play_installing", Page::Play, SnapVariant::Installing),
        ("play_extracting", Page::Play, SnapVariant::Extracting),
        ("play_quest", Page::Play, SnapVariant::QuestSide),
        ("play_notice", Page::Play, SnapVariant::Notice),
        ("install_ask", Page::Install, SnapVariant::InstallAsk),
        ("install_ask_new", Page::Install, SnapVariant::InstallAskNew),
        (
            "install_ask_found",
            Page::Install,
            SnapVariant::InstallAskFound,
        ),
        (
            "install_ask_choose",
            Page::Install,
            SnapVariant::InstallAskChoose,
        ),
        ("dialog_error", Page::Play, SnapVariant::DialogError),
        (
            "dialog_install_error",
            Page::Play,
            SnapVariant::DialogInstallError,
        ),
        ("play_quest_fresh", Page::Play, SnapVariant::QuestFresh),
        ("mods_no_loader", Page::Mods, SnapVariant::ModsNoLoader),
        ("mods_options", Page::Mods, SnapVariant::ModsOptions),
        ("dialog_confirm", Page::Install, SnapVariant::DialogConfirm),
        ("dialog_browser", Page::Play, SnapVariant::DialogBrowser),
        (
            "settings_delete_cache",
            Page::Settings,
            SnapVariant::DeleteCache,
        ),
        (
            "settings_upload_logs",
            Page::Settings,
            SnapVariant::UploadLogs,
        ),
        ("settings_logs_sent", Page::Settings, SnapVariant::LogsSent),
        ("settings_credits", Page::Settings, SnapVariant::Credits),
        ("play_rail_open", Page::Play, SnapVariant::RailOpen),
        ("play_updates", Page::Play, SnapVariant::UpdatesFound),
        (
            "settings_updates",
            Page::Settings,
            SnapVariant::UpdatesFound,
        ),
        ("install_rail_open", Page::Install, SnapVariant::RailOpen),
        ("mods_rail_open", Page::Mods, SnapVariant::RailOpen),
        ("settings_rail_open", Page::Settings, SnapVariant::RailOpen),
        ("settings_uninstall", Page::Settings, SnapVariant::Uninstall),
        ("install_menu", Page::Install, SnapVariant::MenuOpen),
        ("install_fresh", Page::Install, SnapVariant::Fresh),
        ("install_extracting", Page::Install, SnapVariant::Extracting),
        ("install_quest", Page::Install, SnapVariant::QuestSide),
        (
            "install_quest_fresh",
            Page::Install,
            SnapVariant::QuestFresh,
        ),
        ("install_installing", Page::Install, SnapVariant::Installing),
        ("play_launching", Page::Play, SnapVariant::Launching),
        ("play_running", Page::Play, SnapVariant::Running),
        ("play_stop", Page::Play, SnapVariant::RunningOurs),
        ("play_server_here", Page::Play, SnapVariant::ServerHere),
        ("play_version_menu", Page::Play, SnapVariant::VersionMenu),
        ("play_quest_running", Page::Play, SnapVariant::QuestRunning),
        (
            "echovrce_signed_in",
            Page::EchoVrce,
            SnapVariant::VrceSignedIn,
        ),
        ("echovrce_signing", Page::EchoVrce, SnapVariant::VrceSigning),
        ("servers_live", Page::Servers, SnapVariant::ServersLive),
        (
            "servers_history",
            Page::Servers,
            SnapVariant::ServersHistory,
        ),
        ("servers_start", Page::Servers, SnapVariant::ServersStart),
        ("servers_join_link", Page::Servers, SnapVariant::JoinLobby),
        (
            "servers_invites",
            Page::Servers,
            SnapVariant::ServersInvites,
        ),
        ("servers_share", Page::Servers, SnapVariant::ServersShare),
        ("play_friends", Page::Play, SnapVariant::PlayFriends),
        ("friends_signed_in", Page::Friends, SnapVariant::FriendsPage),
        ("play_login_code", Page::Play, SnapVariant::LoginCode),
        ("play_login_message", Page::Play, SnapVariant::LoginMessage),
        ("settings_links", Page::Settings, SnapVariant::SettingsLinks),
        (
            "settings_steamvr",
            Page::Settings,
            SnapVariant::SettingsSteamVr,
        ),
        (
            "settings_echoxr",
            Page::Settings,
            SnapVariant::SettingsEchoXr,
        ),
        ("mods_echoxr", Page::Mods, SnapVariant::SettingsEchoXr),
        (
            "install_placeholder",
            Page::Install,
            SnapVariant::Placeholder,
        ),
        ("play_patch", Page::Play, SnapVariant::NewPlayer),
        ("play_licence", Page::Play, SnapVariant::Licence),
        ("play_licence_link", Page::Play, SnapVariant::LicenceLink),
        (
            "install_ask_quest",
            Page::Install,
            SnapVariant::InstallAskQuest,
        ),
        ("play_owner", Page::Play, SnapVariant::Owner),
        ("play_event", Page::Play, SnapVariant::EventSelected),
        ("install_event", Page::Install, SnapVariant::EventSelected),
        (
            "install_ask_event",
            Page::Install,
            SnapVariant::InstallAskEvent,
        ),
        ("play_relay_account", Page::Play, SnapVariant::RelayAccount),
        ("settings_event", Page::Settings, SnapVariant::EventSelected),
        (
            "play_licence_waiting",
            Page::Play,
            SnapVariant::LicenceWaiting,
        ),
    ] {
        shots.push(Shot {
            name: format!("launcher_{n}"),
            page,
            variant: Some(v),
            hover: None,
            press: false,
        });
    }
    // The concept's hover states (PLAY, CHECK FOR UPDATES, the switch, a rail icon, the
    // grey PLAY with nothing installed) and its "clicked" ones (held down).
    for (n, variant, at, press) in [
        ("play_hover", None, (200.0, 276.0), false),
        ("play_hover_update", None, (520.0, 276.0), false),
        ("play_hover_switch", None, (800.0, 290.0), false),
        ("play_hover_rail", None, (47.0, 335.0), false),
        (
            "play_hover_grey",
            Some(SnapVariant::Fresh),
            (200.0, 276.0),
            false,
        ),
        ("play_press", None, (200.0, 276.0), true),
        ("play_press_update", None, (520.0, 276.0), true),
    ] {
        shots.push(Shot {
            name: format!("launcher_{n}"),
            page: Page::Play,
            variant,
            hover: Some(at),
            press,
        });
    }
    if let Ok(only) = std::env::var("ECHOVR_SNAPSHOTS_ONLY") {
        let terms: Vec<&str> = only.split(',').map(str::trim).collect();
        shots.retain(|s| terms.iter().any(|t| s.name.contains(t)));
    }
    shots
}

pub struct Snapshotter {
    /// `ECHOVR_SNAPSHOTS_DEMO=1`: render the dashboard with made-up versions.
    pub demo: bool,
    dir: PathBuf,
    shots: Vec<Shot>,
    idx: usize,
    frames: u32,
    requested: bool,
    shown_at: Option<std::time::Instant>,
}

impl Snapshotter {
    pub fn from_env() -> Option<Snapshotter> {
        let dir = PathBuf::from(std::env::var_os("ECHOVR_SNAPSHOTS")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Snapshotter {
            demo: std::env::var_os("ECHOVR_SNAPSHOTS_DEMO").is_some(),
            dir,
            shots: shots(),
            idx: 0,
            frames: 0,
            requested: false,
            shown_at: None,
        })
    }

    fn next(&mut self) {
        self.idx += 1;
        self.frames = 0;
        self.requested = false;
        self.shown_at = None;
        *LAST.lock().unwrap() = None;
    }

    pub fn current(&self) -> Option<&Shot> {
        self.shots.get(self.idx)
    }

    /// Drives one frame. `target` is the viewport the current shot belongs to.
    /// Returns true when all shots are done.
    pub fn tick(&mut self, ctx: &egui::Context, target: ViewportId) -> bool {
        let Some(shot) = self.shots.get(self.idx).cloned() else {
            return true;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
        let since = *self.shown_at.get_or_insert_with(std::time::Instant::now);
        let age = since.elapsed().as_millis();
        if age > 10_000 {
            tracing::warn!("snapshot {} never arrived; skipping", shot.name);
            self.next();
            return self.idx >= self.shots.len();
        }
        // Let the page settle (fonts, textures, fades), then ask; re-ask every 1.5 s in
        // case a reply was lost to a window resize.
        let slot = (age.saturating_sub(500) / 1500) as u32;
        if age >= 500 && self.frames != slot + 1 {
            self.frames = slot + 1;
            self.requested = true;
            ctx.send_viewport_cmd_to(
                target,
                egui::ViewportCommand::Screenshot(Default::default()),
            );
        }
        let got = LAST.lock().unwrap().take();
        if let Some((vid, img)) = got {
            if vid == target {
                let path = self.dir.join(format!("{}.png", shot.name));
                let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) =
                    image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, rgba)
                {
                    let _ = buf.save(&path);
                }
                tracing::info!("snapshot {}", path.display());
                self.next();
            }
        }
        self.idx >= self.shots.len()
    }
}
