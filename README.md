# Quiver

Modular toolkit for live streamers, built on strict UNIX philosophy: small
headless tools, each doing one job well, composed via explicit configs.
A separate UI module (future) generates those configs — it is never a
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

## Layout

| Path | Role |
|---|---|
| `crates/quiver-config` | config subsystem: RON loading, validation, JSON Schema export, commented default-config generation |
| `crates/quiver-twitch` | THE Twitch layer — facade over `twitch-irc` + minimal Helix client |
| `crates/quiver-chat` | first tool: OBS–Twitch chat widget engine (`quiver-chat` binary) |
| `crates/quiver-chat/widget` | widget frontend (plain HTML/CSS/ES module — no build step) |
| `ui/` | reserved for the future UI app (outside the Cargo workspace) |
| `skills/` | agent-facing context (product philosophy in `init-prompt`) |
| `LICENSE` | GPLv3 |

Config files are the tool's interface: verbose, explicit RON, authored by
the UI and consumed directly by headless tools. Every tool's config model
lives in its crate; doc comments on the structs are the single source of
truth for both the JSON Schema contract (`--print-schema`) and commented
sample generation (`--sample-config`).

## Quickstart

```sh
cargo build --release

# Generate a commented starter config, point it at a channel, run:
./target/release/quiver-chat --sample-config > chat.ron
#   ... edit channel (and credentials — see below) ...
./target/release/quiver-chat --config chat.ron

# Open http://127.0.0.1:4783 in a browser, or add it as an OBS
# browser source (transparent background, no cache).
```

Twitch API credentials (`twitch.client_id` / `client_secret`) enable badge
images and channel-specific 7TV emotes; keep them in git-ignored local
configs (`*.local.ron`).

## Features

- Live chat with first-party + third-party emotes (7TV, BTTV, FFZ — each
  toggleable), badges, `/me` styling, reply-thread cards
- Transient event banners: subs, resubs, gift subs, mystery gifts, raids
- Message filters: allowlist/denylist per dimension (display name, login,
  user id, content regex, message kind, role badge)
- Per-role CSS (`role_css`) and full custom CSS, both hot-reloaded
- Whole-config hot reload (file watch + `SIGHUP`) incl. live channel
  swaps and port rebinding; frontend hot reload for the widget files
- Commented config generation that can never drift from the types

## Commands

```sh
quiver-chat --config PATH        # run with config
quiver-chat --sample-config      # print a commented default config
quiver-chat --print-schema       # print the config JSON Schema (UI contract)
```

## License

GPLv3 — see [LICENSE](LICENSE).