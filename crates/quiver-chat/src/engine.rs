//! Render engine: turns chat events into widget render state and
//! broadcasts wire messages to connected browser clients.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use quiver_twitch::{ChatMessage, Event, IrcChatSource};
use serde::Serialize;
use tracing::warn;

/// One message as rendered by the widget.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderedMessage {
    pub id: String,
    pub user_login: String,
    pub display_name: String,
    pub color: Option<String>,
    pub text: String,
}

impl From<ChatMessage> for RenderedMessage {
    fn from(cm: ChatMessage) -> Self {
        Self {
            id: cm.id,
            user_login: cm.user_login,
            display_name: cm.display_name,
            color: cm.color,
            text: cm.text,
        }
    }
}

/// Bounded message history — the snapshot new WS clients receive.
#[derive(Debug)]
pub struct EngineState {
    messages: VecDeque<RenderedMessage>,
    max_messages: usize,
}

impl EngineState {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: VecDeque::new(),
            max_messages,
        }
    }

    pub fn push(&mut self, msg: RenderedMessage) {
        while self.messages.len() >= self.max_messages {
            self.messages.pop_front();
        }
        self.messages.push_back(msg);
    }

    pub fn messages(&self) -> impl Iterator<Item = &RenderedMessage> {
        self.messages.iter()
    }
}

/// Shared handle used by the server and the pump task.
pub type SharedState = Arc<Mutex<EngineState>>;

/// Consume events from `source` forever, updating `state` and broadcasting
/// wire JSON on `tx`. Ends when the source ends (client shut down).
pub async fn pump(
    mut source: IrcChatSource,
    state: SharedState,
    tx: tokio::sync::broadcast::Sender<String>,
) {
    loop {
        let Some(event) = source.next_event().await else {
            warn!("chat feed ended");
            return;
        };
        if let Event::ChatMessage(cm) = event {
            let rendered = RenderedMessage::from(cm);
            if let Ok(mut st) = state.lock() {
                st.push(rendered.clone());
            }
            let wire = serde_json::json!({ "type": "message", "message": rendered });
            let _ = tx.send(wire.to_string());
        }
    }
}
