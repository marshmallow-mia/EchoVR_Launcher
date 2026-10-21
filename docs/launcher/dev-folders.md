# Testing mods and plugins before they're published

With a **dev code** the launcher lists your own mods and launcher plugins from your **dev
folder** on release.echovr.de, before they're in the published catalogues. Only whoever has
the code sees them. Third-party developers get theirs with their first approved mod
([EchoTools/echo-mods](https://github.com/EchoTools/echo-mods)): SFTP with the SSH keys of
their GitHub account, the code in `DEV-CODE.txt` in the folder. Others ask the launcher's
maintainers and send them an SSH public key; they get a code and an `sftp` command.

## In the launcher

Settings → **Advanced settings** → **Dev mods and plugins**: paste a code and press **Add
code**. Since 0.13 several codes can be set (a tester can follow several developers): each
gets a line saying its channel's name and how many mods and plugins its folder lists, with
**Remove**. The entries appear on the Mods page (More mods) and the Plugins page with a **Dev**
tag, and the status bar shows **DEV CODE** (or **DEV CODES (n)**). An entry with the same id
as a published one replaces it (a mod also replaces one with the same file); with several
codes, the first code's goes first. A mod's **More** → Source picks where it comes from when
the catalogue and dev folders both have it. **Remove** goes back to the published ones;
installed dev mods keep loading, tagged as no longer listed.

`launcher.json` keeps the codes in `dev_codes` and the first one in `dev_code`, which
launchers before 0.13 read.

A folder can name its channel in a root-owned `channel.json` (`{"name": "…"}`; `echo-dev-add`
writes it): Advanced settings shows the name, and alerts can aim at the channel
([alerts.md](alerts.md)).

Third-party developers' channels (since 0.13.0-alpha.2) are behind the EchoVRCE sign-in, at
`launcher/gated/dev/<code>/`: *restricted* ones open for the developer's own EchoVRCE account
and the accounts in their folder's `access.json`, *public* ones for any signed-in account
(the maintainers choose which, never the developer). A code that isn't under `launcher/dev/`
is looked for there with the player's EchoVRCE session (sent to release.echovr.de's
`launcher/gated/` only, never to the CDN or anywhere else); its line in Advanced settings says
**Sign in** (with a button) or that the account isn't allowed. Which codes are gated is kept
by a hash of the code in `dev-gated.json`. Signed out, or no longer allowed: its mods aren't
listed any more, installed ones keep loading.

`access.json` (yours to overwrite): `{"schema": 1, "accounts": ["<EchoVRCE account ID>", ...]}`,
at most 50. Account IDs only: a username can change and then be someone else's. Testers copy
theirs on the launcher's EchoVRCE page (**Copy ID**). `ACCESS-STATUS.txt` beside it (read-only)
says which were found, what was refused, and when each last opened the channel. The same list
opens your **private** mods (visibility `private` in `echo-mod.json`). Your own account is the
one the maintainers noted when they approved your first mod.

## Your folder

```
/                 your folder (SFTP shows it as /)
├── mods.json     your mods catalogue: overwrite it
├── plugins.json  your launcher plugins catalogue: overwrite it
├── files/        your DLLs and zips
├── channel.json  your channel's name (read-only)
├── access.json   who else may use a gated channel and your private mods: overwrite it
├── ACCESS-STATUS.txt   what access.json was found to hold (read-only)
└── DEV-CODE.txt  your code (read-only; never served)
```

The last three are only in a third-party developer's (gated) channel.

Upload with any SFTP client, e.g. `sftp -P 2999 dev-<name>@168.119.2.94`, then
`put NvrMyMod.dll files/` and `put mods.json`. You can't make links or anything outside
`files/`, and there is no shell.

The catalogues are the published ones' format ([mods.md](mods.md),
[plugins.json](plugins.json)). A `url` is relative to your folder (`files/NvrMyMod.dll`), or
an absolute URL inside it; anything else is left out. Every download is checked against its
`sha256` (`shasum -a 256 file` / `sha256sum file`), so update it with each upload.

```json
{
  "schema": 1,
  "mods": [
    {
      "id": "my-mod",
      "name": "My Mod",
      "summary": "What it does, in one or two sentences.",
      "author": "you",
      "version": "0.1.0",
      "file": "NvrMyMod.dll",
      "url": "files/NvrMyMod.dll",
      "sha256": "<64 hex characters>",
      "api": 5,
      "capabilities": ["cosmetic"]
    }
  ]
}
```

```json
{
  "schema": 1,
  "plugins": [
    {
      "id": "my-plugin",
      "name": "My Plugin",
      "summary": "What it adds to the launcher.",
      "author": "you",
      "version": "0.1.0",
      "url": "files/my-plugin-0.1.0.zip",
      "sha256": "<64 hex characters>"
    }
  ]
}
```

A content pack (a mod with game data, [mods.md](mods.md#content-packs)) has its zip's
`url` and `sha256` in `pack`, relative to your folder the same way
(`"pack": {"url": "files/MyPack-0.1.0.zip", ...}`). The pack needs `NvrContentOverlay.dll`
from the catalogue: upload it with a mods entry of its own when the published catalogue
doesn't have it yet.

### Publishing a content pack (EchoCombat)

1. Build the zip and its catalogue entry with the url inside your folder. EchoCombat:
   `python tools/make_release.py --url-base files/` writes `release/EchoCombat-<version>.zip`
   and `release/echocombat.mods.json` (its README: Publishing to the launcher).
2. `sftp -P 2999 dev-<name>@168.119.2.94`, then `put release/EchoCombat-<version>.zip files/`
   and `get mods.json`.
3. In `mods.json`, replace the pack's entry (same `id`) with the new one, keep the
   `content-overlay` entry, and `put mods.json`.
4. **Use code** again in the launcher (or wait for the next update check): a newer
   `version` shows as UPDATE on the pack's rows; GET installs it the first time.

A pack's plugins only run on a nEVR with the early load pass (the launcher's Beta channel
for now): on another one the Mods page keeps the pack off and its **Channel…** button opens
Advanced settings. A pack that changes gameplay also needs its own game servers: players
point their game at one under Settings → Launch options → **<Pack> server**
([mods.md](mods.md#a-packs-own-server)).

A new version: upload the file under a new name (or overwrite it), change `version` and
`sha256`, upload the catalogue. The launcher reads your catalogues when it starts, when
you press **Use code**, every 30 seconds while the launcher is open (every 15 minutes from the
tray while it is closed), and when **Refresh** on the Plugins or Mods page is pressed. A new version is then offered as an update, and a plugin your folder
lists for the first time is announced (status bar, and a desktop notification). Keep the code to yourself: anyone with it can
download what's in your folder.

### Quest dev folders

The Quest installer has dev folders of its own, at
`https://release.echovr.de/quest/launcher/dev/<code>/`, with the same files (`mods.json`,
`plugins.json`, `files/`). They have their own codes, 10 characters (`abcde-fghij`) to type
on a headset, and their own logins
(`sftp -P 2999 qdev-<name>@168.119.2.94`): a launcher dev code opens no Quest folder, and a
Quest code opens no launcher folder. The launcher doesn't read Quest dev folders.

Besides `mods.json` and `plugins.json`, a Quest folder has `versions.json`: test builds, as there
is no Quest mod loader yet. It lists apps side by side, each with its own Android `package`, its
APK (`url`, `sha256`) and optional data (`data_url`, `data_sha256`, `data_target`), for example
Echo VR and EchoCombat. An entry with a published `id` (`quest-latest`) replaces that build for
whoever uses the code. Upload the files into `files/` first and `versions.json` last.
