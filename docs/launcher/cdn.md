# Downloads through the CDN, and going back

## Where files come from

Since 0.11.18 the launcher downloads what is on `release.echovr.de` from the Bunny CDN,
`https://cdn.echovr.de/`, first. bunny.echo copies release.echovr.de there every minute
(`bunny-sync`, with purges of what changed). release.echovr.de stays the fallback for
every file, and the source of truth: catalogues and manifests keep naming
`https://release.echovr.de/...`, only the fetch goes to the CDN (`src/core/sources.rs`).

The CDN can be behind for a minute or two after a release, or longer if the sync stalls.
So the launcher takes a file from the CDN only when it knows the file's SHA-256 beforehand,
and checks it:

- **Small files** (the catalogues, update manifests, `SHA256SUMS`, `rollback.json`): their
  checksums come from **`https://release.echovr.de/index.json`**, always read from
  release.echovr.de first:

  ```json
  {"schema": 1, "serial": 42, "files": {"launcher/mods.json": "<sha256>", "...": "..."}}
  ```

  A CDN copy that doesn't match is fetched from release.echovr.de instead. A file the
  index doesn't list comes from release.echovr.de only, as before 0.11.18.
- **Big files** (game zips, update files, mods, plugins, launcher downloads) carry their
  checksum in what names them (a manifest, a catalogue, `SHA256SUMS`, or the index for the
  game zips). A CDN copy that doesn't match, or a CDN that fails, hands over to
  release.echovr.de (and the evr.echo.taxi bucket for the game zips). A partial download
  from the CDN isn't continued from another server.
- **When release.echovr.de can't be reached**, the CDN's copy of `index.json` stands in, but
  never one with a lower `serial` than the launcher has seen: a CDN that is behind leaves
  the launcher where it is rather than taking it back to older versions.
- **Never from the CDN:** `launcher/logs`, the dev folders (`launcher/dev/`,
  `quest/launcher/dev/`), the live feed (`launcher/feed/`), `stats/`, `index.json` itself.

`index.json` is written on release.echo by `echo-cdn-index` whenever a listed file
changes; its `serial` goes up by one each time. Debug builds take `ECHOVR_CDN=<url>` (a
test server standing in for the CDN) or `ECHOVR_CDN=off`.

## Going back to the version before an update

Settings → **Go back** lists every part that can go back: the launcher, each version's
game update (nEVR and the rest of the update), each mod, each plugin. Each update keeps
what it replaced, one step back:

| Part | Kept in |
| --- | --- |
| Game update | `<game>/_local/previous-update/` (`previous.json` + `files/`) |
| Mod | `<game>/_local/previous-mods/<file>/` |
| Plugin | `<plugins folder>/.<id>.previous/` |
| Launcher | the release on the download servers (`launcher_previous` in launcher.json) |

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

Launchers before 0.11.18 don't read it, and players without a kept copy (installed after
the update) have no way back: for them, publish the old files again (the old manifest
lines in `updates-nevr/`, the old entry in a catalogue). Their launchers take that as an
update.
