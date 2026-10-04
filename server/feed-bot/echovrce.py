#!/usr/bin/env python3
"""A logged-in session with the EchoVRCE game service (EchoTools' Nakama), for the status
service's read-only calls: the matchmaking queue and leaderboards.

The login comes from state/echovrce.creds (chmod 600), in any of these forms:
  - JSON or KEY=value lines with a refresh token (refresh_token / refreshToken), and/or
  - a password with one of user_id, discord_id, username (the password login documented
    in the EchoTools nakama wiki), or email and password, or
  - an HTTP "authorization: Bearer <session JWT>" header copied from echovrce.com. While
    that session is valid, the service links itself as a device of the account (the
    device login echovrce.com offers at /login/device: request a code, approve it with
    the session, collect the device's tokens), and from then on renews the device's
    session by itself.
The session (JWT and refresh token) is kept in state/echovrce.session.json (chmod 600) and
renewed before it expires. Tokens and passwords are never printed or logged.

The API keys the calls need are the ones echovrce.com's own web app publishes in its
config.json; they are read from there at runtime, never stored in this repository.

  python3 echovrce.py --check    log in, say how, and try the calls the feed uses
"""

from __future__ import annotations

import base64
import json
import logging
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
STATE = Path(os.environ.get("STATUS_STATE", HERE / "state"))
CREDS = STATE / "echovrce.creds"
SESSION = STATE / "echovrce.session.json"
API = os.environ.get("ECHOVRCE_API", "https://g.echovrce.com/v2")
WEB_CONFIG = "https://echovrce.com/config.json"
UA = "echovr-launcher-status/1 (+files.echovr.de)"
# Renew the session when it has less than this left.
MARGIN_S = 300

log = logging.getLogger("echovrce")

KEY_ALIASES = {
    "refresh_token": {"refreshtoken", "refresh", "rt"},
    "token": {"token", "jwt", "accesstoken", "sessiontoken", "authtoken"},
    "password": {"password", "pass", "pw", "passwd"},
    "user_id": {"userid", "nakamaid", "id", "uuid"},
    "discord_id": {"discordid", "discord"},
    "username": {"username", "user", "name", "login", "ign"},
    "email": {"email", "mail"},
}


class AuthError(Exception):
    pass


def from_header(value: str) -> dict[str, str]:
    """An Authorization header's credentials: a Bearer token, or Basic user:password."""
    scheme, _, rest = value.strip().partition(" ")
    if scheme.lower() == "bearer" and rest.strip():
        return {"token": rest.strip()}
    if scheme.lower() == "basic" and rest.strip():
        try:
            user, _, password = base64.b64decode(rest.strip()).decode().partition(":")
        except ValueError:
            return {}
        if password:
            return {"username": user, "password": password}
    return {}


def normalize(raw: dict[str, Any]) -> dict[str, str]:
    out: dict[str, str] = {}
    for key, value in raw.items():
        if str(key).lower() == "authorization" and isinstance(value, str):
            for k, v in from_header(value).items():
                out.setdefault(k, v)
            continue
        if not isinstance(value, (str, int)) or str(value).strip() == "":
            continue
        k = re.sub(r"[^a-z]", "", str(key).lower())
        for name, aliases in KEY_ALIASES.items():
            if k == name.replace("_", "") or k in aliases:
                out.setdefault(name, str(value).strip())
    return out


def load_creds(path: Path = CREDS) -> dict[str, str]:
    """The credentials file as {refresh_token, token, password, user_id, ...}."""
    text = path.read_text(encoding="utf-8").strip()
    try:
        data = json.loads(text)
        if isinstance(data, dict):
            flat: dict[str, Any] = {}
            for k, v in data.items():
                if isinstance(v, dict):
                    flat.update(v)
                else:
                    flat[k] = v
            return normalize(flat)
    except ValueError:
        pass
    pairs: dict[str, str] = {}
    lines = [l.strip() for l in text.splitlines() if l.strip() and not l.lstrip().startswith("#")]
    for line in lines:
        m = re.match(r"^(?:export\s+)?([A-Za-z_][A-Za-z0-9_ .-]*?)\s*[=:]\s*(.+)$", line)
        if m and not line.startswith("eyJ"):
            pairs[m.group(1)] = m.group(2).strip().strip("\"'")
    if pairs:
        return normalize(pairs)
    # A bare token on its own line: JWTs start with eyJ; anything else is a refresh token.
    if len(lines) == 1:
        return {"token" if lines[0].count(".") == 2 and lines[0].startswith("eyJ") else "refresh_token": lines[0]}
    raise AuthError(f"can't read the credentials in {path}")


