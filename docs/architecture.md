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
| `crates/quiver-chat/widget` | browser-source frontend (plain HTML/CSS/ES-module, not cargo, no build step) |
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
   The frontend is deliberately framework-free vanilla ES modules: no npm,
   no bundler — the browser loads it directly. Reintroduce tooling only
   when the frontend's real complexity demands it.

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

## Wire protocol (`/ws`)

All frames are single-line JSON objects with a `type` field:

| Frame | Shape | Meaning |
|---|---|---|
| `snapshot` | `{type, messages[], meta}` | full state for (re)connects and lag resyncs |
| `message` | `{type, message}` | one new rendered message |
| `expire` | `{type, ids[]}` | messages removed by count cap or age |
| `config` | `{type, meta}` | hot reload: theme/badges/custom_css changed |
| `clear` | `{type}` | channel swapped — history wiped |

`meta` carries `{theme:{font_size_px, max_messages}, badges:{"set/version": url},
custom_css}`. The engine owns the lifecycle: snapshots are always correct for
late joiners, and clients never compute expiry themselves.

Emote positions are char indices into `text`, end exclusive (twitch-irc
normalizes Twitch's UTF-16 inclusive wire format). Badge URLs require
optional Twitch API credentials (`twitch.client_id`/`client_secret`,
client-credentials flow); without them badges flow as data but render empty.

## Hot reload

The whole config file is watched (debounced directory watch + SIGHUP).
A change triggers load → validate → diff → apply. Rejections (parse error,
validation failure, invalid channel login) keep the current config — no
half-applied state. Applied effects per field:

- theme / custom_css → live, pushed to clients as a `config` frame
- message_lifetime_secs → picked up by the sweep on its next tick
- twitch.channel → live part/join on the same connection; history cleared
- credentials → badge map refetch
- server.widget_dist → resolved per request from disk
- server.listen → HTTP listener rebinds live; WebSocket handlers are freed
  via a generation token and pages auto-reconnect (a page pointed at the
  old port cannot follow — repoint the OBS source URL once)

Custom CSS is linted with lightningcss at load/reload; a parse failure is a
warning, never fatal (browsers apply what they accept).

## Testing policy

Integration tests with hand-written ground truth only — expected values are
constants computed by hand, never derived in-test. Fixtures are real RON
files on disk. IRC parsing is tested through real wire lines (tags included)
via the library's own parser path. No live network in tests.

## Roadmap pointers

- `ui/`: reads `docs/schemas/*.json`, generates RON configs.
- Optional `quiver` dispatcher binary calling tool libs directly.
