# Launcher plugins: their pages

A launcher plugin is an app inside the launcher. It gets its own tab in the rail's plugin
section, above the Plugins page (the puzzle piece), and its page is what its `plugin.json` describes. The launcher draws
it with its own cards, lists, fields and buttons. No plugin code runs in the launcher: a
page fetches JSON over HTTP, shows it, and its buttons run the steps it lists.

Mods are different: they are Echo VR add-ons, and nEVR loads them into the game
([../launcher/mods.md](../launcher/mods.md)).

The example is the Event lobbies plugin: [event-lobbies/plugin.json](event-lobbies/plugin.json).
The launcher's code is `src/core/launcher/plugin_page.rs` (the format),
`src/core/launcher/plugins.rs` (installing) and `src/ui/launcher/plugins.rs` (drawing).

## Where plugins are

- **The folder:** each plugin is a folder `plugins/<id>/` in the launcher's data folder, holding its
  `plugin.json` and its settings. The data folder is `%LOCALAPPDATA%\EchoVR_Launcher` on Windows and
  `~/.local/share/EchoVR_Launcher` on Linux.
- **Its tab in the rail:**
  - A new plugin's tab goes last in the rail's plugin section.
  - **Unpin** on the Plugins page takes it off the rail, and its page opens from there; **Pin**
    puts it back. A right click on the tab unpins or moves it (Move up, Move down).
  - The order and the unpinned plugins are kept in `launcher.json` (`rail_order`,
    `rail_unpinned`).
  - With more tabs, the gaps above them get tighter. The ones that still don't fit go under **⋯**
    (a taller window shows more).