def jwt_exp(token: str | None) -> float:
    """A JWT's expiry (unix time), or 0."""
    try:
        payload = token.split(".")[1]  # type: ignore[union-attr]
        payload += "=" * (-len(payload) % 4)
        return float(json.loads(base64.urlsafe_b64decode(payload)).get("exp", 0))
    except Exception:  # noqa: BLE001
        return 0.0


def _post(url: str, body: dict[str, Any], headers: dict[str, str] | None = None, timeout: float = 20) -> tuple[int, Any]:
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        method="POST",
        headers={"Content-Type": "application/json", "User-Agent": UA, **(headers or {})},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        raw, status = e.read(), e.code
    try:
        return status, json.loads(raw or b"null")
    except ValueError:
        return status, raw.decode(errors="replace")[:200]


def _error(data: Any) -> str:
    if isinstance(data, dict):
        return str(data.get("message") or data.get("error") or "")[:160]
    return str(data)[:160]


class Session:
    def __init__(self, creds_path: Path = CREDS, session_path: Path = SESSION) -> None:
        self.creds_path = creds_path
        self.session_path = session_path
        self._keys: dict[str, str] | None = None
        self.method = ""
        self.token: str | None = None
        self.refresh_token: str | None = None
        self._load()

    def _load(self) -> None:
        """Takes the saved session when it's newer than ours (the refresh token rotates on
        every renewal, so only the latest one is good)."""
        try:
            s = json.loads(self.session_path.read_text())
        except (FileNotFoundError, ValueError):
            return
        if jwt_exp(s.get("token")) >= jwt_exp(self.token):
            self.token = s.get("token")
            self.refresh_token = s.get("refresh_token") or self.refresh_token

    # -- keys the web app publishes --

    def keys(self) -> dict[str, str]:
        if self._keys is None:
            req = urllib.request.Request(WEB_CONFIG, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=20) as r:
                cfg = json.load(r)
            self._keys = {
                "http": cfg.get("VITE_NAKAMA_HTTP_KEY", ""),
                "server": cfg.get("VITE_NAKAMA_SERVER_KEY", ""),
            }
        return self._keys

    def _http_key(self) -> str:
        return urllib.parse.quote(self.keys()["http"])

    def _basic_server(self) -> dict[str, str]:
        return {"Authorization": "Basic " + base64.b64encode(f"{self.keys()['server']}:".encode()).decode()}

    # -- session --

    def _take(self, data: Any, method: str) -> bool:
        if not isinstance(data, dict):
            return False
        token = data.get("token") or data.get("access_token")
        if not token:
            return False
        self.token = token
        self.refresh_token = data.get("refresh_token") or data.get("refreshToken") or self.refresh_token
        self.method = method
        STATE.mkdir(parents=True, exist_ok=True)
        tmp = self.session_path.with_name(f".{self.session_path.name}.tmp")
        tmp.write_text(json.dumps({"token": self.token, "refresh_token": self.refresh_token}))
        os.chmod(tmp, 0o600)
        os.replace(tmp, self.session_path)
        return True

    def _refresh(self, refresh_token: str) -> bool:
        attempts = [
            (
                "device refresh",
                f"{API}/rpc/device/auth/refresh?unwrap&http_key={self._http_key()}",
                {"refresh_token": refresh_token, "token": refresh_token},
                {},
            ),
            (
                "password-RPC refresh",
                f"{API}/rpc/account/authenticate/password?unwrap&http_key={self._http_key()}",
                {"refresh_token": refresh_token},
                {"Authorization": "Basic Og=="},
            ),
            ("session refresh", f"{API}/account/session/refresh", {"token": refresh_token}, self._basic_server()),
        ]
        for name, url, body, headers in attempts:
            status, data = _post(url, body, headers)
            if status == 200 and self._take(data, name):
                return True
            log.info("%s: HTTP %s %s", name, status, _error(data))
        return False

    def _login(self, creds: dict[str, str]) -> bool:
        if "password" in creds:
            ident = next((k for k in ("user_id", "discord_id", "username") if k in creds), None)
            if ident:
                status, data = _post(
                    f"{API}/rpc/account/authenticate/password?unwrap&http_key={self._http_key()}",
                    {ident: creds[ident], "password": creds["password"]},
                )
                if status == 200 and self._take(data, f"password login ({ident})"):
                    return True
                log.info("password login (%s): HTTP %s %s", ident, status, _error(data))
            if "email" in creds:
                status, data = _post(
                    f"{API}/account/authenticate/email?create=false",
                    {"email": creds["email"], "password": creds["password"]},
                    self._basic_server(),
                )
                if status == 200 and self._take(data, "email login"):
                    return True
                log.info("email login: HTTP %s %s", status, _error(data))
        return False

    def device_link(self, session_token: str) -> bool:
        """Links this service as a device of the account the session belongs to, and takes
        the device's session (with a refresh token that keeps it going)."""
        auth = {"Authorization": f"Bearer {session_token}"}
        status, data = _post(f"{API}/rpc/device/auth/request?unwrap", {}, auth)
        code = data.get("code") if isinstance(data, dict) else None
        if status != 200 or not code:
            log.info("device code: HTTP %s %s", status, _error(data))
            return False
        status, data = _post(f"{API}/rpc/device/auth/verify?unwrap", {"code": code}, auth)
        if status != 200:
            log.info("device approval: HTTP %s %s", status, _error(data))
            return False
        for _ in range(10):
            status, data = _post(f"{API}/rpc/device/auth/poll?unwrap", {"code": code}, auth)
            if status == 200 and self._take(data, "device link"):
                return True
            time.sleep(2)
        log.info("device tokens: HTTP %s %s", status, _error(data))
        return False

    def ensure(self) -> str:
        """A token valid for at least MARGIN_S more seconds."""
        if self.token and jwt_exp(self.token) - time.time() > MARGIN_S:
            return self.token
        self._load()
        if self.token and jwt_exp(self.token) - time.time() > MARGIN_S:
            return self.token
        if self.refresh_token and self._refresh(self.refresh_token):
            return self.token  # type: ignore[return-value]
        creds = load_creds(self.creds_path)
        if creds.get("refresh_token") and creds["refresh_token"] != self.refresh_token:
            if self._refresh(creds["refresh_token"]):
                return self.token  # type: ignore[return-value]
        if self._login(creds):
            return self.token  # type: ignore[return-value]
        token = creds.get("token")
        if token and jwt_exp(token) > time.time():
            if self.device_link(token) and self.refresh_token:
                return self.token  # type: ignore[return-value]
            self.token, self.method = token, "session token from the credentials file (no refresh token)"
            return self.token
        raise AuthError("no way to log in with the given credentials (see the log for each attempt)")

    def rpc(self, name: str, payload: dict[str, Any] | None = None) -> Any:
        """Calls an RPC as the logged-in account; renews the session once on a 401."""
        for attempt in (1, 2):
            status, data = _post(f"{API}/rpc/{name}?unwrap", payload or {}, {"Authorization": f"Bearer {self.ensure()}"})
            if status == 401 and attempt == 1:
                self.token = None
                continue
            if status != 200:
                raise AuthError(f"rpc {name}: HTTP {status} {_error(data)}")
            return data
        raise AuthError(f"rpc {name}: not authorized")


