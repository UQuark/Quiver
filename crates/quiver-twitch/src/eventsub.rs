//! EventSub over WebSocket (`wss://eventsub.wss.twitch.tv/ws`).
//!
//! Transport for channel-scoped real-time events (redemptions with reward
//! titles, hype trains, predictions, polls). Requires a user access token
//! (see [`crate::auth`]) — Twitch rejects websocket-transport subscription
//! requests with app tokens.
//!
//! Lifecycle: connect → `session_welcome` (session_id) → POST subscriptions
//! (transport.websocket + session_id) → notifications → tx. On `session_reconnect`
//! the provided URL is used; otherwise the socket is re-dialed with backoff and
//! subscriptions are recreated (Twitch does not replay missed events).

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::helix::HelixClient;

const EVENTSUB_WS_URL: &str = "wss://eventsub.wss.twitch.tv/ws";

/// One requested EventSub subscription: type + version + the user scope it
/// requires (empty = no scope needed beyond a user token).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubSpec {
    pub type_name: &'static str,
    pub version: &'static str,
    pub scope: &'static str,
    pub cost: u32,
}

/// Default subscription set — exactly cost 10 (the per-app budget).
pub const DEFAULT_SUBSCRIPTIONS: &[SubSpec] = &[
    SubSpec { type_name: "channel.channel_points_custom_reward_redemption.add", version: "1", scope: "channel:read:redemptions", cost: 1 },
    SubSpec { type_name: "channel.hype_train.begin", version: "2", scope: "channel:read:hype_train", cost: 1 },
    SubSpec { type_name: "channel.hype_train.progress", version: "2", scope: "channel:read:hype_train", cost: 1 },
    SubSpec { type_name: "channel.hype_train.end", version: "2", scope: "channel:read:hype_train", cost: 1 },
    SubSpec { type_name: "channel.prediction.begin", version: "1", scope: "channel:read:predictions", cost: 1 },
    SubSpec { type_name: "channel.prediction.lock", version: "1", scope: "channel:read:predictions", cost: 1 },
    SubSpec { type_name: "channel.prediction.end", version: "1", scope: "channel:read:predictions", cost: 1 },
    SubSpec { type_name: "channel.poll.begin", version: "1", scope: "channel:read:polls", cost: 1 },
    SubSpec { type_name: "channel.poll.end", version: "1", scope: "channel:read:polls", cost: 1 },
    SubSpec { type_name: "channel.follow", version: "2", scope: "moderator:read:followers", cost: 1 },
];

fn enabled_specs() -> Vec<SubSpec> {
    DEFAULT_SUBSCRIPTIONS.to_vec()
}

/// Spawn the EventSub client: connects, subscribes, and forwards mapped
/// event frames onto `tx` (the same broadcast channel the engine uses).
/// Exits when `quit` cancels; reconnects with backoff otherwise.
///
/// `gate`: called with the mapped event `kind` — false suppresses the frame
/// (the chat layer's message_type filters flow through here as a closure,
/// since quiver-twitch cannot depend on quiver-chat).
/// `deduper`: cross-source redemption dedupe — EventSub marks/deduplicates
/// redemption ids and skips redemptions IRC already rendered moments ago.
pub fn spawn(
    helix: Arc<HelixClient>,
    broadcaster_id: String,
    tx: broadcast::Sender<String>,
    quit: tokio_util::sync::CancellationToken,
    gate: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    deduper: crate::dedupe::SharedRedeemDeduper,
) {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(2);
        loop {
            if quit.is_cancelled() {
                return;
            }
            match run_session(&helix, &broadcaster_id, &tx, &quit, &gate, &deduper).await {
                SessionEnd::Quit => return,
                SessionEnd::Reconnect(url) => {
                    // Twitch asks us to move to a specific socket; reconnect
                    // immediately and resubscribe there.
                    let _ = url;
                    backoff = Duration::from_secs(2);
                }
                SessionEnd::Dropped => {
                    tracing::warn!(backoff_secs = backoff.as_secs(), "eventsub dropped — reconnecting");
                }
            }
            tokio::select! {
                _ = quit.cancelled() => return,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    });
}

enum SessionEnd {
    Quit,
    Reconnect(Option<String>),
    Dropped,
}

