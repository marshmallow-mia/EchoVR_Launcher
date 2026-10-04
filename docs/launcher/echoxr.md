# EchoXR in the launcher, and how it compares to RiftLift

Echo VR talks to the Oculus runtime through LibOVR (`LibOVRRT64_1.dll`). Without
Meta's runtime (on SteamVR, or on Linux), something has to answer those calls over
OpenXR. The launcher uses [EchoTools/EchoXR](https://github.com/EchoTools/EchoXR) for
that: the OpenXR layer of heisthecat31's EchoXR, without the hand tracking.
[RiftLift](https://github.com/Villagers654/RiftLift) solves the same problem for any Rift
game on Linux. This page covers how the launcher uses EchoXR, and what each of the two
does. It was written against EchoXR 0.3.0 (`2057ecb`) and RiftLift 0.10.2.5 (`66454d0`),
as of 2026-10, and updated for EchoXR 0.4.0 (`v0.4.0-rc1`), which the launcher pins now.

## How the launcher uses EchoXR

The pinned `EchoXR-OpenXR-v0.4.0.zip` (EchoXR's GitHub release, SHA-256 checked, as is
each file in it) is unpacked into the game's `bin/win10`:

| File | What it is |
|---|---|
| `EchoXR.exe` | starts the game; no window, no updater |
| `EchoXR/LibOVRRT64_1.dll` | ReviveXR's LibOVR on OpenXR |
| `EchoXR/openxr_loader.dll` | Khronos' loader |

0.4.0 is built with the static C runtime, so it needs no Visual C++ runtime. It has no
`echoxr.ini` any more; the launcher deletes the one older launchers wrote (only if it's
exactly theirs).

`EchoXR.exe` does four things, then waits for the game and returns its exit code:

1. **Patched copy of the game.** If `echovr_openxr.exe` is missing, it makes it: a copy of
   `echovr.exe` with the Oculus signature check at RVA `0x1365bd0` turned into
   `mov eax,1; ret`. It only does this after checking the 14 bytes there, so only the
   final live build works.
2. **Headset event.** It holds the `OculusHMDConnected` event.
3. **Environment.** It points `LIBOVR_DLL_DIR` and `PATH` at `EchoXR\`, and (on Windows)
   `XR_RUNTIME_JSON` at SteamVR's `steamxr_win64.json`. Under Proton it turns OpenXR on
   itself (through `wineopenxr`) when Proton didn't, so no OpenVR runtime is needed.
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
- **Login DLLs.** `pnsovr.dll` logs in through Meta's Platform SDK loader,
  `LibOVRPlatform64_1.dll`. That loads the game's own `LibOVRPlatformImpl64_1.dll` (the
  community update ships it, so the launcher never touches it), which needs
  `LibOVRP2P64_1.dll`. The loader and P2P library are read out of Meta's own runtime
  package (`securecdn id=3766757683456363`, pinned by hash) and are never shipped:
  - On Linux they go next to the game, since a Wine prefix has no Meta app.
  - On Windows they're only needed without the Meta app, and then go into `EchoXR\`.
    That folder is on `PATH` only when EchoXR starts the game, so a Meta Link start
    never sees them.
- **Argument quoting.** EchoXR wraps each argument in quotes without escaping, so an
  extra argument containing `"`, or ending in `\`, breaks.

### Windows: SteamVR through EchoXR

Settings → Game → SteamVR → **SteamVR through: Revive | EchoXR**. With EchoXR:

- **SET UP** downloads EchoXR and Meta's loader, then puts EchoXR into the selected
  version's folder.
  - The Meta library's folder (under Program Files) needs administrator rights. The
    launcher's elevated helper does that step, only for the Meta library's Echo VR,
    from the pinned zip. It runs `EchoXR.exe --setup-only` to make the copy there.
- **PLAY** runs `bin/win10/EchoXR.exe` with the launch options.
- **Not supported:**
  - Event builds don't run this way: EchoXR only patches the live build. Pick Revive
    for them.
  - The SteamVR library entry and artwork belong to Revive and aren't used.

### Linux

GE-Proton runs `EchoXR.exe` (`proton waitforexitandrun`) with:

- `wineopenxr` passing OpenXR on to the system's runtime (`XR_RUNTIME_JSON`, or the
  active one); no OpenVR runtime is needed since 0.4.0;
- `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`;
- no DLL override: the mod loader, nEVR runtime, is the game's `BugSplat64.dll`, which
  Wine has no builtin of;
- `ECHOXR_VR_SERVICE=ready` when SteamVR's `vrserver`, or Monado's or WiVRn's socket, is
  there. Without a VR service EchoXR gives up after 20 s (exit code 5) instead of hanging.

GE-Proton11-3's `wineopenxr` has to offer what 0.4.0 calls to turn OpenXR on
(`wineopenxr_init_registry`); that is still to be checked on hardware.

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
| **Platform SDK (login)** | Not handled. The README asks for `LibOVRPlatform64_1.dll` and `LibOVRPlatformImpl64_1.dll` next to the game on Linux; the launcher brings them. | Unpacks Meta's whole runtime into the prefix's `Program Files/Oculus/Support/oculus-runtime`, sets the registry `Base`, and replaces the Impl with its own shim (faked login and entitlement). |
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
