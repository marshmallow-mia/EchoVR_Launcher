# Writing a plugin and loading it from your PC

A plugin is a 64-bit Windows DLL that runs inside Echo VR. The mod loader, nEVR runtime
(the game's `BugSplat64.dll`), loads it when the game starts and calls the functions it
exports. On Linux the same DLL runs under Proton.

This guide covers plugins you build yourself and load from your own PC: how nEVR loads
a plugin, a minimal one, how to build it, and how to load it with the launcher.

> A plugin has the same rights as the game and as you: it can read and change anything
> the game can, and anything your user account can. Load only DLLs you built yourself or
> got from people you trust.

## 1. How nEVR loads a plugin

- **Where:** plugins live in the game's `bin\win10\plugins\` folder, next to
  `echovr.exe`. nEVR loads only the ones its config names. It never loads every DLL in
  the folder.
- **The config:** it is `ready-at-dawn-echo-arena\_local\config.yaml`, and the launcher
  writes it before every start. Each entry under `plugins:` has these fields:

  | Field | Meaning |
  |---|---|
  | `name` | Required. |
  | `file` | Defaults to `<name>.dll`. |
  | `enabled` | Defaults to `true`. |
  | `required` | |
  | `args` | Options for the plugin. |

- **Load order:** nEVR loads every listed DLL first, then calls each plugin's init.
  Plugins that declare no capabilities come first, plugins with their own detours last.
  Within one group, the config's order is kept.
- **When a plugin fails:** a missing `NvrPluginGetInfo`, or an init that returns
  non-zero, unloads that plugin, and nEVR logs why. Any other plugin still loads.

### The exports

All exports are `extern "C"` and x64. Only `NvrPluginGetInfo` is required.

| Export | Called | What it does |
|---|---|---|
| `NvrPluginInfo NvrPluginGetInfo(void)` | at load | Name, description and version. The strings must stay valid while the game runs (string literals or static storage). A NULL name fails the load. |
| `uint32_t NvrPluginGetApiVersion(void)` | at load | The `NEVR_PLUGIN_API_VERSION` you built against (5). Without it the plugin counts as version 1. |
| `uint32_t NvrPluginGetCapabilities(void)` | at load | What the plugin does, as `NEVR_PLUGIN_CAP_*` bits (see below). |
| `int NvrPluginInitEx(const NvrGameContext* ctx, const char* args_json)` | once, after every plugin is loaded | Set up here. Return 0 on success; anything else unloads the plugin. `args_json` is described under "Options". |
| `int NvrPluginInit(const NvrGameContext* ctx)` | once, if there is no `InitEx` | The same without options (an older interface). Export both for older hosts. |
| `void NvrPluginOnFrame(const NvrGameContext* ctx)` | every game frame | Runs on the game's thread: keep it short. |
| `void NvrPluginOnGameStateChange(const NvrGameContext* ctx, uint32_t old_state, uint32_t new_state)` | when the game's state changes | `ctx->net_game` is set from here on. |
| `void NvrPluginShutdown(void)` | at unload, not guaranteed | Remove your hooks and close files. Don't depend on it being called. |

`NvrGameContext` gives you these fields:

| Field | What it is |
|---|---|
| `base_addr` | `echovr.exe`'s base address. |
| `net_game` | The game's `CR15NetGame*` once available, else null. |
| `game_state` | The current state. |
| `flags` | `NEVR_HOST_HAS_NETGAME`, `IS_SERVER`, `IS_CLIENT`, `COMBAT_MODE`, `IS_HEADLESS`. |
| `ctx_size` | The struct's size as the host sees it. |
| `get_plugin_count()` / `get_plugin_info(i)` | The other loaded plugins. |

nEVR gives a plugin nothing else. There is no logging, hooking or address API. A plugin
brings what it needs itself, for example MinHook for detours.

### Capabilities

Declare honestly what your plugin does. nEVR orders plugins by these bits, and the
launcher shows them.

| Bit | Meaning |
|---|---|
| `NEVR_PLUGIN_CAP_OBSERVES_ONLY` | Reads game state, never writes it. |
| `NEVR_PLUGIN_CAP_COSMETIC` | Visuals or audio only. |
| `NEVR_PLUGIN_CAP_ALTERS_GAMEPLAY` | Physics, weapons, movement. |
| `NEVR_PLUGIN_CAP_ALTERS_RULES` | Match rules, scoring, the game mode. |
| `NEVR_PLUGIN_CAP_NETWORK` | Opens sockets or talks to a service. |
| `NEVR_PLUGIN_CAP_HOOKS_ENGINE` | Installs its own detours. Your MinHook doesn't share the host's hook table. Never hook a function nEVR hooks itself: nEVR checks after every init, and on a server that check ends the game. |

### Options (`args`)

The options a plugin gets come from the `args:` of its config entry, which the launcher's
**Options** editor writes. They arrive in `NvrPluginInitEx` as a flat JSON object whose
values are all strings. Nested keys are joined with dots, and lists with commas:

```json
{"logging":"verbose","limits.max":"5"}
```

With no options the string is `"{}"`; it is never null. A plugin that exports only
`NvrPluginInit` doesn't get them. The launcher doesn't accept `${...}` in an option,
because nEVR would read it as an environment variable.

### Threads and DllMain

- Every callback runs on the game's thread. Do slow work (files, network) on a thread of
  your own, started in `InitEx`.
- In `DllMain`, call only `DisableThreadLibraryCalls`. Do the real setup in `InitEx`.

### Logging

nEVR logs the load and init of every plugin, as `[NEVR.PLUGIN]` lines, to:

- `%LOCALAPPDATA%\EchoVR\logs\nevr-<time>.jsonl`
- `bin\win10\logs\nevr-boot.jsonl`

The launcher reads these after each start and shows a state per plugin:

| State | Meaning |
|---|---|
| Loaded | Loaded and initialised. |
| Failed | The load failed, with nEVR's reason. |
| Skipped | Skipped. |
| Next start | Not started yet since it was added. |

For your own log, follow what the other plugins do: write it to
`bin\win10\plugin_logs\<your name>\<your name>.log`. The **Logs** button on the Mods page
opens that folder, and **Upload logs** in the launcher's Settings sends it along.

## 2. A minimal plugin

`hello.cpp`. Take `nevr_plugin_interface.h` (API version 5) from the nEVR runtime
(`src/extension/plugin_interface.h`) or from one of the plugins that use it.

```cpp
#include <windows.h>
#include <cstdio>
#include "nevr_plugin_interface.h"

static FILE* g_log = nullptr;

extern "C" {

NvrPluginInfo NvrPluginGetInfo(void) {
    NvrPluginInfo info = {};
    info.name = "hello";
    info.description = "Says hello in its log";
    info.version_major = 1;
    return info;
}

uint32_t NvrPluginGetApiVersion(void) { return NEVR_PLUGIN_API_VERSION; }

uint32_t NvrPluginGetCapabilities(void) { return NEVR_PLUGIN_CAP_OBSERVES_ONLY; }

int NvrPluginInitEx(const NvrGameContext* ctx, const char* args_json) {
    CreateDirectoryA("plugin_logs", nullptr);
    CreateDirectoryA("plugin_logs\\hello", nullptr);
    g_log = fopen("plugin_logs\\hello\\hello.log", "a");
    if (g_log) {
        fprintf(g_log, "hello: game at %p, options %s\n", (void*)ctx->base_addr, args_json);
        fflush(g_log);
    }
    return 0;  // anything else: nEVR unloads the plugin
}

int NvrPluginInit(const NvrGameContext* ctx) { return NvrPluginInitEx(ctx, "{}"); }

void NvrPluginOnGameStateChange(const NvrGameContext*, uint32_t from, uint32_t to) {
    if (g_log) { fprintf(g_log, "state %u -> %u\n", from, to); fflush(g_log); }
}

void NvrPluginShutdown(void) {
    if (g_log) { fclose(g_log); g_log = nullptr; }
}

}  // extern "C"

BOOL WINAPI DllMain(HINSTANCE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) DisableThreadLibraryCalls(module);
    return TRUE;
}
```

The game's working folder is `bin\win10`, so relative paths start there.

`hello.def` lists the exports, so nothing else leaks out of the DLL:

```
LIBRARY hello
EXPORTS
    NvrPluginGetInfo
    NvrPluginGetApiVersion
    NvrPluginGetCapabilities
    NvrPluginInitEx
    NvrPluginInit
    NvrPluginOnGameStateChange
    NvrPluginShutdown
```

## 3. Building it

The plugin has to be a 64-bit DLL that needs nothing beside it. nEVR loads plugins with
a restricted DLL search path, so a DLL that needs `libstdc++-6.dll` or the Visual C++
runtime from somewhere else fails to load. Link everything statically.

### With MinGW-w64 (from Linux, macOS or Windows)

This is how the asset patches and the XMLHTTP fix are built. Use CMake and Ninja with
the toolchain file `cmake/mingw-toolchain.cmake`:

```cmake
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_SHARED_LINKER_FLAGS_INIT "-static -static-libgcc -static-libstdc++ -s")
```

The `CMakeLists.txt`:

```cmake
cmake_minimum_required(VERSION 3.20)
project(hello LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 17)
add_library(hello SHARED hello.cpp hello.def)
set_target_properties(hello PROPERTIES OUTPUT_NAME "hello" PREFIX "")
```

Build it:

```sh
cmake -S . -B build -G Ninja -DCMAKE_TOOLCHAIN_FILE=cmake/mingw-toolchain.cmake
cmake --build build      # build/hello.dll
```

For detours, fetch MinHook (v1.3.4) and link it statically, as `NvrXmlHttpFix` does in
its `CMakeLists.txt`.

### With MSVC or clang-cl

Build an x64 DLL with the static runtime (`/MT`, in CMake
`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`). Export with the same `.def` file.

### Check before you load it

The launcher doesn't look inside a DLL when you add it. If something is wrong, you only
see it after a start, as **Failed** with nEVR's reason. Check beforehand that the exports
are there and that the DLL needs nothing beyond Windows' own DLLs:

```sh
x86_64-w64-mingw32-objdump -p build/hello.dll | grep -E "DLL Name|Nvr"
```

## 4. Loading it with the launcher

The launcher loads only verified plugins: those from the community update and those
from its plugin catalogue. A plugin from your PC isn't verified, so its loading is off
until you turn it on.

1. **Turn on local plugins for the version you test with.** Open that version's loader
   config, `<install folder>\ready-at-dawn-echo-arena\_local\config.yaml`. Find the
   install folder with **Install → Manage → Open folder**; the file is there after the
   version's first start with mods. Add this line at the top, without spaces in front:

   ```yaml
   x-local-plugins: true
   ```

   The launcher rewrites this file before every start but keeps that line, and nEVR
   ignores it. It applies to that version only. To turn it off again, delete the line or
   set it to `false`.

2. **Add the DLL:** **Mods → Plugins → Add DLL**. Confirm, then pick your DLL. The
   launcher copies it into the version's `plugins` folder and notes its checksum. The
   plugin is listed with the tag **Your DLL** and is on.

3. **Options:** if your plugin reads options, set them with **Options** in its row. They
   reach `NvrPluginInitEx` as described above.

4. **Start the game with PLAY**, then look at the Mods page: your plugin's state
   (Loaded, Failed with the reason) is there. **Logs** opens the plugins' log folder.

5. **After rebuilding:** add the new DLL again with **Add DLL**. It replaces the old one,
   with its new checksum. Copying it over the old file by hand doesn't work: the launcher
   sees that the file no longer has the checksum it noted and leaves it out at the next
   start.

6. **Removing it:** **Remove** in its row deletes the file from the plugins folder.

### What the launcher does with unverified plugins

| | Local plugins off (the default) | `x-local-plugins: true` |
|---|---|---|
| **Add DLL** | Greyed out | Adds the DLL |
| A plugin from your PC (**Your DLL**) | Listed as **Not loaded**, not in nEVR's config | Loads, if its file still has the checksum it had when added |
| A DLL put into `plugins` by hand | Listed as **Unverified**, **Not loaded** | Loads |
| Plugins from the community update and the catalogue | Load | Load |

**Start without mods** on the Mods page turns off everything except what the game needs,
your plugins included.

## 5. When it doesn't load

| What you see | Why, and what to do |
|---|---|
| Add DLL is greyed out | Local plugins are off for this version: see step 1. |
| **Not loaded** | The plugin isn't verified and local plugins are off: see step 1. |
| **Next start** | The game hasn't started since the plugin was added. |
| **Failed**, "missing NvrPluginGetInfo export" | The export isn't there or has a C++ name: use `extern "C"` and the `.def` file. |
| **Failed**, the DLL can't be loaded | It needs a DLL that isn't there (the C++ runtime): link statically. Or it's 32-bit: build x64. |
| **Failed**, init returned non-zero | Your `NvrPluginInitEx` returned something other than 0: look in your own log. |
| Left out, "changed since it was added" (launcher log) | The file was replaced by hand: add it again with **Add DLL**. |
| No plugin loads at all | The loader config can't be read (for example, an unset `${...}` variable) and nEVR ignores the whole file: let the launcher write it again by starting from the launcher. |
