//! EchoVRCE page: your EchoVRCE account, and echovrce.com itself inside the window.
//!
//! Signing in shows a code to approve on echovrce.com (opened in the browser with the code
//! filled in); tokens can be pasted instead. The session is checked at start and every ten
//! minutes and renewed before it runs out; when it can't be, the page and a dot on its
//! rail icon ask to sign in again.
//!
//! Signed in, the page shows the site (Windows, macOS), signed in too: with a second
//! session linked for it once per account, which the site then keeps renewing itself.
//! The game gets one of its own the same way ([`GameSignIn`]): nEVR then never asks in
//! the browser.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{hero, panel, setup, Dashboard, HEADER};
use crate::core::echovrce::{self as vrce, Account, DeviceCode, Poll, Tokens};
use crate::core::launcher::nevr::{self, GameLogin};
use crate::core::launcher::store::InstalledVersion;
use crate::ui::design::{self, dz, Dr};
use crate::ui::kit::Kit;
use crate::ui::parts::Worker;
use crate::ui::style::Icon;
use crate::ui::widgets::{Tone, BTN_H};

/// How often the session is checked (and renewed when due).
const CHECK_EVERY: Duration = Duration::from_secs(600);
/// After the game's sign-in couldn't be given: when to try again.
const RETRY_GAME_SIGN_IN: Duration = Duration::from_secs(3600);
/// After EchoVRCE didn't answer: when to try again.
const RETRY_AFTER: Duration = Duration::from_secs(120);
/// How often a sign-in code is asked about.
const POLL_EVERY: Duration = Duration::from_secs(3);

// Geometry in design pixels: the account card, and what EchoVRCE is, under the header.
const ACCOUNT: Dr = Dr::new(137.0, 156.0, 1146.0, 470.0);
const ABOUT: Dr = Dr::new(1318.0, 156.0, 555.0, 470.0);
/// The site, under the header strip to the window's bottom.
const SITE: Dr = Dr::new(138.0, 146.0, 1734.0, 900.0);
/// The page the site opens on.
const SITE_HOME: &str = "https://echovrce.com/home";

enum Msg {
    /// The saved session (or none).
    Loaded(Option<Tokens>),
    /// A sign-in code, or why there is none.
    Code(Result<DeviceCode, Failed>),
    /// Signed in (code approved, or tokens pasted), or why not.
    SignedIn(Result<(Tokens, Account), Failed>),
    /// While a code waits: EchoVRCE isn't answering its polls (why), or is again (`None`).
    Polling(Option<String>),
    /// The regular check: the renewed session if it was renewed, and the account.
    Checked(Result<(Option<Tokens>, Account), Ended>),
    /// A session linked for the site: (account id, its tokens), or why not.
    SiteLinked(Result<(String, Tokens), String>),
}

/// Why signing in didn't work, and whether it's because EchoVRCE isn't answering.
struct Failed {
    why: String,
    down: bool,
}

impl Failed {
    fn of(e: &anyhow::Error) -> Failed {
        Failed {
            why: format!("{e:#}"),
            down: vrce::is_unavailable(e),
        }
    }
}

impl From<String> for Failed {
    fn from(why: String) -> Failed {
        Failed { why, down: false }
    }
}

/// Why a check failed.
enum Ended {
    /// The session is over: sign in again.
    SignedOut,
    /// EchoVRCE didn't answer; try later.
    Unreachable(String),
}

/// A code waiting to be approved.
pub(super) struct Signing {
    pub code: DeviceCode,
    pub since: Instant,
    cancel: Arc<AtomicBool>,
}

/// The EchoVRCE session and what the page shows about it.
#[derive(Default)]
pub(super) struct Vrce {
    worker: Worker<Msg>,
    pub tokens: Option<Tokens>,
    pub account: Option<Account>,
    /// Loading the saved session, checking it, or asking for a code.
    pub busy: bool,
    pub signing: Option<Signing>,
    /// The session ended and couldn't be renewed: sign in again.
    pub ended: bool,
    /// EchoVRCE isn't answering right now (why): the session stays, checks go on.
    pub down: Option<String>,
    /// Why the last sign-in didn't work, until the next one.
    pub failed: Option<String>,
    /// When to check the session next.
    next_check: Option<Instant>,
    loaded: bool,
    /// Linking a session for the site.
    pub linking_site: bool,
    /// A finished link, for the page to open the site with.
    pub site_linked: Option<Result<(String, Tokens), String>>,
}

