//! Render engine: turns chat events into widget render state and
//! broadcasts wire messages to connected browser clients.
//!
//! The engine owns the message LIFECYCLE (arrival, count cap, time-based
//! expiry) and the widget stays a dumb renderer: every state change is
//! announced as a wire frame so late joiners get correct snapshots.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use quiver_twitch::{Badge, ChatMessage, EmoteRef, Event, IrcChatSource};
use serde::Serialize;
use tracing::warn;

/// Badge reference on the wire (`id`/`version` pair).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WireBadge {
    pub id: String,
    pub version: String,
}

impl From<&Badge> for WireBadge {
    fn from(b: &Badge) -> Self {
        Self {
            id: b.id.clone(),
            version: b.version.clone(),
        }
    }
}

/// Emote position on the wire. `start`/`end` are char indices into
/// `text`, end EXCLUSIVE (normalized by quiver-twitch).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WireEmote {
    pub id: String,
    pub start: usize,
    pub end: usize,
}

impl From<&EmoteRef> for WireEmote {
    fn from(e: &EmoteRef) -> Self {
        Self {
            id: e.id.clone(),
            start: e.start,
            end: e.end,
        }
    }
}

/// One message as rendered by the widget.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RenderedMessage {
    pub id: String,
    pub user_login: String,
    pub display_name: String,
    pub color: Option<String>,
    /// `/me` action message — rendered italic in the sender's color.
    pub is_action: bool,
    pub text: String,
    pub emotes: Vec<WireEmote>,
    pub badges: Vec<WireBadge>,
}

impl From<ChatMessage> for RenderedMessage {
    fn from(cm: ChatMessage) -> Self {
        Self {
            id: cm.id,
            user_login: cm.user_login,
            display_name: cm.display_name,
            color: cm.color,
            is_action: false, // twitch-irc strips /me markers; see TODO below
            text: cm.text.clone(),
            emotes: cm.emotes.iter().map(WireEmote::from).collect(),
            badges: cm.badges.iter().map(WireBadge::from).collect(),
        }
    }
}

// TODO(lifetime-followup): map PrivmsgMessage.is_action once it is carried
// through quiver-twitch's ChatMessage instead of dropped at the facade.

/// Bounded, age-aware message history.
#[derive(Debug)]
struct Enqueued {
    at: Instant,
    msg: RenderedMessage,
}

/// Shared handle used by the server and the pump task.
#[derive(Debug)]
pub struct EngineState {
    messages: VecDeque<Enqueued>,
    max_messages: usize,
}

impl EngineState {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: VecDeque::new(),
            max_messages,
        }
    }

    /// Append a message; returns ids EVICTED by the count cap (oldest
    /// first) so they can be announced to clients as expired.
    pub fn push(&mut self, msg: RenderedMessage) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.messages.len() >= self.max_messages {
            if let Some(old) = self.messages.pop_front() {
                evicted.push(old.msg.id);
            }
        }
        self.messages.push_back(Enqueued {
            at: Instant::now(),
            msg,
        });
        evicted
    }

    /// Drop messages older than `lifetime`; returns their ids oldest-first.
    pub fn sweep_expired(&mut self, now: Instant, lifetime: Duration) -> Vec<String> {
        let mut expired = Vec::new();
        while let Some(front) = self.messages.front() {
            if now.duration_since(front.at) >= lifetime {
                let old = self.messages.pop_front().expect("front existed");
                expired.push(old.msg.id);
            } else {
                break;
            }
        }
        expired
    }

    pub fn messages(&self) -> impl Iterator<Item = &RenderedMessage> {
        self.messages.iter().map(|e| &e.msg)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.messages.len()
    }
}

/// Shared handle used by the server and the pump task.
pub type SharedState = Arc<Mutex<EngineState>>;

