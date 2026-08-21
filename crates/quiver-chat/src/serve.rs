//! Widget host: localhost HTTP server serving the widget frontend plus a
//! WebSocket endpoint pushing live chat state to the browser page.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::services::ServeDir;
use tracing::{info, warn};

use crate::config::ChatConfig;
use crate::engine;

#[derive(Clone)]
struct AppState {
    messages: engine::SharedState,
    tx: broadcast::Sender<String>,
}

const PLACEHOLDER_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>quiver-chat</title></head>
<body style="font-family: sans-serif; background:#18181b; color:#efeff1; padding:2em">
<h1>quiver-chat is running</h1>
<p>No widget frontend is being served. Build it:</p>
<pre>cd crates/quiver-chat/widget &amp;&amp; npm install &amp;&amp; npm run build</pre>
<p>Then point <code>server.widget_dist</code> in your RON config at
<code>crates/quiver-chat/widget/dist</code> and restart.</p>
<p>WebSocket feed is live at <code>/ws</code> regardless.</p>
</body></html>
"#;

/// Run the chat tool until Ctrl-C.
pub async fn run(cfg: ChatConfig) -> anyhow::Result<()> {
    let (tx, _rx) = broadcast::channel::<String>(256);
    let state = Arc::new(Mutex::new(engine::EngineState::new(
        cfg.theme.max_messages as usize,
    )));

    // Chat feed. Failure to connect must not kill the widget server —
    // OBS keeps rendering; feed just stays empty until restart.
    match quiver_twitch::IrcChatSource::connect_anonymous(cfg.twitch.channel.clone()) {
        Ok(source) => {
            info!(channel = %cfg.twitch.channel, "joining twitch chat");
            tokio::spawn(engine::pump(source, state.clone(), tx.clone()));
        }
        Err(e) => warn!("chat feed unavailable (widget will show no messages): {e}"),
    }

    let app_state = AppState {
        messages: state,
        tx,
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
    axum::serve(listener, app).await?;

    Ok(())
}

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // 1. Snapshot of current history.
    let snapshot = {
        let Ok(st) = state.messages.lock() else {
            return;
        };
        serde_json::json!({ "type": "snapshot", "messages": st.messages().collect::<Vec<_>>() })
    };
    if socket
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
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
                    // Client missed n messages — resync with a fresh snapshot.
                    tracing::debug!(missed = n, "ws client lagged, resyncing");
                    let snapshot = {
                        let Ok(st) = state.messages.lock() else { return };
                        serde_json::json!({ "type": "snapshot", "messages": st.messages().collect::<Vec<_>>() })
                    };
                    if socket.send(Message::Text(snapshot.to_string().into())).await.is_err() {
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