impl Vrce {
    /// Every frame: loads the saved session once, takes in what the workers found, and
    /// checks the session when due. Returns a notice for the status bar.
    pub(super) fn tick(&mut self, ctx: &egui::Context, demo: bool) -> Option<String> {
        if demo {
            return None;
        }
        if !self.loaded {
            self.loaded = true;
            self.busy = true;
            self.worker
                .spawn(ctx, |tx| tx.send(Msg::Loaded(vrce::store::load())));
        }
        let mut notice = None;
        for m in self.worker.drain() {
            match m {
                Msg::Loaded(t) => {
                    self.busy = false;
                    self.tokens = t;
                    self.next_check = Some(Instant::now());
                }
                Msg::Code(Ok(code)) => {
                    self.busy = false;
                    self.down = None;
                    crate::core::platform::open_url(&vrce::approve_url(&code.code));
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.wait_for_approval(ctx, &code, cancel.clone());
                    self.signing = Some(Signing {
                        code,
                        since: Instant::now(),
                        cancel,
                    });
                }
                Msg::Code(Err(f)) | Msg::SignedIn(Err(f)) => {
                    self.busy = false;
                    self.signing = None;
                    tracing::warn!("echovrce sign-in: {}", f.why);
                    if f.down {
                        self.down = Some(f.why.clone());
                    }
                    self.failed = Some(f.why.clone());
                    notice = Some(f.why);
                }
                Msg::Polling(down) => self.down = down,
                Msg::SignedIn(Ok((tokens, account))) => {
                    self.busy = false;
                    self.signing = None;
                    self.ended = false;
                    self.down = None;
                    self.failed = None;
                    notice = Some(format!("Signed in to EchoVRCE as {}", account.name()));
                    self.tokens = Some(tokens);
                    self.account = Some(account);
                    self.next_check = Some(next_check(self.tokens.as_ref()));
                }
                Msg::Checked(Ok((renewed, account))) => {
                    self.busy = false;
                    self.down = None;
                    if renewed.is_some() {
                        self.tokens = renewed;
                    }
                    self.account = Some(account);
                    self.next_check = Some(next_check(self.tokens.as_ref()));
                }
                Msg::Checked(Err(Ended::SignedOut)) => {
                    self.busy = false;
                    self.tokens = None;
                    self.account = None;
                    self.ended = true;
                    self.next_check = None;
                    notice = Some("Your EchoVRCE session has ended: sign in again".into());
                }
                Msg::SiteLinked(r) => {
                    self.linking_site = false;
                    self.site_linked = Some(r);
                }
                Msg::Checked(Err(Ended::Unreachable(why))) => {
                    self.busy = false;
                    tracing::info!("echovrce check: {why}");
                    self.down = Some(why);
                    self.next_check = Some(Instant::now() + RETRY_AFTER);
                }
            }
        }
        let due = self.next_check.is_some_and(|t| Instant::now() >= t);
        if due && !self.busy && self.signing.is_none() {
            if let Some(tokens) = self.tokens.clone() {
                self.check(ctx, tokens);
            }
        }
        if let Some(t) = self.next_check {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
        notice
    }

    /// Renews the session when due and reads the account, on a worker.
    fn check(&mut self, ctx: &egui::Context, tokens: Tokens) {
        self.busy = true;
        self.worker.spawn(ctx, move |tx| {
            let r = match vrce::check(&tokens) {
                Ok((renewed, account)) => {
                    if let Some(t) = &renewed {
                        if let Err(e) = vrce::store::save(t) {
                            tracing::warn!("echovrce: saving the renewed session: {e:#}");
                        }
                    }
                    Ok((renewed, account))
                }
                Err(e) if vrce::is_signed_out(&e) => {
                    vrce::store::forget();
                    Err(Ended::SignedOut)
                }
                Err(e) => Err(Ended::Unreachable(format!("{e:#}"))),
            };
            tx.send(Msg::Checked(r));
        });
    }

    /// Asks for a sign-in code; the browser opens on it once it's there.
    pub(super) fn sign_in(&mut self, ctx: &egui::Context) {
        self.busy = true;
        self.failed = None;
        self.worker.spawn(ctx, |tx| {
            tx.send(Msg::Code(vrce::request_code().map_err(|e| Failed::of(&e))))
        });
    }

    /// Signed in, but EchoVRCE isn't answering: the session is kept and checked again
    /// every [`RETRY_AFTER`].
    pub(super) fn waiting_for_server(&self) -> bool {
        self.tokens.is_some() && self.account.is_none() && self.down.is_some()
    }

    /// What to say while EchoVRCE isn't answering (`None`: it is): why, and what happens
    /// meanwhile. With `headline`, it starts by saying that it isn't answering (where no
    /// heading says so already).
    pub(super) fn down_note(&self, headline: bool) -> Option<String> {
        let why = self.down.as_deref()?.trim_end_matches('.');
        let cause = match why.strip_prefix("EchoVRCE isn't answering right now (") {
            Some(status) => format!("It answered {}.", status.trim_end_matches(')')),
            None => format!("{why}."),
        };
        let every = RETRY_AFTER.as_secs() / 60;
        let then = if self.tokens.is_some() {
            format!("You stay signed in: the launcher asks again every {every} minutes.")
        } else {
            "Signing in can fail until it answers again.".to_string()
        };
        let head = if headline {
            "EchoVRCE isn't answering right now. "
        } else {
            ""
        };
        Some(format!("{head}{cause} {then}"))
    }

    /// Checks the session now instead of at the next try.
    pub(super) fn retry_now(&mut self, ctx: &egui::Context) {
        if let Some(tokens) = self.tokens.clone().filter(|_| !self.busy) {
            self.check(ctx, tokens);
        }
    }

    /// Asks every few seconds whether `code` was approved, until it is, runs out, or is
    /// cancelled.
    fn wait_for_approval(&self, ctx: &egui::Context, code: &DeviceCode, cancel: Arc<AtomicBool>) {
        let (code, until) = (code.code.clone(), Instant::now() + code.expires_in);
        let mut down = false;
        self.worker.spawn(ctx, move |tx| loop {
            std::thread::sleep(POLL_EVERY);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if Instant::now() > until {
                tx.send(Msg::SignedIn(Err(Failed::from(
                    "The sign-in code ran out before it was approved. Try again.".to_string(),
                ))));
                return;
            }
            match vrce::poll(&code) {
                Ok(Poll::Pending) => {
                    if down {
                        down = false;
                        tx.send(Msg::Polling(None));
                    }
                }
                Ok(Poll::Approved(tokens)) => {
                    tx.send(Msg::SignedIn(finish_sign_in(tokens)));
                    return;
                }
                // EchoVRCE not answering isn't the code's fault: ask again.
                Err(e) if vrce::is_unavailable(&e) => {
                    tracing::info!("echovrce sign-in: {e:#}; asking again");
                    down = true;
                    tx.send(Msg::Polling(Some(format!("{e:#}"))));
                }
                Err(e) => {
                    tx.send(Msg::SignedIn(Err(Failed::of(&e))));
                    return;
                }
            }
        });
    }

    /// Snapshots: signed in as a made-up account, or waiting for a made-up code.
    pub(super) fn demo(&mut self, signed_in: bool) {
        use base64::Engine;
        if signed_in {
            let exp = time::OffsetDateTime::now_utc().unix_timestamp() + 3 * 3600;
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!(r#"{{"exp":{exp}}}"#));
            self.tokens = Some(Tokens {
                token: format!("eyJhbGciOiJIUzI1NiJ9.{payload}.c2ln"),
                refresh_token: Some("demo".into()),
            });
            self.account = Some(Account {
                id: "demo".into(),
                username: "marshmallow".into(),
                display_name: "Marshmallow".into(),
                discord_id: "100000000000000001".into(),
            });
        } else {
            self.signing = Some(Signing {
                code: DeviceCode {
                    code: "C6N4-QX7Z".into(),
                    expires_in: Duration::from_secs(300),
                },
                since: Instant::now(),
                cancel: Arc::default(),
            });
        }
    }

    /// Snapshots: EchoVRCE not answering (502), signed in (`signed_in`: the session is kept)
    /// or after a sign-in that failed on it.
    pub(super) fn demo_down(&mut self, signed_in: bool) {
        let why = "EchoVRCE isn't answering right now (502)".to_string();
        if signed_in {
            self.demo(true);
            self.account = None;
        } else {
            self.failed = Some(why.clone());
        }
        self.down = Some(why);
        self.loaded = true;
    }

    pub(super) fn cancel_sign_in(&mut self) {
        if let Some(s) = self.signing.take() {
            s.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Signs in with pasted tokens: a refresh token is renewed into a session first.
    pub(super) fn use_pasted(
        &mut self,
        ctx: &egui::Context,
        token: Option<String>,
        refresh: Option<String>,
    ) {
        self.busy = true;
        self.failed = None;
        self.worker.spawn(ctx, move |tx| {
            let tokens = match (token, refresh) {
                (Some(token), refresh) => Ok(Tokens {
                    token,
                    refresh_token: refresh,
                }),
                (None, Some(rt)) => vrce::refresh(&rt).map_err(|e| {
                    if vrce::is_signed_out(&e) {
                        Failed::from("EchoVRCE didn't accept that refresh token.".to_string())
                    } else {
                        Failed::of(&e)
                    }
                }),
                (None, None) => Err(Failed::from("There is no token in that.".to_string())),
            };
            tx.send(Msg::SignedIn(tokens.and_then(finish_sign_in)));
        });
    }

    /// Links a second session of the account for the site inside the window.
    fn link_site(&mut self, ctx: &egui::Context) {
        let (Some(tokens), Some(account)) = (self.tokens.clone(), self.account.clone()) else {
            return;
        };
        self.linking_site = true;
        self.worker.spawn(ctx, move |tx| {
            let r = vrce::link_device(&tokens.token)
                .map(|t| (account.id.clone(), t))
                .map_err(|e| format!("{e:#}"));
            tx.send(Msg::SiteLinked(r));
        });
    }

    /// Ends the session here and on EchoVRCE.
    pub(super) fn sign_out(&mut self, ctx: &egui::Context) {
        self.cancel_sign_in();
        let tokens = self.tokens.take();
        self.account = None;
        self.ended = false;
        self.next_check = None;
        self.worker.spawn(ctx, move |_| {
            if let Some(t) = tokens {
                vrce::logout(&t);
            }
            vrce::store::forget();
        });
    }
}

/// The game's own EchoVRCE sign-in (nEVR's `_local/.credentials.json`), for the account the
/// launcher is signed in with: each live version with nEVR that has no good one for it
/// gets a session linked for it (a device of the account, so the game's and the launcher's
/// refresh tokens never rotate each other out). Checked every ten minutes; signing out
/// takes them away.
#[derive(Default)]
pub(super) struct GameSignIn {
    /// Whether every version that needed a sign-in got one.
    worker: Worker<bool>,
    busy: bool,
    account: Option<String>,
    next: Option<Instant>,
}

impl GameSignIn {
    pub(super) fn tick(
        &mut self,
        ctx: &egui::Context,
        session: Option<(Tokens, Account)>,
        versions: &[InstalledVersion],
        demo: bool,
    ) {
        if demo {
            return;
        }
        for all_good in self.worker.drain() {
            self.busy = false;
            // A sign-in that couldn't be written or linked: each try links another device
            // of the account, so wait longer before the next.
            if !all_good {
                self.next = Some(Instant::now() + RETRY_GAME_SIGN_IN);
            }
        }
        let id = session.as_ref().map(|(_, a)| a.id.clone());
        if self.account != id {
            // Another account (or none): look again now. A session that ended keeps the
            // game's sign-in; only signing out takes it away.
            self.account = id;
            self.next = None;
        }
        let Some((tokens, account)) = session else {
            return;
        };
        if self.busy || self.next.is_some_and(|t| Instant::now() < t) {
            return;
        }
        self.busy = true;
        self.next = Some(Instant::now() + CHECK_EVERY);
        let live: Vec<InstalledVersion> = versions
            .iter()
            .filter(|v| v.publisher_lock.is_none() && v.present())
            .cloned()
            .collect();
        self.worker.spawn(ctx, move |tx| {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            let mut all_good = true;
            for v in &live {
                if !crate::core::launcher::mods::nevr_in(&v.bin_dir())
                    || nevr::read_login(v).is_some_and(|l| l.good_for(&account.id, now))
                {
                    continue;
                }
                let linked = vrce::link_device(&tokens.token).and_then(|t| {
                    let rt = t
                        .refresh_token
                        .ok_or_else(|| anyhow::anyhow!("EchoVRCE gave no refresh token"))?;
                    let login = GameLogin::new(&rt, &account.id, &account.username, now);
                    nevr::write_login(v, &login)
                });
                match linked {
                    Ok(()) => tracing::info!("{}: the game has its own EchoVRCE sign-in", v.id),
                    Err(e) => {
                        all_good = false;
                        tracing::warn!("{}: no EchoVRCE sign-in for the game: {e:#}", v.id)
                    }
                }
            }
            tx.send(all_good);
        });
    }

    /// Signing out: the games' sign-ins for `account` go too.
    pub(super) fn signed_out(
        &mut self,
        ctx: &egui::Context,
        account: &str,
        versions: &[InstalledVersion],
    ) {
        let (account, versions) = (account.to_string(), versions.to_vec());
        self.account = None;
        self.worker.spawn(ctx, move |_| {
            for v in &versions {
                nevr::forget_login(v, &account);
            }
        });
    }
}

/// When to check `tokens` next: every ten minutes, and before the session gets too close
/// to running out to be renewed.
fn next_check(tokens: Option<&Tokens>) -> Instant {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let until_due = tokens
        .and_then(Tokens::expires)
        .map(|exp| (exp - now - vrce::MARGIN.as_secs() as i64).max(0) as u64)
        .map_or(CHECK_EVERY, |s| Duration::from_secs(s).min(CHECK_EVERY));
    Instant::now() + until_due
}

impl Vrce {
    /// The signed-in session and account, for calls made with it.
    pub(super) fn session(&self) -> Option<(Tokens, Account)> {
        self.tokens.clone().zip(self.account.clone())
    }
}

/// The other pages' sign-in prompt: what signing in gives (`intro`), or, while
/// EchoVRCE isn't answering, that it isn't (`titled`: the card's title says so already).
/// Returns the text's height.
pub(super) fn prompt_text(
    d: &Dashboard,
    kit: &Kit,
    (x, y, w): (f32, f32, f32),
    intro: &str,
    size: f32,
    titled: bool,
) -> f32 {
    let waiting = d.vrce.waiting_for_server();
    let (text, color) = match d.vrce.down_note(!(waiting && titled)) {
        Some(note) if waiting && titled => (note, design::BODY),
        Some(note) if waiting => (note, design::QUEST_WARN),
        Some(note) => (format!("{intro}\n\n{note}"), design::BODY),
        None => (intro.to_string(), design::BODY),
    };
    kit.caps_text(x, y, w, &text, size, color, dz(10.0))
}

/// Sign in (it opens this page), or Retry now while signed in and EchoVRCE isn't
/// answering, at (`x`, `y`).
pub(super) fn sign_in_button(
    d: &mut Dashboard,
    kit: &mut Kit,
    ctx: &egui::Context,
    key: &str,
    x: f32,
    y: f32,
) {
    if d.vrce.waiting_for_server() {
        let label = "Retry now";
        let bw = kit
            .button_width(label, Some(Icon::Refresh), BTN_H)
            .max(dz(180.0));
        if kit
            .button(
                key,
                x,
                y,
                bw,
                BTN_H,
                Tone::Blue,
                Some(Icon::Refresh),
                label,
                !d.vrce.busy,
                "Ask EchoVRCE again now",
            )
            .clicked
        {
            d.vrce.retry_now(ctx);
        }
        return;
    }
    let bw = kit.button_width("Sign in", None, BTN_H).max(dz(180.0));
    if kit
        .button(
            key,
            x,
            y,
            bw,
            BTN_H,
            Tone::Go,
            None,
            "Sign in",
            !d.vrce.busy,
            "Sign in with EchoVRCE",
        )
        .clicked
    {
        d.page = super::Page::EchoVrce;
        d.vrce.sign_in(ctx);
    }
}

/// A new session: checked against the account (renewed if it must be), then saved.
fn finish_sign_in(tokens: Tokens) -> Result<(Tokens, Account), Failed> {
    let (renewed, account) = vrce::check(&tokens).map_err(|e| {
        if vrce::is_signed_out(&e) {
            Failed::from("EchoVRCE didn't accept those tokens.".to_string())
        } else {
            Failed::of(&e)
        }
    })?;
    let tokens = renewed.unwrap_or(tokens);
    vrce::store::save(&tokens)
        .map_err(|e| Failed::from(format!("Couldn't save the session: {e:#}")))?;
    Ok((tokens, account))
}

// ---- the page ----

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let signed_in = d.vrce.account.is_some() && d.vrce.tokens.is_some() && d.vrce.signing.is_none();
    // Signed in: the site itself (snapshots show where it goes).
    if signed_in && (crate::ui::web::supported() || d.demo) {
        site(d, kit, ctx);
        return;
    }
    if !signed_in && d.web.is_open() {
        d.web.close();
    }
    let half = kit.dx();
    account_card(d, kit, ctx, ACCOUNT.wider(half));
    panel::at_right(kit, |k| about_card(k, ABOUT));
}

/// Signs out here, on EchoVRCE, and in the site inside the window.
fn sign_out(d: &mut Dashboard, ctx: &egui::Context) {
    if let Some(a) = &d.vrce.account {
        d.game_sign_in.signed_out(ctx, &a.id, &d.state.versions);
    }
    d.web.run(&vrce::site_sign_in_script("", None));
    d.web.close();
    d.state.vrce_site_account = None;
    d.save();
    d.vrce.sign_out(ctx);
}

/// Takes in a session linked for the site: opens the site signed in with it.
pub(super) fn site_linked(d: &mut Dashboard) {
    match d.vrce.site_linked.take() {
        Some(Ok((account, tokens))) => {
            d.state.vrce_site_account = Some(account.clone());
            d.save();
            d.web.open(
                SITE_HOME,
                &vrce::site_sign_in_script(&account, Some(&tokens)),
            );
        }
        Some(Err(why)) => d.web.error = Some(why),
        None => {}
    }
}

/// The site under the header strip, with the account and Reload, In browser and Sign out
/// on the strip.
fn site(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let account = d.vrce.account.clone().unwrap_or_default();
    let header = HEADER.wider(kit.dx());
    let cy = header.y + header.h / 2.0;
    // Right to left: Sign out, In browser, Reload, then who is signed in.
    let mut right = header.right() - 22.0;
    let mut link = |kit: &mut Kit, key: &str, text: &str, tip: &str| {
        let g = kit.spaced_galley(
            &text.to_uppercase(),
            design::din(14.0),
            design::TEXT,
            dz(0.5),
            true,
        );
        let w = g.size().x / dz(1.0);
        right -= w;
        let clicked = kit
            .link(key, dz(right), dz(cy) - g.size().y / 2.0, text, 14.0, tip)
            .clicked;
        right -= 26.0;
        clicked
    };
    if link(
        kit,
        "vrce-site-sign-out",
        "Sign out",
        "Sign out here and on EchoVRCE",
    ) {
        sign_out(d, ctx);
        return;
    }
    if link(
        kit,
        "vrce-site-browser",
        "In browser",
        "Open echovrce.com in your browser",
    ) {
        crate::core::platform::open_url(SITE_HOME);
    }
    if link(kit, "vrce-site-reload", "Reload", "Load the page again") {
        d.web.reload();
    }
    let who = kit.spaced_galley(
        &format!("Signed in as {}", account.name()).to_uppercase(),
        design::din(14.0),
        design::GREY,
        dz(0.5),
        false,
    );
    let wx = dz(right) - who.size().x;
    kit.put(wx, dz(cy) - who.size().y / 2.0, who);

    let area = SITE.wider(kit.dx()).taller(kit.dy());
    kit.image_d("card_bg.png", area);
    kit.gradient_frame(
        kit.drect(area),
        dz(6.0),
        dz(2.0),
        design::RIM_TOP,
        design::RIM_BOTTOM,
    );
    let inner = kit.drect(area).shrink(dz(2.0));
    let (tx, ty, tw) = (dz(area.x + 30.0), dz(area.y + 30.0), dz(area.w - 60.0));
    // Drawn unseen to warm the page up: nothing is opened or linked, nor placed.
    if kit.ghost {
        return;
    }
    if d.web.is_open() {
        d.web.place(inner);
        kit.caps_text(tx, ty, tw, "Opening echovrce.com…", 17.0, design::BODY, 0.0);
    } else if let Some(err) = d.web.error.clone() {
        let text = format!("echovrce.com couldn't be shown here: {err}");
        let h = kit.caps_text(tx, ty, tw, &text, 17.0, design::DANGER, 0.0);
        if kit
            .link(
                "vrce-site-retry",
                tx,
                ty + h + dz(20.0),
                "Try again",
                16.5,
                "",
            )
            .clicked
        {
            d.web.error = None;
            d.state.vrce_site_account = None;
        }
    } else if d.vrce.linking_site || d.demo {
        kit.caps_text(tx, ty, tw, "Opening echovrce.com…", 17.0, design::BODY, 0.0);
    } else if d.state.vrce_site_account.as_deref() == Some(account.id.as_str()) {
        // Signed in there before: the site keeps its own session.
        d.web
            .open(SITE_HOME, &vrce::site_sign_in_script(&account.id, None));
        // Placed (and made) on the next frame.
        ctx.request_repaint();
    } else {
        d.vrce.link_site(ctx);
    }
}

/// The session's end, for the account card: "until 14:32" (local time).
fn valid_until(tokens: &Tokens) -> Option<String> {
    let exp = time::OffsetDateTime::from_unix_timestamp(tokens.expires()?).ok()?;
    let local = crate::core::launcher::feed::local(exp);
    Some(format!("{:02}:{:02}", local.hour(), local.minute()))
}

fn account_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context, r: Dr) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "Your account");
    let by = bottom - BTN_H;
    let mut bx = x;
    let mut button = |k: &mut Kit,
                      key: &str,
                      tone: Tone,
                      icon: Option<Icon>,
                      label: &str,
                      enabled: bool,
                      tip: &str| {
        let bw = k.button_width(label, icon, BTN_H).max(dz(180.0));
        let clicked = k
            .button(key, bx, by, bw, BTN_H, tone, icon, label, enabled, tip)
            .clicked;
        bx += bw + dz(14.0);
        clicked
    };

