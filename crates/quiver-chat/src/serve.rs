//! Widget host: HTTP server for the widget frontend plus a WebSocket feed.
//!
//! Hot-reload aware: everything here reads through [`LiveConfig`] so a
//! config reload changes behavior without restarting the process. The
//! listener itself rebinds when `server.listen` changes; WebSockets are
//! freed via a generation token and pages reconnect automatically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::{Notify, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::ChatConfig;
use crate::engine;
use crate::live::{LiveConfig, SharedLive};

const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A feed that lived this long counts as healthy; reset backoff after it.
const HEALTHY_FEED_RUNTIME: Duration = Duration::from_secs(300);

pub(crate) type SharedBadges = Arc<RwLock<HashMap<String, String>>>;

#[derive(Clone)]
pub(crate) struct AppState {
    pub live: SharedLive,
    pub messages: engine::SharedState,
    pub tx: broadcast::Sender<String>,
    pub badges: SharedBadges,
    pub custom_badges: crate::badges::SharedBadgeCache,
    /// Resolved custom CSS text for the widget (inline or from URI source).
    pub custom_css: SharedCss,
    pub emotes: crate::emotes::SharedEmotes,
    /// Generation token: cancelled on rebind/shutdown so WS handlers
    /// return instead of blocking graceful drain forever.
    pub ws_token: CancellationToken,
}

pub(crate) struct FeedHandle {
    pub swap_tx: mpsc::UnboundedSender<String>,
}

const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>quiver-chat</title></head>
<body style="font-family: sans-serif; background:#18181b; color:#efeff1; padding:2em">
<h1>quiver-chat is running</h1>
<p>No widget frontend is being served. Point <code>server.widget_dist</code>
in your RON config at the widget directory:</p>
<pre>crates/quiver-chat/widget</pre>
<p>and reload the page. The WebSocket feed is live at <code>/ws</code> regardless.</p>
</body></html>
"#;

