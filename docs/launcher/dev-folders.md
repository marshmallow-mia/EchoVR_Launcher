# Testing mods and plugins before they're published

With a **dev code** the launcher lists your own mods and launcher plugins from your **dev
folder** on release.echovr.de, before they're in the published catalogues. Only whoever has
the code sees them. Ask the launcher's maintainers for a folder and send them an SSH public
key; you get a code and an `sftp` command.

## In the launcher

Settings → **Advanced settings** → **Dev mods and plugins**: paste the code and press **Use
code**. The card says how many mods and plugins your folder lists. Your entries appear on the
Mods page (More mods) and the Plugins page with a **Dev** tag, and the status bar shows
**DEV CODE** while a code is set. An entry with the same id as a published one replaces it
(a mod also replaces one with the same file). **Remove** goes back to the published ones;
installed dev mods then offer the published version as an update.

## Your folder

```
/                 your folder (SFTP shows it as /)
├── mods.json     your mods catalogue: overwrite it
├── plugins.json  your launcher plugins catalogue: overwrite it
└── files/        your DLLs and zips
```

Upload with any SFTP client, e.g. `sftp -P 2999 dev-<name>@168.119.2.94`, then
`put NvrMyMod.dll files/` and `put mods.json`. You can't make links or anything outside
`files/`, and there is no shell.

The catalogues are the published ones' format ([mods.md](mods.md),
[plugins.json](plugins.json)). A `url` is relative to your folder (`files/NvrMyMod.dll`), or
an absolute URL inside it; anything else is left out. Every download is checked against its
`sha256` (`shasum -a 256 file` / `sha256sum file`), so update it with each upload.

```json
{
  "schema": 1,
  "mods": [
    {
      "id": "my-mod",
      "name": "My Mod",
      "summary": "What it does, in one or two sentences.",
      "author": "you",
      "version": "0.1.0",
      "file": "NvrMyMod.dll",
      "url": "files/NvrMyMod.dll",
      "sha256": "<64 hex characters>",
      "api": 5,
      "capabilities": ["cosmetic"]
    }
  ]
}
```

```json
{
  "schema": 1,
  "plugins": [
    {
      "id": "my-plugin",
      "name": "My Plugin",
      "summary": "What it adds to the launcher.",
      "author": "you",
      "version": "0.1.0",
      "url": "files/my-plugin-0.1.0.zip",
      "sha256": "<64 hex characters>"
    }
  ]
}
```

A new version: upload the file under a new name (or overwrite it), change `version` and
`sha256`, upload the catalogue. The launcher reads your catalogues when it starts and when
you press **Use code**; your mods catalogue also with every update check (every 15 minutes),
which then offers the new version as an update. Keep the code to yourself: anyone with it can
download what's in your folder.
