---
name: init-prompt
opencode/autoinvoke: true
description: >
  Development context and architectural principles for Quiver, a modular
  UNIX-philosophy toolkit for Twitch streamers and OBS. Quiver consists of
  independent headless CLI tools with extensive machine-oriented configuration,
  orchestrated by a separate user-friendly UI that generates those configs.
  The initial product component is a highly customizable OBS-Twitch chat widget.
  Use this skill to preserve Quiver's architectural philosophy and product
  boundaries when performing development tasks.
---

# Quiver — Architecture and Product Context

Quiver is a modular toolkit for live streamers, built around a strict **UNIX philosophy**.

The system should not be designed as one monolithic application. Instead, functionality is split into small, focused, **headless tools** that each do one job well. These tools are intended to be composable, scriptable, independently executable, and usable without the graphical interface.

The headless tools may expose CLI interfaces and rely on deliberately explicit, extensive configuration files. Their configuration format can be verbose and unpleasant for humans to author manually. **That is intentional.**

A separate **UI module** sits on top of these tools. Its purpose is not to replace their configuration model, but to make it accessible to normal users. The UI provides a user-friendly, mouse-oriented configuration experience—checkboxes, selectors, previews, forms, toggles, etc.—and generates the underlying detailed configuration consumed by the headless tools.

The conceptual architecture is therefore:

```text
                    ┌─────────────────────┐
                    │      Quiver UI      │
                    │                     │
                    │ user-friendly       │
                    │ configuration       │
                    └──────────┬──────────┘
                               │
                     generates config
                               │
                               ▼
              ┌────────────────────────────┐
              │     Headless Quiver tools  │
              │                            │
              │ CLI + extensive config     │
              │ one focused responsibility  │
              └────────────────────────────┘
                         │       │
                         ▼       ▼
                       OBS    Twitch
```

The GUI should **not become a hidden source of business logic** that the CLI tools depend on. The headless components should remain meaningful and functional independently. The UI is an orchestration/configuration layer over them.

This also means that configuration files should be treated as a legitimate first-class interface. They can be highly detailed and expose capabilities that the UI may not initially expose. The UI should generate valid configuration rather than forcing the underlying tools into a simplified configuration model.

## Initial component

The first component being developed is a **highly customizable OBS–Twitch chat widget**.

It is intended to display Twitch chat inside OBS, most likely through the mechanisms appropriate for OBS browser-based widgets.

The widget should ultimately provide extensive control over its appearance and behavior rather than being limited to a fixed chat-box design.

Relevant customization may include things such as:

* message layout
* usernames and username styling
* message styling
* badges
* emotes
* animations
* message timing and lifecycle
* moderation/system messages
* user-specific styling
* chat behavior
* appearance and typography
* spacing, sizing, positioning, and other visual properties
* configurable reactions to chat events

However, these are **product-domain context, not an implied implementation task or complete requirements specification**. The actual behavior and scope of the widget will be defined by subsequent tasks.

The widget should fit the broader Quiver philosophy: the underlying component should be capable of being configured through a detailed, explicit configuration model, while a future Quiver UI can expose the same capabilities through a significantly more approachable interface.

Do not infer specific implementation technologies, architecture beyond these principles, features, APIs, schemas, or acceptance criteria unless explicitly requested by a subsequent task.

