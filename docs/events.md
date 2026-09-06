# Stream Events

Besides chat messages, Quiver renders **transient banners** for rich chat
events. Banners prepend above the chat flow, auto-dismiss after 8 seconds
(`.event`, `.event-icon` classes), and are **never pruned** by overflow —
their timer is their only exit.

| Event | Banner text | From |
|---|---|---|
| Sub / resub | `★ Name subscribed!` / `★ Name resubbed (12 mo, 7 streak)` | Twitch `sub`/`resub` |
| Gift sub | `🎁 Gifter gifted a sub to Recipient` | Twitch `subgift`/`anonsubgift` |
| Mystery gift | `🎁🎁 Gifter is gifting 10 subs!` | Twitch `submysterygift`/`anonsubmysterygift` |
| Raid | `⚔ Raider raided with 42 viewers!` | Twitch `raid` |

Like chat messages, events pass through [Filters](filters.md) by kind
(`sub`, `gift_sub`, `mystery_gift`, `raid`) — a `message_type` allowlist of
`["message"]` hides all banners, for example.

These need no credentials — they arrive over the same anonymous chat feed
as messages.

Related surfaces:

- [Moderation](moderation.md) — deleted messages, cleared chat
- [Protocol](protocol.md#event) — the wire frame behind banners