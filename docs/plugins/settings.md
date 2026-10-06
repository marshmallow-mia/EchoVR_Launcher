# Plugin settings

A plugin can describe its settings in a small JSON file, and the launcher draws them on the
Mods page: a **Settings** button on the plugin's row opens a sheet with a control for each
setting, and up to two settings can sit on the row itself. You write no UI code, and players
need no extra program: the same sheet works on Windows and Linux.

The description says *what* can be set (type, label, help, range, choices), *how* its
control looks (a style from a fixed list, so every plugin matches the launcher), and *where*
the values go: nEVR's arguments for your plugin, or a settings file of your own.

A plugin without a description keeps the **Options** editor (free key and value fields,
see [local-plugins.md](local-plugins.md#options-args)).

## Where the launcher finds it

In this order, the first that reads wins:

1. **The catalogue entry's `settings`** (`release.echovr.de/launcher/mods.json`, see
   [mods.md](../launcher/mods.md#the-mods-catalogue)): the same object as `settings` below.
   Use it when your plugin is a single DLL from the catalogue.
2. **`<Stem>.plugin.json` beside the DLL**: `plugins/MyPlugin.dll` →
   `plugins/MyPlugin.plugin.json`. Ship it in your package, or put it there with your DLL
   while you develop (it is read again every few seconds while the Mods page is open).
3. **The launcher's own**, for a few plugins whose releases don't ship one yet
   (EchoXR Hands: [echoxr-hands.plugin.json](echoxr-hands.plugin.json)).

A description is only read: it runs nothing, and the only file it may name is a settings
file in `plugins/` (below). Mistakes are logged in the launcher's log and the faulty part is
left out; the rest still shows.

## The file

```json
{
  "$schema": "./plugin.schema.json",
  "schema": 1,
  "settings": {
    "store": { "kind": "args" },
    "row": ["enabled"],
    "sections": [
      {
        "title": "Panel",
        "help": "What the wrist panel shows after a round.",
        "fields": [
          { "key": "enabled", "type": "bool", "label": "Wrist panel", "default": true, "style": "switch" },
          { "key": "corner", "type": "choice", "label": "Corner", "default": "left", "style": "segmented",
            "choices": [ { "value": "left", "label": "Left" }, { "value": "right", "label": "Right" } ] },
          { "key": "opacity", "type": "number", "label": "Opacity", "min": 0, "max": 100, "step": 5,
            "default": 80, "unit": "%", "style": "slider" }
        ]
      }
    ]
  }
}
```

- `schema`: `1`. A newer schema shows what this launcher knows of it.
- `settings.store`: where values go (see **Stores**). Default: nEVR's arguments.
- `settings.row`: keys of up to two `bool` or `choice` settings to show on the plugin's
  row (as a checkbox, or a dropdown).
- `settings.sections`: titled groups of `fields`. A plain `"fields": [...]` instead of
  `sections` is one group without a title.
- A section has `title`, `help` (a grey line under the title), `tooltip` (shown while the
  pointer is on the title) and `fields`.

`$schema` is for your editor (point it at [plugin.schema.json](plugin.schema.json)); the
launcher ignores it.

## Fields

Every field has a `type`, and a `key` unless it is an `action`, a `note` or a `link`.
Keys are letters, digits, `.`, `-` and `_` (at most 64), unique in the file.

| Key | |
|---|---|
| `label` | Shown left of the control (the key if missing). |
| `help` | Under the control. |
| `tooltip` | Shown while the pointer is on the label or the control. Without one, `help` is. |
| `default` | Its value when none is kept, and what **Reset all** sets. |
| `advanced` | `true`: hidden until **Show advanced** is ticked. |
| `visible_if` | `{ "key": value, ... }`: shown only while those settings have those values. |
| `enabled_if` | The same, but greyed out instead of hidden. |
| `restart` | `true`: applies at the next start although the store is live (marked **Next start**); `false`: the other way round. |
| `style` | How its control looks (per type below). A style that doesn't fit the type is ignored. |
| `store` | A store of its own (as `settings.store`). |

### Types

| `type` | Kept as | `style` (first: default) | More |
|---|---|---|---|
| `bool` | `values[0]` on, `values[1]` off (default `"true"`, `"false"`) | `check`, `switch` | `values`: `["1", "0"]`, `["on", "off"]`, … |
| `text` | the text, trimmed | `field`, `area` (several lines) | `placeholder`, `min_len`, `max_len`, `format`: `url`, `hex`, `digits`, `name` (letters, digits, space, `.-_`) |
| `secret` | the text, as typed | a field showing dots | `placeholder` |
| `choice` | one `value` of `choices` | `dropdown`, `segmented`, `radio` | `choices`: `[{ "value", "label", "help", "tooltip" }]`, or plain values |
| `multi` | the chosen values, comma-separated, in the list's order | `checks`, `chips` | `choices` |
| `number` | the number as text (`0.5`, `12`) | `field`, `slider`, `stepper` | `min`, `max`, `step`, `integer`, `unit`. A slider needs `min` and `max`. |
| `color` | `#rrggbb` (`#rrggbbaa` with `alpha`) | a swatch with its field | `alpha` |
| `key` | `Ctrl+Alt+C`, `F5`, `Space` (modifiers `Ctrl`, `Alt`, `Shift`, `Win`) | a box: click it, press the key (Delete clears it) | `allow_modifiers` (default `true`) |
| `action` | — | a button | `sets`: `{ "key": value, ... }` written once when clicked; `tone`: `"danger"` for a red one; `confirm`: a question asked first |
| `note` | — | a grey paragraph | `text` |
| `link` | — | an underlined link | `url` (https only) |

Segmented buttons and chips that don't fit the sheet's width become a dropdown and checks.

**Hover texts.** A setting's `tooltip` shows while the pointer is on its label or its
control; without one, its `help` does. A choice's `tooltip` shows on its own button, radio
button, check or dropdown row; without one, its `help` (which a dropdown also shows beside
it), else the setting's. Settings shown on the plugin's row use the same texts.

Values are checked before they are kept: a number outside its range, a choice not in the
list or text in the wrong format is marked red with the reason, and nothing is written.
Numbers land on the step's grid (from `min`).

An `action` may set keys the description doesn't list (e.g. `{ "Recalibrate": "1" }`):
your plugin notices the value and does the work, and clears it again if it wants to be
told twice.

## Stores

### `args`: nEVR's arguments (the default)

```json
"store": { "kind": "args" }
```

Values go into the launcher's choices for the version (`_local/launcher-mods.json`), then
into `args:` of your entry in nEVR's `_local/config.yaml` before every start. They reach
`NvrPluginInitEx(ctx, args_json)` as strings, as every argument does
([local-plugins.md](local-plugins.md#options-args)). They apply **at the next start**. A
value equal to the catalogue's default isn't kept.

### `file`: a settings file of your own

```json
"store": { "kind": "file", "file": "MyPlugin.txt", "format": "keyvalue", "live": true }
```

Values go into `plugins/<file>`. Use this when your plugin reads its settings while the
game runs: with `"live": true` the sheet says changes apply right away.

- `file`: a plain name in `plugins/` ending in `.txt`, `.ini`, `.cfg` or `.json`. No
  folders, no DLLs, not a `.plugin.json`.
- `format`: `keyvalue` (default; `json` for a `.json` file).
  - **`keyvalue`**: `Key = value` lines. Comments start with `#` or `;` at a line's start,
    or after a value and a space (`Network = 1   # share`). `Color = #ff8800` is a value. The
    launcher changes only the value on the key's line (its comment stays), adds a missing key
    at the end, and leaves every other line as it is. Values are one line.
  - **`json`**: one flat object. Bools kept as `true`/`false` and numbers are written as JSON
    bools and numbers, everything else as strings; other keys stay.
- The file is written in place (a temporary file, then renamed), so a plugin that re-reads
  it never sees half of it.
- Ship the file with its defaults: the sheet shows a field's `default` while its key isn't
  in the file.

## Testing yours

1. Put `MyPlugin.plugin.json` beside `MyPlugin.dll` in the version's `plugins` folder.
2. Open **Mods**: the plugin's row has **Settings** instead of **Options**, and the settings
   named in `row` beside it.
3. Mistakes are in the launcher's own log, as
   `plugin settings of MyPlugin.dll (plugin.json): …`.
