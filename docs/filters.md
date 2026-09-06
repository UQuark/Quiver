# Filters

Filter which messages render, per dimension. Every dimension supports both
**allow** and **deny** behavior; dimensions combine with **AND** — a message
shows only when every active dimension passes.

```ron
filters: (
    # Regex against the sender's display name
    display_name: Some(( mode: denylist, items: ["^StreamElements"] )),

    # Regex against the sender's login (always lowercase)
    username: Some(( mode: denylist, items: ["nightbot", "moobot"] )),

    # EXACT Twitch user id (or login handle)
    user_id: None,

    # Regex against message text — e.g. hide chat commands:
    content: Some(( mode: denylist, items: ["^!"] )),

    # Message kind names: message | sub | gift_sub | mystery_gift | raid
    message_type: None,

    # Badge ids the sender carries ("moderator", "vip", ...)
    role: None,
)
```

## Semantics

- A dimension with an **empty** items list is **inactive** (an empty
  allowlist means "match everything", deliberately not "drop everything").
- **allowlist** — at least one item must match, or the message is dropped.
- **denylist** — if any item matches, the message is dropped.
- Dimensions AND: every active dimension must pass.
- `user_id` is exact-match (regex metacharacters are literal — `"7*"` matches
  only the string `7*`).
- Events (subs/raids/…) filter by kind; their text is empty and they carry
  no badges, so a `content` or `role` filter sees an empty target.

## Errors are loud (on purpose)

Regex patterns that fail to compile, or unknown message-kind names, are
**hard validation errors**: the config is rejected at startup, and a hot
reload with a broken filter is refused — the previous filters stay active.
Moderation must never silently stop working.

## Examples

Keep the chat to staff and fan-favorites:

```ron
filters: (
    role: Some(( mode: allowlist, items: ["moderator", "vip", "subscriber"] )),
)
```

Hide bots and command spam:

```ron
filters: (
    username: Some(( mode: denylist, items: ["nightbot", "streamelements", "moobot"] )),
    content:  Some(( mode: denylist, items: ["^!"] )),
)
```

Combined with [Badges](badges.md)' `hide_native` and
[Emotes](emotes.md)'s per-provider toggles, most "what should the overlay
show" questions are config-only.