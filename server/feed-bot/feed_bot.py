#!/usr/bin/env python3
"""Echo VR launcher feed: Community News.

Mirrors the Community News messages that moderators pick from the Echo VR Discord into
static files the launcher reads from https://files.echovr.de/launcher/feed/:

  news.json  one configurable message per slot ("main" feeds the banner and the first card,
             "community" the second card), plus each message's first image as
             news-<slot>.<hash>.jpg

SERVER INFO (servers.json) comes from status_feed.py, which needs no Discord at all.

Which messages are shown is set in Discord with /launcher news set (Manage Server), and
every command only works in the control channel.

Privacy by design: the bot subscribes to no message events. It only fetches the configured
messages by their ID (which is what the Message Content intent is needed for), so it never
sees any other message. See PRIVACY.md.

  python feed_bot.py           run the bot
  python feed_bot.py --dump    print the configured news messages as JSON, then exit
  python feed_bot.py --once    export once, then exit
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import io
import json
import logging
import logging.handlers
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import aiohttp
import discord
from discord import app_commands
from discord.ext import tasks
from PIL import Image

HERE = Path(__file__).resolve().parent
CONFIG_PATH = HERE / "config.json"
CREDS_PATH = Path(os.environ.get("FEED_CREDS", HERE / ".bot.creds"))
OUT_DIR = Path(os.environ.get("FEED_OUT", "/var/www/EchoClientHosting/launcher/feed"))
LOG_PATH = Path(os.environ.get("FEED_LOG", "/root/log/launcher_feed.log"))

DEFAULT_CONFIG: dict[str, Any] = {
    "guild": 779349159852769310,
    # The only channel the bot's commands work in.
    "control_channel": 779349591438524457,
    # Where a bare message ID given to /launcher news set is looked up.
    "news_channel": 779435355086258186,
    "news": {
        "main": {"channel": 779435355086258186, "message": 1543000182226952344},
        "community": None,
    },
}
SLOTS = ("main", "community")
NEWS_EVERY_S = 300
LOG_DAYS = 30
# The rights the bot needs: see channels and read their history. Anything more is reported
# at startup (slash-command replies need no permission).
NEEDED = {"view_channel", "read_message_history"}
NEWS_MAX_W = 1900

log = logging.getLogger("feed")

MESSAGE_LINK = re.compile(r"discord(?:app)?\.com/channels/(?:\d+|@me)/(\d+)/(\d+)")
MASKED_LINK = re.compile(r"\[([^\]\n]+)\]\(<?(https?://[^)\s>]+)>?\)")
BARE_URL = re.compile(r"https?://[^\s<>()]+")
IMAGE_EXT = (".png", ".jpg", ".jpeg", ".gif", ".webp")


# ---- config and credentials ----


def load_token() -> str:
    """The bot token from .bot.creds: JSON with a *token* key, KEY=value lines (the first
    key containing TOKEN), or the bare token on a line of its own. Never logged."""
    text = CREDS_PATH.read_text(encoding="utf-8").strip()
    try:
        data = json.loads(text)
        if isinstance(data, dict):
            for k, v in data.items():
                if "token" in str(k).lower() and isinstance(v, str) and v.strip():
                    return v.strip()
    except ValueError:
        pass
    lines = [l.strip() for l in text.splitlines() if l.strip() and not l.lstrip().startswith("#")]
    for line in lines:
        m = re.match(r"^(?:export\s+)?([A-Za-z0-9_.\- ]+?)\s*[=:]\s*(.+)$", line)
        if m and "TOKEN" in m.group(1).upper():
            return m.group(2).strip().strip("\"'")
    for line in lines:
        if re.fullmatch(r"[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]{4,}\.[A-Za-z0-9_\-]{20,}", line):
            return line
    raise SystemExit(f"no bot token found in {CREDS_PATH}")


def load_config() -> dict[str, Any]:
    try:
        cfg = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    except FileNotFoundError:
        cfg = json.loads(json.dumps(DEFAULT_CONFIG))
        save_config(cfg)
    for key in ("control_channel", "news_channel"):
        cfg.setdefault(key, DEFAULT_CONFIG[key])
    cfg.setdefault("news", {})
    for slot in SLOTS:
        cfg["news"].setdefault(slot, None)
    return cfg


def save_config(cfg: dict[str, Any]) -> None:
    write_atomic(CONFIG_PATH, (json.dumps(cfg, indent=2) + "\n").encode(), mode=0o600)


def write_atomic(path: Path, data: bytes, mode: int = 0o644) -> None:
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_bytes(data)
    os.chmod(tmp, mode)
    os.replace(tmp, path)


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def iso(dt: datetime | None) -> str | None:
    return dt.astimezone(timezone.utc).isoformat(timespec="seconds") if dt else None


# ---- text ----


def resolve(text: str, msg: discord.Message) -> str:
    """Mentions and custom emoji as plain text (the launcher can't look them up).
    Timestamps (<t:...>) stay: the launcher shows them in the viewer's local time."""
    if not text:
        return ""
    users = {str(u.id): getattr(u, "display_name", u.name) for u in msg.mentions}
    roles = {str(r.id): r.name for r in msg.role_mentions}
    guild = msg.guild

    def channel_name(cid: str) -> str:
        ch = guild.get_channel_or_thread(int(cid)) if guild else None
        return f"#{ch.name}" if ch else "#channel"

    text = re.sub(r"<@!?(\d+)>", lambda m: "@" + users.get(m.group(1), "user"), text)
    text = re.sub(r"<@&(\d+)>", lambda m: "@" + roles.get(m.group(1), "role"), text)
    text = re.sub(r"<#(\d+)>", lambda m: channel_name(m.group(1)), text)
    text = re.sub(r"<a?:(\w+):\d+>", r":\1:", text)
    text = re.sub(r"</([\w -]+):\d+>", r"/\1", text)
    return text


def strip_markdown(line: str) -> str:
    line = re.sub(r"^\s*(?:#{1,3}\s+|>\s+|-#\s+)", "", line)
    line = MASKED_LINK.sub(r"\1", line)
    for mark in ("***", "**", "__", "~~", "*", "_", "`"):
        if line.startswith(mark) and line.endswith(mark) and len(line) > 2 * len(mark):
            line = line[len(mark) : -len(mark)]
    return line.strip()


def url_key(url: str) -> str:
    """Discord CDN links carry expiring signature parameters: compare without them."""
    return url.split("?", 1)[0]


# ---- images ----


class Images:
    """Downloads and normalizes images, once per source (by URL without its query)."""

    def __init__(self) -> None:
        self.session: aiohttp.ClientSession | None = None
        self.done: dict[tuple[str, str], str] = {}

    async def fetch(self, url: str) -> bytes:
        if self.session is None:
            self.session = aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=60))
        async with self.session.get(url) as r:
            r.raise_for_status()
            return await r.read()

    async def save(self, url: str, prefix: str, fmt: str, max_w: int) -> str | None:
        """Saves the image at `url` as <prefix>.<hash>.<ext> in OUT_DIR, removes older
        <prefix>.* files, and returns the file name."""
        key = (prefix, url_key(url))
        name = self.done.get(key)
        if name and (OUT_DIR / name).exists():
            return name
        try:
            raw = await self.fetch(url)
            img = Image.open(io.BytesIO(raw))
            img.load()
        except Exception as e:  # noqa: BLE001 - a broken image must not stop the export
            log.warning("image %s: %s", url_key(url), e)
            return None
        if img.width > max_w:
            img = img.resize((max_w, round(img.height * max_w / img.width)), Image.LANCZOS)
        buf = io.BytesIO()
        if fmt == "png":
            img.convert("RGBA").save(buf, "PNG", optimize=True)
            ext = "png"
        else:
            img.convert("RGB").save(buf, "JPEG", quality=88, optimize=True, progressive=True)
            ext = "jpg"
        data = buf.getvalue()
        name = f"{prefix}.{hashlib.sha256(data).hexdigest()[:8]}.{ext}"
        write_atomic(OUT_DIR / name, data)
        for old in OUT_DIR.glob(f"{prefix}.*"):
            if old.name != name:
                old.unlink(missing_ok=True)
        self.done[key] = name
        log.info("saved %s", name)
        return name

    async def close(self) -> None:
        if self.session:
            await self.session.close()


