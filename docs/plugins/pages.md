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
  - A new plugin's tab goes last in the rail's plugin section (small tabs, like EchoVRCE's and
    Friends').
  - The pin on its card on the Plugins page takes it off the rail, and its page opens from the card
    (**Open**); the pin puts it back. A right click on the tab unpins or moves it (Move up, Move
    down).
  - The order and the unpinned plugins are kept in `launcher.json` (`rail_order`,
    `rail_unpinned`).
  - With more tabs, the gaps above them get tighter; with more still, the plugin tabs scroll (a
    taller window shows more).
- **The Plugins page** (the rail's puzzle piece) is the library: a card for every plugin, the
  installed ones first (in the rail's order), then the catalogue's others. **All**, **Installed**
  and **Available** pick which. An installed one's card has **Open**, **Update** when there is one,
  its pin, and **⋯** (or a right click) for Update, Pin, Turn off, Remove and its homepage.
- **Turned off** (0.12.4 on): no tab, its page fetches nothing, a game server it chose is
  EchoVRCE again and EchoXR's settings it keeps go; **Turn on** brings it back. Kept in
  `launcher.json` (`plugins_off`).
- **Getting one:** the Plugins page gets plugins from the catalogue. Since 0.12.4 that is the
  signed one, `https://release.echovr.de/launcher/plugins.v2.json` (see
  [The signed catalogue](#the-signed-catalogue)); older launchers, and newer ones until they
  have seen a signed one, read `https://release.echovr.de/launcher/plugins.json`. The
  launcher's built-in draft is [../launcher/plugins.json](../launcher/plugins.json).
- **A catalogue entry:** it names a zip with `plugin.json` at its root (or in one top folder) and its
  `sha256`, like a mod: `id`, `name`, `summary`, `author`, `version`, `url`, `sha256`, `homepage`,
  and `icon` (as in `plugin.json`: the card's icon before the plugin is got).
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
- **Its files as installed** (0.12.4 on): the launcher notes the checksum of each of a plugin's
  own files when it installs one (`.installed.json`; one installed before is noted the first
  time it is read), not its settings or presets. A plugin whose files changed since shows
  **Changed since install**. Only a plugin as installed from a catalogue keeps EchoXR's
  settings beside the game (`"echoxr": true`, [settings.md](settings.md)); one added by hand
  or changed does in debug builds only.

- **What it can do, allowed by the player** (0.12.4 on): see [Permissions](#permissions).
- **Signed by its makers** (0.12.4 on): see [Signatures](#signatures).

What a description gets wrong is left out and logged. The Plugins page says how many problems
there were.

### Permissions

The launcher reads from a plugin's description what it can do, and the player allows that
before it does anything:

| What | Read from |
|---|---|
| Talk to `<server>` | a request whose URL names its server (`https://api.example.com/…`, or a `'literal'` fallback) |
| Talk to the classic lobbies server your settings name | `{launcher.relay.server}` in a request's server |
| Talk to servers you type in its settings | `{settings.…}` in a request's server |
| Use your classic lobbies password | `launcher.relay.password` anywhere |
| Change the server Echo VR plays on | a `server` step with an address |
| Set up a game server in Echo VR's folder | a `host` step |
| Change EchoXR's settings in every installed version | an `echoxr` settings store |
| Start Echo VR | a `play` step |
| Run game servers on this PC | `"builtin": "server-host"` |

- **Getting one** asks "Get <plugin>?" with that list once it is downloaded, before it is put
  in place; Cancel drops it. One that can do none of it is put in place at once.
- **An update** asks only about what it can do that the version before couldn't ("Version x
  can also: …"); Cancel keeps the version installed.
- **On its page:** a plugin added by hand, one whose files changed since it was installed, and
  one an older launcher updated show **Review and allow** instead of their page, and do
  nothing (no requests, no actions, no EchoXR settings) until the player allows them. One
  installed from the catalogue before 0.12.4 has what it could do then.
- What the player allowed is kept in `launcher.json` (`plugin_grants`), and goes with
  **Remove**.

### Signatures

A first-party plugin carries `plugin.sig`: an Ed25519 signature of its id, version and the
checksum of each of its files, made with the plugins key (not the mods index's). The launcher
checks it every time it reads the plugin; the player's settings and presets, and the
launcher's dot files, aren't part of it.

- **What it unlocks:** `"builtin": "server-host"` (Server host's page) only for a signed
  plugin, its files as signed; an unsigned copy's page says to update it. A signed plugin
  may also keep EchoXR's settings when it wasn't installed from a catalogue. Debug builds
  don't ask for a signature.
- **Signing one:** `scripts/sign.py plugin docs/plugins/<id> --zip <id>-<version>.zip` (the
  plugins key is `~/echovr-signing-key/plugins.key`, kept offline; OpenSSL 3 makes the
  signature). It writes `plugin.sig` into the folder and prints the zip's size and `sha256`
  for the catalogue entry. Change any file afterwards and sign again.
- `plugin.sig`:

  ```json
  {
    "schema": 1,
    "statement": "echovr-plugin-v1\nid controllers\nversion 0.1.3\n<sha256>  plugin.json\n",
    "signatures": [{"key": "f1258b9bc6b67b2d", "sha256": "<of the statement>", "sig": "<base64>"}]
  }
  ```

  `key` is the first 16 hex digits of the public key's SHA-256. Files are listed by path
  (`/` between folders), one `<sha256>  <path>` line each, sorted.

### The signed catalogue

`plugins.v2.json` has the same entries as `plugins.json`, and `plugins.v2.json.sig` beside it
holds its signature by the plugins key (`scripts/sign.py file plugins.v2.json`, which keeps
the signature before beside the new one).

- **Read first** since 0.12.4. Once a launcher has seen a signed one, it keeps it and never
  reads the unsigned `plugins.json` again: when the signed one can't be fetched or its
  signature is wrong, the one kept is used.
- **`blocked`:** plugin versions turned off for everyone, only in the signed catalogue:

  ```json
  "blocked": [{"id": "event-lobbies", "versions": ["0.1.2"], "reason": "It sent your password twice."}]
  ```

  No `versions`: every version. The launcher turns such a plugin off, says why once, tags its
  card **Turned off by its makers**, and doesn't let it be turned on while it is listed. An
  update to a version not listed works as usual.
- **Publishing:** keep both catalogues' entries alike (old launchers read only
  `plugins.json`), and upload `plugins.v2.json.sig` together with `plugins.v2.json`.

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
- **`page.builtin`:** a page the launcher draws itself, for one plugin only: `"server-host"` for
  the plugin `server-host` (0.12.3 on, `launcher.features.server_host`). Launchers without it
  draw `columns`, so those say to update. A built-in page named by another plugin is left out
  with a warning. See [server-host/README.md](server-host/README.md).

## Text: templates

Every text in a page is a template. `{path}` is filled from the page's context:

| Path | What it is |
|---|---|
| `settings.<key>` | the plugin's settings (text) |
| `page.<key>` | values the page sets while the launcher runs (fields, choices, `save`, `set`) |
| `data.<name>` | what data source `<name>` fetched (its JSON). When it failed: `{"error": "…", "failed": "…"}`, `failed` saying why: `unreachable` (no connection, or no answer in time), `missing` (the server doesn't have that address: 404, 405, 501), `server` (another error answer) or `refused` (the launcher didn't send it: see **Where requests go**). The next fetch that works replaces it |
| `launcher.relay.server`, `.name` | the classic lobbies server and account name from the launcher's settings |
| `launcher.relay.password` | that account's password, **only in a request's `body` to that server** (`http://` + `launcher.relay.server`) and in a `server` step to it; everywhere else it is empty (0.12.4 on; before, every page had it) |
| `launcher.event_builds` | the installed event builds: `[{"id": "halloween", "version": "pc-halloween-2018", "name": "Halloween 2018"}]` |
| `launcher.server.address`, `.name`, `.kind`, `.discord_id`, `.display_name`, `.plugin` | the server the live build plays on instead of EchoVRCE, as a plugin's `server` step set it (empty: EchoVRCE; `kind`: `nakama` or `echorelay`), and that plugin's id. Its key and password aren't shown. Launchers before 0.11.11-beta.1 have no `launcher.server` |
| `launcher.features.server`, `.echorelay`, `.host` | `true` when this launcher has the `server` step, its EchoRelay kind (0.11.12 on) and the `host` step (0.11.15 on): a plugin can say when it needs a newer launcher |
| `launcher.features.form`, `.presets`, `.echoxr_settings` | `true` when this launcher draws the `form` and `presets` blocks and keeps an `echoxr` store beside EchoXR (0.11.16 on) |
| `launcher.features.server_host` | `true` when this launcher has the Server host plugin's built-in page (0.12.3 on) |
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

### Where requests go

A data source's or a step's request (0.12.4 on):

- **Its server comes from the description, the plugin's settings or `launcher.*`:** the URL's
  scheme, host and port must be the same with `data`, `page` and `item` left empty. So
  `http://{settings.server|launcher.relay.server}/api/join/{item.id}` is sent, and
  `{data.next.url}` or `http://{page.host}/…` is refused (`failed`: `refused`): a server's
  answer can't send the plugin's requests, and what they carry, elsewhere.
- **Redirects:** followed only on the same server (scheme, host and port), at most five. A
  redirect elsewhere fails the request.
- **No cookies** are kept between requests.
- **The classic lobbies password** is filled in only in a body sent to that server (see
  `launcher.relay.password` above).

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
| `{"host": {"folder": "server", "discord_id": "…", "password": "…", "regions": "…", "guilds": "…", "port": "6792", "socket_uri": "…", "serverdb_uri": "…", "http_uri": "…", "http_key": "…", "server_key": "…", "mods": true, "save": "page.host"}}` | writes a game server's own setup into the live build once the player says yes (0.12.4 on: before, without asking). What it wrote goes to `save`. See **A game server's setup** |

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
own `_local/config.yaml` (one that isn't the launcher's is kept as `config.yaml.old`
first). A server signs in only as its own account, with nEVR 4.1.0 on: it keeps its sign-in
beside its own `config.yaml`, never the player's. A guild it hosts for has to give that
account its server host role, or it registers for no guild. Content packs on a server need
nEVR 4.1.0 or newer.

`socket_uri` is always written: EchoVRCE's (`ws://g.echovrce.com:80/spr`) when the plugin
leaves it empty, since nEVR refuses to start a server without one.

The Server host plugin itself (0.2.0 on) no longer uses this step: it has a page of its
own, [server-host/README.md](server-host/README.md).