    if let Some((code, left)) = d.vrce.signing.as_ref().map(|s| {
        (
            s.code.code.clone(),
            s.code
                .expires_in
                .saturating_sub(s.since.elapsed())
                .as_secs(),
        )
    }) {
        // Waiting for the code to be approved.
        k.caption(x, y, "Your sign-in code");
        let g = k.spaced_galley(&code, design::conthrax(44.0), design::TEXT, dz(6.0), false);
        let gh = k.put(x, y + dz(30.0), g).height();
        let text = format!(
            "Approve it on echovrce.com: the page opened in your browser with the code filled in. Waiting for you… ({}:{:02} left)",
            left / 60,
            left % 60
        );
        k.caps_text(
            x,
            y + dz(30.0) + gh + dz(20.0),
            w,
            &text,
            16.0,
            design::BODY,
            0.0,
        );
        ctx.request_repaint_after(Duration::from_millis(500));
        let url = vrce::approve_url(&code);
        if button(
            k,
            "vrce-reopen",
            Tone::Blue,
            Some(Icon::Globe),
            "Open the page again",
            true,
            &url,
        ) {
            crate::core::platform::open_url(&url);
        }
        if button(k, "vrce-cancel", Tone::Dark, None, "Cancel", true, "") {
            d.vrce.cancel_sign_in();
        }
        return;
    }

