//! RIGHT NOW, the Play page's right-hand panel: what is going on (players online, the
//! queue, the matches per mode and the week's top 3, from the status feed), then, signed
//! in with EchoVRCE, the invites waiting for you and your friends with the match each is
//! in, to JOIN. The tiles are the Servers page's.

use super::{panel, servers, Dashboard};
use crate::core::launcher::feed::Servers;
use crate::ui::design::{self, dz};
use crate::ui::kit::Kit;

/// The panel's content width and its padding at the bottom (design pixels).
const W: f32 = panel::PANEL.w - 2.0 * (panel::X - panel::PANEL.x);
const BOTTOM_PAD: f32 = 31.0;
/// A number's row: the number, then its caption.
const STAT_H: f32 = 76.0;
/// Invites shown at most, so the friends keep their room.
const MAX_INVITES: usize = 2;

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    panel::frame(kit, "Right now");
    let (x, w) = (dz(panel::X), dz(W));
    let bottom = dz(panel::PANEL.bottom() + kit.dy() - BOTTOM_PAD);
    let mut y = dz(panel::BODY_Y);
    y = match &d.feed.status {
        Some(s) => overview(kit, s, x, y, w),
        None => {
            let text = if d.feed.status_failed {
                "What's going on is unavailable right now."
            } else {
                "Loading..."
            };
            y + kit.caps_text(x, y, w, text, 15.0, design::GREY, 0.0)
        }
    };
    y += dz(30.0);

    let Some((_, account)) = d.vrce.session() else {
        signed_out(d, kit, ctx, x, y, w);
        return;
    };
    y = servers::invite_tiles(d, kit, ctx, x, y, w, bottom, MAX_INVITES);
    kit.caption(x, y, "Friends");
    let caption_y = y;
    y += dz(28.0);
    let mut scroll = d.servers.now_scroll;
    let playing = servers::friend_tiles(
        d,
        kit,
        ctx,
        &account,
        "now-friends",
        (x, y, w, bottom),
        &mut scroll,
        false,
    );
    d.servers.now_scroll = scroll;
    if let Some(n) = playing {
        let g = kit.label_galley(
            &format!("{n} in a match"),
            design::din(14.0),
            design::GREY,
            f32::INFINITY,
        );
        kit.put(x + w - g.size().x, caption_y, g);
    }
}

/// Players online, the queue and the matches running, a number each; the matches per mode;
/// the week's top 3. Returns where the next thing goes.
fn overview(kit: &Kit, s: &Servers, x: f32, y: f32, w: f32) -> f32 {
    let m = &s.modes;
    let matches = m.lobby.public.matches + m.arena.public.matches + m.combat.public.matches;
    let mut stats = vec![(s.players.online.to_string(), "Online".to_string())];
    if let Some(q) = &s.queue {
        let wait = q
            .wait_s
            .filter(|_| q.total > 0)
            .map(|w| format!(" ~{}:{:02}", w / 60, w % 60))
            .unwrap_or_default();
        stats.push((q.total.to_string(), format!("In queue{wait}")));
    }
    stats.push((matches.to_string(), "Matches".to_string()));
    let col = w / stats.len() as f32;
    for (i, (value, caption)) in stats.iter().enumerate() {
        let cx = x + i as f32 * col;
        let g = kit.label_galley(value, design::conthrax(30.0), design::TEXT, col);
        kit.put(cx, y, g);
        kit.caption(cx, y + dz(44.0), caption);
    }
    let mut y = y + dz(STAT_H);
    if s.status != "ok" {
        let g = kit.label_galley(
            "These numbers may be out of date.",
            design::din(13.0),
            design::GREY,
            w,
        );
        y = kit.put(x, y - dz(6.0), g).max.y - kit.origin.y + dz(12.0);
    }

    // "Arena 6 · Combat 2 · Social 11": the public matches of each mode.
    let modes = [
        ("Arena", m.arena.public.matches),
        ("Combat", m.combat.public.matches),
        ("Social", m.lobby.public.matches),
    ]
    .map(|(name, n)| format!("{name} {n}"))
    .join("   ·   ");
    let g = kit.label_galley(&modes, design::din(16.0), design::BODY, w);
    y = kit.put(x, y, g).max.y - kit.origin.y + dz(22.0);

    if let Some(top) = s.top.as_ref().filter(|t| !t.entries.is_empty()) {
        kit.caption(x, y, "Top Arena this week");
        y += dz(28.0);
        let col = w / 3.0;
        for (i, e) in top.entries.iter().take(3).enumerate() {
            let cx = x + i as f32 * col;
            let name = kit.label_galley(
                &format!("{}. {}", e.rank, e.name),
                design::din(16.0),
                design::TEXT,
                col - dz(10.0),
            );
            kit.put(cx, y, name);
            let wins = kit.label_galley(
                &format!("{} wins", e.wins),
                design::din(13.0),
                design::GREY,
                col - dz(10.0),
            );
            kit.put(cx, y + dz(24.0), wins);
        }
        y += dz(46.0);
    }
    y
}

/// Not signed in: what signing in gives, and SIGN IN (or, signed in while EchoVRCE
/// isn't answering, that it isn't, and Retry now).
fn signed_out(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context, x: f32, y: f32, w: f32) {
    let th = super::echovrce::prompt_text(
        d,
        kit,
        (x, y, w),
        "Sign in with EchoVRCE to see your friends, the matches they are in, and join them.",
        15.0,
        false,
    );
    super::echovrce::sign_in_button(d, kit, ctx, "now-sign-in", x, y + th + dz(18.0));
}
