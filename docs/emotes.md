# Emotes

Emotes come from two worlds: **first-party** Twitch emotes (delivered per-
message with exact positions) and **third-party** providers (7TV, BetterTTV,
FrankerFaceZ) whose emotes are bare words resolved against a channel's
emote set.

Everything is toggleable per provider in the `emotes` section:

```ron
emotes: (
    twitch: true,   # first-party Twitch emotes
    unicode: true,  # native emoji characters
    seventv: true,  # 7TV global + channel sets
    bttv: true,     # BetterTTV global + channel/shared sets
    ffz: true,      # FrankerFaceZ global + room sets
)
```

**Disabled = gone**: turning a provider off doesn't show its emote as text —
third-party tokens are stripped, unicode emoji characters are removed from
text, and first-party emote images are skipped.

## Twitch emotes

First-party emotes render from the wire's exact character ranges.

- **Numeric emote IDs** (the classic `Kappa` family) use the static CDN path.
- **`emotesv2_*` IDs** are the animated generation — they render as animated
  GIFs via the `v2` CDN path, with the static image as an automatic fallback
  (in case a particular emote has no animated variant).

## Third-party emotes

7TV / BTTV / FFZ emotes appear as bare words in chat text. The engine
fetches each provider's **global + channel** sets:

- 7TV: the channel's active emote set (plus global)
- BTTV: channel emotes + shared emotes (plus global `:tf:`-style emotes)
- FFZ: the room set + global default sets

…merges them into one map served at `/emotes.json` (auto-refreshed on
reload), and the widget replaces matching words with images.

The map is fetched **once per page load** and cached by the browser with
`no-cache` semantics.

## Unicode emoji

Native emoji characters (`😂`) pass through as text by default. With
`unicode: false` they are **stripped out of message text entirely** —
including ZWJ sequences and variation selectors.

> Note: emote & third-party rendering needs no Twitch credentials; the
> channel-specific 7TV/BTTV/FFZ sets *do* (they're keyed by your channel's
> numeric ID). See [Badges](badges.md#twitch-credentials).