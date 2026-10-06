//! FRIENDS: find players by name and ask them to be friends, answer the requests you got
//! (and take back the ones you sent), your friends with the matches they are in (JOIN,
//! INVITE, remove), and the players of your last matches to add. It all comes from
//! EchoVRCE with your session (`servers::Servers` fetches it) and stays in memory.

use egui::Color32;

use super::{hero, panel, servers, Dashboard};
use crate::core::echovrce::game::{self, FriendState};
use crate::core::echovrce::Account;
use crate::core::launcher::feed;
use crate::ui::design::{self, dz, Dr};
use crate::ui::dialogs::Icon as DlgIcon;
use crate::ui::kit::Kit;
use crate::ui::widgets::{Tone, BTN_H};

/// FIND PLAYERS on the left, FRIENDS in the middle (sharing the extra width), PLAYED WITH
/// in the right-hand column.
/// (Under the line saying where friends come from.)
const FIND: Dr = Dr::new(137.0, 118.0, 560.0, 928.0);
const LIST: Dr = Dr::new(717.0, 118.0, 581.0, 928.0);
const PLAYED: Dr = Dr::new(1318.0, 118.0, 555.0, 928.0);
/// Where friends come from, over the cards.
const ABOUT_Y: f32 = 74.0;
const ROW_H: f32 = 64.0;
const ROW_GAP: f32 = 10.0;
const PAD: f32 = 14.0;
/// How many of your last matches PLAYED WITH lists the players of.
pub(super) const RECENT_MATCHES: usize = 10;
const REMOVE_KEY: &str = "friends-remove";

pub(super) fn show(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    if !kit.ghost {
        answers(d, ctx);
    }
    let (half, dy) = (kit.dx() / 2.0, kit.dy());
    let about = format!(
        "Your friends are kept on your EchoVRCE account, not on Discord. Find players by their \
         in-game name; Played with lists the players of your last {RECENT_MATCHES} Arena and \
         Combat matches."
    );
    let w = PLAYED.x + PLAYED.w - FIND.x + kit.dx();
    kit.caps_text(
        dz(FIND.x + 4.0),
        dz(ABOUT_Y),
        dz(w - 8.0),
        &about,
        14.0,
        design::SUBTLE,
        0.0,
    );
    let Some((tokens, account)) = d.vrce.session() else {
        signed_out(d, kit, ctx);
        return;
    };
    let token = tokens.token;
    find_card(d, kit, ctx, &token, &account, FIND.wider(half).taller(dy));
    list_card(
        d,
        kit,
        ctx,
        &token,
        &account,
        LIST.moved(half).wider(half).taller(dy),
    );
    panel::at_right(kit, |k| {
        played_card(d, k, ctx, &token, &account, PLAYED.taller(dy))
    });
}

/// "Remove <name>?": yes removes them.
fn answers(d: &mut Dashboard, ctx: &egui::Context) {
    let Some(a) = d.dialogs.take(REMOVE_KEY) else {
        return;
    };
    let (Some((user, name)), Some((tokens, _))) = (d.servers.remove_asked.take(), d.vrce.session())
    else {
        return;
    };
    if a.is_yes() {
        d.servers
            .change_friend(ctx, &tokens.token, &user, &name, false);
    }
}

fn signed_out(d: &mut Dashboard, kit: &mut Kit, ctx: &egui::Context) {
    let r = Dr::new(FIND.x, FIND.y, FIND.w + LIST.w + 20.0 + kit.dx(), 300.0);
    let title = if d.vrce.waiting_for_server() {
        "EchoVRCE isn't answering"
    } else {
        "Sign in to find friends"
    };
    let (x, y, w, bottom) = hero::card_frame(kit, r, title);
    super::echovrce::prompt_text(d, kit, (x, y, w), "Finding players, friend requests, your friends and the players you played with come from EchoVRCE: sign in with your EchoVRCE account.", 17.0, true);
    super::echovrce::sign_in_button(d, kit, ctx, "friends-sign-in", x, bottom - BTN_H);
}