# ---- the bot ----


class FeedTree(app_commands.CommandTree):
    """Refuses every interaction outside the control channel."""

    async def interaction_check(self, interaction: discord.Interaction) -> bool:
        allowed = int(self.client.cfg["control_channel"])  # type: ignore[attr-defined]
        if interaction.channel_id == allowed:
            return True
        name = interaction.command.qualified_name if interaction.command else interaction.type.name
        log.info("refused /%s from %s in channel %s", name, interaction.user, interaction.channel_id)
        if not interaction.response.is_done():
            await interaction.response.send_message(
                f"Launcher commands only work in <#{allowed}>.", ephemeral=True
            )
        return False


class FeedBot(discord.Client):
    def __init__(self, mode: str) -> None:
        # No message events: messages are only ever fetched by ID. The Message Content
        # intent is what makes a fetched message's text and embeds visible.
        intents = discord.Intents.none()
        intents.guilds = True
        intents.message_content = True
        super().__init__(intents=intents)
        self.mode = mode
        self.cfg = load_config()
        self.tree = FeedTree(self)
        self.images = Images()
        self.lock = asyncio.Lock()
        self.last_news: str | None = None
        self.news_items: dict[str, dict[str, Any] | None] = {}
        self.exported_at: dict[str, str] = {}

    @property
    def guild_obj(self) -> discord.Object:
        return discord.Object(id=int(self.cfg["guild"]))

    async def setup_hook(self) -> None:
        OUT_DIR.mkdir(parents=True, exist_ok=True)
        if self.mode != "run":
            return
        self.tree.add_command(build_commands(self), guild=self.guild_obj)
        try:
            synced = await self.tree.sync(guild=self.guild_obj)
            log.info("slash commands synced: %s", ", ".join(c.name for c in synced))
        except discord.HTTPException as e:
            log.error("slash command sync failed: %s", e)

    async def on_ready(self) -> None:
        log.info("logged in as %s (%s)", self.user, self.user.id if self.user else "?")
        self.check_rights()
        if self.mode == "dump":
            await self.dump()
            await self.close()
            return
        await self.export_all()
        if self.mode == "once":
            await self.close()
            return
        if not self.news_loop.is_running():
            self.news_loop.start()

    def check_rights(self) -> None:
        """Warns when the bot may do more than read (it should be read-only)."""
        guild = self.get_guild(int(self.cfg["guild"]))
        if guild is None or guild.me is None:
            log.warning("not a member of guild %s", self.cfg["guild"])
            return
        extra = sorted(name for name, on in guild.me.guild_permissions if on and name not in NEEDED)
        if extra:
            log.warning("the bot has more rights than it needs, remove them: %s", ", ".join(extra))
        refs = [r for r in self.cfg["news"].values() if r]
        for cid in {int(r["channel"]) for r in refs}:
            ch = guild.get_channel(cid)
            if ch is None:
                log.warning("channel %s: not visible to the bot", cid)
                continue
            p = ch.permissions_for(guild.me)
            if not (p.view_channel and p.read_message_history):
                log.warning("#%s: the bot can't read it (needs View Channel + Read Message History)", ch.name)
            if p.send_messages:
                log.warning("#%s: the bot may send messages there; it doesn't need to", ch.name)

    async def close(self) -> None:
        await self.images.close()
        await super().close()

    # -- polling --

    @tasks.loop(seconds=NEWS_EVERY_S)
    async def news_loop(self) -> None:
        await self.export_news()

    @news_loop.before_loop
    async def _wait_ready(self) -> None:
        await self.wait_until_ready()
        await asyncio.sleep(5)

    # -- fetching --

    async def message(self, channel_id: int, message_id: int) -> discord.Message:
        channel = self.get_channel(channel_id) or await self.fetch_channel(channel_id)
        return await channel.fetch_message(message_id)  # type: ignore[union-attr]

    async def export_all(self) -> None:
        await self.export_news()

    async def news_item(self, slot: str, ref: dict[str, Any]) -> dict[str, Any] | None:
        msg = await self.message(int(ref["channel"]), int(ref["message"]))
        text = msg.content
        title = ""
        image_url = None
        embed = msg.embeds[0] if msg.embeds else None
        if embed:
            if not text.strip():
                text = embed.description or ""
            title = embed.title or ""
            image_url = (embed.image and embed.image.url) or (embed.thumbnail and embed.thumbnail.url)
        for a in msg.attachments:
            if (a.content_type or "").startswith("image/") or a.filename.lower().endswith(IMAGE_EXT):
                image_url = a.url
                break
        text = resolve(text, msg).strip()
        lines = text.splitlines()
        if not title:
            while lines and not lines[0].strip():
                lines.pop(0)
            title = strip_markdown(lines.pop(0)) if lines else ""
        body = "\n".join(lines).strip()
        masked = MASKED_LINK.search(text)
        bare = BARE_URL.search(text)
        if masked:
            link_label, link_url = masked.group(1), masked.group(2)
        elif bare:
            link_label, link_url = "READ MORE", bare.group(0)
        else:
            link_label, link_url = "READ MORE", msg.jump_url
        image = await self.images.save(image_url, f"news-{slot}", "jpg", NEWS_MAX_W) if image_url else None
        return {
            "id": str(msg.id),
            "channel": str(msg.channel.id),
            "title": resolve(title, msg),
            "body": body,
            "image": image,
            "link_label": link_label,
            "link_url": link_url,
            "jump_url": msg.jump_url,
            "posted_at": iso(msg.created_at),
            "edited_at": iso(msg.edited_at),
        }

    async def export_news(self) -> None:
        async with self.lock:
            for slot in SLOTS:
                ref = self.cfg["news"].get(slot)
                if not ref:
                    self.news_items[slot] = None
                    continue
                try:
                    self.news_items[slot] = await self.news_item(slot, ref)
                except (discord.NotFound, discord.Forbidden) as e:
                    log.warning("news %s message gone or hidden: %s", slot, e)
                    self.news_items[slot] = None
                except discord.HTTPException as e:
                    # Keep the last good item on a transient error.
                    log.warning("news %s message unavailable: %s", slot, e)
            body = {"slots": {s: self.news_items.get(s) for s in SLOTS}}
            key = json.dumps(body, sort_keys=True)
            if key == self.last_news and (OUT_DIR / "news.json").exists():
                return
            self.last_news = key
            body["updated_at"] = now_iso()
            write_atomic(OUT_DIR / "news.json", json.dumps(body, indent=1).encode())
            self.exported_at["news"] = body["updated_at"]
            log.info("news.json written")

    async def dump(self) -> None:
        out: dict[str, Any] = {}
        refs = {f"news:{k}": v for k, v in self.cfg["news"].items() if v}
        for name, ref in refs.items():
            try:
                m = await self.message(int(ref["channel"]), int(ref["message"]))
                out[name] = {
                    "from_bot": m.author.bot,
                    "created_at": iso(m.created_at),
                    "edited_at": iso(m.edited_at),
                    "content": m.content,
                    "embeds": [e.to_dict() for e in m.embeds],
                    "attachments": [
                        {"filename": a.filename, "content_type": a.content_type, "url": url_key(a.url)}
                        for a in m.attachments
                    ],
                }
            except discord.HTTPException as e:
                out[name] = {"error": str(e)}
        print(json.dumps(out, indent=2, ensure_ascii=False))


