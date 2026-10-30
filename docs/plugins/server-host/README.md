# Server host

The Server host plugin runs Echo VR game servers on your PC with nEVR. Each server signs in
to EchoVRCE (from nEVR 6.9.0 on by a code it shows, approved once), registers with
EchoVRCE, hosts matches and starts again after every match. The servers keep running when the launcher is closed.

It runs on Windows and on Linux. On Linux, the servers run through the launcher's GE-Proton.

It is a schema 2 plugin ([../v2.md](../v2.md)). Its source is `plugins/server-host`. It
uses the launcher's `server-host` service, which only a plugin signed by the launcher's
makers may use. It needs launcher 0.12.4 or newer. Launchers before 0.12.4 keep the older
Server host from the v1 catalogue.

The launcher does the work: the supervisor, the servers' folders, sign-ins, passwords,
the webhook and the firewall rule. The plugin sees none of the secrets. A password or
webhook typed into its page stays in the launcher. The plugin draws the page and asks for
operations (`src/core/plugin_host/services.rs` and `src/ui/launcher/plugin_services.rs`).

## The page

- **Servers.** Each server shows:
  - its state: Ready, In a match, Starting, Approve its sign-in, Waiting to restart or
    Stopped on an error;
  - the address EchoVRCE registered it at;
  - its port, regions and nEVR version;
  - how many matches and restarts it has had, and how long it has been up.

  Its buttons are Start/Stop, Restart, and three small ones: its console (i), Edit (the
  gear) and Delete (the bin). Stop and Restart ask
  first when the server is in a match: after the match, or now. **Add** adds servers with the
  defaults on the next free ports. **Start all** and **Stop all** do every server.
- **Console.** The live log of one server: nEVR's log lines and the server host's own (`[host]`).
  - **Follow** keeps the newest lines in view.
  - **Noise** also shows nEVR's high-volume diagnostics.
  - The **filter** keeps lines that contain its text.
  - **Copy** copies the lines shown. The folder button opens the server's folder.
- **Doctor.** Checks:
  - the game and its nEVR;
  - Proton, on Linux;
  - the account;
  - free disk space;
  - the servers' ports;
  - this PC's public address;
  - on Windows, the firewall and Windows Security.

  A check that fails has a button that fixes it, or leads you to the fix.
- **Account.** How the servers sign in:
  - **No account (nEVR 6.9.0 on, the default):** each server shows a code the first time it
    starts. Its row says **Approve its sign-in** and gets an **Approve** button, which opens
    echovrce.com with the code: sign in there as the account the server should play as. A
    code runs out after 5 minutes and the server shows a new one; until then it doesn't
    count as hanging. nEVR keeps the sign-in in the server's own nEVR root, so later starts
    ask no code. **Sign out** forgets those sign-ins: each shows a new code at its next
    start. The launcher reads the code from the game's error output (nEVR's log has it
    masked). Older nEVR versions need one of the ways below.
  - **Use my EchoVRCE account:** links the servers to the account the launcher is signed in
    with. No password is stored.
  - **Link another:** approve a code on echovrce.com while signed in as another account.
  - **Password:** the account's Discord ID and password. The password is kept in the Windows
    Credential Manager, or in a file only you can read. It reaches the game only through the
    environment when the game starts, never through a file.

  **Test** signs in and lists the guilds the account may host for. **On EchoVRCE** lists the
  servers EchoVRCE has registered for the account.

  A linked account must have no password on EchoVRCE, or EchoVRCE refuses the servers'
  sign-in. `/reset-password` in the Echo VR Lounge Discord clears it.
- **In the background:**
  - whether the server host runs;
  - **Start the servers with this PC**;
  - desktop alerts;
  - an optional Discord webhook for the same alerts. Alerts are sent when a server stops on
    an error, keeps restarting, is down or comes back.
- **Settings** (the gear button):
  - the game version the servers run;
  - what new servers start with (regions, guilds, tick rate);
  - the services (EchoVRCE's, or another Nakama's);
  - the publisher lock;
  - an optional bind address and public address (`network.internal_ip`, `network.external_ip`).

## What a server needs

- **The live build with nEVR**, as the launcher installs and updates it. Event builds can't
  host servers.
- **An account with the server host role** in each guild it should host for. Without that
  role, a server hosts public matches only.
- **Its port forwarded.** Forward each server's port, UDP and TCP, to this PC with the same
  number outside. EchoVRCE pings that port when the server registers. The first port is
  6792. Each server also uses its port + 1000 on 127.0.0.1 for the game's HTTP API.
- **On Windows,** the firewall has to let the ports in. Doctor's **Let them in** adds a rule
  for exactly those ports and asks for administrator rights.

## How it runs

- `EchoVR_Launcher --server-host` runs the servers. The page starts it when a server
  starts. It ends 10 seconds after no server is left to run.
- Each server has a folder in the launcher's data folder: `server-host/servers/<name>/`.
  It holds:
  - `root/`: the server's own game folder.
    - `bin/win10` is a copy of the game's files and plugins.
    - Everything else links to the installed game.
    - `_local` has the server's `config.yaml`, `config.json` and sign-in.
  - `status.json`, `console.log` and `control`.

  The installed game itself never changes. A game or mod update never meets a file a server
  holds. A server picks up a new nEVR at its next start.
- nEVR ignores `-port`, so each server has its own `netconfig_dedicatedserver.json` with
  its port.
- nEVR ends a server after every match. The server host starts it again after 5 seconds.
  - After an error it waits 5 seconds, then doubles the wait up to 5 minutes.
  - A refused sign-in or wrong settings wait 10 minutes, or until something changes.
  - A server that doesn't register within 180 seconds is stopped. That happens when
    ServerDB is unreachable or the port can't be reached.
- **Stopping a server** sends it Ctrl+C. nEVR unregisters it and then hangs (nevr-runtime
  #723), so the game is ended 3 seconds after its "teardown complete" line.
  - **Linux:** the game runs in its own process group and its own Wine prefix
    (`linux/servers/host-<name>`). That prefix's wineserver is ended too.
  - **Windows:** the game runs in a Job Object. The console window nEVR opens is hidden, and
    Ctrl+C reaches the game through it.

This follows Echo-VR-Server-on-Docker (evr-docker and evr-supervisor), Mia's servers on
Linux.