/// Consume events from `source`, announce every state change on `tx`,
/// sweep expired messages every second. Returns when the source ends.
pub async fn pump(
    mut source: IrcChatSource,
    state: SharedState,
    tx: tokio::sync::broadcast::Sender<String>,
    lifetime: Duration,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            event = source.next_event() => match event {
                None => {
                    warn!("chat feed ended");
                    return;
                }
                Some(Event::ChatMessage(cm)) => {
                    let rendered = RenderedMessage::from(cm);
                    let evicted = state.lock().map(|mut st| st.push(rendered.clone())).unwrap_or_default();
                    let frame = serde_json::json!({ "type": "message", "message": rendered });
                    let _ = tx.send(frame.to_string());
                    if !evicted.is_empty() {
                        let frame = serde_json::json!({ "type": "expire", "ids": evicted });
                        let _ = tx.send(frame.to_string());
                    }
                }
                Some(_) => {}
            },
            _ = ticker.tick() => {
                let expired = state
                    .lock()
                    .map(|mut st| st.sweep_expired(Instant::now(), lifetime))
                    .unwrap_or_default();
                if !expired.is_empty() {
                    let frame = serde_json::json!({ "type": "expire", "ids": expired });
                    let _ = tx.send(frame.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str) -> RenderedMessage {
        RenderedMessage {
            id: id.to_string(),
            user_login: "u".into(),
            display_name: "U".into(),
            color: None,
            is_action: false,
            text: "t".into(),
            emotes: vec![],
            badges: vec![],
        }
    }

    /// Ground truth BY HAND:
    /// ages 130s / 65s / 5s with lifetime 60s → first two expired, last stays.
    #[test]
    fn sweep_expires_exactly_the_old_ones() {
        let now = Instant::now();
        let mut st = EngineState::new(10);
        for (id, age) in [("a", 130), ("b", 65), ("c", 5)] {
            st.push(msg(id));
            if let Some(front) = st.messages.back_mut() {
                front.at = now - Duration::from_secs(age);
            }
        }
        assert_eq!(st.len(), 3);

        let expired = st.sweep_expired(now, Duration::from_secs(60));
        assert_eq!(expired, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(st.len(), 1);
        assert_eq!(st.messages().next().map(|m| m.id.as_str()), Some("c"));
    }

    /// Ground truth BY HAND: max=2, pushing a third evicts exactly the first.
    #[test]
    fn push_evicts_oldest_beyond_cap() {
        let mut st = EngineState::new(2);
        assert!(st.push(msg("one")).is_empty());
        assert!(st.push(msg("two")).is_empty());
        let evicted = st.push(msg("three"));
        assert_eq!(evicted, vec!["one".to_string()]);
        assert_eq!(st.len(), 2);
        let ids: Vec<_> = st.messages().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["two", "three"]);
    }

    /// Ground truth BY HAND: fresh message within lifetime never swept.
    #[test]
    fn fresh_message_survives_sweep() {
        let now = Instant::now();
        let mut st = EngineState::new(5);
        st.push(msg("fresh"));
        let expired = st.sweep_expired(now, Duration::from_secs(60));
        assert!(expired.is_empty());
        assert_eq!(st.len(), 1);
    }

    /// Ground truth BY HAND: mapping carries emotes/badges positions over.
    #[test]
    fn chat_message_maps_fully_to_rendered() {
        let cm = ChatMessage {
            id: "m1".into(),
            channel_login: "chan".into(),
            user_login: "user".into(),
            display_name: "User".into(),
            color: Some("#123456".into()),
            badges: vec![Badge {
                id: "moderator".into(),
                version: "1".into(),
            }],
            emotes: vec![EmoteRef {
                id: "25".into(),
                start: 4,
                end: 9,
            }],
            text: "hey Kappa world".into(),
        };
        let r = RenderedMessage::from(cm);
        assert_eq!(
            r.emotes,
            vec![WireEmote {
                id: "25".into(),
                start: 4,
                end: 9
            }]
        );
        assert_eq!(
            r.badges,
            vec![WireBadge {
                id: "moderator".into(),
                version: "1".into()
            }]
        );
        assert_eq!(r.text, "hey Kappa world");
        assert_eq!(r.color.as_deref(), Some("#123456"));
    }
}
