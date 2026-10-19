# Downloads through the CDN, and going back

## Where files come from

The launcher downloads what is on `release.echovr.de` from the Bunny CDN,
`https://cdn.echovr.de/`, first (since 0.11.18; everything, the dev folders and the index
included, since 0.12.0). bunny.echo copies release.echovr.de there every minute
(`bunny-sync`, with purges of what changed). release.echovr.de stays the fallback for every
file, and the source of truth: catalogues and manifests keep naming
`https://release.echovr.de/...`, only the fetch goes to the CDN (`src/core/sources.rs`).

**By checksum (0.11.19).** release.echovr.de keeps every public file under its SHA-256 as
well, `store/sha256/<first two hex digits>/<sha256>` (`echo-cdn-index`). Such an address
only ever holds that one file, so no cache can serve an old copy of it, or one mixed from
two versions. The launcher asks the CDN only by these addresses, and only for a file whose
checksum it knows beforehand; what comes back is checked anyway:

1. `https://cdn.echovr.de/store/sha256/<aa>/<sha>`
2. `https://release.echovr.de/store/sha256/<aa>/<sha>`
3. the file's own path on release.echovr.de (and the evr.echo.taxi bucket for the game
   zips), checked as before.

A partial download from a checksum address may be continued from the other one (the same
bytes), not from a path. Repairing single game files reads them out of the game's zip at
its checksum addresses too, block by block, going on from the next one when one fails.

**A CDN that can't be reached** (no connection within 5 s, a timeout) is left out for ten
minutes: everything comes straight from release.echovr.de meanwhile. A 404 or a wrong file
doesn't count: the CDN is there, release.echovr.de stands in for that one file.

Where the checksums come from:

- **Small files** (the catalogues, update manifests, `SHA256SUMS`, `rollback.json`,
  `history.json`): **`index.json`**, read from the CDN first (0.12.0; before, from
  release.echovr.de first):

  ```json
  {"schema": 1, "serial": 42, "checked": "2026-10-19T17:00:00+00:00",
   "files": {"launcher/mods.json": "<sha256>", "...": "..."}}
  ```

  A file the index doesn't list comes from release.echovr.de only.
- **Dev folders** (`launcher/dev/<code>/`, `quest/launcher/dev/<code>/`) have an
  `index.json` of their own, listing everything in the folder (paths inside it), read the
  same way; the public index never names them (their paths hold the codes). Their files
  are in the store too.
- **Big files** (game zips, update files, mods, plugins, launcher downloads, the Revive
  artwork) carry their checksum in what names them (a manifest, a catalogue,
  `SHA256SUMS`, a pinned constant, or the index for the game zips and the Quest's game
  data).
- **release.echovr.de's index instead** when the CDN's has a lower `serial` than one the
  launcher has seen (the CDN is behind: the launcher never goes back to older versions for
  it), or when `checked` is more than 20 minutes older than the CDN's answer (its `Date`):
  echo-cdn-index writes it again at least every 5 minutes, so the sync to the CDN has
  stopped. A stale CDN index is still used when release.echovr.de can't be reached either.
- **Never from the CDN:** `launcher/logs`, the live feed (`launcher/feed/`), `stats/`.

`index.json` is written on release.echo by `echo-cdn-index` whenever a listed file
changes (its `serial` goes up by one each time), and every 5 minutes anyway (a new
`checked`). bunny-sync uploads every `index.json` last, after what it names (a dev folder's
only after release-main uploaded the folder's store objects), so the CDN's index never
names a file the CDN doesn't have yet. Debug builds take `ECHOVR_CDN=<url>` (a test server
standing in for the CDN) or `ECHOVR_CDN=off`.

**The plain copy.** cdn.echovr.de also serves every file at its plain path
(`https://cdn.echovr.de/updates-nevr/BugSplat64.dll`), for links and for 0.11.18, which asks
for those (and checks them). Right after a file changes, its plain path can serve the whole
previous file for a minute or two, until bunny-sync's upload and purge; never a mixed one
(large-file slicing is off on the pull zone), and `echo-cdn-check` on bunny.echo fetches
every changed file from the CDN and purges again when it isn't right. Launchers since
0.11.19 don't use plain paths.

