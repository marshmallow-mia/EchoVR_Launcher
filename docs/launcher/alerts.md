# Alerts: important notices in the launcher

`https://release.echovr.de/launcher/feed/alerts.json` (written with `echo-alert` on
release.echo) holds notices every player, or some, must see: a mod found to be harmful, a
server problem, an urgent update. Since 0.13 the launcher reads it with every catalogue check
(every 30 seconds while it's open, every 15 minutes from the tray). Code:
`src/core/launcher/alerts.rs`, `src/ui/launcher/alerts.rs`.

```json
{"schema": 1, "alerts": [
  {"id": "2026-10-21-cool-mod", "severity": "warning",
   "title": "Remove Cool Mod 1.2.0",
   "body": "It **crashes** the game in combat. 1.2.1 fixes it.",
   "link_url": "https://github.com/EchoTools/echo-mods/issues/7", "link_label": "Details",
   "starts": "2026-10-21T00:00:00Z", "expires": "2026-11-21T00:00:00Z",
   "dismissible": true,
   "target": {"mods": [{"id": "cool-mod", "versions": "1.2.0"}]}}
]}
```

| Field | Meaning |
|---|---|
| `id` | Unique; dismissals and desktop notifications are remembered by it |
| `severity` | `info`, `warning` or `critical` |
| `title`, `body` | `body` is Discord markdown |
| `link_url`, `link_label` | Only `https://` on echovr.de, echovrce.com, github.com, discord.gg, discord.com (and their subdomains); any other link is dropped |
| `starts`, `expires` | RFC 3339, UTC; shown only in between |
| `dismissible` | Default: true, except for `critical` |
| `target` | Every condition set must hold; none: everyone |

`target`:

| Field | Matches |
|---|---|
| `platforms` | `windows`, `linux` |
| `launcher` | The launcher's version, a semver range (`<0.13.2`) |
| `channels` | The launcher channel followed: `main`, `beta`, `alpha` |
| `mods` | `[{"id", "versions"}]`: players with that mod installed in any version (a range; empty or `*`: any) |
| `dev_channels` | Players with one of these dev channels (by the name in its `channel.json`) |

## What the player sees

- **info**: in the top bar (blue), while no notice or job is shown there, with **More** (the
  whole text and the link) and **×** to dismiss it.
- **warning**: the same in orange, plus one desktop notification (Windows and Linux).
- **critical**: a card when the launcher starts (and as soon as the alert arrives) that has
  to be acknowledged with **I've read it**; it comes back at every start until it expires,
  unless it's dismissible. Plus one desktop notification, also from the tray while the
  window is closed.

Dismissed alerts are kept in `launcher.json` (`alerts_seen`), notified ones in
`alerts-notified.json` in the launcher's data folder. Without the file, or offline, there are
no alerts.

A block (`echo-mod reject … --alert <id>`, [cdn.md](cdn.md#pulling-a-version-for-everyone))
can name an alert that says more about it.
