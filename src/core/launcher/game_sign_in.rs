//! The game's own EchoVRCE sign-in (nEVR's `_local/.credentials.json`) for the account the
//! launcher is signed in with: each live version with nEVR that has no good one for it gets
//! a session linked for it (a device of the account, so the game's and the launcher's
//! refresh tokens never rotate each other out). The window renews it while it's open;
//! without the window, the tray and every `--play` do ([`renew_stored`]), so Echo VR started
//! from anywhere stays signed in.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::core::echovrce::{self as vrce, Account, Tokens};
use crate::core::launcher::nevr::{self, GameLogin};
use crate::core::launcher::store::InstalledVersion;

/// Gives each live version in `versions` with nEVR and without a good sign-in for `account`
/// one, linked from `tokens`. `hold` says whether it may still be written (`None`: signed
/// out meanwhile, nothing more is written) and holds that while it is. Returns whether
/// every version that needed one got it.
pub fn renew<G>(
    tokens: &Tokens,
    account: &Account,
    versions: &[InstalledVersion],
    hold: impl Fn() -> Option<G>,
) -> bool {
    let now = vrce::now();
    let mut all_good = true;
    for v in versions
        .iter()
        .filter(|v| v.publisher_lock.is_none() && v.present())
    {
        if !crate::core::launcher::mods::nevr_in(&v.bin_dir())
            || nevr::read_login(v).is_some_and(|l| l.good_for(&account.id, now))
        {
            continue;
        }
        // `Some`: signed out meanwhile, the linked session isn't written.
        let linked = vrce::link_device(&tokens.token).and_then(|t| {
            let rt = t
                .refresh_token
                .clone()
                .ok_or_else(|| anyhow::anyhow!("EchoVRCE gave no refresh token"))?;
            let login = GameLogin::new(&rt, &account.id, &account.username, now);
            let Some(_held) = hold() else {
                return Ok(Some(t));
            };
            nevr::write_login(v, &login).map(|()| None)
        });
        match linked {
            Ok(None) => tracing::info!("{}: the game has its own EchoVRCE sign-in", v.id),
            Ok(Some(t)) => {
                tracing::info!(
                    "{}: signed out while the game's sign-in was linked: not written",
                    v.id
                );
                let _ = vrce::logout(&t);
                break;
            }
            Err(e) => {
                all_good = false;
                tracing::warn!("{}: no EchoVRCE sign-in for the game: {e:#}", v.id)
            }
        }
    }
    all_good
}

/// Without the launcher's window (the tray, `--play`): the kept EchoVRCE session (renewed
/// when due), then [`renew`]. Nothing while a window runs: it keeps both itself, and two
/// renewing one session would rotate its refresh token out from under each other.
pub fn renew_stored(versions: &[InstalledVersion]) {
    if crate::core::tray::window_running() {
        return;
    }
    let Some(_renewing) = Renewing::take() else {
        return;
    };
    let Some(tokens) = vrce::store::load() else {
        return;
    };
    let era = vrce::store::era();
    let (tokens, account) = match vrce::check(&tokens) {
        Ok((Some(renewed), account)) => {
            if let Err(e) = vrce::store::save(&renewed, era) {
                tracing::warn!("echovrce: saving the renewed session: {e:#}");
            }
            (renewed, account)
        }
        Ok((None, account)) => (tokens, account),
        Err(e) => {
            // The window says so (and forgets an ended session) when it opens.
            tracing::info!("echovrce: the game's sign-in not renewed: {e:#}");
            return;
        }
    };
    renew(&tokens, &account, versions, || {
        (vrce::store::era() == era).then_some(())
    });
}

/// Waits (up to `limit`) while a tray or `--play` renews the kept session, so the window
/// reads it afterwards rather than the one about to be replaced.
pub fn wait_for_renewal(limit: Duration) {
    let since = std::time::Instant::now();
    while Renewing::path().is_file() && !Renewing::stale() && since.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A renewal of the kept session under way in this process (a file, for the others).
struct Renewing;

impl Renewing {
    /// One older than this was left by a process that died mid-way.
    const STALE: Duration = Duration::from_secs(120);

    fn path() -> PathBuf {
        crate::core::paths::data_dir().join("echovrce-renewing")
    }

    fn stale() -> bool {
        std::fs::metadata(Self::path())
            .and_then(|m| m.modified())
            .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() > Self::STALE)
    }

    /// `None` while another process renews it.
    fn take() -> Option<Renewing> {
        let path = Self::path();
        if Self::stale() {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()
            .map(|_| Renewing)
    }
}

impl Drop for Renewing {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(Self::path());
    }
}
