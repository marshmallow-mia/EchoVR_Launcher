# Working on the Echo VR Launcher

How to build, change, test and release the launcher. It is written for coding agents and
people alike; [CONTRIBUTING.md](CONTRIBUTING.md) points here.

## What is where

- `src/main.rs`: the entry point. Besides the window it has modes without one:
  - `--play`: Echo VR started from Steam, WiVRn or a shortcut (`core::launcher::headless`).
  - `--server-host`: the Server host's supervisor.
  - `--tray`: the tray icon that looks for updates while the launcher is closed.

  `--after-update` is passed to the new launcher when its own update finishes.
- `src/core/`: everything without UI.
  - `launcher/`: installs, versions, mods, plugins, nEVR, updates, the launcher's own
    update (`self_update.rs`), channels (`update_check.rs`).
  - `server_host/`: the game servers the Server host runs.
  - `linux/`: Proton, Steam, the prefix, desktop fixes.
  - `logs.rs`: Upload logs.
  - `signing.rs`, `nevr_release.rs`: signature checks, with pinned keys.
- `src/ui/`: the egui interface. `launcher/` has the pages, `snapshot.rs` and
  `headless.rs` the screenshots, `cuts.rs` the check for text cut with "…".
- `plugins/`: the first-party launcher plugins (Rust → WebAssembly components, own
  workspace). They are built into `docs/plugins/<id>/plugin.wasm`.
- `docs/launcher/`: the formats the launcher reads from the release server (catalogues,
  alerts, CDN, dev folders, mods). `docs/plugins/`: how to write a plugin.
- `windows/`: the manifest, resources and NSIS installer.
- `scripts/`: packaging, plugin builds, plugin signing, platform-tools refresh.

## Build

A stable Rust toolchain (rustup) is all the launcher needs:

```sh
cargo run              # debug build
cargo build --release  # target/release/EchoVR_Launcher(.exe)
```

On macOS with rustup, put `~/.cargo/bin` first in `PATH`
(`PATH=~/.cargo/bin:$PATH cargo …`). Another `cargo` on the path, such as Homebrew's, can
build with a toolchain that is too old, and a filtered build then looks clean when it
isn't.

### Windows exe from macOS or Linux

This is what the Release workflow does:

```sh
rustup target add x86_64-pc-windows-gnu
# mingw-w64 with posix threads (Homebrew's is; on Debian/Ubuntu use update-alternatives
# to pick x86_64-w64-mingw32-gcc-posix), plus nsis and 7z for the packages
cargo build --release --target x86_64-pc-windows-gnu
```

### Plugins

`scripts/build-plugins.sh [plugin …]` builds them. It needs
`rustup target add wasm32-unknown-unknown` and `cargo install wasm-tools`. A plugin that
is published must be signed again (`scripts/sign.py`, with the offline key), so
rebuilding one changes its release. See `docs/plugins/v2.md`.

## Test

### Before every commit

CI (`.github/workflows/rust.yml`) runs these on Windows, macOS and Ubuntu 22.04. A red
check blocks the change.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

### Ignored tests

`cargo test` skips the tests marked `#[ignore]`. Run the ones that touch your change by
name with `-- --ignored`. The reason in each `ignore` says what the test needs:

- **network**: talks to the real release servers, GitHub or EchoVRCE.
- **`ECHOVR_TEST_*`**: works on a real game folder, library or install that you name, for
  example `ECHOVR_TEST_BIN=<game>/bin/win10`, `ECHOVR_TEST_ROOT`, `ECHOVR_TEST_LIBRARY`.
  Use a copy of the game, never one somebody plays with.
- **a local log upload service**: `logs::tests` sends a real upload to
  `ECHOVR_LOG_UPLOAD_URL`.
- **a GPU**: the UI tests in `src/ui/headless.rs` (see Screenshots below).

### Screenshots

Every UI change gets its screenshots looked at. That means the pages it touches, light and
dark where it matters, and at least one narrow window size.

```sh
ECHOVR_SNAPSHOTS=<dir> cargo test headless -- --ignored
```

This renders every page offscreen to PNGs in `<dir>`, with made-up versions and nothing
saved. It works without a desktop session. Options:

- `ECHOVR_SNAPSHOTS_ONLY=<name>`: only the shots whose name contains this.
- `ECHOVR_SNAPSHOTS_SCALE=2`: render at 2x.
- `ECHOVR_SNAPSHOTS_SIZE=1680x720`: use that window size.
- `ECHOVR_SNAPSHOTS_RAIL=1`: unfold the rail.
- `ECHOVR_SNAPSHOTS_FEED=live`: show the real news and numbers.

The run also writes `<dir>/cuts.txt`, which lists every text cut with "…" and the shot it
was cut in. A new line in it is a bug: shorten the text or make room.

`ECHOVR_SNAPSHOTS=<dir> cargo run` takes the same shots from a real window. Add
`ECHOVR_SNAPSHOTS_DEMO=1` to use the made-up versions there too.

### Debug-build switches

These only work in debug builds:

