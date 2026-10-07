# Launcher catalogues

The launcher reads `https://release.echovr.de/launcher/versions.json` (this folder has the
current draft). Until it is published, a built-in list with the same content is used.

Rules the launcher enforces (see `src/core/launcher/catalog.rs`):

- `id`: lowercase letters, digits, `.`, `-`, `_` (max 64). It becomes the install folder
  name inside the library, so it must never change for a published version.
- `url` / `data_url`: either a path relative to the download mirrors
  (`release.echovr.de` / `evr.echo.taxi`, the fastest is picked, and the other is asked
  when it doesn't have the file), or an absolute `https://` URL on one of those hosts or `release.echovr.de` (the event
  builds, `pc.zip` and their manifests).
  An empty `url` lists a build that can't be downloaded yet: INSTALL says so.
- `update_manifest`: absolute `https://` URL on those three hosts, in the usual
  `add <path> <sha256>` / `del <path>` format. Applied after install and by "Update".
- `sha256`: optional; when set, the downloaded zip must match before it is extracted.
- `files_manifest`: optional absolute `https://` URL of the build's file manifest
  (`<archive>.manifest` beside it, e.g. `pc.zip.manifest`): an `add <path> <sha256>` line
  for every file in the zip, and `# Archive:` / `# Root:` headers naming the zip and its
  one top folder. After an install every game file is checked against it, and broken
  ones are fetched again out of the zip with range requests. REINSTALL on the Install
  page does only that check and repair, without downloading the zip again. The game's
  own `_local/` and `_temp/` folders are never checked.
- `publisher_lock`: makes it an event build, played on the classic lobbies
  ([EchoRelay](https://github.com/heisthecat31/EchoRelay)) server set in Settings. Its
  install gets EchoLoader 2 in the crash reporter's place (`BugSplat64.dll`, as nEVR is on
  the live build) and EchoRelay's patch as its plugin (`plugins/EchoRelay.Patch.dll`;
  Halloween 2017 also gets `NvrMissingTextures.dll` for the textures its package lacks).
  A build set up the way EchoRelay's own installer does it (the patch as `dbgcore.dll`, or
  `dbghelp.dll` on the 2017 builds) is moved over at its next PLAY. Every PLAY writes its
  `_local/config.json` with the server, this lock and the player's account. It gets no
  update and starts without arguments. Use the lock EchoRelay expects: `rad15_live` for
  Christmas 2017, not its own `ea_rel6_0`.
- An archive with one top folder (`echo-vr-6/`, `Echo VR Halloween 2017/`) is unpacked
  into `ready-at-dawn-echo-arena/`, as every install is.
- `size`: optional, bytes, shown in the list.
- `hosted`: optional, `"live"` or `"event"` for builds the community's main servers run.
  The Install page lists these first, with a red LIVE BUILD or orange EVENT BUILD tag,
  above a divider; everything else goes below it. Unknown values count as not hosted.
- `summary`: optional, one short line shown next to the name in the Install page's list.
- `version`, `released`: optional, the build's own version number and its release date
  (`YYYY-MM-DD`), shown under its name in that list ("V1.76 · 2017-10-19").
- `exe`: optional, the executable's file name when it isn't `echovr.exe` (the 2017
  builds start `EchoArena.exe`). The launcher looks for it in
  `ready-at-dawn-echo-arena/bin/win10`, then `bin/win7`.

An entry that breaks a rule (or has a `platform` this launcher doesn't know) is left out;
the rest of the list is still used.

To publish a new build side by side with the current one, add an entry with a new `id`,
its own zip and (if it gets updates) its own update manifest.