def shape(x: Any, depth: int = 0) -> str:
    """The structure of a response (keys and types), without its values."""
    pad = "  " * depth
    if isinstance(x, dict):
        lines = []
        for k, v in list(x.items())[:30]:
            if isinstance(v, (dict, list)):
                lines.append(f"{pad}{k}: {type(v).__name__}{'[' + str(len(v)) + ']' if isinstance(v, list) else ''}")
                if depth < 2:
                    lines.append(shape(v, depth + 1))
            else:
                lines.append(f"{pad}{k}: {type(v).__name__}")
        return "\n".join(l for l in lines if l)
    if isinstance(x, list) and x:
        return f"{pad}[0]:\n" + shape(x[0], depth + 1)
    return ""


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s", stream=sys.stderr)
    if "--check" not in sys.argv:
        print(__doc__)
        return
    s = Session()
    s.token = None  # always go through the login/refresh path
    s.ensure()
    left = (jwt_exp(s.token) - time.time()) / 60
    print(f"logged in via {s.method}; session valid for {left:.0f} more minutes; "
          f"refresh token: {'yes' if s.refresh_token else 'no'}")
    for name, payload in [
        ("matchmaker/state", {}),
        ("guildgroup", {}),
        ("leaderboard/records", {"game_mode": "echo_arena", "stat_name": "ArenaWins", "reset_schedule": "daily", "limit": 3}),
    ]:
        try:
            data = s.rpc(name, payload)
            print(f"\n== {name}: OK\n{shape(data)}")
        except AuthError as e:
            print(f"\n== {name}: {e}")


if __name__ == "__main__":
    main()
