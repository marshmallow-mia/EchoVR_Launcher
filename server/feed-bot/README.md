# Launcher feed

What the launcher's Play page shows from `https://files.echovr.de/launcher/feed/`, made on
`files.echo` in `/root/EchoLauncherFeed`. [Terms of Service](TERMS.md) ·
[Privacy Policy](PRIVACY.md).

- **`servers.json`** (SERVER INFO): `status_feed.py` as `echo-launcher-status.service`.
  It reads the EchoVRCE status API every 30 s and publishes aggregate numbers only, plus
  distinct players per hour, 24 h and 30 days, counted with keyed-hash pseudonyms kept 30
  days in `state/` (the key is `state/history.key`; never copy it off the server). No
  Discord, no dependencies. `python3 status_feed.py --forget <player ID>` stops counting
  a player who asks; `--hide-top <player ID>` hides a name in the top 3. Log: `/root/log/launcher_status.log`.
  With an EchoVRCE login in `state/echovrce.creds` (`echovrce.py`; `python3 echovrce.py
  --check` tests it) it adds the matchmaking queue, the week's Arena top 3, and merges the
  official daily/weekly Arena player lists into the counts. The login file can hold a
  refresh token, a password with user ID, Discord ID or username, or a copied
  `authorization: Bearer …` header (which lasts only until that session expires).
- **`news.json`** (Community News): `feed_bot.py` as `echo-launcher-feed.service`, a
  read-only Discord bot. It receives no message events and only fetches the picked
  messages by ID, every 5 min. Log: `/root/log/launcher_feed.log`.

Logs rotate daily and are kept 30 days. Tests: `python3 -m unittest test_status_feed
test_log_upload`.

## Log uploads

Settings → Upload logs in the launcher sends the player's logs (the launcher's, Echo VR's,
EchoXR's, plugins', the Quest's) as one plain-text bundle to
`https://files.echovr.de/launcher/logs`. `log_upload.py` (as `echo-launcher-logs.service`,
on `127.0.0.1:8787` behind Apache) keeps an upload only when it is logs: see its docstring
for every check. In short: 10 attempts per hour per IP; the launcher's user agent; strict
UTF-8 text without control, format or private-use characters, line and size limits,
checked while it streams in; ClamAV on every file as it arrives; Magika (Google's file
type detection) on every file and every 2 KiB of it, refusing code and scripts. Nothing
is kept from a refused upload. Kept uploads are deleted after 30 days; no IP is stored.

The player gets an 8-character reference to give you. On the server:

```sh
L='/opt/echo-launcher-logs/.venv/bin/python /opt/echo-launcher-logs/log_upload.py --dir /var/lib/echo-launcher-logs'
ssh files.echo "$L --list"
ssh files.echo "$L --show K7Q4MZ2A" | less     # never -R: the text is checked, but stay safe
ssh files.echo "$L --delete K7Q4MZ2A"          # on request
ssh files.echo "$L --check /path/to/some.log"  # every check on files, to tune
```

Deploy (ClamAV needs about 1.2 GiB of RAM, 2.4 GiB while it reloads its signatures):

```sh
ssh files.echo 'apt install -y clamav-daemon clamav-freshclam && systemctl enable --now clamav-freshclam clamav-daemon'
ssh files.echo 'mkdir -p /opt/echo-launcher-logs'
scp log_upload.py requirements-log-upload.txt files.echo:/opt/echo-launcher-logs/
scp echo-launcher-logs.service files.echo:/etc/systemd/system/
ssh files.echo 'cd /opt/echo-launcher-logs && python3 -m venv .venv && .venv/bin/pip install -r requirements-log-upload.txt'
# Before going live: real Echo VR and EchoXR logs must pass (copy some over first).
ssh files.echo "$L --check /tmp/sample-logs/*"
ssh files.echo 'systemctl daemon-reload && systemctl enable --now echo-launcher-logs'
```

Apache (`a2enmod proxy proxy_http headers`), in the `files.echovr.de` virtual host, before
any other `ProxyPass` for that path; then `apache2ctl configtest && systemctl reload
apache2`:

```apache
<Location /launcher/logs>
    LimitRequestBody 35651584
    RequestHeader set X-Real-IP "expr=%{REMOTE_ADDR}"
    ProxyPass http://127.0.0.1:8787/launcher/logs timeout=600
</Location>
```

`X-Real-IP` is what the rate limit counts by, and the service only believes it from
127.0.0.1. The timeout covers the checks after the upload (Magika on every 2 KiB of up to
32 MiB takes a while).

## Discord setup

- Developer Portal → Bot: enable **Message Content Intent** (a switch for bots in fewer than
  100 servers). It only makes the text of fetched messages visible; it grants no rights.
- Invite with the `bot` and `applications.commands` scopes and no permissions. Then allow
  its role only *View Channel* and *Read Message History* in the SERVER INFO channel and in
  each channel news is picked from, and *View Channel* in the control channel. It needs no
  *Send Messages*: command replies are private interaction responses. At startup the bot
  logs a warning for every right beyond these.
- Terms of Service URL / Privacy Policy URL (General Information):
  `https://github.com/marshmallow-mia/EchoVR_Launcher/blob/main/server/feed-bot/TERMS.md`
  and `.../PRIVACY.md` (live once this directory is on `main`).

## Commands (Manage Server; ephemeral replies)

They only work in the control channel (`control_channel` in `config.json`, #779349591438524457);
anywhere else the bot answers with a pointer to it. To also hide `/launcher` in other
channels, a server admin can limit it under Server Settings → Integrations → the bot.

- `/launcher news set message:<link or ID> [slot] [channel]`: show that message.
  `main` feeds the banner and the first card, `community` the second card. A bare ID is
  looked up in the announcements channel unless `channel` is given.
- `/launcher news clear slot:<slot>`
- `/launcher news show`
- `/launcher refresh`: export everything now.

The chosen messages are kept in `config.json` next to the bot.

## Deploy

```sh
ssh files.echo 'mkdir -p /root/EchoLauncherFeed'
scp feed_bot.py status_feed.py requirements.txt files.echo:/root/EchoLauncherFeed/
scp echo-launcher-feed.service echo-launcher-status.service files.echo:/etc/systemd/system/
ssh files.echo 'cd /root/EchoLauncherFeed && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt'
# The token goes into /root/EchoLauncherFeed/.bot.creds (chmod 600): the bare token,
# TOKEN=... lines or JSON with a "token" key.
ssh files.echo 'cd /root/EchoLauncherFeed && .venv/bin/python feed_bot.py --dump'
ssh files.echo 'systemctl daemon-reload && systemctl enable --now echo-launcher-status echo-launcher-feed'
```
