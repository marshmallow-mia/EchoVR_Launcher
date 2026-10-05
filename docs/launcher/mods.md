# Mods in the launcher

The live PC build's plugins are loaded by
[nEVR runtime](https://github.com/EchoTools/nevr-runtime), which is the game's crash
reporter, `bin/win10/BugSplat64.dll`, so it loads on every start path. Besides loading
plugins it points the game at EchoVRCE, signs it in with Discord, and brings friends and
parties into the game. This page covers what the launcher does with it
(`src/core/launcher/nevr.rs`, `src/core/launcher/mods.rs`, the Mods page
`src/ui/launcher/mods.rs`).

## Who writes what

The community update (`https://release.echovr.de/updates-nevr/update.manifest`: the one with nEVR, which every install and update gets; the old installer's channel without it is `updates/` on files.echovr.de) owns
`BugSplat64.dll`, the plugins it ships and `asset_patches/manifest.json`: Update and Repair
put them back, Verify reports any change. The launcher never edits them. It writes:

| File | What it holds |
|---|---|
| `_local/config.yaml` (the game's `_local`) | nEVR's plugin list, written before every start from the plugins in `bin/win10/plugins` and the launcher's choices; with mods off only the required plugins (NvrAssetPatches with `required_only`) |
| `_local/launcher-mods.json` | the choices: "Start without mods" (`enabled`), a plugin on or off and its arguments (`overrides`), the plugins it added (`add`, each with its `sha256`) |
| `_local/.credentials.json` | the game's own EchoVRCE sign-in (see below) |
| `bin/win10/asset_patches/manifest.local.json` | every asset patch, or one of them, on or off |
| `bin/win10/plugins/<file>.dll` | only the plugins it installed (from the catalogue) or added (from disk) |

Every DLL in `plugins/` is listed (on unless turned off), except EchoRelay's
`dbgcore.dll`, which the update still ships but nEVR can't load (it has no
`NvrPluginGetInfo`; nEVR does what it did). An entry the launcher adds carries
`{"catalog_id": "…", "version": "…"}` for a catalogue mod, `{"local": true}` for a DLL from
disk. That is what makes it removable: REMOVE deletes only such files, never one the update
ships. Before every start the launcher checks an added plugin against its `sha256` and
leaves it out of `config.yaml` when the file changed.

**Verified plugins only.** A plugin is verified when it comes from the mods catalogue, or
its file is one the catalogue names (the community update's), or it is EchoXR Hands'
(pinned by checksum). Anything else (a DLL added from disk, or one put into `plugins/` by
hand) is listed as not loaded and left out of `config.yaml`, and Add DLL is off, until the
version's loader config turns local plugins on: a top-level `x-local-plugins: true` in
`_local/config.yaml`, which nEVR ignores (it leaves `x-` keys to others) and the launcher
keeps when it writes the file. Writing a plugin and loading it:
[docs/plugins/local-plugins.md](../plugins/local-plugins.md).

**Required plugins.** The catalogue marks plugins the game needs (`"required": true`:
NvrAssetPatches for the netgun fixes, NvrXmlHttpFix for windowed mode under nEVR 4.0.0).
They can't be turned off, an old "off" choice is ignored and dropped, and they stay in
`config.yaml` with mods off. Inside NvrAssetPatches the update's
`asset_patches/manifest.json` marks patches `required` (the `netgun_*` ones): the plugin
loads them whatever `manifest.local.json` says, and the page shows them locked.

**Arguments.** A plugin's defaults are the catalogue's `args` for its file, written into
`config.yaml` unless you changed them. The Options editor resets one argument (or takes
out one that isn't a default) and has "Reset to default" for all of them. The launcher keeps
the last catalogue it fetched (`mods.json` in its data folder) to use offline before a
start.

**The game's own config.** nEVR doesn't need `_local/config.json`: without one it supplies
its built-in game config, with friends, parties, presence and matchmaking on. Before PLAY
the launcher moves an obsolete EchoVRCE one aside (to `config.json.pre-nevr`): only the old
service hosts and `publisher_lock`, every host on echovrce.com. One pointing at another
server, or with anything else in it, stays and nEVR uses it. "Use my own config.json" on the
Mods page turns the moving off and puts a file moved aside back.

nEVR looks for `_local/config.yaml` beside the exe, then one and two folders up (the
game's own `_local`); the first found wins, so the page warns when one nearer the exe
shadows the launcher's. nEVR refuses to start with a `dbgcore.dll` beside the exe (the old
loader's place): PLAY takes it out first, and the update manifest deletes it.

The page tells the DLL in the slot apart: nEVR (its `[NEVR.BOOT]` log string, version from
its build identity), the game's own crash reporter (its checksum), or something else. It
reads what nEVR loaded and why not from nEVR's newest log
(`%LOCALAPPDATA%\EchoVR\logs\nevr-<time>.jsonl`, the `[NEVR.PLUGIN]` lines). Mods are for
the live build only: event builds run EchoRelay's patch in the loader's place.

## The game's sign-in

nEVR signs the game in with an EchoVRCE device code, opened in the browser at start, and
keeps the refresh token in `_local/.credentials.json`. While the launcher is signed in
with EchoVRCE it hands every live version with nEVR a sign-in of its own instead: a
session linked as another device of the account (as for echovrce.com inside the window),
written as `{"refresh_token", "refresh_token_expiry", "user_id", "username"}`, so the game
starts signed in and its refresh token never rotates the launcher's out. It is renewed when
it is for another account or has less than a day left, checked every ten minutes. Signing
out of the launcher deletes it. The file never leaves the PC: "Upload logs" doesn't send
it.

## Linux

Proton loads `BugSplat64.dll` from the game's folder by itself (Wine has no builtin of
it). Flat starts with `-windowed` (nEVR's: no headset, in a window) instead of `-noovr`.
nEVR's logs are in the prefix's `drive_c/users/steamuser/AppData/Local/EchoVR/logs`.

"Upload logs" sends nEVR's newest start logs and crash records, its
`bin/win10/logs/nevr-boot.jsonl`, and each plugin's newest logs from its folder in
`plugin_logs`.

## The mods catalogue

The launcher reads `https://release.echovr.de/launcher/mods.json` (this folder has the
current draft, which is also built in for when it can't be fetched):

```json
{ "schema": 1,
  "mods": [
    { "id": "combat-stats", "name": "Combat Stats", "summary": "One line on what it does.",
      "author": "…", "version": "0.3.1", "file": "CombatStats.dll",
      "url": "mods/CombatStats-0.3.1.dll", "sha256": "…", "size": 412000,
      "api": 5, "capabilities": ["observes-only"], "args": { "key": "value" },
      "homepage": "https://github.com/…", "shipped": false } ] }
```

- `id`: lowercase letters, digits, `.`, `-`, `_` (max 64).
- `file`: the plugin's file name in `plugins/`: letters, digits, `.`, `-`, `_`, ending
  in `.dll`; never `BugSplat64.dll`.
- `url`: relative to the download mirrors (`release.echovr.de` / `evr.echo.taxi`), or an
  absolute `https://` URL on one of them or `release.echovr.de`. Required, with `sha256`, unless `shipped`.
- `sha256`: the file's checksum. The launcher checks the download against it and notes it
  in `launcher-mods.json`, so a file changed later is left out of `config.yaml`.
- `shipped`: the community update brings it; the page lists it under Plugins when it is
  there, never downloads it, and never offers it under Additional Plugins.
- `required`: the game needs it: always on, also with mods off, and never offered under
  Additional Plugins.
- `version`: shown, and compared with the installed one: a different version puts UPDATE
  on its row under Plugins.
- `api`, `capabilities` (`observes-only`, `cosmetic`, `alters-gameplay`, `alters-rules`,
  `network`, `hooks-engine`), `args` (its default arguments), `homepage`, `author`,
  `summary`, `size`: optional, shown on the page.

An entry that breaks a rule is left out; the rest of the list is still used.

The page's **Plugins** card lists only what is installed. **Additional Plugins** offers
the catalogue's entries that are neither required nor shipped and aren't in `plugins/`
(GET; "Coming soon" without a `url`), plus the VR parts not in use: EchoXR Hands, and on
Windows EchoXR while SteamVR plays through Revive. Remove on a row puts it back there.
