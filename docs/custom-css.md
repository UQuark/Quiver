# Custom CSS

Three styling layers, applied in order:

1. built-in widget styles
2. per-role CSS (`theme.role_css`)
3. your custom CSS (`theme.custom_css`) — always wins equal-specificity ties

## Your own CSS

`theme.custom_css` accepts either inline text or a URI source:

```ron
theme: (
    # inline (the classic form)
    custom_css: Some(".msg { border-radius: 4px; }"),

    # …or load from a file / URL — swaps live on reload
    custom_css: Some(( uri: "file:///home/user/quiver-theme.css" )),
    custom_css: Some(( uri: "https://cdn.example.com/theme.css" )),
)
```

URI sources resolve like badges do (see [Badges](badges.md#caching)): `file://`
read fresh from disk, `http(s)://` fetched and cached. An **active** source
re-resolves on every config reload — edit the file, save the config, done.

The CSS is linted with a real parser at load/reload; a syntax problem is a
warning, never a blocker (browsers apply what they accept).

## Per-role CSS

Style messages by the sender's badge:

```ron
theme: (
    role_css: Some({
        "moderator":     ".msg.role-moderator { border-left: 2px solid #00AD03; }",
        "broadcaster":   ".msg.role-broadcaster { outline: 1px solid red; }",
        "subscriber":    ".user { opacity: 0.9; }",
    }),
)
```

Message rows get a `role-<badgeid>` class for every badge the sender carries
that appears in the map. Styles inject **before** your `custom_css`, so your
global rules still win.

## Styling hooks

### Element classes

| Class | Element |
|---|---|
| `.msg` | one message card (also `.msg.action`, `.msg.has-reply`) |
| `.badges` / `.badge` / `.custom-badge` | badge row and images |
| `.user`, `.sep`, `.text`, `.body` | name / colon / wrapped text / inline body |
| `.reply`, `.reply-arrow`, `.reply-user`, `.reply-text` | reply thread header |
| `.gif` | GIPHY GIF rendering |
| `.event`, `.event-icon` | transient event banners |

### Container-level hooks

`#chat` is the message column (add `overflow-scroll` class in scroll mode).
`html`, `body` are transparent by default — your OBS scene shows through the
cards (`rgba(28,28,33,.5)` default card background, `.reply` at `rgba(42,42,46,.5)`).

### Utility state

Messages being expired get `.expiring` (a fade-out transition) before removal.

> Sizing tip: emotes are `1.4em`, badges `1em`, GIFs `max-height: 5em` —
> all overrideable via the classes above.