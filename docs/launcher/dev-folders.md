# Testing mods and plugins before they're published

With a **dev code** the launcher lists your own mods and launcher plugins from your **dev
folder** on release.echovr.de, before they're in the published catalogues. Only whoever has
the code sees them. Ask the launcher's maintainers for a folder and send them an SSH public
key; you get a code and an `sftp` command.

## In the launcher

Settings → **Advanced settings** → **Dev mods and plugins**: paste the code and press **Use
code**. The card says how many mods and plugins your folder lists. Your entries appear on the
Mods page (More mods) and the Plugins page with a **Dev** tag, and the status bar shows
**DEV CODE** while a code is set. An entry with the same id as a published one replaces it
(a mod also replaces one with the same file). **Remove** goes back to the published ones;
installed dev mods then offer the published version as an update.

## Your folder

```
/                 your folder (SFTP shows it as /)
├── mods.json     your mods catalogue: overwrite it
├── plugins.json  your launcher plugins catalogue: overwrite it
└── files/        your DLLs and zips
```

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
you press **Use code**, with every update check (every 15 minutes) and when the Plugins page's
**Refresh** is pressed. A new version is then offered as an update, and a plugin your folder
lists for the first time is announced (status bar, and a desktop notification). Keep the code to yourself: anyone with it can
download what's in your folder.
