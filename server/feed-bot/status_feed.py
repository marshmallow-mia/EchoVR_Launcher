#!/usr/bin/env python3
"""Server status for the launcher's RIGHT NOW panel.

Polls the EchoVRCE status API (https://g.echovrce.com/status/matches) and writes aggregate
numbers only to servers.json next to the other feed files: servers and how busy they are
(total, public, private, and per region), the players online, how many different players
were seen in the last hour, 24 hours and 30 days, the matches per mode, and where the
servers are (for the map).

With an EchoVRCE login (state/echovrce.creds, see echovrce.py) it adds what only the
logged-in API has: the matchmaking queue, the week's Arena top 3, and the official Arena
player lists of the day and week, which fill in the player counts (Combat and the social
lobby have no such lists, so those are counted here). Without a working login these extras
are simply left out.

The API lists every player in every match. None of that is published (except the top 3's
names). To count different players over time, each player's ID is replaced by a keyed hash
(HMAC with a secret key that never leaves this server), stored with the time it was last
seen and dropped after 30 days; the official lists are turned into the same pseudonyms and
only kept in memory. See PRIVACY.md.

Standard library only.

  python status_feed.py                  run (every 30 s)
  python status_feed.py --once           fetch and write once, then exit
  python status_feed.py --forget <ID>    stop counting a player (on request); works while
                                         the service runs
  python status_feed.py --hide-top <ID>  show a player as "Hidden player" in the top 3
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import logging
import logging.handlers
import os
import secrets
import sys
import time
import urllib.request
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import echovrce

HERE = Path(__file__).resolve().parent
API = os.environ.get("STATUS_API", "https://g.echovrce.com/status/matches")
OUT = Path(os.environ.get("FEED_OUT", "/var/www/release/launcher/feed")) / "servers.json"
STATE = Path(os.environ.get("STATUS_STATE", HERE / "state"))
LOG_PATH = Path(os.environ.get("STATUS_LOG", "/var/log/echo-launcher/launcher_status.log"))
EVERY_S = 30
LOG_DAYS = 30
HISTORY_S = 30 * 86400
WINDOWS = {"last_hour": 3600, "last_24h": 86400, "last_30d": HISTORY_S}
# The API is "stale" when its data is older than this.
STALE_S = 300
# Servers in this group take public matches; the others are private (hosted for a guild).
PUBLIC_GROUP = "147afc9d-2819-4197-926d-5b3f92790edc"
MODES = {"social_2.0": "lobby", "echo_arena": "arena", "echo_combat": "combat"}
# The official lists and the top 3 are refreshed this often (paging ~15 requests).
OFFICIAL_EVERY_S = 600
# Official data older than this is dropped rather than shown.
OFFICIAL_MAX_AGE_S = 1800
TOP_BOARD = {"game_mode": "echo_arena", "stat_name": "ArenaWins", "reset_schedule": "weekly"}
# EchoTools' leaderboard encoding (wiki: Leaderboard Score To Float64).
SCORE_OFFSET = 1_000_000_000_000_000
SCORE_SCALE = 1_000_000_000
REGIONS = {
    "NA": {"US", "CA", "MX"},
    "EU": {
        "GB", "JE", "GG", "IM", "IE", "DE", "FR", "NL", "BE", "LU", "AT", "CH", "IT", "ES", "PT",
        "DK", "SE", "NO", "FI", "IS", "PL", "CZ", "SK", "HU", "SI", "HR", "RO", "BG", "GR",
        "EE", "LV", "LT", "UA", "RS", "BA", "MK", "AL", "ME", "MT", "CY",
    },
    "OCE": {"AU", "NZ"},
}
# Map positions (lat, lon) for the API's default_region codes; unknown codes fall back to
# their country, and are logged.
LOCATIONS = {
    "us-illinois": (41.88, -87.63),
    "us-ne": (41.26, -95.94),
    "us-texas": (32.78, -96.80),
    "us-pennsylvania": (39.95, -75.17),
    "us-wisconsin": (43.07, -89.40),
    "us-alabama": (33.52, -86.80),
    "us-northcarolina": (35.78, -78.64),
    "us-newjersey": (40.74, -74.17),
    "us-colorado": (39.74, -104.99),
    "us-oregon": (45.52, -122.68),
    "gb-england": (51.51, -0.13),
    "je-sthelier": (49.19, -2.11),
    "de-th": (50.98, 11.03),
    "au-queensland": (-27.47, 153.03),
}
COUNTRIES = {
    "US": (39.8, -98.6), "CA": (56.1, -106.3), "MX": (23.6, -102.6), "GB": (54.0, -2.0),
    "JE": (49.2, -2.1), "IE": (53.4, -8.2), "DE": (51.2, 10.4), "FR": (46.2, 2.2),
    "NL": (52.1, 5.3), "PL": (51.9, 19.1), "SE": (60.1, 18.6), "FI": (61.9, 25.7),
    "ES": (40.5, -3.7), "IT": (41.9, 12.6), "AU": (-25.3, 133.8), "NZ": (-40.9, 174.9),
    "BR": (-14.2, -51.9), "JP": (36.2, 138.3), "SG": (1.35, 103.8), "ZA": (-30.6, 22.9),
}

log = logging.getLogger("status")


def now_iso(t: float | None = None) -> str:
    return datetime.fromtimestamp(t if t is not None else time.time(), timezone.utc).isoformat(timespec="seconds")


def parse_time(s: str | None) -> float | None:
    if not s:
        return None
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def write_atomic(path: Path, data: bytes, mode: int = 0o644) -> None:
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_bytes(data)
    os.chmod(tmp, mode)
    os.replace(tmp, path)


def region_of(country: str | None) -> str | None:
    for name, countries in REGIONS.items():
        if country in countries:
            return name
    return None


def pct(part: int, whole: int) -> int:
    return round(100 * part / whole) if whole else 0


# ---- player history (pseudonymised) ----


class History:
    """When each (pseudonymised) player was last seen, for the last 30 days."""

    def __init__(self, state: Path) -> None:
        self.dir = state
        self.dir.mkdir(parents=True, exist_ok=True)
        os.chmod(self.dir, 0o700)
        key_file = self.dir / "history.key"
        if not key_file.exists():
            write_atomic(key_file, secrets.token_bytes(32), mode=0o600)
        self.key = key_file.read_bytes()
        self.file = self.dir / "history.json"
        try:
            data = json.loads(self.file.read_text())
        except (FileNotFoundError, ValueError):
            data = {}
        self.since: float = data.get("since") or time.time()
        self.seen: dict[str, float] = data.get("seen", {})
        # Players who asked not to be counted, as pseudonyms.
        self.forget_file = self.dir / "forget.txt"

    def pseudonym(self, user_id: str) -> str:
        return hmac.new(self.key, user_id.encode(), hashlib.sha256).hexdigest()[:24]

    def forgotten(self) -> set[str]:
        try:
            return set(self.forget_file.read_text().split())
        except FileNotFoundError:
            return set()

    def forget(self, user_id: str) -> None:
        """Stops counting a player; the running service drops their entry on its next pass."""
        with self.forget_file.open("a") as f:
            f.write(self.pseudonym(user_id) + "\n")
        os.chmod(self.forget_file, 0o600)

    def record(self, user_ids: set[str], t: float) -> None:
        skip = self.forgotten()
        for uid in user_ids:
            self.seen[self.pseudonym(uid)] = t
        for k in skip:
            self.seen.pop(k, None)
        cutoff = t - HISTORY_S
        self.seen = {k: v for k, v in self.seen.items() if v >= cutoff}
        self.since = max(self.since, cutoff)

    def within(self, t: float, span: float) -> set[str]:
        return {k for k, v in self.seen.items() if v >= t - span}

    def counts(self, t: float, extra: dict[str, set[str] | None] | None = None) -> dict[str, int]:
        """Different players per window; `extra` adds pseudonyms from official lists."""
        extra = extra or {}
        skip = self.forgotten()
        return {
            name: len((self.within(t, span) | (extra.get(name) or set())) - skip)
            for name, span in WINDOWS.items()
        }

    def save(self) -> None:
        body = json.dumps({"since": self.since, "seen": self.seen}, separators=(",", ":"))
        write_atomic(self.file, body.encode(), mode=0o600)


# ---- official data (logged in) ----


def decode_score(score: int, subscore: int = 0) -> float:
    if score >= SCORE_OFFSET:
        return (score - SCORE_OFFSET) + subscore / SCORE_SCALE
    return -((SCORE_OFFSET - 1 - score) + (1 - subscore / (SCORE_SCALE - 1)))


def seconds(ts: Any) -> float | None:
    if isinstance(ts, dict) and "seconds" in ts:
        return float(ts["seconds"]) + float(ts.get("nanos", 0)) / 1e9
    return None


def queue_numbers(state: dict[str, Any]) -> dict[str, Any]:
    """Players searching per mode, and the typical wait of the latest matches made."""
    searching = {"arena": 0, "combat": 0, "lobby": 0, "other": 0}
    for ticket in state.get("index") or []:
        mode = MODES.get((ticket.get("StringProperties") or {}).get("game_mode", ""), "other")
        searching[mode] += int(ticket.get("Count") or 1)
    waits = sorted(
        c - s
        for comp in (state.get("stats") or {}).get("completions") or []
        if (s := seconds(comp.get("create_time"))) is not None and (c := seconds(comp.get("complete_time"))) is not None and c >= s
    )
    median = waits[len(waits) // 2] if waits else None
    return {"searching": searching, "total": sum(searching.values()), "wait_s": round(median) if median is not None else None}


def record_name(r: dict[str, Any]) -> str:
    meta = r.get("metadata") or {}
    for v in (meta.get("display_name"), meta.get("displayName"), r.get("display_name"), r.get("username")):
        if isinstance(v, dict):
            v = v.get("value")
        if isinstance(v, str) and v.strip():
            return v.strip()
    return "?"


def hidden_from_top(history: History) -> set[str]:
    try:
        return set((history.dir / "hide_top.txt").read_text().split())
    except FileNotFoundError:
        return set()


def hide_from_top(history: History, user_id: str) -> None:
    f = history.dir / "hide_top.txt"
    with f.open("a") as out:
        out.write(history.pseudonym(user_id) + "\n")
    os.chmod(f, 0o600)


class Official:
    """The logged-in extras, kept fresh and dropped when they get too old."""

    def __init__(self, history: History) -> None:
        self.history = history
        self.session: echovrce.Session | None = None
        self.boards_at = 0.0
        self.daily: set[str] | None = None
        self.weekly: set[str] | None = None
        self.top: list[dict[str, Any]] | None = None
        self.queue: dict[str, Any] | None = None
        self.failing = False

    def _players(self, s: echovrce.Session, schedule: str) -> set[str]:
        """Everyone on the Arena board of the day or week, as pseudonyms."""
        out: set[str] = set()
        cursor = None
        for _ in range(80):
            q = {"group_id": PUBLIC_GROUP, "game_mode": "echo_arena", "stat_name": "ArenaWins",
                 "reset_schedule": schedule, "limit": 100}
            if cursor:
                q["cursor"] = cursor
            page = s.rpc("leaderboard/records", q)
            out.update(self.history.pseudonym(r["owner_id"]) for r in page.get("records") or [] if r.get("owner_id"))
            cursor = page.get("next_cursor")
            if not cursor:
                break
        return out

    def _top(self, s: echovrce.Session) -> list[dict[str, Any]]:
        page = s.rpc("leaderboard/records", {"group_id": PUBLIC_GROUP, **TOP_BOARD, "limit": 3})
        hidden = hidden_from_top(self.history)
        return [
            {"rank": int(r.get("rank") or i + 1),
             "name": "Hidden player" if self.history.pseudonym(r.get("owner_id") or "") in hidden else record_name(r),
             "wins": round(decode_score(int(r.get("score") or 0), int(r.get("subscore") or 0)))}
            for i, r in enumerate(page.get("records") or [])
        ]

    def update(self, t: float) -> None:
        if self.session is None:
            if not echovrce.CREDS.exists():
                return
            self.session = echovrce.Session()
        s = self.session
        try:
            self.queue = queue_numbers(s.rpc("matchmaker/state", {}))
            if t - self.boards_at >= OFFICIAL_EVERY_S:
                self.daily, self.weekly, self.top = self._players(s, "daily"), self._players(s, "weekly"), self._top(s)
                self.boards_at = t
            if self.failing:
                log.info("EchoVRCE data available again")
            self.failing = False
        except (echovrce.AuthError, OSError, ValueError, KeyError) as e:
            if not self.failing:
                log.warning("EchoVRCE data unavailable: %s", e)
            self.failing = True
            self.queue = None
            if t - self.boards_at > OFFICIAL_MAX_AGE_S:
                self.daily = self.weekly = self.top = None


# ---- aggregation ----


def aggregate(data: dict[str, Any], history: History, t: float, official: Official | None = None) -> dict[str, Any]:
    """The published numbers. Takes nothing about players beyond counting them (and the
    top 3's names)."""
    servers = data.get("gameservers") or []
    matches = data.get("labels") or []

    def is_public(s: dict[str, Any]) -> bool:
        return PUBLIC_GROUP in (s.get("group_ids") or [])

    public_servers = sum(1 for s in servers if is_public(s))
    public_matches = sum(1 for m in matches if is_public(m.get("broadcaster") or {}))
    region_servers: dict[str, int] = defaultdict(int)
    region_matches: dict[str, int] = defaultdict(int)
    for s in servers:
        if r := region_of(s.get("country_code")):
            region_servers[r] += 1
    for m in matches:
        if r := region_of((m.get("broadcaster") or {}).get("country_code")):
            region_matches[r] += 1

    modes: dict[str, dict[str, dict[str, int]]] = {
        mode: {kind: {"matches": 0, "players": 0, "limit": 0, "spectators": 0} for kind in ("public", "private")}
        for mode in MODES.values()
    }
    seen: set[str] = set()
    for m in matches:
        players = m.get("players") or []
        seen.update(p["user_id"] for p in players if p.get("user_id"))
        mode = MODES.get(m.get("mode", ""))
        if not mode:
            continue
        kind = "private" if m.get("lobby_type") == "private" else "public"
        slot = modes[mode][kind]
        slot["matches"] += 1
        slot["players"] += int(m.get("player_count") or 0)
        slot["limit"] += int(m.get("player_limit") or m.get("limit") or 0)
        slot["spectators"] += sum(1 for p in players if p.get("team") == "spectator")
    history.record(seen, t)

    locations: dict[str, dict[str, Any]] = {}
    for s in servers:
        code = s.get("default_region") or ""
        loc = locations.get(code)
        if loc is None:
            pos = LOCATIONS.get(code) or COUNTRIES.get(s.get("country_code") or "")
            if pos is None:
                log.warning("no map position for region %r (%s)", code, s.get("country_code"))
                continue
            if code not in LOCATIONS:
                log.info("region %r placed at its country (%s)", code, s.get("country_code"))
            loc = locations[code] = {
                "region": code,
                "name": s.get("region") or code,
                "country": s.get("country_code"),
                "lat": pos[0],
                "lon": pos[1],
                "servers": 0,
                "matches": 0,
            }
        loc["servers"] += 1
    for m in matches:
        code = (m.get("broadcaster") or {}).get("default_region") or ""
        if code in locations:
            locations[code]["matches"] += 1

    source = parse_time(data.get("update_time"))
    uptime = data.get("uptime_mins")
    total = len(servers)
    return {
        "status": "ok" if source and t - source < STALE_S else "stale",
        "source_time": data.get("update_time"),
        "started_at": now_iso(source - 60 * uptime) if source and isinstance(uptime, (int, float)) else None,
        "servers": {"total": total, "public": public_servers, "private": total - public_servers},
        "usage": {
            "total": pct(len(matches), total),
            "public": pct(public_matches, public_servers),
            "private": pct(len(matches) - public_matches, total - public_servers),
        },
        "regions": {
            r: {"servers": region_servers[r], "usage": pct(region_matches[r], region_servers[r])} for r in REGIONS
        },
        "players": {
            "online": int(data.get("player_count") or 0),
            **history.counts(
                t,
                {"last_24h": official.daily, "last_30d": official.weekly} if official else None,
            ),
            "since": now_iso(history.since),
            # Official Arena lists (None without a working EchoVRCE login).
            "arena_today": len(official.daily) if official and official.daily is not None else None,
            "arena_week": len(official.weekly) if official and official.weekly is not None else None,
        },
        "queue": official.queue if official else None,
        "top": {"board": "arena_wins_week", "entries": official.top} if official and official.top else None,
        "modes": modes,
        "locations": sorted(locations.values(), key=lambda l: -l["servers"]),
    }


def fetch() -> dict[str, Any]:
    req = urllib.request.Request(API, headers={"User-Agent": "echovr-launcher-status/1 (+release.echovr.de)"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def cycle(history: History, official: Official, last: dict[str, Any] | None) -> dict[str, Any] | None:
    t = time.time()
    official.update(t)
    try:
        body = aggregate(fetch(), history, t, official)
        history.save()
    except Exception as e:  # noqa: BLE001 - keep running, report the outage
        log.warning("status API unavailable: %s", e)
        if last is None:
            return None
        body = {**last, "status": "down"}
    body["updated_at"] = now_iso(t)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    write_atomic(OUT, json.dumps(body, indent=1).encode())
    return body


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--once", action="store_true", help="fetch and write once, then exit")
    ap.add_argument("--forget", metavar="PLAYER_ID", help="stop counting this player (EchoVRCE user ID)")
    ap.add_argument("--hide-top", metavar="PLAYER_ID", help="hide this player's name in the top 3")
    args = ap.parse_args()
    if args.hide_top:
        hide_from_top(History(STATE), args.hide_top.strip())
        print("hidden: shown as 'Hidden player' from the next top-3 refresh (within 10 minutes)")
        return
    if args.forget:
        History(STATE).forget(args.forget.strip())
        print("forgotten: that player is no longer counted (applied within 30 s)")
        return
    handlers: list[logging.Handler] = [logging.StreamHandler(sys.stderr)]
    if not args.once:
        LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
        handlers.append(
            logging.handlers.TimedRotatingFileHandler(LOG_PATH, when="midnight", backupCount=LOG_DAYS, utc=True)
        )
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)-7s %(name)s: %(message)s", handlers=handlers)
    history = History(STATE)
    official = Official(history)
    last = None
    log.info("polling %s every %s s into %s", API, EVERY_S, OUT)
    while True:
        last = cycle(history, official, last) or last
        if args.once:
            return
        time.sleep(EVERY_S)


if __name__ == "__main__":
    main()
