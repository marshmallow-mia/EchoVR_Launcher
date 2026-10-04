# Privacy Policy: Echo VR Launcher Feed

*Last updated: October 3, 2026*

This policy explains what personal data the Echo VR Launcher Feed processes, why, and what
your rights are. The feed has three parts:

- **The Discord bot ("the Bot"):** it copies the community news messages that moderators
  pick.
- **The status service:** it turns the public EchoVRCE server status into the numbers
  shown under SERVER INFO.
- **The log upload service:** it receives the logs a player chooses to send with "Upload
  logs" in the launcher, to help them with a problem.

The first two publish files that the Echo VR Launcher shows; uploaded logs are never
published.

## Who is responsible

Mia Hentschel
Email: echo@mia-hentschel.de

## What the feed does not do

- The Bot does **not read the server's conversations.** It is not subscribed to any message
  events. It only fetches the specific announcement messages that moderators pick, by their
  message ID. It never receives, reads or stores any other message.
- Neither part posts messages, reacts, sends direct messages or changes anything in the
  Discord server.
- No names, usernames or IDs of players or authors are published, and none are used for
  advertising, profiling or tracking. No data is sold or shared. No cookies are set.

## Data processed, purposes and legal basis

### Community news (the Bot)

For the announcements that moderators pick with `/launcher news set` (at most two at a time),
the Bot reads the message text, embeds and the first image.

- **Mentions:** mentions of users, roles and channels in the text are replaced by their
  display names.
- **Not copied:** the message author's name and user IDs.
- **Where it goes:** the result is written as `news.json` and an image file to
  `https://files.echovr.de/launcher/feed/`, where it is **publicly accessible**.

Purpose: showing community news in the launcher. Legal basis: our legitimate interest in
informing the community (Art. 6(1)(f) GDPR). The announcements are already published to the
community by the server's moderators.

### Server status and player counts (the status service)

Every 30 seconds the status service reads the public EchoVRCE status API
(`https://g.echovrce.com/status/matches`). The API lists the game servers and the running
matches, including the players in them (display name, username, Discord ID, player ID).
The service uses it as follows:

- **Published:** only aggregate numbers are written to `servers.json`: servers, how busy
  they are, players online, matches per mode, and server locations by region.
- **Player IDs:** to count how many *different* players played in the last hour, 24 hours
  and 30 days, each player's ID is replaced by a keyed hash (a pseudonym). The key never
  leaves the server. Only the pseudonym and the time it was last seen are stored.
- **Everything else:** display names, usernames and Discord IDs are neither stored nor
  published. The rest of the API response is discarded after each update.

Purpose: showing server and player statistics in the launcher. Legal basis: our legitimate
interest in informing players about the state of the game servers (Art. 6(1)(f) GDPR). The
pseudonyms cannot be linked to a player by anyone without the key.

### Use of the Bot's commands

When someone uses a `/launcher` command, Discord sends the Bot what the command needs: the
user's ID and username, the server, the channel and the command's options. The Bot uses this
to carry out the command and to allow it only in the designated channel.

- **What is logged:** the time, username, command and channel, for commands that change
  which messages are shown and for attempts refused outside the designated channel.

Purpose: operating the Bot and tracing changes. Legal basis: legitimate interest in secure
and traceable operation (Art. 6(1)(f) GDPR).

### Stored configuration

The IDs of the channels and messages the Bot shows are stored on the server. They contain no
personal data.

### Log uploads

When you choose **Upload logs** in the launcher's Settings, and confirm after it has shown
what goes, the launcher sends log files from your PC: the launcher's own logs, Echo VR's
logs (from each installed version), EchoXR's logs, plugins' logs, and the Quest logs you
saved last. They can contain your computer's user name (in folder paths), where Echo VR
and the launcher are installed, your headset's model and serial number, your Echo VR
account name and the matches you joined, the versions and options you use, and error
messages.

- **Purpose:** finding the cause of a problem you asked for help with.
- **Legal basis:** your consent (Art. 6(1)(a) GDPR), given by confirming the upload. You
  can withdraw it at any time by asking for the upload to be deleted.
- **Who sees them:** only the developer. They are stored on the server, not published,
  and not shared.
- **What else is stored with them:** the time, the launcher's version, and the files'
  names, sizes and checksums. Your IP address is **not** stored with them; it is held in
  memory for one hour only, to limit uploads to 10 per hour per address (legitimate
  interest in keeping the service available, Art. 6(1)(f) GDPR).
- **Checks:** every upload is checked automatically (a virus scanner, ClamAV, and a file
  type check, Magika, both running on the server). An upload that isn't plain-text logs is
  discarded without being kept.

### Web server logs

When the launcher (or anyone) downloads the feed files, the web server at `files.echovr.de`
logs the IP address, time, requested file and the client's user agent. This is done for
operation and security, based on legitimate interest (Art. 6(1)(f) GDPR).

## Retention

| Data | Kept for |
|---|---|
| Feed files (`news.json`, `servers.json`, images) | Replaced on every update. Unselected or deleted news leaves the feed within about 5 minutes. |
| Player pseudonyms with their last-seen time | 30 days after that player was last seen |
| Uploaded logs | 30 days, or until you ask for their deletion |
| Bot, status-service and log-upload-service logs | 30 days |
| Web server logs | 14 days |
| Configuration (channel and message IDs) | Until changed |

## Recipients and hosting

- **Hosting:** the feed runs on a server hosted by Hetzner Online GmbH in Germany, who
  processes the data on our behalf.
- **Public feed:** the feed files are public, so anyone who downloads them receives their
  content.
- **Discord:** Discord processes your data on its platform under its own privacy policy:
  <https://discord.com/privacy>.
- **EchoVRCE:** the status API is operated by EchoVRCE under its own terms.

## Your rights

Under the GDPR you have the right to:

- **access** your data (Art. 15);
- **rectification** (Art. 16);
- **erasure** (Art. 17);
- **restriction** of processing (Art. 18);
- **data portability** (Art. 20);
- **object** to processing based on legitimate interest (Art. 21).

You can also lodge a complaint with a data protection supervisory authority (Art. 77).

**Removing content or a pseudonym:** to have something removed from the launcher feed, ask a
server moderator to unselect the message (`/launcher news clear`), or email
echo@mia-hentschel.de. To stop being counted, send us your EchoVRCE player ID: we delete
your pseudonym and the service skips you from then on.

## Changes

We update this policy when the feed changes. The date at the top shows the current version.