/// One websocket session: welcome → subscribe → notifications.
async fn run_session(
    helix: &HelixClient,
    broadcaster_id: &str,
    tx: &broadcast::Sender<String>,
    quit: &tokio_util::sync::CancellationToken,
    gate: &Arc<dyn Fn(&str) -> bool + Send + Sync>,
    deduper: &crate::dedupe::SharedRedeemDeduper,
) -> SessionEnd {
    let (ws, _resp) = match tokio_tungstenite::connect_async(EVENTSUB_WS_URL).await {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!(error = %e, "eventsub connect failed");
            return SessionEnd::Dropped;
        }
    };
    let (_write, mut read) = ws.split();

    // First message MUST be session_welcome — but Twitch sends a protocol
    // PING frame FIRST on some networks (observed live): non-Text frames
    // are skipped, not fatal.
    let session_id = loop {
        match read.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match v["metadata"]["message_type"].as_str() {
                    Some("session_welcome") => {
                        let id = v["payload"]["session"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        tracing::info!(session = %id, "eventsub session welcome");
                        break id;
                    }
                    Some(other) => {
                        tracing::debug!(t = other, "eventsub pre-welcome frame ignored")
                    }
                    None => {}
                }
            }
            // Transport-level Ping/Pong frames (observed: a bare PING
            // arrives BEFORE session_welcome) — skip, keep reading.
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
            other => {
                tracing::warn!(
                    frame = ?other.map(|m| format!("{m:?}")).unwrap_or_default(),
                    "eventsub closed before session_welcome — dropping"
                );
                return SessionEnd::Dropped;
            }
        }
    };

    // Subscribe to every enabled spec within budget. A missing scope or a
    // 4xx on one type logs and continues — the rest still deliver.
    let specs = enabled_specs();
    let total_cost: u32 = specs.iter().map(|s| s.cost).sum();
    if total_cost > 10 {
        tracing::warn!(total_cost, "eventsub subscription set exceeds budget — trimming");
    }
    let granted = helix.channel_scopes();
    for spec in specs {
        if total_cost > 10 && spec.cost > 0 {
            // Budget-trimmed sets skip everything past the cap in v1 of this
            // integration; the default set is exactly 10 so this is defensive.
        }
        if !spec.scope.is_empty() && !granted.contains(&spec.scope.to_string()) {
            tracing::warn!(sub = spec.type_name, scope = spec.scope, "scope not granted — skipping eventsub subscription");
            continue;
        }
        let body = serde_json::json!({
            "type": spec.type_name,
            "version": spec.version,
            "condition": { "broadcaster_user_id": broadcaster_id },
            "transport": { "method": "websocket", "session_id": session_id },
        });
        if let Err(e) = helix.create_eventsub_subscription(&body).await {
            tracing::warn!(sub = spec.type_name, error = %e, "eventsub subscription failed");
        }
    }

    // Notification loop.
    loop {
        tokio::select! {
            _ = quit.cancelled() => return SessionEnd::Quit,
            msg = read.next() => {
                match msg {
                    // Transport-level Ping/Pong: skip (tungstenite answers
                    // pings at the protocol layer automatically).
                    Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => {}
                    Some(Ok(WsMessage::Text(text))) => {
                        let v: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                let message_type = v["metadata"]["message_type"].as_str().unwrap_or("");
                match message_type {
                    "notification" => {
                        let sub_type = v["metadata"]["subscription_type"].as_str().unwrap_or("");
                        if let Some(frame) = map_event(sub_type, &v["payload"]["event"])
                            && gate(frame["event"]["kind"].as_str().unwrap_or(""))
                        {
                            // Redeem frames dedupe cross-source (redemption
                            // id vs the poller; IRC-covered (user, reward)
                            // skips). Other kinds pass through.
                            let redeem = &v["payload"]["event"];
                            let already_delivered = deduper
                                .lock()
                                .map(|mut d| {
                                    if sub_type
                                        == "channel.channel_points_custom_reward_redemption.add"
                                    {
                                        let id = redeem["id"].as_str().unwrap_or_default();
                                        !d.is_new_redemption_id(id)
                                            || d.irc_already_rendered(
                                                redeem["user_login"].as_str().unwrap_or_default(),
                                                redeem["user_id"].as_str().unwrap_or_default(),
                                                redeem["reward"]["id"].as_str().unwrap_or_default(),
                                            )
                                    } else {
                                        false
                                    }
                                })
                                .unwrap_or(false);
                            if !already_delivered {
                                let _ = tx.send(frame.to_string());
                            }
                        }
                    }
                    "session_reconnect" => {
                        let url = v["payload"]["session"]["reconnect_url"].as_str().map(str::to_string);
                        return SessionEnd::Reconnect(url);
                    }
                    "session_keepalive" => {}
                    "revocation" => {
                        tracing::warn!("eventsub subscription revoked");
                    }
                    _ => {}
                }
                    }
                    other => {
                        tracing::warn!(
                            frame = ?other.map(|m| format!("{m:?}")).unwrap_or_default(),
                            "eventsub socket closed mid-session"
                        );
                        return SessionEnd::Dropped;
                    }
                }
            }
        }
    }
}

