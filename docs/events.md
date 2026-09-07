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
| Redeem | `🏅 Name redeemed «Song Request»: <input>` | PRIVMSG `custom-reward-id` / EventSub redemption |
| Hype train | `🚂 Hype Train level 3 — 456/500` | EventSub `channel.hype_train.*` |
| Prediction | `📊 Prediction started: Will X win?` | EventSub `channel.prediction.*` |
| Poll | `🗳️ Poll started: <title>` | EventSub `channel.poll.*` |

Like chat messages, events pass through [Filters](filters.md) by kind
(`sub`, `gift_sub`, `mystery_gift`, `raid`, `redeem`, `hype_train`,
`prediction`, `poll`) — a `message_type` allowlist of `["message"]` hides
all banners, for example.

Subs, gift subs, mystery gifts, and raids arrive over the same anonymous
chat feed as messages — no credentials needed. **Redeems**, hype trains,
predictions, and polls arrive via the **channel OAuth** layer: run
`quiver-chat --auth` once (the broadcaster or a moderator of the channel
approves) — see [OAuth](oauth.md). Redeem banners show the reward name when
OAuth is present; without it they fall back to
`Name redeemed a channel point reward: <input>`.

Related surfaces:

- [Moderation](moderation.md) — deleted messages, cleared chat
- [Protocol](protocol.md#event) — the wire frame behind banners