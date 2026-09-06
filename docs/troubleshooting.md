# Troubleshooting

## OBS shows stale styles/scripts

OBS's embedded Chromium can serve cached CSS/JS even after edits. Quiver is
already hardened against this:

- every static response sends `Cache-Control: no-cache` (+ legacy headers),
- asset URLs are **versioned** in `index.html` (`?av=<mtime>`), so any
  frontend file change produces a fresh URL a cache can't serve from memory.

If you *still* see stale content after a widget edit, it's the first refresh
after the change — reload the browser source once (reopen or toggle it) and
the new URL is picked up; after that it self-corrects.

## A frontend edit did nothing visible

1. Did the server reload the page? The frontend watcher pushes
   `{ "type": "reload" }` — a connected widget reloads itself. If nothing
   happened, check the server log for `widget frontend changed`.
2. **The running binary may be old.** Features are only in the binary you
   actually restarted with. `quiver-chat --config chat.ron` from a stale
   `target/debug` build can look "broken" for a feature you just committed.
   Rebuild (`cargo build`) and restart before testing.

## Config edits are rejected

Validation is fail-closed on purpose: a broken filter regex, unknown
message kind, or dangling badge reference refuses the whole reload and
keeps the old config. Watch the server log for `reload rejected` with the
specific `path: message`. Fix the file, save again.

Advisory-only exceptions: `custom_css` lint warnings (parse issues are
logged, not fatal) and badge *fetch* failures (one broken badge is skipped,
the rest still resolve).

## Channel has no messages — but the streamer is live?

- Chat is anonymous by default; `twitch.channel` must match the login (lowercase).
- `filters` may be culling everything — an `allowlist` on `role` with no
  matches hides the channel (that's correct behavior).
- Emotes needing credentials (channel sets) won't resolve without
  `client_id`/`client_secret` — see [Badges](badges.md#twitch-credentials).

## Cache directory not where I expected

HTTP-cached badge images live under `$XDG_CACHE_HOME/Quiver/badges/`
(fallback `~/.cache/Quiver/badges/`). Override with `badges.cache_dir`.

## WebSocket keeps disconnecting

`/health` responds when the server is up. If the page reconnects forever,
the URL port in OBS doesn't match `server.listen` (a live port rebind
requires re-pointing the OBS source). See [Configuration](configuration.md#hot-reload).

## Where does the wire data come from?

The complete frame set: [Protocol](protocol.md). Cards/layout/overflow:
[Message Rendering](chat.md). Styling: [Custom CSS](custom-css.md).