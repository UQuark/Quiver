# quiver-chat widget frontend

Browser page loaded by OBS as a browser source. Dumb renderer: connects to
the engine's WebSocket feed (`/ws`) and draws what it receives. All
behavior/config lives in the Rust engine.

**No build step** — plain HTML + CSS + a native ES module. Point
`server.widget_dist` in your quiver-chat RON config at this `widget/`
directory and start `quiver-chat`.

OBS browser source URL: `http://127.0.0.1:4783` (per `server.listen`).

Wire contract (must match Rust `RenderedMessage` in
`crates/quiver-chat/src/engine.rs`):

```json
{ "type": "snapshot", "messages": [RenderedMessage...] }
{ "type": "message",  "message":  RenderedMessage }
```