    match (d.vrce.account.clone(), d.vrce.tokens.clone()) {
        (Some(acc), Some(tokens)) => {
            k.caption(x, y, "Signed in as");
            let g = k.spaced_fit(
                &acc.name().to_uppercase(),
                design::conthrax(30.0),
                design::TEXT,
                dz(3.0),
                false,
                w,
            );
            let mut ty = y + dz(30.0) + k.put(x, y + dz(30.0), g).height() + dz(8.0);
            if !acc.username.is_empty() && acc.username != acc.name() {
                let g = super::install::myriad(
                    k,
                    &format!("@{}", acc.username),
                    design::myriad(19.0),
                    design::SUBTLE,
                    w,
                    true,
                );
                ty += k.put(x, ty, g).height();
            }
            ty += dz(30.0);
            k.caption(x, ty, "Session");
            let session = match (valid_until(&tokens), tokens.refresh_token.is_some()) {
                (Some(t), true) => format!("Valid until {t}; renews by itself before then"),
                (Some(t), false) => {
                    format!("Valid until {t}. Pasted without a refresh token, it can't be renewed")
                }
                (None, _) => "Signed in".to_string(),
            };
            k.caps_text(x, ty + dz(28.0), w, &session, 16.0, design::TEXT, 0.0);
            if button(
                k,
                "vrce-open",
                Tone::Blue,
                Some(Icon::Globe),
                "Open EchoVRCE",
                true,
                vrce::WEB,
            ) {
                crate::core::platform::open_url(vrce::WEB);
            }
            if button(
                k,
                "vrce-sign-out",
                Tone::Dark,
                None,
                "Sign out",
                true,
                "Forget this session here and end it on EchoVRCE",
            ) {
                sign_out(d, ctx);
            }
        }
        (None, Some(_)) => {
            // Signed in, but EchoVRCE isn't answering: say so, and keep trying.
            let Some(note) = d.vrce.down_note(false) else {
                k.caps_text(
                    x,
                    y,
                    w,
                    "Checking your EchoVRCE session…",
                    17.0,
                    design::BODY,
                    0.0,
                );
                return;
            };
            let ty =
                y + k.caps_text(
                    x,
                    y,
                    w,
                    "Signed in, but EchoVRCE isn't answering right now",
                    19.0,
                    design::QUEST_WARN,
                    0.0,
                ) + dz(16.0);
            k.caps_text(x, ty, w, &note, 16.0, design::BODY, 0.0);
            if button(
                k,
                "vrce-retry",
                Tone::Blue,
                Some(Icon::Refresh),
                "Retry now",
                !d.vrce.busy,
                "Ask EchoVRCE again now",
            ) {
                d.vrce.retry_now(ctx);
            }
            if button(
                k,
                "vrce-open",
                Tone::Dark,
                Some(Icon::Globe),
                "Open EchoVRCE",
                true,
                vrce::WEB,
            ) {
                crate::core::platform::open_url(vrce::WEB);
            }
            if button(
                k,
                "vrce-sign-out",
                Tone::Dark,
                None,
                "Sign out",
                true,
                "Forget this session here and end it on EchoVRCE",
            ) {
                sign_out(d, ctx);
            }
        }
        _ => {
            let mut ty = y;
            // Why the last sign-in didn't work, and that EchoVRCE isn't answering.
            if let Some(why) = &d.vrce.failed {
                ty += k.caps_text(
                    x,
                    ty,
                    w,
                    &format!("Signing in didn't work: {}", why.trim_end_matches('.')),
                    16.0,
                    design::DANGER,
                    0.0,
                ) + dz(14.0);
            }
            // Under a failure that says why already: only what to do.
            let note = match (&d.vrce.failed, &d.vrce.down) {
                (Some(_), Some(_)) => Some(
                    "Try again in a few minutes: signing in works once EchoVRCE answers again."
                        .to_string(),
                ),
                _ => d.vrce.down_note(true),
            };
            if let Some(note) = note {
                ty += k.caps_text(x, ty, w, &note, 16.0, design::QUEST_WARN, 0.0) + dz(20.0);
            }
            if d.vrce.ended {
                ty += k.caps_text(
                    x,
                    ty,
                    w,
                    "Your session ended and couldn't be renewed: sign in again.",
                    17.0,
                    design::DANGER,
                    0.0,
                ) + dz(20.0);
            }
            k.caps_text(
                x,
                ty,
                w,
                "Sign in with your EchoVRCE account. The launcher shows a code, you approve it on echovrce.com, and it stays signed in: the session renews by itself.\n\nOr paste tokens you already have.",
                17.0,
                design::BODY,
                dz(18.0),
            );
            let busy = d.vrce.busy;
            if button(
                k,
                "vrce-sign-in",
                Tone::Go,
                None,
                "Sign in",
                !busy,
                "Get a code to approve on echovrce.com",
            ) {
                d.vrce.sign_in(ctx);
            }
            if button(
                k,
                "vrce-tokens",
                Tone::Dark,
                None,
                "Enter tokens…",
                !busy,
                "Paste a refresh token, a token, or an authorization header",
            ) {
                d.overlay = Some(setup::Overlay::VrceTokens {
                    input: String::new(),
                });
            }
            if button(
                k,
                "vrce-open",
                Tone::Dark,
                Some(Icon::Globe),
                "Open EchoVRCE",
                true,
                vrce::WEB,
            ) {
                crate::core::platform::open_url(vrce::WEB);
            }
        }
    }
}

