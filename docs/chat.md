# Message Rendering

How chat messages are laid out, kept in bounds, and given a lifetime.

## Layout

Each message renders as a card:

```
[badges] [Username]: message text…
```

- **Badges and username are always glued together** — they share one inline
  flow, so long messages wrap only the text, never the identity.
- Text wraps at word boundaries; long unbroken tokens (URLs) are the only
  thing allowed to break mid-word.
- Messages are **anchored to the bottom** of the container. When old messages
  are removed for any reason, the remaining ones stay put — the chat never
  "jumps up".

The style classes are the hooks: `.msg`, `.badges`, `.user`, `.sep`, `.text`,
`.body`. See [Custom CSS](custom-css.md).

## Message lifecycle

Three independent layers decide how long a message lives:

1. **Count cap** — `theme.max_messages` (default 30). A sanity limit: more
   than N messages in history, the oldest is evicted. Counts `.msg` nodes
   only — event banners are never trimmed this way.
2. **Age expiry** — `theme.message_lifetime_secs` (default 60). Messages
   older than this fade out and are removed. Snapshots for late joiners
   exclude expired messages automatically.
3. **Height overflow** — when the fixed-height container runs out of room,
   `theme.overflow_mode` decides what happens (next section).

Deletion by moderators is a separate, instant path — see
[Moderation](moderation.md).

## Config re-rendering

A hot reload that changes badge maps, custom-badge heights, or emote flags
re-renders *every existing message* automatically — no snapshot refresh
required. Internally the widget keeps a bounded history of rendered wire
messages and replays `renderMessage()` on each. Event banners survive the
redraw; observer handles for height measurement are cleaned up and
re-established.

This means: change `badges.vip-star.height` in your config, save, and all
VIP badges resize immediately in your OBS source.

## Overflow: `prune` vs `scroll`

The widget fills the OBS browser source's pixel box exactly. When variable-
height content (GIFs, big emotes, reply cards) exceeds that box:

- **`prune`** (default): the oldest `.msg` nodes are removed until everything
  fits. **The newest message is never pruned** — an oversized GIF still shows
  *something*. Event banners are never pruned either.
- **`scroll`**: the container scrolls, pinned to the bottom while you're
  already at the bottom. The scrollbar is hidden (override via custom CSS on
  `#chat.overflow-scroll`).

```ron
theme: (
    max_messages: 30,
    message_lifetime_secs: 60,
    overflow_mode: prune,   # or: scroll
)
```

Switching modes is hot-reloaded: edit, save, done.

## Reply threads

When a message replies to another, a compact header strip renders *above* it
showing `➚ ParentUser: parent text…` on a gray card. The whole reply —
header + message — shares one card background.

Twitch prepends `@ParentName ` to reply text on the wire; that prefix is
hidden automatically, since the parent is already visible above. (A message
that manually starts with a *different* `@mention` is left alone.)

Customize replies with these classes: `.has-reply`, `.reply`, `.reply-arrow`,
`.reply-user`, `.reply-text`.

## GIF messages

Tier-2/3 subscribers can post GIPHY GIFs directly in chat. These render
inline at their position in the message: looping muted `<video>` for `.mp4`
URLs, `<img>` otherwise, capped at 5em (`theme`-independent; override with
`.gif`). See [Protocol](protocol.md#message-shape) for the wire shape.