/// Run the chat tool until Ctrl-C. Returns when shut down.
pub async fn run(cfg: ChatConfig, config_path: PathBuf) -> anyhow::Result<()> {
    let live: SharedLive = Arc::new(RwLock::new(LiveConfig::from(&cfg)));
    let (tx, _rx) = broadcast::channel::<String>(256);
    let messages = Arc::new(Mutex::new(engine::EngineState::new(
        cfg.theme.max_messages as usize,
    )));

    // Lint the RESOLVED css (inline text or the file/uri content).
    let custom_css: SharedCss = Arc::new(RwLock::new(
        resolve_custom_css(&cfg.theme.custom_css, &reqwest::Client::new()).await,
    ));
    let role_css = &cfg.theme.role_css;
    crate::config::report_css_lint(
        custom_css.read().ok().and_then(|c| c.clone()).as_deref(),
        role_css.as_ref(),
    );

    let initial_badges = load_badge_map_opt(
        cfg.twitch.client_id.as_deref(),
        cfg.twitch.client_secret.as_deref(),
        &cfg.twitch.channel,
    )
    .await;
    info!(badge_count = initial_badges.len(), "badge map ready");
    let badges: SharedBadges = Arc::new(RwLock::new(initial_badges));

    let initial_emotes = crate::emotes::load_third_party_emotes(
        cfg.twitch
            .client_id
            .as_deref()
            .zip(cfg.twitch.client_secret.as_deref()),
        &cfg.twitch.channel,
    )
    .await;
    let emotes: crate::emotes::SharedEmotes = Arc::new(RwLock::new(initial_emotes));

    let custom_badges = match &cfg.badges {
        Some(badge_cfg) => {
            let http = reqwest::Client::new();
            match crate::badges::resolve_full(badge_cfg, &http).await {
                Ok(state) => {
                    info!(
                        count = state.resolved.definitions.len(),
                        "custom badge cache ready"
                    );
                    Arc::new(RwLock::new(Some(state)))
                }
                Err(e) => {
                    warn!(error = %e, "custom badge resolution failed — starting without");
                    Arc::new(RwLock::new(None))
                }
            }
        }
        None => Arc::new(RwLock::new(None)),
    };

    let quit = CancellationToken::new();
    let rebind = Arc::new(Notify::new());

    // Ctrl-C cancels `quit`; every component observes it.
    {
        let quit = quit.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown signal received");
            quit.cancel();
        });
    }

    // Fail-closed honesty: a filter that cannot compile must never boot a
    // tool that silently moderates nothing. (validate() already rejects
    // broken patterns; this is the belt to those suspenders.)
    let compiled_filters = crate::filters::CompiledFilters::compile(&cfg.filters)
        .map_err(|e| anyhow::anyhow!("filters failed to compile: {e}"))?;
    let filters: crate::filters::SharedCompiled = Arc::new(RwLock::new(Some(compiled_filters)));

    let feed = spawn_feed(live.clone(), messages.clone(), tx.clone(), filters.clone());

    // Frontend hot reload: watch the widget dir, push {type:reload} frames.
    let (fe_watch_tx, fe_watch_rx) = tokio::sync::mpsc::unbounded_channel::<Option<PathBuf>>();
    crate::reload::spawn_frontend_watcher(
        live.read().unwrap().widget_dist.clone(),
        fe_watch_rx,
        tx.clone(),
    );

    let ctx = crate::reload::ReloadCtx {
        live: live.clone(),
        messages: messages.clone(),
        tx: tx.clone(),
        badges: badges.clone(),
        feed_swap: feed.swap_tx,
        fe_watch: fe_watch_tx,
        custom_badges: custom_badges.clone(),
        custom_css: custom_css.clone(),
        emotes: emotes.clone(),
        filters: filters.clone(),
        rebind: rebind.clone(),
    };
    crate::reload::spawn_watcher(config_path, ctx, quit.clone());

    loop {
        let addr = live.read().unwrap().listen.clone();
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                warn!(%addr, %e, "cannot bind — retrying in 2s (fix server.listen in the config)");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        // Per-binding tokens: `stop_bind` ends this server instance,
        // `ws_token` (its child) frees WebSocket handlers on drain.
        let stop_bind = quit.child_token();
        let ws_token = stop_bind.child_token();
        let app = router(AppState {
            live: live.clone(),
            messages: messages.clone(),
            tx: tx.clone(),
            badges: badges.clone(),
            custom_badges: custom_badges.clone(),
            custom_css: custom_css.clone(),
            emotes: emotes.clone(),
            ws_token,
        });

        info!(%addr, "widget server listening");

        // SINGLE consumer rule: the graceful-shutdown future watches ONLY
        // `stop_bind`. The `rebind` Notify has exactly one waiter — the
        // outer select below — which then cancels `stop_bind`. Two
        // notified() waiters would race for notify_one's single permit.
        let mut serve = std::pin::pin!(
            axum::serve(listener, app)
                .with_graceful_shutdown({
                    let stop_bind = stop_bind.clone();
                    async move { stop_bind.cancelled().await }
                })
                .into_future()
        );

        enum Stop {
            Quit,
            Rebind,
        }
        let stop = tokio::select! {
            r = &mut serve => {
                r?;
                Stop::Quit
            }
            _ = quit.cancelled() => Stop::Quit,
            _ = rebind.notified() => {
                stop_bind.cancel();
                Stop::Rebind
            }
        };

        // Let graceful drain finish (WS handlers see the token and exit).
        serve.as_mut().await?;

        match stop {
            Stop::Quit => break,
            Stop::Rebind => info!("listener rebound"),
        }
    }

    info!("widget server stopped");
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .route("/badge-cache/{hash}", get(badge_cache_handler))
        .route("/badge-file/{hash}", get(badge_file_handler))
        .route(
            "/emotes.json",
            get(|State(state): State<AppState>| async move {
                let map = state.emotes.read().map(|m| m.clone()).unwrap_or_default();
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    [(header::CACHE_CONTROL, "no-cache")],
                    serde_json::json!({ "providers": map }).to_string(),
                )
            }),
        )
        .fallback(get(static_fallback))
        .with_state(state)
}

// ---- feed -----------------------------------------------------------------

