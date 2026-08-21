//! Widget host: localhost HTTP server serving the widget frontend plus a
//! WebSocket endpoint pushing live chat state to the browser page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::services::ServeDir;
use tracing::{info, warn};

use crate::config::{ChatConfig, ThemeConfig};
use crate::engine;

const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A feed that lived this long counts as healthy; reset backoff after it.
const HEALTHY_FEED_RUNTIME: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct AppState {
    messages: engine::SharedState,
    tx: broadcast::Sender<String>,
    theme: ThemeConfig,
    /// `"set_id/version"` -> image URL. Empty when credentials are absent.
    badges: Arc<HashMap<String, String>>,
}

const PLACEHOLDER_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>quiver-chat</title></head>
<body style="font-family: sans-serif; background:#18181b; color:#efeff1; padding:2em">
<h1>quiver-chat is running</h1>
<p>No widget frontend is being served. Point <code>server.widget_dist</code>
in your RON config at the widget directory:</p>
<pre>crates/quiver-chat/widget</pre>
<p>and restart. The WebSocket feed is live at <code>/ws</code> regardless.</p>
</body></html>
"#;

/// Run the chat tool until Ctrl-C.
pub async fn run(cfg: ChatConfig) -> anyhow::Result<()> {
    let (tx, _rx) = broadcast::channel::<String>(256);
    let state = Arc::new(Mutex::new(engine::EngineState::new(
        cfg.theme.max_messages as usize,
    )));

    let badges = load_badges(&cfg).await;
    info!(badge_count = badges.len(), "badge map ready");

    // Supervised feed: reconnects forever with capped backoff. Failure to
    // connect must not kill the widget server — OBS keeps rendering.
    spawn_feed(cfg.clone(), state.clone(), tx.clone());

    let app_state = AppState {
        messages: state,
        tx,
        theme: cfg.theme.clone(),
        badges: Arc::new(badges),
    };

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(|| async { "ok" }))
        .with_state(app_state);

    app = match &cfg.server.widget_dist {
        Some(dir) if dir.is_dir() => {
            info!(dir = %dir.display(), "serving widget frontend from disk");
            app.fallback_service(ServeDir::new(dir))
        }
        Some(dir) => {
            warn!(
                dir = %dir.display(),
                "server.widget_dist does not exist — serving placeholder page"
            );
            app.fallback(get(|| async { PLACEHOLDER_PAGE }))
        }
        None => {
            info!("no server.widget_dist configured — serving placeholder page");
            app.fallback(get(|| async { PLACEHOLDER_PAGE }))
        }
    };

    let listener = TcpListener::bind(&cfg.server.listen).await?;
    info!(addr = %cfg.server.listen, "widget server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("widget server stopped");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}

/// Fetch global + channel badge URLs when credentials are configured.
/// Absent or failing credentials degrade gracefully to an empty map.
async fn load_badges(cfg: &ChatConfig) -> HashMap<String, String> {
    let (Some(client_id), Some(client_secret)) = (&cfg.twitch.client_id, &cfg.twitch.client_secret)
    else {
        info!("no twitch api credentials configured — badges disabled");
        return HashMap::new();
    };

    let result = (|| async {
        let helix = quiver_twitch::HelixClient::new(client_id.clone(), client_secret.clone())?;
        // Channel id may be unresolvable while global badges still work.
        let broadcaster_id = match helix.user_id(&cfg.twitch.channel).await {
            Ok(id) => Some(id),
            Err(e) => {
                warn!(
                    channel = %cfg.twitch.channel,
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

fn spawn_feed(cfg: ChatConfig, state: engine::SharedState, tx: broadcast::Sender<String>) {
    let lifetime = Duration::from_secs(cfg.theme.message_lifetime_secs);
    tokio::spawn(async move {
        let mut backoff = INITIAL_BACKOFF;
        loop {
            match quiver_twitch::IrcChatSource::connect_anonymous(cfg.twitch.channel.clone()) {
                Ok(source) => {
                    info!(channel = %cfg.twitch.channel, "joining twitch chat");
                    let started_at = Instant::now();
                    engine::pump(source, state.clone(), tx.clone(), lifetime).await;
                    if started_at.elapsed() > HEALTHY_FEED_RUNTIME {
                        backoff = INITIAL_BACKOFF;
                    }
                    warn!("chat feed ended");
                }
                Err(e) => warn!(error = %e, "could not start chat feed"),
            }
            warn!(sleep_secs = backoff.as_secs(), "restarting chat feed");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

/// Full state for a newly connected client: history + theme + badge urls.
fn snapshot_frame(app: &AppState) -> Option<String> {
    let st = app.messages.lock().ok()?;
    let messages: Vec<&engine::RenderedMessage> = st.messages().collect();
    Some(
        serde_json::json!({
            "type": "snapshot",
            "messages": messages,
            "meta": {
                "theme": {
                    "font_size_px": app.theme.font_size_px,
                    "max_messages": app.theme.max_messages,
                },
                "badges": *app.badges,
            },
        })
        .to_string(),
    )
}

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // 1. Snapshot of current history + meta.
    let Some(snapshot) = snapshot_frame(&state) else {
        return;
    };
    if socket.send(Message::Text(snapshot.into())).await.is_err() {
        return;
    }

    // 2. Live updates.
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
                    // Client missed n frames — resync with a fresh snapshot.
                    tracing::debug!(missed = n, "ws client lagged, resyncing");
                    let Some(snapshot) = snapshot_frame(&state) else { return };
                    if socket.send(Message::Text(snapshot.into())).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            _ = socket.recv() => {
                // Client closed or sent a frame; skeleton treats both as done.
                return;
            }
        }
    }
}
