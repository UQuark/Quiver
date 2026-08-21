# quiver-chat widget frontend

Browser page loaded by OBS as a browser source. Dumb renderer: connects to
the engine's WebSocket feed (`/ws`) and draws what it receives. All
behavior/config lives in the Rust engine.

## Build

```sh
npm install
npm run build   # outputs dist/
```

Then set `server.widget_dist` in your quiver-chat RON config to the
absolute path of this `dist/` directory and start `quiver-chat`.

OBS browser source URL: `http://127.0.0.1:4783` (per `server.listen`).
