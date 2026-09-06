# Moderation

Quiver reacts to chat moderation in real time. All of it is automatic — no
configuration needed.

## Message deletion

When a moderator or the streamer deletes an individual message, Twitch sends
a `CLEARMSG` frame. quiver-chat:

1. removes the message from its history (late-joining clients never see it),
2. broadcasts `{ "type": "delete", "id": … }` — connected widgets hide that
   message **instantly**. No fade: deletion is decisive.

## Whole-chat clear

"Clear Chat" in Twitch empties the entire channel. quiver-chat handles the
parameterless `CLEARCHAT` by wiping history and broadcasting
`{ "type": "clear" }` — the widget empties itself in one frame.

Bans and timeouts are *not* treated as clears: each deleted message arrives
as its own `CLEARMSG` and is removed individually.

## GIF Keyboard messages

Twitch's Tier-2/3 subscriber GIF Keyboard (GIPHY) posts GIFs into chat as
messages carrying a `gifs` tag (position + signed media URL). Quiver:

- renders the GIF **inline at its exact position** — looping muted `<video>`
  for `.mp4` URLs, `<img>` otherwise, capped at 5em (`.gif` class),
- keeps the bracketed GIF name text (shown instead on devices where
  animations can't play) — it's part of the message text, and the widget
  hides only the redundant `@parent` prefix on *replies* (see
  [Message Rendering](chat.md#reply-threads)).

Moderation of GIF messages is Twitch-server-side (they're ordinary
messages to us — deletable via `CLEARMSG` like any other).

See [Protocol](protocol.md) for the exact wire frames behind all of the
above.