/// A player's row: a tile with their name and a grey line under it (and a dot when
/// `dot` is set). Returns where its action goes: the right edge and the button's top.
fn row(
    k: &mut Kit,
    x: f32,
    ty: f32,
    w: f32,
    name: &str,
    sub: &str,
    dot: Option<Color32>,
) -> (f32, f32) {
    let h = dz(ROW_H);
    hero::tile(k, x, ty, w, h, false);
    let pad = dz(PAD);
    let mut tx = x + pad;
    if let Some(c) = dot {
        let at = k.rect(x + pad + dz(5.0), ty + h / 2.0, 0.0, 0.0).min;
        k.ui.painter().circle_filled(at, dz(5.0), c);
        tx += dz(22.0);
    }
    let text_w = w - (tx - x) - dz(250.0);
    let g = k.label_galley(name, design::din(16.0), design::TEXT, text_w);
    k.put(tx, ty + dz(11.0), g);
    let g = k.label_galley(sub, design::din(13.0), design::GREY, text_w);
    k.put(tx, ty + dz(35.0), g);
    (x + w - pad, ty + (h - dz(36.0)) / 2.0)
}

/// What can be done with player `user`, at a row's right: Add, Accept, or where you
/// stand. Returns `true` when Add or Accept was clicked.
fn add_action(
    d: &Dashboard,
    k: &mut Kit,
    key: &str,
    user: &str,
    me: &str,
    right: f32,
    by: f32,
) -> bool {
    let chip = |k: &mut Kit, text: &str, color: Color32| {
        let cw = k.chip_width(text);
        k.chip(right - cw, by + dz(4.5), text, color);
        false
    };
    let busy = d.servers.changing.contains(user);
    let bw = dz(120.0);
    match d.servers.friendship(user) {
        _ if user == me => chip(k, "You", design::QUEST_OFF),
        Some(FriendState::Friend) => chip(k, "Friend", design::QUEST_ON),
        Some(FriendState::Sent) => chip(k, "Requested", design::QUEST_OFF),
        Some(FriendState::Blocked) => chip(k, "Blocked", design::QUEST_OFF),
        Some(FriendState::Received) => {
            k.button(
                key,
                right - bw,
                by,
                bw,
                dz(36.0),
                Tone::Go,
                None,
                "Accept",
                !busy,
                "They asked you: accept their friend request",
            )
            .clicked
        }
        None => {
            k.button(
                key,
                right - bw,
                by,
                bw,
                dz(36.0),
                Tone::Blue,
                None,
                "Add",
                !busy,
                "Send them a friend request",
            )
            .clicked
        }
    }
}

/// A grey line in a card (a hint, or why the list is empty).
fn note(k: &mut Kit, x: f32, y: f32, w: f32, text: &str) {
    k.caps_text(x, y, w, text, 15.0, design::GREY, 0.0);
}