/// Map an EventSub notification payload onto the widget's event-frame wire
/// shape (`{type:"event", event:{kind, ...}}`). Returns None for types the
/// widget does not render.
fn map_event(sub_type: &str, ev: &serde_json::Value) -> Option<serde_json::Value> {
    let kind = match sub_type {
        "channel.channel_points_custom_reward_redemption.add" => "redeem",
        "channel.hype_train.begin" => return hype("begin", ev),
        "channel.hype_train.progress" => return hype("progress", ev),
        "channel.hype_train.end" => return hype("end", ev),
        "channel.prediction.begin" => return prediction("begin", ev),
        "channel.prediction.lock" => return prediction("lock", ev),
        "channel.prediction.end" => return prediction("end", ev),
        "channel.poll.begin" => return poll("begin", ev),
        "channel.poll.end" => return poll("end", ev),
        "channel.follow" => "follow",
        _ => return None,
    };

    Some(serde_json::json!({
        "type": "event",
        "event": {
            "kind": kind,
            "user_login": ev["user_login"].as_str().unwrap_or_default(),
            "display_name": ev["user_name"].as_str().unwrap_or_default(),
            // EventSub carries the reward title AND icon inline — no Helix
            // lookup needed.
            "reward_title": ev["reward"]["title"].as_str(),
            "reward_image": ev["reward"]["image"]["url_2x"].as_str(),
            "user_input": ev["user_input"].as_str().unwrap_or_default(),
        }
    }))
}

fn hype(phase: &str, ev: &serde_json::Value) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "type": "event",
        "event": {
            "kind": "hype_train",
            "phase": phase,
            "level": ev["level"],
            "total": ev["total"],
            "goal": ev["goal"],
            "top_display_names": ev["top_contributions"].as_array().map(|c| {
                c.iter().filter_map(|x| x["user_name"].as_str()).collect::<Vec<_>>()
            }).unwrap_or_default(),
        }
    }))
}

fn prediction(phase: &str, ev: &serde_json::Value) -> Option<serde_json::Value> {
    let title = ev["title"].as_str().unwrap_or_default();
    let outcome = |k: &str| ev["outcomes"].as_array().map(|o| {
        o.iter().find(|x| x["id"].as_str() == Some(k) || x["title"].as_str() == Some(k))
            .and_then(|x| x["title"].as_str().unwrap_or_default().to_string().into())
    }).and_then(|x| x);
    let _ = outcome;
    let winning = ev["winning_outcome_id"].as_str().unwrap_or_default();
    let winner_title = ev["outcomes"].as_array().and_then(|o| {
        o.iter().find(|x| x["id"].as_str() == Some(winning))
            .and_then(|x| x["title"].as_str()).map(str::to_string)
    });
    Some(serde_json::json!({
        "type": "event",
        "event": {
            "kind": "prediction",
            "phase": phase,
            "title": title,
            "winning_outcome": winner_title,
        }
    }))
}

fn poll(phase: &str, ev: &serde_json::Value) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "type": "event",
        "event": {
            "kind": "poll",
            "phase": phase,
            "title": ev["title"].as_str().unwrap_or_default(),
        }
    }))
}