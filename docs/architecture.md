# Quiver Architecture

Modular toolkit for live streamers, built on strict UNIX philosophy:
small headless tools, each doing one job, composed via explicit configs.
A separate UI sits on top and *generates* those configs — it is never a
hidden source of business logic.

```
                ┌─────────────────────┐
                │      Quiver UI      │   user-friendly configuration
                └──────────┬──────────┘   (future; reserved ui/ dir)
                           │ generates RON configs
                           ▼
            ┌─────────────────────────────┐
            │    Headless Quiver tools    │   CLI + extensive RON config
            └──────────┬──────────────────┘
                       ▼
                 OBS · Twitch
```

## Workspace layout

| Path | Role |
|---|---|
| `crates/quiver-config` | lib — RON loading, semantic validation, JSON Schema export |
| `crates/quiver-twitch` | lib — THE Twitch layer (facade over `twitch-irc`) |
| `crates/quiver-chat` | tool 1 — chat widget engine (`quiver-chat` binary) |
| `crates/quiver-chat/widget` | browser-source frontend (TS/vite, not cargo) |
| `ui/` | reserved for the future UI app; outside the Cargo workspace |
| `docs/schemas/` | generated JSON Schemas (the UI↔tool contract) |

## Decision log

1. **One binary per tool** (`quiver-chat`, ...). Tools stay independently
   executable, composable, scriptable. Tool crates are lib+bin: the bin is
   thin (args → config → run); logic lives in the lib so a future `quiver`
   umbrella dispatcher can call libs without rework.

2. **RON as the config format.** Verbose/explicit on purpose. Comments +
   trailing commas + deep nesting where TOML gets painful. Config files are
   a first-class interface, authored by the UI, consumed directly by tools.

3. **JSON Schema export is the UI contract.** `quiver-chat --print-schema`
   emits the schema derived from the Rust config model (schemars 1.x,
   draft 2020-12). The UI reads schemas to build forms and writes RON —
   it never needs to know tool internals.

4. **twitch-irc behind a facade.** `quiver-twitch` maps library messages
   onto Quiver's own `Event`/`ChatMessage` model. twitch-irc types never
   leak into tool crates, so the backend can be swapped without touching
   them. A handrolled IRC parser was tried and deleted: without
   `CAP REQ :twitch.tv/tags` it silently received no badges/emotes/colors,
   and reconnect/rate-limit/TLS/USERNOTICE handling was all still missing.

5. **Widget frontend ships inside its tool crate.** Engine serves static
   files from `server.widget_dist` (config override) plus a live WebSocket
   feed at `/ws`; OBS points at the listen address. Page = dumb renderer;
   engine = headless brain. Placeholder page is served when no dist exists.

6. **Config-first failure mode.** Chat feed failure must not kill the
   widget server — OBS keeps rendering; the feed stays empty until restart.

## Gotchas (do not relearn these)

- Twitch emote ranges arrive as UTF-16 code units with inclusive end;
  twitch-irc normalizes to char indices with EXCLUSIVE end. Our
  `EmoteRef` documents and relies on that.
- `PrivmsgMessage::try_from` requires tags `room-id`, `user-id`,
  `display-name`, `tmi-sent-ts`, `badge-info`, `badges`. Real Twitch sends
  empty-string forms (`badges=`, `color=`, `emotes=`) rather than omitting
  them — test fixtures must mirror that.
- brew-installed rustup does not add cargo/rustc to PATH automatically;
  toolchains live under `~/.rustup/toolchains/`.
- schemars 1.x emits nested structs under `$defs` referenced by `$ref`
  (draft 2020-12), not inline.

## Testing policy

Integration tests with hand-written ground truth only — expected values are
constants computed by hand, never derived in-test. Fixtures are real RON
files on disk. IRC parsing is tested through real wire lines (tags included)
via the library's own parser path. No live network in tests.

## Roadmap pointers

- `ui/`: reads `docs/schemas/*.json`, generates RON configs.
- Optional `quiver` dispatcher binary calling tool libs directly.
- Widget assets embedded into the binary once a build exists (rust-embed),
  with `widget_dist` remaining as override.