/// FIND PLAYERS: the name field, Search, and who it found.
fn find_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    token: &str,
    account: &Account,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "Find players");
    let bw = k.button_width("Search", None, BTN_H).max(dz(120.0));
    let ended = k.field(
        "friends-search",
        &mut d.servers.search,
        x,
        y,
        w - bw - dz(10.0),
        BTN_H,
        "A player's name",
        false,
        "Their name in Echo VR (EchoVRCE display name)",
    );
    let text = d.servers.search.trim().to_string();
    let can = game::search_pattern(&text).is_some() && !d.servers.searching;
    let enter = ended && ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let clicked = k
        .button(
            "friends-search-go",
            x + w - bw,
            y,
            bw,
            BTN_H,
            Tone::Blue,
            None,
            "Search",
            can,
            "Find players with this name",
        )
        .clicked;
    if can && (clicked || enter) {
        d.servers.search_players(ctx, token, &text);
    }
    let top = y + BTN_H + dz(20.0);
    let found = match &d.servers.found {
        _ if d.servers.searching => {
            note(k, x, top, w, "Searching…");
            return;
        }
        None => {
            note(
                k,
                x,
                top,
                w,
                "Type at least two letters of a player's name, then Search. Or add the players of your last matches, on the right.",
            );
            return;
        }
        Some((_, Err(e))) => {
            note(k, x, top, w, &format!("Couldn't search: {e}"));
            return;
        }
        Some((text, Ok(v))) if v.is_empty() => {
            note(k, x, top, w, &format!("No player is named like “{text}”."));
            return;
        }
        Some((_, Ok(v))) => v.clone(),
    };
    let pitch = dz(ROW_H + ROW_GAP);
    let list_h = bottom - top;
    let mut scroll = d.servers.friends_page_scroll[0];
    k.scroll_area(
        "friends-found",
        x,
        top,
        w + dz(16.0),
        list_h,
        found.len() as f32 * pitch - dz(ROW_GAP),
        &mut scroll,
    );
    d.servers.friends_page_scroll[0] = scroll;
    let mut add = None;
    k.clipped(x, top, w, list_h, |k| {
        for (i, f) in found.iter().enumerate() {
            let ty = top - scroll + i as f32 * pitch;
            if ty + pitch < top || ty > bottom {
                continue;
            }
            let sub = if f.username.is_empty() || f.username.eq_ignore_ascii_case(&f.name) {
                String::new()
            } else {
                format!("@{}", f.username)
            };
            let (right, by) = row(k, x, ty, w, &f.name, &sub, None);
            let key = format!("found-add-{}", f.user_id);
            if add_action(d, k, &key, &f.user_id, &account.id, right, by) {
                add = Some((f.user_id.clone(), f.name.clone()));
            }
        }
    });
    if let Some((user, name)) = add {
        d.servers.change_friend(ctx, token, &user, &name, true);
    }
}

/// FRIENDS: requests first (Accept and Decline, or Cancel for yours), then your friends
/// with their matches.
fn list_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    token: &str,
    account: &Account,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "Friends");
    let mut requests = d.servers.requests.clone();
    // Theirs first.
    requests.sort_by_key(|f| {
        (
            f.state != FriendState::Received,
            f.name.to_ascii_lowercase(),
        )
    });
    let mut top = y;
    if !requests.is_empty() {
        k.caption(x, top, "Requests");
        top += dz(28.0);
        let pitch = dz(ROW_H + ROW_GAP);
        // At most half the card; the rest scroll.
        let list_h = (requests.len() as f32 * pitch - dz(ROW_GAP)).min((bottom - top) / 2.0);
        let mut scroll = d.servers.friends_page_scroll[1];
        k.scroll_area(
            "friends-requests",
            x,
            top,
            w + dz(16.0),
            list_h,
            requests.len() as f32 * pitch - dz(ROW_GAP),
            &mut scroll,
        );
        d.servers.friends_page_scroll[1] = scroll;
        let mut change = None;
        let changing = &d.servers.changing;
        k.clipped(x, top, w, list_h, |k| {
            for (i, f) in requests.iter().enumerate() {
                let ty = top - scroll + i as f32 * pitch;
                if ty + pitch < top || ty > top + list_h {
                    continue;
                }
                let theirs = f.state == FriendState::Received;
                let sub = if theirs {
                    "Wants to be your friend"
                } else {
                    "You asked them"
                };
                let (right, by) = row(k, x, ty, w, &f.name, sub, None);
                let enabled = !changing.contains(&f.user_id);
                let bw = dz(110.0);
                let (no, no_tip) = if theirs {
                    ("Decline", "Turn their request down")
                } else {
                    ("Cancel", "Take your request back")
                };
                if k.button(
                    &format!("request-no-{}", f.user_id),
                    right - bw,
                    by,
                    bw,
                    dz(36.0),
                    Tone::Dark,
                    None,
                    no,
                    enabled,
                    no_tip,
                )
                .clicked
                {
                    change = Some((f.clone(), false));
                }
                if theirs
                    && k.button(
                        &format!("request-yes-{}", f.user_id),
                        right - 2.0 * bw - dz(8.0),
                        by,
                        bw,
                        dz(36.0),
                        Tone::Go,
                        None,
                        "Accept",
                        enabled,
                        "You become friends",
                    )
                    .clicked
                {
                    change = Some((f.clone(), true));
                }
            }
        });
        if let Some((f, add)) = change {
            d.servers
                .change_friend(ctx, token, &f.user_id, &f.name, add);
        }
        top += list_h + dz(24.0);
        k.caption(x, top, "Your friends");
        top += dz(28.0);
    }
    let mut scroll = d.servers.friends_page_scroll[2];
    let playing = servers::friend_tiles(
        d,
        k,
        ctx,
        account,
        "friends-page",
        (x, top, w, bottom),
        &mut scroll,
        true,
    );
    d.servers.friends_page_scroll[2] = scroll;
    if let Some((user, name)) = d.servers.remove_clicked.take() {
        d.dialogs.confirm(
            REMOVE_KEY,
            &format!("Remove {name}?"),
            &format!(
                "{name} is taken off your friends, and you off theirs. You can ask again later."
            ),
            DlgIcon::Question,
        );
        d.servers.remove_asked = Some((user, name));
    }
    if let Some(playing) = playing {
        let g = k.label_galley(
            &format!("{playing} in a match"),
            design::din(14.0),
            design::GREY,
            f32::INFINITY,
        );
        k.put(x + w - g.size().x, dz(r.y + 28.0), g);
    }
}