| Variable | Effect |
|---|---|
| `ECHOVR_PAGE=<title>` | Opens on that page. |
| `ECHOVR_FAKE_UPDATE=1` | Every Echo VR that gets updates looks outdated. |
| `ECHOVR_FAKE_LAUNCHER_UPDATE=0.99.0` | Pretends that launcher version is out. A version like `0.99.0-beta.1` is seen on beta and alpha. |
| `ECHOVR_UPDATE_MIRROR=<url>` | Fetches the launcher's own update from a test server. |
| `ECHOVR_UPDATE_MANIFEST=<url>` | Reads that PC update manifest instead of the live one. |
| `ECHOVR_QUEST_UPDATES=<url>` | Reads the Quest's update folders from a test server. |
| `ECHOVR_CDN=<url>` or `off` | Uses another CDN, or none. |
| `ECHOVR_NEVR_RELEASES=<url>` | Uses a mirror of nEVR's signed releases. Only releases signed with the pinned keys still pass. |
| `ECHOVR_LOG_UPLOAD_URL=<url>` | Sends Upload logs to a local service. |

New test hooks follow the same pattern: `cfg!(debug_assertions)` plus an `ECHOVR_*`
variable, with a doc comment that starts "Debug builds:".

### On a real system

Unit tests and screenshots don't show that Echo VR starts. If a change touches installs,
updates, nEVR, Proton/Steam, PLAY or the Server host, run it on a real Linux system as well:

- Give the test launcher its own state, so it doesn't touch an installed launcher:
  `XDG_DATA_HOME=<dir> XDG_CACHE_HOME=<dir>`.
- Give it its own copy of the game.
- Start the game from the shell with `EchoVR_Launcher --play`. On Linux this goes through
  Steam's Proton, so Steam has to be installed.
- The logs are in `<data>/EchoVR_Launcher/logs`. Read them; a window that opened proves
  little.

A Windows exe builds on CI and from the Mac. When a change is Windows-only and wasn't run
on Windows, say so in the commit or PR.

### Reporting tests

- Report what ran and what it showed.
- A test that hung, timed out, or couldn't reach the part you changed is not a pass. Find
  out why, or say plainly that the change was not verified there.
- Finish or report one test before starting the next.

## Code and wording

- Write like the code around it. Doc comments are full sentences in plain English and say
  what something is for and why, not what each line does. Name the player-visible
  behaviour.
- Comments in the code name the cases they handle by their real names (nEVR versions,
  Proton, the Quest, Windows, Linux), as the existing ones do.
- Text players see is short and plain. It says what happens and what to do, without
  jargon or blame.
- Two words are kept apart:
  - **Plugins** are launcher apps (the rail's Plugins page, `docs/plugins`).
  - **Mods** are Echo VR add-ons (the Mods page).
- nEVR is something Echo VR is updated to: write "update to nEVR", never "replace Echo
  VR".
- Credits and authors in plugin and mod manifests change only when the change asks for
  it.
- Signature checks (`core::signing`, `core::nevr_release`) refuse by default. Never add a
  way around them, not even a debug one.
- Crash dumps (`.dmp`), sign-in files and tokens never leave the player's machine, in
  Upload logs or anywhere else.

## Commits and pull requests

- Work on a branch from `main`, or from the channel branch the change is for. It moves
  onto `main`, `beta` or `alpha` when it is released there.
- One change per commit. The message's first line is a full sentence that says what
  changes for the player, in the same plain words as the UI. The body says why, and what
  was and wasn't tested.
- Commits carry the author's own git identity. No co-author or "generated by" trailers.
- Keep private things out of this public repository:
  - the release server's server-side code
  - host names and logins of test machines
  - keys and tokens
  - players' logs

## Channels and releases

The version in `Cargo.toml` decides a build's channel:

| Version | Channel |
|---|---|
| `X.Y.Z` | main |
| `X.Y.Z-beta.N` | beta |
| `X.Y.Z-alpha.N` | alpha |

The `main`, `beta` and `alpha` branches hold what each channel ships. A launcher updates
to the newest release on the channel it follows (`core::launcher::update_check`).

A release, done by a maintainer:

1. Commit "Version X.Y.Z: <what's new>". It changes only the version in `Cargo.toml` and
   `Cargo.lock`.
2. Build the packages with the Release workflow (Actions → Release → Run workflow). It
   makes them with `scripts/package.sh`:
   - `Echo_VR_Launcher-<v>-windows.zip` and `-windows-setup.exe`
   - `Echo_VR_Launcher-<v>-linux.zip`, `-linux.tar.gz` and `-x86_64.AppImage`
   - `SHA256SUMS`

   The launcher's self-update looks these files up by exactly these names and checks
   them against `SHA256SUMS`. Never rename them or change a zip's layout (one
   `EchoVR_Launcher/` folder).
3. Create the GitHub release `v<v>` on that commit: `--latest` for main, `--prerelease`
   for beta and alpha.
4. Put the same files on `release.echovr.de/launcher/download/`, and add their lines to the
   `SHA256SUMS` there. Launchers fetch updates from that mirror first and from GitHub
   second.

Release notes are written for players. Say what changed for them first. Leave out the
internals.