/// Supervised Twitch feed. Keeps ONE source alive across config reloads:
/// channel swaps are performed live via part/join on the client handle.
fn spawn_feed(
    live: SharedLive,
    messages: engine::SharedState,
    tx: broadcast::Sender<String>,
    filters: crate::filters::SharedCompiled,
) -> FeedHandle {
    let (swap_tx, mut swap_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut backoff = INITIAL_BACKOFF;
        'outer: loop {
            let channel = live.read().unwrap().channel.clone();
            let Ok(mut source) = quiver_twitch::IrcChatSource::connect_anonymous(channel.clone())
            else {
                warn!(%channel, "could not start chat feed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            };
            info!(%channel, "joining twitch chat");

            'session: loop {
                let started_at = Instant::now();

                enum FeedEvent {
                    Ended,
                    Swap(Option<String>),
                }
                // Scope the pump future so `source` frees for the swap path.
                let event = {
                    let mut pump_fut = std::pin::pin!(engine::pump(
                        live.clone(),
                        messages.clone(),
                        tx.clone(),
                        filters.clone(),
                        &mut source
                    ));
                    tokio::select! {
                        _ = &mut pump_fut => FeedEvent::Ended,
                        swapped = swap_rx.recv() => FeedEvent::Swap(swapped),
                    }
                };

                match event {
                    FeedEvent::Ended => {
                        if started_at.elapsed() > HEALTHY_FEED_RUNTIME {
                            backoff = INITIAL_BACKOFF;
                        }
                        warn!("chat feed ended");
                        break 'session;
                    }
                    FeedEvent::Swap(None) => break 'outer, // watcher gone: shutdown
                    FeedEvent::Swap(Some(new_channel)) => {
                        // Same connection keeps flowing; resume pumping.
                        swap_channel(&source, &channel, &new_channel, &messages, &tx);
                        continue 'session;
                    }
                }
            }

            warn!(sleep_secs = backoff.as_secs(), "restarting chat feed");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
    FeedHandle { swap_tx }
}

/// Part the old channel, join the new one, wipe history, tell clients.
/// The new login was validated before the reload was accepted, so join
/// failures here are logged and non-fatal (old feed keeps flowing).
fn swap_channel(
    source: &quiver_twitch::IrcChatSource,
    old: &str,
    new: &str,
    messages: &engine::SharedState,
    tx: &broadcast::Sender<String>,
) {
    let client = source.client();
    client.part(old.to_string());
    if let Err(e) = client.join(new.to_string()) {
        warn!(%new, %e, "join failed after channel swap");
        return;
    }
    let cleared = messages.lock().map(|mut m| m.clear()).unwrap_or_default();
    debug!(count = cleared.len(), "history cleared on channel swap");
    let _ = tx.send(r#"{"type":"clear"}"#.to_string());
    info!(from = %old, to = %new, "chat channel swapped");
}

// ---- badges ---------------------------------------------------------------

pub(crate) async fn load_badge_map(
    client_id: &str,
    client_secret: &str,
    channel_login: &str,
) -> HashMap<String, String> {
    let result = (|| async {
        let helix = quiver_twitch::HelixClient::new(client_id, client_secret)?;
        let broadcaster_id = match helix.user_id(channel_login).await {
            Ok(id) => Some(id),
            Err(e) => {
                warn!(
                    channel = %channel_login,
                    error = %e,
                    "channel id lookup failed — using global badges only"
                );
                None
            }
        };
        helix.badge_map(broadcaster_id.as_deref()).await
    })()
    .await;

    match result {
        Ok(map) => map,
        Err(e) => {
            warn!(error = %e, "badge lookup failed — widgets will show no badges");
            HashMap::new()
        }
    }
}

async fn load_badge_map_opt(
    client_id: Option<&str>,
    client_secret: Option<&str>,
    channel: &str,
) -> HashMap<String, String> {
    match (client_id, client_secret) {
        (Some(id), Some(secret)) => load_badge_map(id, secret, channel).await,
        _ => {
            info!("no twitch api credentials configured — badges disabled");
            HashMap::new()
        }
    }
}

// ---- wire -----------------------------------------------------------------

/// Meta block shared by snapshots and config-update frames.
pub(crate) fn meta_value(
    live: &LiveConfig,
    badges: &HashMap<String, String>,
    custom_badges: Option<&crate::badges::ResolvedCustomBadges>,
    custom_css: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "theme": {
            "font_size_px": live.theme.font_size_px,
            "max_messages": live.theme.max_messages,
            "overflow_mode": live.theme.overflow_mode,
        },
        "badges": badges,
        "custom_css": custom_css,
        "role_css": live.theme.role_css,
        "emote_flags": {
            "twitch": live.emotes.twitch,
            "unicode": live.emotes.unicode,
            "seventv": live.emotes.seventv,
            "bttv": live.emotes.bttv,
            "ffz": live.emotes.ffz,
        },
        "custom_badges": custom_badges,
    })
}

fn snapshot_frame(app: &AppState) -> Option<String> {
    let live = app.live.read().ok()?;
    let badges = app.badges.read().ok()?;
    let custom_badges = app.custom_badges.read().ok()?;
    let custom = custom_badges.as_ref().map(|c| &c.resolved);
    let css = app.custom_css.read().ok().and_then(|c| c.clone());
    let st = app.messages.lock().ok()?;
    let messages: Vec<&engine::RenderedMessage> = st.messages().collect();
    Some(
        serde_json::json!({
            "type": "snapshot",
            "messages": messages,
            "meta": meta_value(&live, &badges, custom, css.as_deref()),
        })
        .to_string(),
    )
}

// ---- handlers -------------------------------------------------------------

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let Some(snapshot) = snapshot_frame(&state) else {
        return;
    };
    if socket.send(Message::Text(snapshot.into())).await.is_err() {
        return;
    }

    let mut rx = state.tx.subscribe();
    loop {
        tokio::select! {
            res = rx.recv() => match res {
                Ok(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(missed = n, "ws client lagged, resyncing");
                    let Some(snapshot) = snapshot_frame(&state) else { return };
                    if socket.send(Message::Text(snapshot.into())).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            _ = state.ws_token.cancelled() => return,
            _ = socket.recv() => return,
        }
    }
}

/// Static files resolved PER REQUEST from the live widget_dist path —
/// hot-swapping `server.widget_dist` needs no router rebuild.
/// Serve a cached custom badge body by content hash. Content-addressed →
/// immutable → browser caches aggressively with no revalidation.
async fn badge_cache_handler(
    State(state): State<AppState>,
    AxumPath(hash): AxumPath<String>,
) -> Response {
    let guard = match state.custom_badges.read() {
        Ok(g) => g,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "cache lock").into_response(),
    };
    let Some(cache) = guard.as_ref() else {
        return (StatusCode::NOT_FOUND, "no badge cache").into_response();
    };
    match crate::badges::read_cached_file(cache, &hash) {
        Some((bytes, content_type)) => (
            [
                (header::CONTENT_TYPE, content_type.as_str()),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Serve a file:// custom badge by url-hash — pure passthrough from the
/// canonical path (no caching: user-controlled local files are read fresh).
async fn badge_file_handler(
    State(state): State<AppState>,
    AxumPath(hash): AxumPath<String>,
) -> Response {
    let guard = match state.custom_badges.read() {
        Ok(g) => g,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "cache lock").into_response(),
    };
    let Some(cache) = guard.as_ref() else {
        return (StatusCode::NOT_FOUND, "no badge cache").into_response();
    };
    let Some(path) = cache.file_badges.get(&hash) else {
        return (StatusCode::NOT_FOUND, "unknown file badge").into_response();
    };
    match std::fs::read(path) {
        Ok(bytes) => {
            let ct = crate::badges::sniff_content_type(&bytes);
            (
                [
                    (header::CONTENT_TYPE, ct),
                    (header::CACHE_CONTROL, "no-cache"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "file badge missing").into_response(),
    }
}

async fn static_fallback(State(state): State<AppState>, req: Request) -> Response {
    let dist = state.live.read().unwrap().widget_dist.clone();
    let Some(dist) = dist.filter(|d| d.is_dir()) else {
        return Html(PLACEHOLDER_HTML).into_response();
    };

    let requested = req.uri().path().trim_start_matches('/');
    let rel = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    let rel_path = PathBuf::from(rel);
    // Reject anything that is not a plain relative path (no `..`, absolute).
    if rel_path
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }

    let mut full = dist.join(&rel_path);
    if full.is_dir() {
        full.push("index.html");
    }
    match std::fs::read(&full) {
        Ok(bytes) => {
            // Cache-busting: bake the widget dir's newest mtime into the
            // asset URLs in index.html. A cache that ignores no-cache
            // still cannot serve a stale file across a DIFFERENT URL.
            let body = if full.ends_with("index.html") && dist.is_dir() {
                String::from_utf8_lossy(&bytes).replace(
                    "__QUIVER_VERSION__",
                    &quiver_widget_version(&dist).to_string(),
                )
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            };
            (
                [
                    (header::CONTENT_TYPE, mime_of(&full)),
                    // Reloads must always pull fresh bytes from disk — without
                    // this, Chromium heuristically caches main.js and a stale
                    // copy keeps rendering no matter how many reloads fire.
                    (header::CACHE_CONTROL, "no-cache"),
                    // Legacy CEF builds may not trust no-cache alone.
                    (header::PRAGMA, "no-cache"),
                    (header::EXPIRES, "0"),
                ],
                body,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Newest mtime (epoch millis) of any file under the widget dir
/// (recursive — assets live in subdirs like src/) — the cache-busting
/// version for asset URLs.
fn quiver_widget_version(dist: &Path) -> u64 {
    let mut newest = 0u64;
    let mut stack: Vec<PathBuf> = vec![dist.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_dir() {
                        stack.push(entry.path());
                    } else if meta.is_file()
                        && let Ok(t) = meta.modified()
                        && let Ok(ms) = t.duration_since(std::time::UNIX_EPOCH)
                    {
                        newest = newest.max(ms.as_millis() as u64);
                    }
                }
            }
        }
    }
    newest
}

fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Fetch CSS source bytes from a file:// or http(s):// URI — the same
/// scheme semantics as badges. file:// is a passthrough read (canonical
/// path); http(s) is fetched and stored by content hash in the badges
/// cache dir (dedup, reusable across badge/css swapping).
pub(crate) async fn fetch_css_bytes(
    uri: &str,
    http: &reqwest::Client,
    cache_dir: &std::path::Path,
) -> Result<Vec<u8>, String> {
    if uri.starts_with("file://") {
        let path_str = uri
            .strip_prefix("file://")
            .ok_or_else(|| format!("{uri}: invalid file URI"))?;
        let canonical = std::fs::canonicalize(path_str).map_err(|e| format!("{uri}: {e}"))?;
        return std::fs::read(&canonical).map_err(|e| format!("{uri}: {e}"));
    }
    if uri.starts_with("http://") || uri.starts_with("https://") {
        let resp = http
            .get(uri)
            .send()
            .await
            .map_err(|e| format!("{uri}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{uri}: HTTP {}", resp.status()));
        }
        let body = resp.bytes().await.map_err(|e| format!("{uri}: {e}"))?;
        let bytes = body.to_vec();
        // Persist by content hash in the cache dir (badge-style storage).
        let chash = crate::badges::sha256_hex_public(&bytes);
        let _ = std::fs::create_dir_all(cache_dir);
        let _ = std::fs::write(cache_dir.join(format!("css-{chash}.bin")), &bytes);
        return Ok(bytes);
    }
    Err(format!(
        "{uri}: unsupported scheme (expected file:// or http(s)://)"
    ))
}

// ---- resolved custom CSS -------------------------------------------------

/// Resolved custom CSS text shared between the meta builder and reloads.
pub(crate) type SharedCss = Arc<RwLock<Option<String>>>;

/// Resolve the configured custom CSS source to plain text (inline as-is,
/// URI via file passthrough / http fetch). Fails softly: returns None on
/// any error with a WARN — live rendering must never break on a bad CSS
/// source (same philosophy as badge skip-on-error).
pub(crate) async fn resolve_custom_css(
    source: &Option<crate::config::CustomCssSource>,
    http: &reqwest::Client,
) -> Option<String> {
    let Some(src) = source else { return None };
    let cache_dir = crate::badges::default_cache_dir();
    match src.resolve(http, &cache_dir).await {
        Ok(text) => {
            crate::config::report_css_lint(Some(&text), None);
            Some(text)
        }
        Err(e) => {
            tracing::warn!(error = %e, "custom_css source failed to resolve — ignoring");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EmotesConfig, ThemeConfig};

    fn live_with(custom: Option<&str>, role: Option<HashMap<String, String>>) -> LiveConfig {
        LiveConfig {
            listen: "127.0.0.1:1".into(),
            widget_dist: None,
            channel: "chan".into(),
            creds: None,
            filters: crate::config::FiltersConfig::default(),
            badges: None,
            theme: ThemeConfig {
                font_size_px: 18,
                max_messages: 30,
                message_lifetime_secs: 60,
                custom_css: custom.map(|c| crate::config::CustomCssSource::Inline(c.to_string())),
                role_css: role,
                overflow_mode: crate::config::OverflowMode::Prune,
            },
            emotes: EmotesConfig::default(),
        }
    }

    /// Ground truth by hand: meta mirrors exactly what was passed in.
    #[test]
    fn meta_carries_custom_and_role_css() {
        let mut roles = HashMap::new();
        roles.insert("moderator".to_string(), ".msg{}".to_string());
        let live = live_with(Some("/*c*/"), Some(roles));
        let m = meta_value(&live, &HashMap::new(), None, Some("/*c*/"));

        assert_eq!(m["custom_css"], "/*c*/");
        assert_eq!(m["role_css"]["moderator"], ".msg{}");
        assert_eq!(m["theme"]["font_size_px"], 18);
    }

    #[test]
    fn meta_omits_unset_css_fields_as_null() {
        let live = live_with(None, None);
        let m = meta_value(&live, &HashMap::new(), None, None);
        assert!(m["custom_css"].is_null());
        assert!(m["role_css"].is_null());
    }
}