/// PLAYED WITH: the players of your last Arena and Combat matches (on blue or orange),
/// the latest first, to add.
fn played_card(
    d: &mut Dashboard,
    k: &mut Kit,
    ctx: &egui::Context,
    token: &str,
    account: &Account,
    r: Dr,
) {
    let (x, y, w, bottom) = hero::card_frame(k, r, "Played with");
    let recent = match &d.servers.history {
        None => {
            note(k, x, y, w, "Loading your last matches…");
            return;
        }
        Some(Err(e)) => {
            note(k, x, y, w, &format!("Your matches couldn't be read: {e}"));
            return;
        }
        Some(Ok(h)) => {
            let mut h = h.clone();
            h.sort_by_key(|m| std::cmp::Reverse(m.played));
            h.truncate(RECENT_MATCHES);
            game::recent_players(&h, &account.id)
        }
    };
    if recent.is_empty() {
        note(
            k,
            x,
            y,
            w,
            "The players of your last Arena and Combat matches show up here, to add as friends. Social lobbies don't count.",
        );
        return;
    }
    let pitch = dz(ROW_H + ROW_GAP);
    let list_h = bottom - y;
    let mut scroll = d.servers.played_scroll;
    k.scroll_area(
        "friends-played",
        x,
        y,
        w + dz(16.0),
        list_h,
        recent.len() as f32 * pitch - dz(ROW_GAP),
        &mut scroll,
    );
    d.servers.played_scroll = scroll;
    let mut add = None;
    k.clipped(x, y, w, list_h, |k| {
        for (i, p) in recent.iter().enumerate() {
            let ty = y - scroll + i as f32 * pitch;
            if ty + pitch < y || ty > bottom {
                continue;
            }
            let when = p
                .played
                .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
                .map(feed::footer_time)
                .unwrap_or_default();
            let sub = if when.is_empty() {
                p.mode.label().to_string()
            } else {
                format!("{} · {when}", p.mode.label())
            };
            let (right, by) = row(k, x, ty, w, &p.name, &sub, None);
            let key = format!("played-add-{}", p.user_id);
            if add_action(d, k, &key, &p.user_id, &account.id, right, by) {
                add = Some((p.user_id.clone(), p.name.clone()));
            }
        }
    });
    if let Some((user, name)) = add {
        d.servers.change_friend(ctx, token, &user, &name, true);
    }
}
