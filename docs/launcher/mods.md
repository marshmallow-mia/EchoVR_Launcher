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
put them back, Verify reports any change.

**A test channel.** To try a community update before it goes live, point the launcher at
another manifest: `"update_manifest": "https://release.echovr.de/updates-nevr-test/update.manifest"`
in `launcher.json` (in the launcher's data folder), or the environment variable
`ECHOVR_UPDATE_MANIFEST` with the same URL, which comes first. Only
`https://release.echovr.de/updates-*/update.manifest` or the same on files.echovr.de is
taken; anything else is ignored and logged. Update, Check for updates and Verify then use it
for the live PC version, and Settings says so ("updates from the test channel …", with
**Back to the live channel** for the `launcher.json` one). Back on the live channel, the next
update or Verify puts the live files back.

The launcher never edits the community update's files. It writes:

| File | What it holds |
|---|---|
| `_local/config.yaml` (the game's `_local`) | nEVR's plugin list, written before every start from the plugins in `bin/win10/plugins` and the launcher's choices; with mods off only the required plugins (NvrAssetPatches with `required_only`). When a pack that is on has a server of its own, also that server and the account there ([below](#a-packs-own-server)) |
| `_local/launcher-mods.json` | the choices: "Start without mods" (`enabled`), a plugin on or off and its arguments (`overrides`), the plugins it added (`add`, each with its `sha256`) |
| `_local/.credentials.json` | the game's own EchoVRCE sign-in (see below); `.credentials.json.echovrce` while a start plays on a pack's own server |
| `_local/launcher-shipped.json` | the plugins the last community update placed in `plugins/`, by their checksums (0.12.4 on; going back to the update before forgets it) |
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
(the launcher's own copy, put in before every start). Since 0.12.4 a plugin the update
ships is checked by its checksum too: once `launcher-shipped.json` notes the update's
plugins, a file that is neither what the update placed nor the catalogue's version (one
replaced by hand) is left out like a DLL from disk; a version updated before keeps going by
the file's name until its next update. Anything else (a DLL added from disk, or one put into
`plugins/` by hand) is listed as not loaded and left out of `config.yaml`, and Add DLL is off, until the
version's loader config turns local plugins on: a top-level `x-local-plugins: true` in
`_local/config.yaml`, which nEVR ignores (it leaves `x-` keys to others) and the launcher
keeps when it writes the file. Writing a plugin and loading it:
[docs/plugins/local-plugins.md](../plugins/local-plugins.md).

**Required plugins.** The catalogue marks plugins the game needs (`"required": true`:
NvrAssetPatches for the netgun fixes; NvrXmlHttpFix for windowed mode under nEVR 4.0.0, which
the 4.0.1 update removes, while its entry stays so a 4.0.0 install not yet updated keeps it).
They can't be turned off, an old "off" choice is ignored and dropped, and they stay in
`config.yaml` with mods off. Inside NvrAssetPatches the update's
`asset_patches/manifest.json` marks patches `required` (the `netgun_*` ones): the plugin
loads them whatever `manifest.local.json` says, and the page shows them locked.

**Arguments.** A plugin's defaults are the catalogue's `args` for its file, written into
`config.yaml` unless you changed them. A plugin with a description (the catalogue's
`settings`, or `<Stem>.plugin.json` beside it) shows its settings in a sheet instead, see
[../plugins/settings.md](../plugins/settings.md). The Options editor resets one argument (or takes
out one that isn't a default) and has "Reset to default" for all of them. The launcher keeps
the last catalogue it fetched (`mods.json` in its data folder) to use offline before a
start.

**The game's own config.** nEVR doesn't need `_local/config.json`: without one it supplies
its built-in game config, with friends, parties, presence and matchmaking on, and it takes
no server from a `config.json` anyway (a plugin's server goes into `config.yaml`). So
before a start with nEVR the launcher keeps any `config.json` in a `_local` nEVR looks in as
`config.json.old` (`.old.2`, … when that is taken; one an older launcher kept as
`config.json.pre-nevr` becomes `.old` too), and says so once. A start without nEVR puts it
back first. The `config.json` the launcher writes for an EchoRelay server stays: it goes
with EchoRelay's other files when nEVR is back.

**A `config.yaml` of your own** is kept as `config.yaml.old` (`.old.2`, …) before a start,
and the launcher writes its own, so the Mods page's choices and a plugin's server reach the
game. The launcher's own starts with `# Written by the Echo VR launcher` (on any line: the
local plugins' switch may be above it), or sets nothing but `x-local-plugins`; a
`config.yaml` nearer the exe, which nEVR would read first, goes too. A switch for local
plugins in the one kept as `.old` stays on. A game server of your own gets a setup in a
folder of its own, which this leaves alone: see [the plugins' `host`
step](../plugins/pages.md#a-game-servers-setup).

nEVR looks for `_local/config.yaml` beside the exe, then one and two folders up (the
game's own `_local`); the first found wins, so one nearer the exe is kept as `.old` too. nEVR refuses to start with a `dbgcore.dll` beside the exe (the old
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
current draft, which is also built in for when it can't be fetched). To test a mod before
it's in there, see [dev folders](dev-folders.md).

```json
{ "schema": 1,
  "mods": [
    { "id": "combat-stats", "name": "Combat Stats", "summary": "One line on what it does.",
      "author": "…", "version": "0.3.1", "file": "CombatStats.dll",
      "url": "mods/CombatStats-0.3.1.dll", "sha256": "…", "size": 412000,
      "api": 5, "capabilities": ["observes-only"], "args": { "key": "value" },
      "settings": { "fields": [ … ] },
      "homepage": "https://github.com/…", "shipped": false } ] }
```

- `id`: lowercase letters, digits, `.`, `-`, `_` (max 64).
- `file`: the plugin's file name in `plugins/`: letters, digits, `.`, `-`, `_`, ending
  in `.dll`; never `BugSplat64.dll`.
- `url`: relative to the download mirrors (`release.echovr.de` / `evr.echo.taxi`), or an
  absolute `https://` URL on one of them or `release.echovr.de`. Required, with `sha256`, unless `shipped`.
- `sha256`: the file's checksum. The launcher checks the download against it and notes it
  in `launcher-mods.json`, so a file changed later is left out of `config.yaml`.
- `shipped`: the community update brings it; the page lists it under Installed mods when it is
  there, never downloads it, and never offers it under More mods.
- `required`: the game needs it: always on, also with mods off, and never offered under
  More mods.
- `version`: shown, and compared with the installed one: a different version puts UPDATE
  on its row under Installed mods.
- `api`, `capabilities` (`observes-only`, `cosmetic`, `alters-gameplay`, `alters-rules`,
  `network`, `hooks-engine`), `args` (its default arguments), `homepage`, `author`,
  `summary`, `size`: optional, shown on the page.
- `settings`: optional, what can be set and how: the `settings` object of a plugin
  description ([../plugins/settings.md](../plugins/settings.md)). It comes before a
  `<Stem>.plugin.json` beside the DLL. The plugin's row then has **Settings** instead of
  **Options**.

An entry that breaks a rule is left out; the rest of the list is still used.

### Third-party mods: catalogue.json

Mods from other developers (submitted at
[EchoTools/echo-mods](https://github.com/EchoTools/echo-mods), made from
[EchoTools/echo-mod-template-repo](https://github.com/EchoTools/echo-mod-template-repo)) are
published by the server's mirror into `launcher/catalogue.json`, which only launchers since
0.13 read: `mods.json` stays the launcher's own mods, so launchers before 0.13 never see a
third-party mod. `catalogue.json` has every version of each mod, newest first, with these
fields beside a `mods.json` entry's (and the mod's `publisher`, its GitHub account):

| Field | Meaning |
|---|---|
| `prerelease` | A semver prerelease (`1.3.0-beta.1`) |
| `requires` | `{"nevr": ">=4.1.0", "mods": [{"id": "…", "version": ">=1.0.0"}]}` |
| `conflicts` | Mod ids that can't be on beside it |
| `blocked` | `{"action", "note", "alert"}`: never offered |

- They're tagged **Community** (More mods says who published them), and the first **Get** of
  one says once what that is (`community_mods_ok` in `launcher.json`).
- The launcher offers each mod's newest version no block covers (here or in
  `rollback.json`). A prerelease only for the mods the player turned **Prereleases** on for
  (the mod's **More** menu), never for all at once; a mod that has only prereleases is offered
  as one, and getting it turns its prereleases on. Under an installed prerelease with
  prereleases off, the row offers **Switch to stable**. Only newer versions (semver) count as
  updates.
- Without `catalogue.json` (nothing published that way yet, or offline with none kept) there
  are no third-party mods to get; installed ones keep loading. A catalogue entry never
  replaces the launcher's own mods (shipped or required ones, or their files) and is never
  shipped or required itself.
- **Unlisted** mods aren't in it: More mods → **Add by id** looks one up
  (`launcher/unlisted/<id>.json`, else a **private** one behind the EchoVRCE sign-in,
  `launcher/gated/private/<id>.json`, for the accounts its maker allowed), checks it has
  something to offer, and lists it (`unlisted_mods` in `launcher.json`).
- **More** on a mod: Prereleases, Source (when the catalogue and dev folders both have it),
  Report this mod (the logs go to the moderators with the mod and a note; needs the EchoVRCE
  sign-in), Homepage.
- Install notes where a mod came from and what its version needs. **Get** offers to install
  the mods it needs too, refuses a version that needs another nEVR (Beta brings the newest)
  and one that can't run beside a mod that's on. **Update** goes through the same checks;
  the update check and Play's update button leave a version that needs the player's OK (a
  block, nEVR, a mod it needs or conflicts with) to the Mods page.
- Before a start a mod stays out when it is blocked, waits for a mod it needs (missing, off,
  out itself, or another version: a prerelease of it counts), or declares a conflict with one
  that's on (only the mod that declares it). What's left out leaves out what needs it, until
  nothing more drops. Its row says why (a chip, its reason on hover;
  `_local/launcher-left-out.json`), as it does for a file changed since it was installed
  (**Reinstall**). Nothing is uninstalled for it, and a mod is never turned off because
  metadata appeared. A blocked content pack serves nothing at all.
- Going back (a block, or **Go back**) takes only sound entries from the history: checked like
  a catalogue's, never a blocked version, never a prerelease of a mod whose prereleases are
  off.
- A mod installed from the catalogue that it doesn't list any more keeps loading, tagged **No
  longer available** (a frozen mod, or one from a removed dev code).
- When Echo VR closes within a minute and a half twice since mods changed, the launcher asks
  whether to start without them (`runs` in `launcher-mods.json`); nothing is turned off
  without that click.

### A release in release-signer's shape

A mod whose plugin is signed ships as a release of three files
([release-signer's `docs/plugin-release.md`](https://github.com/thesprockee/release-signer/blob/teth/plugin-release-spec/docs/plugin-release.md),
section 4): `<id>-v<version>-windows.zip` (the DLL, `LICENSE`, `NOTICE` and, with settings,
`<Stem>.plugin.json`, flat, and a `SHA256SUMS` over them), `echo-mod.json` and `SHA256SUMS`
(a line for the zip, one for the manifest). Its entry has a `release` object in place of
`url` and `sha256`; it is the version object of the mod's echo-mods entry (`tag`, `assets`,
`signer`) and a `base`:

```json
{ "id": "foo", "name": "Foo", "version": "1.2.0", "file": "Foo.dll",
  "release": {
    "base": "https://github.com/EchoTools/foo/releases/download/v1.2.0", "tag": "v1.2.0",
    "assets": { "package":  { "name": "foo-v1.2.0-windows.zip", "sha256": "…", "size": 44100 },
                "manifest": { "name": "echo-mod.json", "sha256": "…", "size": 611 },
                "sums":     { "name": "SHA256SUMS", "sha256": "…", "size": 182 } },
    "signer": { "status": "signed", "record": "https://…", "signed_at": "2026-10-27T09:30:00Z",
                "package": { "sha256": "…", "size": 44100 }, "sums": { "sha256": "…", "size": 182 } } } }
```

- `base`: where the three files are: relative to the download mirrors, `https` on a trusted
  host, or the EchoTools fork's GitHub release of `tag` (`https://github.com/EchoTools/<repo>/releases/download/<tag>`).
- The names are the spec's (`<id>-v<version>-windows.zip`, `echo-mod.json`, `SHA256SUMS`) and
  `tag` is `v<version>`; an entry that differs is left out.
- What each file must hash to: `assets.*.sha256` (what the release workflow attested), except
  that with `signer.status` `signed` (or `revoked`) the zip and the sums are `signer.package`
  and `signer.sums`: signing replaces the DLL and regenerates both sums files. The manifest
  passes through signing untouched.
- Install downloads the three files and checks, before anything is put into `plugins/`: each
  against the pin above; the zip and the manifest against the release's `SHA256SUMS`;
  `echo-mod.json`'s `id`, `version`, `file` and `package` against the entry; the zip's entries
  (exactly the DLL, `LICENSE`, `NOTICE`, `<Stem>.plugin.json` and `SHA256SUMS`; flat; none over
  16 MiB) against the zip's own `SHA256SUMS`. Then the DLL (and its settings file) go into
  `plugins/` and the DLL is noted with its checksum, as for a loose DLL. A mismatch stops
  the install with nothing changed.
- `signer.status` (`unsigned`, `pending`, `approved`, `signed`, `revoked`) is the listing's
  record of the release in signing. A `revoked` release isn't installed, nor one with a
  status this launcher doesn't know. The Mods page shows it: More mods' info line says
  where signing is (Not signed yet, Signing pending, Signed, Signature revoked), and an
  installed mod gets a chip only for `signed` (green) or `revoked` (amber), and only when
  the launcher installed that very copy from the release (`"release": true` in its
  `launcher` note in `launcher-mods.json`), never for a loose DLL of the same version.
- Blocks (`rollback.json`): by id and version as for any mod. A block by `sha` alone
  matches a release entry by its zip, as attested (`assets.package.sha256`) or as signed
  (`signer.package.sha256`); an installed copy is checked at every start by the checksum
  of its DLL as placed. Files on GitHub can't be withdrawn by release.echovr.de, so a
  block of such a release names its versions.
- An entry with a `release` and a `url` uses the release; the loose DLL (`url`, `sha256`)
  stays for entries without one, and for launchers that predate it. A version in
  `catalogue.json` is such an entry too, so it may carry a `release` the same way.
- Where to publish one: `catalogue.json` (0.13+). Launchers before 0.13.0-alpha.4 don't read
  `release` and leave an entry without `url` out; a first-party entry in the live
  `mods.json` keeps its `url` and `sha256` beside a `release`, so older launchers still get
  the loose DLL.

### Content packs

A mod that brings game data as well as plugins (EchoCombat: new gear, scripts, larger
tables) is a **pack**: one zip, and a `pack` object in place of `file`, `url` and
`sha256`:

```json
{ "id": "echocombat", "name": "EchoCombat", "version": "1.0.0", "summary": "…",
  "author": "…", "capabilities": ["alters-gameplay", "network", "hooks-engine"],
  "pack": { "url": "packs/EchoCombat-1.0.0.zip", "sha256": "…", "size": 13784020,
            "content": "echocombat",
            "plugins": ["DroneFix.dll", "PowerBoost.dll", "VeilNet.dll"] } }
```

- The zip holds `plugins/` (the listed plugins and their settings files: `.ini`, `.cfg`,
  `.txt`, `.json`) and `content/<content>/` (its content pack: the manifests, the
  packages they add, script DLLs and `content.json`). Files at its top (a README) are left
  out; anything else refuses the pack.
- Install puts the plugins and settings files into `plugins/` and the content pack into
  `bin/win10/content/<content>/`, each plugin noted with its checksum, all on. Before that
  it installs the catalogue's `NvrContentOverlay.dll` when the version doesn't have it.
- The page treats the pack as one mod: its plugins' rows say "part of <name>", and on, off
  or Remove on any of them is the pack's. Remove takes its plugins, settings files and
  content pack out (the overlay plugin stays).
- `config.yaml` lists the overlay with `early: true` and `args: {"active": "<content>"}`,
  and the pack's plugins, only when everything is in place: the pack on, mods on, its
  files there, and a nEVR in the slot that loads plugins early (the launcher looks for its
  early load pass in `BugSplat64.dll`). Otherwise none of it: a pack's plugins without
  its game data would run on stock data. The nEVR card says why a pack stays off.
- The overlay serves the pack only on stock game files (it checks the manifests' sha256).
  When an older installer changed them (a `.bak` beside a manifest, `.dll.orig` scripts),
  the nEVR card says so: Install, then the version, checks and repairs them.
- With a nEVR that can't load plugins early, the nEVR card has a **Channel…** button (to
  Advanced settings: Beta brings that nEVR), and the pack's rows say "Off: needs Beta's
  nEVR" ("Off: game files changed" when an installer changed them).
- `pack` entries are never `shipped` or `required`. A different `version` in the
  catalogue puts UPDATE on the pack's plugins, which reinstalls the pack.
- One pack is served at a time (the first one on).

#### A pack's own server

A pack that changes gameplay works only in matches whose game server runs it too
(EchoCombat's OVERCHARGE is decided there, VeilNet relays through it), so it plays on
servers of its own. Settings → Launch options has a **<Pack> server** row for every pack
installed in a live version; **Server…** sets (`pack_servers` in `launcher.json`, by the
pack's `id`):

| | |
|---|---|
| Address | what the server's host gives out: `host[:port]` (Nakama's 7350 when left out) or a `ws://` / `wss://` address. Empty: EchoVRCE |
| Server key | the server's socket key, unless the address carries it (`token=`) |
| Discord ID, password | your account there |

While that pack is the one served, every start writes into `config.yaml`:

```yaml
services:
  socket_uri: "ws://host:7350/ws?format=evr&token=<server key>"
identity:
  discord_id: "<your Discord ID>"
auth:
  http_uri: "http://host:7350"
  password: "<your password>"
  server_key: "<server key>"
```

nEVR then logs in, matchmakes and (as a dedicated server, from the same file) registers
there. `http_uri` is the same server's HTTP API: nEVR's own sign-in (a device code approved
on echovrce.com) asks it instead of EchoVRCE, is turned down at once, and the game signs in
with the account above; nEVR tries a sign-in it finds before the config's account, so the
game's EchoVRCE sign-in waits as `.credentials.json.echovrce` meanwhile and is back for the
next start without the pack. `${` in a value is written as `$${` (nEVR would read it as an
environment variable). The password is in plain text there, as nEVR reads it: don't reuse a
real one. Play's line says where a start goes ("EchoCombat on host:7350"). A launcher
plugin can set such a server for every start of the live build, not only with a pack
([../plugins/pages.md](../plugins/pages.md#the-games-server)); a pack that is on with a
server of its own keeps that one.

Today a game server under nEVR's `-server -headless` signs in but never begins a match
(nevr-runtime's `docs/reference/server-mode-multiplayer-hang.md`), so a pack's own server
can't host matches yet; signing in and matchmaking there work.

The page's **Installed mods** card lists only what is installed. **More mods** offers
the catalogue's entries that are neither required nor shipped and aren't in `plugins/`
(GET; "Coming soon" without a `url`), plus the VR parts not in use: EchoXR Hands, and on
Windows EchoXR while SteamVR plays through Revive. Remove on a row puts it back there.
