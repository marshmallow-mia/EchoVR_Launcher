# Echo VR Launcher

Installs, updates and starts Echo VR on PC (Windows, Linux) and on the Quest, and plays it
on the community's servers (EchoVRCE): PLAY on the Play page, with what's going on and
your friends to join on the right; the live servers, your match and invites on the
Servers page; mods on the Mods page.

Download it from the [releases](https://github.com/marshmallow-mia/EchoVR_Launcher/releases).
Help and news are on the [Echo VR Lounge Discord](https://discord.com/invite/echo-vr-lounge).

## Match links

The launcher joins matches from `spark://` and `https://echo.taxi/spark://…` links: paste one
into **Join** on the Play page or **Join from link** on the Servers page. On Windows and Linux
it also opens links clicked in Discord or the browser, if no other app (Spark) handles them
yet. **Open spark:// links** in Settings takes them over from Spark, or turns this off.
macOS can't pass links to the launcher, so paste them there.


## Already have Echo VR?

INSTALL looks for a copy first (the Meta app's libraries, `C:\EchoVR`, the launcher's own
library folder, and Wine prefixes on Linux) and offers **Use the copy on this PC**, or
**Choose echovr.exe** for one somewhere else. That copy is checked against the build's
checksums and added instead of downloading it again.

## SteamVR on Windows

The SteamVR choice runs Echo VR through [Revive](https://github.com/LibreVR/Revive)
(installed by the launcher) or, picked under Settings → Game, through
[EchoXR](https://github.com/EchoTools/EchoXR)'s OpenXR layer in the game's folder: no
injection and no administrator rights (the Meta library's copy excepted). EchoXR runs
only the live build. See [docs/launcher/echoxr.md](docs/launcher/echoxr.md).

## Linux

The PC version plays on Linux through Steam. **SET UP** on the Play page downloads a
private GE-Proton and [EchoXR](https://github.com/EchoTools/EchoXR)'s OpenXR layer (it
answers Echo's Oculus calls over OpenXR, so SteamVR, Monado or WiVRn drive the headset),
reads Meta's Platform SDK loader and P2P library out of Meta's own runtime package, and
adds Echo VR to Steam as a non-Steam game (Steam restarts for that). PLAY then starts it
through Steam.
It needs an active OpenXR runtime (SteamVR, Monado or WiVRn) and its service running; no
OpenVR runtime is needed. Only the live build runs this way; the event builds don't yet.

## Mods

The **Mods** page shows the selected PC version's mod loader
([nEVR runtime](https://github.com/EchoTools/nevr-runtime), the game's `BugSplat64.dll`,
which also brings Discord sign-in, friends and parties into the game) and what it loaded
at the last start, lets you turn plugins and asset patches on or off, set a plugin's
arguments, start without mods, and install mods from the catalogue on release.echovr.de or a
DLL of your own. The launcher keeps its choices in its own files and writes nEVR's
`_local/config.yaml` from them before every start, so updates and Verify never undo them.
Signed in with EchoVRCE, the launcher also signs the game in, so it doesn't ask in the
browser. See
[docs/launcher/mods.md](docs/launcher/mods.md).

## Building from source

The launcher is written in Rust (GUI: [egui](https://github.com/emilk/egui)). With a stable
[Rust toolchain](https://rustup.rs):

```sh
cargo run              # debug build
cargo build --release  # target/release/EchoVR_Launcher(.exe)
cargo test             # unit tests (add `-- --ignored` for the network tests)
```

The bundled `adb` lives in `assets/platform-tools/`; `scripts/fetch-platform-tools.sh <version>`
refreshes it from Google's platform-tools release. Logs are written to the per-user data
directory (`%LOCALAPPDATA%\EchoVR_Launcher\logs` on Windows,
`~/Library/Application Support/EchoVR_Launcher/logs` on macOS,
`~/.local/share/EchoVR_Launcher/logs` on Linux). A folder from the launcher's days as the
Echo VR Installer (`EchoVR_Installer`) is moved over at the first start.

`ECHOVR_SNAPSHOTS=<dir> cargo run` renders every screen to PNGs in `<dir>` and exits, which is
handy for checking UI changes. Add `ECHOVR_SNAPSHOTS_DEMO=1` to render the launcher with
two made-up versions (nothing is saved).

## License

Copyright (C) 2024-2026 the Echo VR Launcher contributors

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the [GNU General Public License](LICENSE) for more
details.

You should have received a copy of the GNU General Public License along with
this program. If not, see <https://www.gnu.org/licenses/>.