# ---- slash commands ----


def build_commands(bot: FeedBot) -> app_commands.Group:
    launcher = app_commands.Group(
        name="launcher",
        description="Echo VR launcher feed",
        guild_only=True,
        default_permissions=discord.Permissions(manage_guild=True),
    )
    news = app_commands.Group(name="news", description="Community News in the launcher", parent=launcher)
    slot_choices = [
        app_commands.Choice(name="main: banner and first card", value="main"),
        app_commands.Choice(name="community: second card", value="community"),
    ]

    @news.command(name="set", description="Show a Discord message as Community News in the launcher")
    @app_commands.describe(
        message="Message link, or message ID",
        slot="Which news block (default: main)",
        channel="The message's channel, for a bare ID (default: announcements)",
    )
    @app_commands.choices(slot=slot_choices)
    async def news_set(
        interaction: discord.Interaction,
        message: str,
        slot: app_commands.Choice[str] | None = None,
        channel: discord.TextChannel | None = None,
    ) -> None:
        await interaction.response.defer(ephemeral=True, thinking=True)
        name = slot.value if slot else "main"
        link = MESSAGE_LINK.search(message)
        if link:
            channel_id, message_id = int(link.group(1)), int(link.group(2))
        elif message.strip().isdigit():
            channel_id = channel.id if channel else int(bot.cfg["news_channel"])
            message_id = int(message.strip())
        else:
            await interaction.followup.send("That is neither a message link nor a message ID.", ephemeral=True)
            return
        ref = {"channel": channel_id, "message": message_id}
        try:
            item = await bot.news_item(name, ref)
        except discord.HTTPException as e:
            await interaction.followup.send(f"I can't read that message: {e}", ephemeral=True)
            return
        bot.cfg["news"][name] = ref
        save_config(bot.cfg)
        bot.last_news = None
        await bot.export_news()
        log.info("news %s set to %s/%s by %s", name, channel_id, message_id, interaction.user)
        await interaction.followup.send(
            f"Community News **{name}** now shows {item['jump_url'] if item else message_id}\n"
            f"Title: **{(item or {}).get('title') or '(none)'}**  ·  image: {'yes' if item and item['image'] else 'no'}"
            f"  ·  link: {(item or {}).get('link_label', '-')}",
            ephemeral=True,
        )

    @news.command(name="clear", description="Stop showing a Community News block")
    @app_commands.choices(slot=slot_choices)
    async def news_clear(interaction: discord.Interaction, slot: app_commands.Choice[str]) -> None:
        await interaction.response.defer(ephemeral=True, thinking=True)
        bot.cfg["news"][slot.value] = None
        save_config(bot.cfg)
        bot.last_news = None
        await bot.export_news()
        log.info("news %s cleared by %s", slot.value, interaction.user)
        await interaction.followup.send(f"Community News **{slot.value}** cleared.", ephemeral=True)

    @news.command(name="show", description="Which messages the launcher shows as Community News")
    async def news_show(interaction: discord.Interaction) -> None:
        lines = []
        for s in SLOTS:
            ref = bot.cfg["news"].get(s)
            if ref:
                url = f"https://discord.com/channels/{bot.cfg['guild']}/{ref['channel']}/{ref['message']}"
                title = (bot.news_items.get(s) or {}).get("title") or "?"
                lines.append(f"**{s}**: {url}  ({title})")
            else:
                lines.append(f"**{s}**: not set")
        lines.append(f"Last export: {bot.exported_at.get('news', 'never')}")
        await interaction.response.send_message("\n".join(lines), ephemeral=True)

    @launcher.command(name="refresh", description="Export the news to the launcher now")
    async def refresh(interaction: discord.Interaction) -> None:
        await interaction.response.defer(ephemeral=True, thinking=True)
        bot.last_news = None
        await bot.export_all()
        await interaction.followup.send("Exported the news.", ephemeral=True)

    return launcher


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    group = ap.add_mutually_exclusive_group()
    group.add_argument("--dump", action="store_true", help="print the raw messages as JSON and exit")
    group.add_argument("--once", action="store_true", help="export once and exit")
    args = ap.parse_args()
    mode = "dump" if args.dump else "once" if args.once else "run"

    handlers: list[logging.Handler] = [logging.StreamHandler(sys.stderr)]
    if mode == "run":
        LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
        # One file per day, kept LOG_DAYS days (the retention PRIVACY.md promises).
        handlers.append(
            logging.handlers.TimedRotatingFileHandler(LOG_PATH, when="midnight", backupCount=LOG_DAYS, utc=True)
        )
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)-7s %(name)s: %(message)s",
        handlers=handlers,
    )
    FeedBot(mode).run(load_token(), log_handler=None)


if __name__ == "__main__":
    main()
