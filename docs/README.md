# Quiver Documentation

Quiver is a modular toolkit for live streamers. This documentation covers the
**quiver-chat** widget — a highly customizable OBS–Twitch chat overlay built
from small headless tools, configured by verbose RON files, and hot-reloaded
as you edit.

Everything here is written for users. Pick a page:

## Getting started
- [Getting Started](getting-started.md) — build, run, first config, OBS setup
- [Configuration](configuration.md) — the RON config system, generator, validation, hot reload

## Chat & behavior
- [Message Rendering](chat.md) — layout, badges, replies, anchoring, overflow, lifecycle
- [Emotes](emotes.md) — first-party Twitch, animated, 7TV/BTTV/FFZ, per-provider toggles
- [Badges](badges.md) — native Twitch badges, custom per-role/per-user badges, caching
- [Custom CSS](custom-css.md) — inline or file/URL sources, per-role CSS, themes
- [Filters](filters.md) — allow/deny lists per dimension (user, role, content, …)
- [Stream Events](events.md) — subs, gift subs, mystery gifts, raids
- [Moderation](moderation.md) — message deletion, whole-chat clear, GIF keyboard messages

## Behind the scenes
- [WebSocket Protocol](protocol.md) — every wire frame the widget understands
- [Troubleshooting](troubleshooting.md) — OBS caching, common gotchas