- **Getting one:** the Plugins page (the rail's puzzle piece) gets plugins from the catalogue,
  `https://release.echovr.de/launcher/plugins.json`. The launcher's built-in draft is
  [../launcher/plugins.json](../launcher/plugins.json).
- **A catalogue entry:** it names a zip with `plugin.json` at its root (or in one top folder) and its
  `sha256`, like a mod: `id`, `name`, `summary`, `author`, `version`, `url`, `sha256`, `homepage`.
  - A different `version` puts **Update** on the plugin.
  - An update keeps the plugin's settings files.
  - **When the catalogue is read:** at the start, every 30 seconds while the launcher is open
    (every 15 minutes from the tray while it is closed) and with **Refresh** on the Plugins page.
  - **What gets announced** (status bar and desktop, once each): an installed plugin's update, and a
    plugin the catalogue (or the dev folder's) lists for the first time. The first check after
    0.11.11-beta.2 only records what is listed.
- **Writing one:** put its folder into `plugins/` by hand. The folder's name must be its `id`. It is
  listed as "added by hand" and read again when the launcher starts. In debug builds,
  `ECHOVR_PAGE=<id>` opens its tab at start.

What a description gets wrong is left out and logged. The Plugins page says how many problems
there were.

## plugin.json

```json
{
  "schema": 1,
  "id": "event-lobbies",
  "name": "Event lobbies",
  "version": "0.1.0",
  "author": "…",
  "summary": "One line on what it does.",
  "icon": "calendar",
  "settings": { … },
  "data": { "<name>": { … } },
  "actions": { "<name>": [ … ] },
  "page": { "columns": [ [ <card>, … ], [ <card>, … ] ] }
}
```

- **`id`:** lowercase letters, digits, `.`, `-`, `_`. **`name`** is the tab's name.
- **`icon`:** the tab's icon: `plugin`, `calendar`, `globe` or `controller`.
- **`settings`:** what the plugin keeps, in the format of mods' settings
  ([settings.md](settings.md)): keys, types, defaults, checks.
  - The values are kept in `settings.json` in the plugin's folder, unless the description names another
    file.
  - A page edits them with fields bound to `settings.<key>`.
- **`page.columns`:** one column fills the page; two put the second into the right-hand panel. A
  column scrolls when its cards are taller than the page.

## Text: templates

Every text in a page is a template. `{path}` is filled from the page's context:

| Path | What it is |
|---|---|
| `settings.<key>` | the plugin's settings (text) |
| `page.<key>` | values the page sets while the launcher runs (fields, choices, `save`, `set`) |
| `data.<name>` | what data source `<name>` fetched (its JSON). When it failed: `{"error": "…", "failed": "…"}`, `failed` saying why: `unreachable` (no connection, or no answer in time), `missing` (the server doesn't have that address: 404, 405, 501) or `server` (another error answer). The next fetch that works replaces it |
| `launcher.relay.server`, `.name`, `.password` | the classic lobbies server and account from the launcher's settings |
| `launcher.event_builds` | the installed event builds: `[{"id": "halloween", "version": "pc-halloween-2018", "name": "Halloween 2018"}]` |
| `launcher.server.address`, `.name`, `.kind`, `.discord_id`, `.display_name`, `.plugin` | the server the live build plays on instead of EchoVRCE, as a plugin's `server` step set it (empty: EchoVRCE; `kind`: `nakama` or `echorelay`), and that plugin's id. Its key and password aren't shown. Launchers before 0.11.11-beta.1 have no `launcher.server` |
| `launcher.features.server`, `.echorelay`, `.host` | `true` when this launcher has the `server` step, its EchoRelay kind (0.11.12 on) and the `host` step (0.11.15 on): a plugin can say when it needs a newer launcher |
| `launcher.features.form`, `.presets`, `.echoxr_settings` | `true` when this launcher draws the `form` and `presets` blocks and keeps an `echoxr` store beside EchoXR (0.11.16 on) |
| `item` | inside a list: its row |

Template syntax:
- **Path segments:** objects by key, lists by index (`data.regions.regions.0`).
- **Fallbacks:** `a|b` takes the first that is set and not empty, so `{settings.server|launcher.relay.server}` is the plugin's own setting, else the launcher's.
- **Literals:** `'text'` is a literal, as in `{page.answer.message|'No answer.'}`.
- **Braces:** `{{` and `}}` give `{` and `}`.
- **Types:** in a request's `body`, a string that is one `{path}` and nothing else keeps the value's type (number, boolean, object).

**Conditions** (`when`, `enabled`, `filter`, `require`) are templates too:
- **What holds:** empty holds; otherwise one holds when its text isn't empty, `false`, `0` or `null`.
- **Comparing:** `{a} == text` and `{a} != text` compare.
- **Negating:** a leading `!` negates.

## data

```json
"data": {
  "matches": { "get": "http://{settings.server}/api/matches", "every": 10 },
  "current": { "post": "http://…/api/matches/current", "body": { "displayname": "{settings.name}" },
               "every": 10, "when": "{settings.name}" }
}
```

- **When it's fetched:** while its tab is open, once, then every `every` seconds (at least 3), and
  only while `when` holds.
- **The answer:** parsed as JSON (text that isn't JSON is kept as a string). A POST answered with
  4xx still counts, so a server's `{"ok": false, "message": …}` reaches the page. A network
  failure or a 5xx answer sets `data.<name>` to `{"error": "…"}`.

## Cards and blocks

```json
{ "title": "Public matches", "help": "Shown under the title.", "when": "…",
  "blocks": [ … ] }
```

A card without a title and with nothing to show isn't drawn. Each block may have a `when`.

| Block | What it shows |
|---|---|
| `{"text": "…", "tone": "muted"\|"good"\|"warn"}` | a paragraph |
| `{"value": "{data.current.match.id}", "label": "Match id", "copy": true}` | a value with a label and, with `copy`, a Copy button |
| `{"field": "settings.name"\|"page.x", "label": "…", "hint": "…", "secret": true}` | an input. A `page.` field sets its value as it is typed; a `settings.` field is saved when typing ends |
| `{"choice": "page.build", "label": "…", "options": "launcher.event_builds", "option_value": "{item.id}", "option_text": "{item.name}"}` | a dropdown over a list; the first option until one is chosen |
| `{"list": "data.matches.matches", "filter": "{item.build} == {page.build}", "title": "…", "detail": "…", "button": { … }, "empty": "…"}` | a row per item, with a button on each |
| `{"button": {"label": "Join", "action": "join", "enabled": "{page.join_id}", "primary": true}}` | a button that runs an action |
| `{"form": "<section>"}` | one section of the plugin's settings as a form, as the Mods page's settings sheet draws it (sliders, switches, segmented choices…), named by its `id` or its title, its title left to the card; `"*"`: every section with its title. Changes are kept at once |
| `{"presets": true}` | the plugin's presets ([settings.md](settings.md#presets)): pick one, which preset the settings are, **Save** them under a name, **Rename** and **Delete** the player's own, **Export…** and **Import…** a preset file, and **Show advanced** for the forms |

A list's button runs its action with `item` set to its row. Buttons are off while one of the
plugin's actions runs.

## actions

An action is a list of steps, run in order. A step that fails ends it and sets `page.error`
to why (the page shows it where it likes, e.g. `{"text": "{page.error}", "tone": "warn",
"when": "{page.error}"}`). `page.error` is cleared when an action starts.

| Step | What it does |
|---|---|
| `{"post": "…", "body": { … }, "save": "page.answer"}` (or `get`) | a request; its JSON answer goes to `save`. A network failure ends the action |
| `{"require": "{page.answer.ok}", "otherwise": "{page.answer.message}"}` | ends the action unless it holds, with `otherwise` as the error |
| `{"set": "page.x"\|"settings.x", "to": "…"}` | sets a value |
| `{"play": "{page.answer.build}"}` | starts that installed version as PLAY does: an event build's classic lobbies id (`halloween`, `summer`, `winter`, `christmas`, `halloween2017`), a catalogue id or a version's id. Event builds don't start on Linux yet |
| `{"copy": "{data.current.match.id}"}` | copies the text |
| `{"refresh": ["matches", "current"]}` | fetches those data sources again now |
| `{"server": {"kind": "nakama", "address": "…", "key": "…", "discord_id": "…", "password": "…", "name": "…"}}` | the live build plays on that Nakama server instead of EchoVRCE from its next start: the launcher asks the player first, and No ends the action. `{"server": null}` (or an empty address) is EchoVRCE again, without asking. See **The game's server** |
| `{"server": {"kind": "echorelay", "address": "host:port", "display_name": "…", "password": "…"}}` | the same for an EchoRelay server, as that name and password |
| `{"host": {"folder": "server", "discord_id": "…", "password": "…", "regions": "…", "guilds": "…", "port": "6792", "socket_uri": "…", "serverdb_uri": "…", "http_uri": "…", "http_key": "…", "server_key": "…", "mods": true, "save": "page.host"}}` | writes a game server's own setup into the live build. What it wrote goes to `save`. See **A game server's setup** |

### The game's server

What a `server` step sets is the server nEVR connects to: the launcher writes it into
`config.yaml` before every start of the live build, as it does for a content pack's own
server ([../launcher/mods.md](../launcher/mods.md#a-packs-own-server): the same fields, the same
rules for the address). The game's own `_local/config.json` can't do it while nEVR runs:
nEVR sends every `ws://` and `wss://` address in it through its bridge to its own server.

- **On an EchoRelay server** the live build starts the way the event builds do: EchoLoader 2
  in nEVR's place (nEVR waits beside it as `BugSplat64.nevr.dll`), EchoRelay's patch as its
  plugin in `plugins-echorelay/`, and a `_local/config.json` naming the server and the account
  (a player's own kept as `config.json.before-echorelay`). The first PLAY sets that up
  (EchoRelay's game files, EchoLoader). Every start copies the mods that are on (what nEVR
  would load, content packs included) beside the patch, their arguments in a marked
  `echoloader.local.json`; `plugins/` itself stays nEVR's. A start on EchoVRCE or a
  Nakama server, Back to EchoVRCE, or removing the plugin puts it all back. JOIN on an
  EchoVRCE match says why it can't while the live build plays on EchoRelay.
- **Which server a start uses:** a content pack that is on and has a server of its own uses
  that one. Otherwise the start uses the server a plugin set, and otherwise EchoVRCE.
  Event builds keep the classic lobbies.
- **Asking first:** the launcher shows the plugin's name, the server and the Discord ID,
  and the player says yes or no. Setting the same server again doesn't ask.
- **Undoing it:** removing the plugin that set the server makes it EchoVRCE again. While a
  server is set, Play's line says `Server: <name>`.
- **The example:** [game-server/plugin.json](game-server/plugin.json), the Game server
  plugin.

### A game server's setup

A `host` step writes a server's own setup into the live build, in `_local/<folder>/`
(`folder`: letters, digits, `-` and `_`; `server` if empty):

- **`config.yaml`:** the server account (`discord_id`, `password`), the services it signs
  in and registers with (`socket_uri`, `serverdb_uri`, `http_uri`, `http_key`, `server_key`),
  its `regions` and `guilds` (each comma-separated), and with `mods` (the default) the
  plugins the Mods page has on, a content pack's included.
- **`config.json`:** the file the game's `-config-path` names. Its folder is where nEVR
  finds that `config.yaml`.
- **A start script:** `start-server.cmd` on Windows, `start-server.sh` on Linux. It runs
  `echovr.exe -server -headless -noovr -port <port> -config-path …`. nEVR ends a server
  after each match, and the script starts it again:
  - **On Windows** the server has a console window of its own. To stop it, close that
    window, then press S in the script's.
  - **On Linux** the script runs the server through the launcher's Proton, in a Wine
    prefix of its own (`linux/servers/<folder>` in the launcher's data), so the player's
    game can run beside it. Ctrl+C stops it.

`save` gets `folder`, `config` (the `config.json`), `script` and `command` (how to run the
script). The step ends the action when:
- the version is an event build, or has no nEVR in its slot (on an EchoRelay server, for
  example);
- the account is missing, or the port isn't digits;
- on Linux, the game was never set up there (PLAY does it).

The launcher writes that folder only in this step: PLAY and its keeper keep to the game's
own `_local/config.yaml`, and a `config.yaml` that isn't the launcher's own is never written
over. A server signs in only as its own account, with nEVR 4.1.0 on: it keeps its sign-in
beside its own `config.yaml`, never the player's. A guild it hosts for has to give that
account its server host role, or it registers for no guild. Content packs on a server need
nEVR 4.1.0 or newer.

The example: [server-host/plugin.json](server-host/plugin.json), the Server host plugin.
