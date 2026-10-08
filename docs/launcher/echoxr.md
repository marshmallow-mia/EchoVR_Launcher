# EchoXR in the launcher, and how it compares to RiftLift

Echo VR talks to the Oculus runtime through LibOVR (`LibOVRRT64_1.dll`). Without
Meta's runtime (on SteamVR, or on Linux), something has to answer those calls over
OpenXR. The launcher uses [marshmallow-mia/EchoXR](https://github.com/marshmallow-mia/EchoXR) for
that: the OpenXR layer of heisthecat31's EchoXR, without the hand tracking.
[RiftLift](https://github.com/Villagers654/RiftLift) solves the same problem for any Rift
game on Linux. This page covers how the launcher uses EchoXR, and what each of the two
does. It was written against EchoXR 0.3.0 (`2057ecb`) and RiftLift 0.10.2.5 (`66454d0`),
as of 2026-10, and updated for EchoXR 0.4.0 and 0.4.1 (`v0.4.1-rc1`), which the launcher
pins now.

## How the launcher uses EchoXR

The pinned `EchoXR-OpenXR-v0.4.1.zip` (EchoXR's GitHub release, SHA-256 checked, as is
each file in it) is unpacked into the game's `bin/win10`:

| File | What it is |
|---|---|
| `EchoXR.exe` | starts the game; no window, no updater |
| `EchoXR/LibOVRRT64_1.dll` | ReviveXR's LibOVR on OpenXR |
| `EchoXR/openxr_loader.dll` | Khronos' loader |

Since 0.4.0 it is built with the static C runtime, so it needs no Visual C++ runtime. It has no
`echoxr.ini` any more; the launcher deletes the one older launchers wrote (only if it's
exactly theirs).

`EchoXR.exe` does four things, then waits for the game and returns its exit code:

1. **Patched copy of the game.** If `echovr_openxr.exe` is missing, it makes it: a copy of
   `echovr.exe` with the Oculus signature check at RVA `0x1365bd0` turned into
   `mov eax,1; ret`. It only does this after checking the 14 bytes there, so only the
   final live build works.
2. **Headset event.** It holds the `OculusHMDConnected` event.
3. **Environment.** It points `LIBOVR_DLL_DIR` and `PATH` at `EchoXR\`, and (on Windows)
   `XR_RUNTIME_JSON` at SteamVR's `steamxr_win64.json`. With `--runtime active` it leaves
   the system's OpenXR runtime instead: the launcher passes that for Virtual Desktop
   through its own OpenXR runtime (VDXR, which VD's streamer registers as the system's).
   Virtual Desktop through SteamVR is the default, SteamVR. Under Proton it turns OpenXR
   on itself (through `wineopenxr`) when Proton didn't, so no OpenVR runtime is needed.
4. **Start.** It starts the copy as its direct child, passing on every argument it
   doesn't know itself. There's no job object.

Its own exit codes, which the launcher turns into a message (a dialog on Windows when the
game never showed up, `play.log` on Linux):

| Code | Meaning |
|---|---|
| 2 | `EchoXR.exe` isn't next to `echovr.exe` in `bin/win10` |
| 3 | its runtime files are missing |
| 4 | `echovr_openxr.exe` couldn't be made |
| 5 | no OpenXR runtime answered within 20 s (or, under Proton, no VR service) |
| 6 | the runtime has no headset |
| 7 | Echo couldn't be started |

Any other code is Echo's own. EchoXR logs to `EchoXR\launcher.log` (every launch) and
`EchoXR\runtime.log` (the OpenXR side); both go into the launcher's log upload.

### Things the launcher handles around EchoXR

- **Detection.** The running game is `echovr_openxr.exe`, which the game monitor counts
  as Echo VR. It's a child of `EchoXR.exe`, so the launcher knows it as the game it
  started, and STOP ends it.
- **Stale copy.** EchoXR never makes the copy again by itself. Before every start, the
  launcher deletes it when `echovr.exe` changed since (the copy is older, or another
  size), so updates and reinstalls take effect.
- **The Platform SDK (login).** `pnsovr.dll` signs in through Oculus' Platform SDK,
  `LibOVRPlatform64_1.dll`. Nothing of Meta's is used: EchoXR (0.4.2 on) brings its own,
  `EchoXR\LibOVRPlatform64_1.dll` (marshmallow-mia's stand-in: a signed-in user with a
  per-machine id, entitled, the microphone through WASAPI; no Oculus service or account).
  - On Windows it stays in `EchoXR\`. That folder is on `PATH` (and in
    `LIBOVR_DLL_DIR`) only when EchoXR starts the game, so a Meta Link start never sees it.
  - On Linux the launcher also puts it next to the game, which the game looks in first,
    for VR and for the flat start with the Oculus login.
  - Meta's loader and P2P library, which older launchers read out of Meta's runtime
    package, are removed where they put them (only those very files, by their hash).
- **Argument quoting.** EchoXR wraps each argument in quotes without escaping, so an
  extra argument containing `"`, or ending in `\`, breaks.

### Windows: SteamVR through EchoXR

Settings → Game → SteamVR, and **EchoXR** got from More mods on the Mods page
(Remove in its Plugins row goes back to Revive).
With EchoXR:

- **PLAY** first downloads EchoXR and Meta's loader when they're missing, puts EchoXR
  into the selected version's folder, then starts the game.
  - The Meta library's folder (under Program Files) needs administrator rights. The
    launcher's elevated helper does that step, only for the Meta library's Echo VR,
    from the pinned zip. It runs `EchoXR.exe --setup-only` to make the copy there.
- **PLAY** runs `bin/win10/EchoXR.exe` with the launch options.
- **Event builds** run this way too (EchoXR 0.5.0 on): `EchoXR.exe` goes into their
  `bin\win7` and patches the build its exe is, found by its PE timestamp.
- **Not supported:** the SteamVR library entry and artwork belong to Revive and aren't
  used.

### Linux

GE-Proton runs `EchoXR.exe` (`proton waitforexitandrun`) with:

- `wineopenxr` passing OpenXR on to the runtime chosen in Settings: `XR_RUNTIME_JSON` is
  SteamVR's `steamapps/common/SteamVR/steamxr_linux64.json` or WiVRn's
  `openxr/1/openxr_wivrn.json` (`/usr/share`, `/usr/local/share`, `XDG_DATA_DIRS` or the
  Flatpak), unless it is set already; no OpenVR runtime is needed since 0.4.0;
- `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`;
- no DLL override: the mod loader, nEVR runtime, is the game's `BugSplat64.dll`, which
  Wine has no builtin of;
- `ECHOXR_VR_SERVICE=ready` once the runtime's service runs: SteamVR's `vrserver`
  (started through `steam://run/250820` when it isn't, waiting up to a minute) or WiVRn's
  `wivrn/comp_ipc` socket (`wivrn-server`, or the Flatpak's, started when it isn't).
  Without a VR service EchoXR gives up after 20 s (exit code 5) instead of hanging.

Tested with a Quest 3 (2026-10-05, GE-Proton11-3, NVIDIA RTX 2080 Ti):

- **EchoXR 0.4.0 doesn't start under GE-Proton** (exit code 5 although the runtime answers):
  GE-Proton patches `wineopenxr`, whose `wineopenxr_init_registry` writes OpenXR's Vulkan
  extensions into `HKCU\Software\Wine\XR`, not `Wine\VR` where 0.4.0 looks. With that
  fixed, Echo stopped at its first swapchain: GE's `wineopenxr` (D3D12) refuses an acquire
  before the images were enumerated (`XR_ERROR_CALL_ORDER_INVALID`). **0.4.1** fixes both.
- **0.4.1 plays** on SteamVR 2.16.7 (Steam Link) and on WiVRn 26.9: session `FOCUSED`, Touch
  controllers bound, quitting from Echo's menu ends the session cleanly.
- **SteamVR 2.17.9 and 2.17.10 lose the GPU** on NVIDIA under load: the compositor reuses
  command buffers the GPU is still using (`Xid 32`, `vkerror=-4`;
  [SteamVR-for-Linux#952](https://github.com/ValveSoftware/SteamVR-for-Linux/issues/952)).
  SteamVR's "previous" branch (2.16.7) doesn't. When the compositor's log shows this during a
  start, `--play` says so in `play.log` and the launcher's window tells to choose "previous".

How a start ended: EchoXR passes Echo's own exit code on, and Echo's codes overlap with
EchoXR's (2-7), so `--play` takes a code as EchoXR's only when `EchoXR\launcher.log` has no
"Echo exited with code" for that start. A failed start (EchoXR's reason, or SteamVR losing
the GPU) is written to `linux/last-start.json`, which the launcher's window shows once.

See `src/core/linux/echoxr.rs`.

## EchoXR and RiftLift

The EchoXR column is 0.3.0, the version first compared; what 0.4.0 changed follows the
table.

| | EchoXR (EchoTools 0.3.0) | RiftLift (Villagers654 0.10.2.5) |
|---|---|---|
| **What it is** | A Windows launcher plus LibOVR runtime, made only for Echo VR. Linux runs it through Proton. | A Linux app (Python/Qt) for any Rift game, plus its own runtime. |
| **Translation layer** | ReviveXR at `ab73167` with a small patch (`xr/patches/revive-echoxr.patch`). | ReviveXR at the same base, with about 100 changed lines each in `Session.cpp` and `REV_CAPI.cpp`. |
| **How the game gets it** | No injection. The game loads `EchoXR\LibOVRRT64_1.dll` through `LIBOVR_DLL_DIR` and `PATH`. | Detours injection: `RiftLiftLauncher.exe` puts `RiftLiftOpenXR64.dll` (or its OpenVR or proxy backends) into the game. It hooks `LoadLibrary*`, `GetModuleHandle*` and `OpenEventW`, and patches imports. |
| **Oculus signature check** | Patches it out of the copy, `echovr_openxr.exe`. | Installs Meta's real, signed runtime package into the prefix, adds Meta's signing root to Wine's certificates, and binary-patches `OVRServer_x64.exe` and others. |
| **Graphics** | ReviveXR's default: D3D11 required; D3D12, Vulkan and GL optional. | Drops D3D12 and Vulkan under wineopenxr (they give duplicate `XR_KHR_vulkan_enable` names on SteamVR), adds `XR_MND_headless`, and sends D3D12-only games to its OpenVR backend. |
| **Session start** | Waits for READY only when the runtime is called `"SteamVR/OpenXR"` (5 s). FOV fallback for `echovr_openxr.exe`. | Waits for READY on every runtime (10 s), because wineopenxr renames Monado's runtime. FOV fallback for all games, an empty bootstrap frame, and StartSession errors passed on. |
| **Controllers** | Suggests both the Touch and Index bindings and lets SteamVR pick (fixes the Steam Frame). Logs the bound profile per hand. | One profile, as Revive has it, plus: a lock around actions, an action sync on every input or pose query, `isActive` checks, and an X+Y → Menu chord on the left Touch controller. |
| **Layers, formats** | ReviveXR's defaults. | Opaque eye layers, D24S8 → D32S8 (for Monado), a fix for `ovrTrackingCap_Position`, and audio GUID leak fixes. |
| **Haptics, guardian, performance stats** | ReviveXR in both: haptics through OpenXR actions, boundary from the STAGE bounds, `ovr_GetPerfStats` zeros. | Same. |
| **Hand tracking** | None any more. `ovr_GetHandPose` and the like return `ovrError_Unsupported` (ReviveXR's). | None. |
| **Platform SDK (login)** | Its own stand-in, `EchoXR\LibOVRPlatform64_1.dll` (0.4.2): a signed-in user with a per-machine id, no Meta service. | Unpacks Meta's whole runtime into the prefix's `Program Files/Oculus/Support/oculus-runtime`, sets the registry `Base`, and replaces the Impl with its own shim (faked login and entitlement). |
| **Settings, logs** | `EchoXR\echoxr.ini`; `--exe`, `--runtime steamvr\|active`, `--setup-only`; logs `EchoXR\launcher.log` and `runtime.log`. | `RIFTLIFT_*` variables (backend, trace, xrizer, offline platform…), `OXR_ZERO_TIME_IS_NOW=1`, and `WINEDLLOVERRIDES=d3d11=n;dxgi=n` with its own DXVK. |
| **Licence** | No licence file. It credits Revive (MIT), OpenXR (Apache-2.0) and Detours (MIT). | GPL-3.0-or-later. |

### What changed in EchoXR 0.4.0

From its commits and release (`v0.4.0-rc1`):

- **`EchoXR.exe`:** no window and no message boxes, the exit codes above, and under
  Proton it turns OpenXR on itself, without an OpenVR runtime.
- **Runtime:** one graphics API under Proton (D3D12 only; on Windows D3D11, plus D3D12,
  Vulkan and GL when the runtime offers them), a start-up probe for every runtime, D24S8
  falling back to D32S8, controller input kept current and serialized, and fixes for the
  microphone lookup and audio errors.
- **Build:** CMake, the static C runtime (no Visual C++ runtime), CI with tests.

### What EchoTools removed from heisthecat31's EchoXR

Commit `2057ecb`, "Keep only the OpenXR layer: remove hand tracking and the installer".
`xr/src/xr_main.cpp` and the ReviveXR patch are byte-identical in both forks, so the
LibOVR to OpenXR translation is the same. What went:

- **Hands plugin.** `plugins\EchoXRHands.dll` was a `dbgcore.dll` plugin that hooked the
  game's hand animator (`CR15HandAnimatorCS::UpdateThumbPistonAnimPoses`) to set finger
  joints.
- **Hands bridge.** `EchoXRHands.exe` was an OpenVR overlay process reading Index finger
  curls or hand skeletons. It sent them to the plugin over UDP `127.0.0.1:8768`, and to
  other players over a WebSocket relay.
- **Login-ID setting.** An experimental setting rewrote the login ID prefix in
  `pnsovr.dll`'s memory.
- **The rest.** The settings UI, the installer, and the bridge's auto-start in
  `EchoXR.exe`.
- **Updater.** It now looks at EchoTools/EchoXR, which has no releases yet. The
  launcher keeps it off anyway.

### Worth taking into EchoXR from RiftLift

RiftLift is GPL-3.0: re-implement the ideas, don't copy code. These were the gaps at
0.3.0; 0.4.0 took up the first four (as its own implementation):

1. **Graphics extensions.** Don't request the D3D12/Vulkan extensions under wineopenxr
   (duplicate extension names on SteamVR).
2. **Session start.** Wait for READY on every runtime, not only `"SteamVR/OpenXR"`;
   under wineopenxr, Monado and WiVRn never match that name.
3. **Depth format.** Map D24S8 to D32S8 for runtimes without it (Monado).
4. **Input.** Keep input current on every query, serialized.
5. **Menu button.** Still open: a menu chord for controllers without a menu button.

## Telling a client from a server

Not EchoXR, but in the same launcher area. Dedicated servers run from the same
executable, so the game monitor (`src/core/launcher/game.rs`) tells them apart the way
EchoRelay does:

- **Server:** a game process with `pnsradgameserver.dll` loaded (that only happens in
  server mode), or started with `-server` or `-headless`.
  - On Windows the module list comes from a Toolhelp snapshot; on Linux from
    `/proc/<pid>/maps`.
  - The 2018/19 lobby builds blank their `-server` flag out of their command line, so
    the module check comes first.
- **Not a server signal:** `-noovr` and `-spectatorstream`, because clients use them too
  (Flat and spectating).
- **Effect on the launcher:** servers never count as "Echo VR is running", and STOP
  never ends one. Patch, update and reinstall still wait for a server that runs from the
  same folder, because it holds those files.