/// What EchoVRCE is, and its website.
fn about_card(k: &mut Kit, r: Dr) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "EchoVRCE");
    k.caps_text(
        x,
        y,
        w,
        "EchoVRCE runs the community's Echo VR servers. Your account there is who you are in game: your name, your matches and your stats are on echovrce.com.",
        16.0,
        design::BODY,
        0.0,
    );
    if k.link(
        "vrce-site",
        x,
        bottom - dz(22.0),
        "Open echovrce.com",
        16.5,
        vrce::WEB,
    )
    .clicked
    {
        crate::core::platform::open_url(vrce::WEB);
    }
}

/// "Enter tokens": paste what you have; the launcher signs in with it.
pub(super) fn tokens_card(d: &mut Dashboard, k: &mut Kit, ctx: &egui::Context) {
    let Some(setup::Overlay::VrceTokens { input }) = &mut d.overlay else {
        return;
    };
    let (w, h) = (dz(960.0), dz(330.0));
    let (x, y, cw, bottom) = setup::card(k, w, h, "Sign in with tokens");
    let hint = "Paste your EchoVRCE refresh token (it keeps you signed in), a session token, or a copied authorization header. They stay on this PC.";
    let th = k.caps_text(x, y, cw, hint, 17.0, design::BODY, 0.0);
    let fy = y + th + dz(20.0);
    let pw = k.button_width("Paste", None, BTN_H).max(100.0);
    let parsed = vrce::parse_pasted(input);
    let invalid = !input.trim().is_empty() && parsed.is_none();
    k.field(
        "vrce-token",
        input,
        x,
        fy,
        cw - pw - 10.0,
        BTN_H,
        "eyJ… or a refresh token",
        invalid,
        "Your EchoVRCE tokens",
    );
    if k.button(
        "vrce-paste",
        x + cw - pw,
        fy,
        pw,
        BTN_H,
        Tone::Dark,
        None,
        "Paste",
        true,
        "Paste from your clipboard",
    )
    .clicked
    {
        if let Some(clip) = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
        {
            *input = clip.trim().to_string();
        }
    }
    if invalid {
        k.caps_text(
            x,
            fy + BTN_H + dz(12.0),
            cw,
            "There is no token in that.",
            16.0,
            design::DANGER,
            0.0,
        );
    }
    let by = bottom - BTN_H;
    let uw = k.button_width("Sign in", None, BTN_H).max(140.0);
    let cw2 = k.button_width("Cancel", None, BTN_H).max(110.0);
    let right = x + cw;
    if k.button(
        "vrce-tokens-cancel",
        right - cw2,
        by,
        cw2,
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
    let go = k
        .button(
            "vrce-tokens-use",
            right - cw2 - 8.0 - uw,
            by,
            uw,
            BTN_H,
            Tone::Go,
            None,
            "Sign in",
            parsed.is_some(),
            "Sign in with these",
        )
        .clicked;
    if let (true, Some((token, refresh))) = (go, parsed) {
        d.overlay = None;
        d.vrce.use_pasted(ctx, token, refresh);
    }
}