## Going back to the version before an update

Settings → **Go back** lists every part that can go back: the launcher, each version's
game update (nEVR and the rest of the update), each mod and content pack, each plugin.
Right after an update the launcher goes back to what it kept (no download); otherwise it
takes the version before from release.echovr.de's history (below), so a player who
installed after the update can go back too, and pressing Go back again steps one more
version back. Each update keeps what it replaced, one step back:

| Part | Kept in |
| --- | --- |
| Game update | `<game>/_local/previous-update/` (`previous.json` + `files/`) |
| Mod | `<game>/_local/previous-mods/<file>/` |
| Plugin | `<plugins folder>/.<id>.previous/` |
| Launcher | the release on the download servers (`launcher_previous` in launcher.json) |
| Anything, without a kept copy | release.echovr.de's history and store (0.11.19) |

### release.echovr.de's history

`https://release.echovr.de/launcher/history.json` (in the index, written by
`echo-cdn-index`, only ever added to) lists every version each update manifest, mod and
plugin had, oldest first; their files stay in the store:

```json
{"schema": 1,
 "updates": {"updates-nevr/update.manifest": [{"sha": "<manifest sha256>", "at": "2026-10-11T17:45:44+00:00", "title": "nEVR runtime 4.1.0"}]},
 "mods":    {"<id>": [{"...the catalogue entry...": "", "at": "..."}]},
 "plugins": {"<id>": [{"...the catalogue entry...": "", "at": "..."}]}}
```

- An entry whose files aren't all in the store has `"incomplete": true` and is never
  offered. `"sha": null` is an update manifest taken away (a channel without an overlay is
  exactly main).
- The game update's versions are main's manifest with the channel's overlays as they were
  at each point in time, the same text the launcher hashed then. Which one is installed is
  told by the files on disk; the way back is the last version before it that changes a
  file. Files the newer version added are deleted.
- Mods, packs and plugins go back to the entry listed before the installed version; from a
  version the history doesn't know, to the newest published one older than it. A plugin
  you put there yourself (not from a catalogue) has none.
- **Dev folders** have a `history.json` of their own (the same format, URLs relative to the
  folder; 0.12.0). With a dev code set it comes in with the published one: a dev build goes
  back to the dev build before it, the first one to the published version before it.
- Going back from the history keeps no copy of the newer version: "Use newest" downloads it
  as an update.

The version gone back from is held (`holds` in launcher.json): it isn't offered as an update
again, a newer one is; **Use newest** lets go of the hold. A plugin's menu, a mod's row and
a version's MANAGE menu go back from where the update was.

### Pulling a version for everyone

`https://release.echovr.de/launcher/rollback.json` lists versions with a serious problem:

```json
{"schema": 1, "pulled": [
  {"part": "game", "sha": "<sha256 of the update manifest it brought>", "note": "Echo VR closes at the loading screen.", "auto": false},
  {"part": "launcher", "version": "0.11.18", "note": "...", "auto": true},
  {"part": "mod", "id": "<catalogue id or file>", "version": "1.1.0", "note": "..."},
  {"part": "plugin", "id": "game-server", "version": "0.3.0", "note": "..."}
]}
```

Whoever has a pulled version and its way back is told (status bar, desktop notification,
the Go back card opens), or with `"auto": true` the launcher goes back by itself when no job
and no Echo VR is running. For the game, `sha` is the sha256 of the manifest text the update
was applied from: main's manifest plus the channel's overlays, as the launcher hashes it
(`sha256` of the manifest file alone on main).

Launchers before 0.11.18 don't read it; 0.11.18 reaches only players with a kept copy;
since 0.11.19 everyone with a version in the history. For the older launchers, publish the
old files again (the old manifest lines in `updates-nevr/`, the old entry in a catalogue):
they take that as an update.
