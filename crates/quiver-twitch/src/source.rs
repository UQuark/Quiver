//! Chat source facade over `twitch-irc`.
//!
//! The library owns the hard parts: TLS, CAP negotiation, reconnect with
//! backoff, connection pooling, rate limiting, PING/PONG. This module maps
//! its message types onto Quiver's own [`Event`] model so tool crates never
//! depend on `twitch-irc` types directly.

use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::{PrivmsgMessage, ServerMessage};
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};

use crate::error::TwitchError;
use crate::events::{Badge, ChatMessage, EmoteRef, Event};

type Client = TwitchIRCClient<SecureTCPTransport, StaticLoginCredentials>;

/// Read-only anonymous chat source for one channel.
pub struct IrcChatSource {
    incoming: tokio::sync::mpsc::UnboundedReceiver<ServerMessage>,
    /// Kept alive on purpose: dropping the last client handle shuts the
    /// client (and its connections) down.
    _client: Client,
}

impl IrcChatSource {
    /// Offline validation for a channel login (no network).
    /// Used by hot reload to reject bad swap targets early.
    pub fn validate_channel_login(login: &str) -> Result<(), twitch_irc::validate::Error> {
        twitch_irc::validate::validate_login(login)
    }

    /// Clone of the underlying client handle — used for live channel
    /// swaps (`part`+`join`) without touching the event stream.
    pub fn client(&self) -> Client {
        self._client.clone()
    }

    /// Connect anonymously (justinfan — read-only, cannot send) and join
    /// `channel_login`. Connection is lazy: background tasks start once
    /// events are consumed.
    pub fn connect_anonymous(channel_login: String) -> Result<Self, TwitchError> {
        let config = ClientConfig::default();
        let (incoming, client) = Client::new(config);
        client.join(channel_login)?;
        Ok(Self {
            incoming,
            _client: client,
        })
    }

    /// Next event. Returns None when the stream has ended (client shut down).
    pub async fn next_event(&mut self) -> Option<Event> {
        loop {
            match self.incoming.recv().await? {
                ServerMessage::Privmsg(pm) => return Some(Event::ChatMessage(map_privmsg(pm))),
                ServerMessage::Ping(ping) => {
                    // PingMessage keeps only the raw IRC frame; the token is
                    // its (optional) first parameter.
                    let token = ping.source.params.first().cloned().unwrap_or_default();
                    return Some(Event::Ping(token));
                }
                _ => continue, // unmodeled message types for now
            }
        }
    }
}

/// Map a library PRIVMSG onto Quiver's own model.
fn map_privmsg(pm: PrivmsgMessage) -> ChatMessage {
    ChatMessage {
        id: pm.message_id,
        channel_login: pm.channel_login,
        user_login: pm.sender.login,
        display_name: pm.sender.name,
        color: pm.name_color.map(|c| c.to_string()),
        badges: pm
            .badges
            .into_iter()
            .map(|b| Badge {
                id: b.name,
                version: b.version,
            })
            .collect(),
        emotes: pm
            .emotes
            .into_iter()
            .map(|e| EmoteRef {
                id: e.id,
                start: e.char_range.start,
                end: e.char_range.end,
            })
            .collect(),
        text: pm.message_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryFrom;
    use twitch_irc::message::IRCMessage;

    // Ground truth computed BY HAND:
    // text "hey Kappa world" → h0 e1 y2 ␣3 K4 a5 p6 p7 a8 ␣9 w10...
    // "Kappa" occupies chars 4..=8 inclusive → Range 4..9 exclusive.
    const LINE: &str = "@badge-info=;badges=broadcaster/1,subscriber/6;color=#FF0000;display-name=QuiverDev;emotes=25:4-8;id=abc-123;login=quiverdev;room-id=11148817;tmi-sent-ts=1594545155039;turbo=0;user-id=29803735 :quiverdev!quiverdev@quiverdev.tmi.twitch.tv PRIVMSG #quiverdev :hey Kappa world";

    fn parse_line_to_privmsg(line: &str) -> PrivmsgMessage {
        let irc_message = IRCMessage::parse(line).expect("line must parse as IRC");
        PrivmsgMessage::try_from(irc_message).expect("line must parse as PRIVMSG")
    }

    #[test]
    fn maps_full_privmsg_with_hand_verified_ground_truth() {
        let cm = map_privmsg(parse_line_to_privmsg(LINE));

        assert_eq!(cm.id, "abc-123");
        assert_eq!(cm.channel_login, "quiverdev");
        assert_eq!(cm.user_login, "quiverdev");
        assert_eq!(cm.display_name, "QuiverDev");
        assert_eq!(cm.color.as_deref(), Some("#FF0000"));
        assert_eq!(
            cm.badges,
            vec![
                Badge {
                    id: "broadcaster".to_string(),
                    version: "1".to_string()
                },
                Badge {
                    id: "subscriber".to_string(),
                    version: "6".to_string()
                },
            ]
        );
        assert_eq!(
            cm.emotes,
            vec![EmoteRef {
                id: "25".to_string(),
                start: 4,
                end: 9
            }]
        );
        assert_eq!(cm.text, "hey Kappa world");
    }

    #[test]
    fn emote_range_slices_the_actual_text() {
        let cm = map_privmsg(parse_line_to_privmsg(LINE));
        let e = &cm.emotes[0];
        assert_eq!(&cm.text[e.start..e.end], "Kappa");
    }

    #[test]
    fn maps_privmsg_without_color_badges_or_emotes() {
        // Realistic "plain user" message: tags present but EMPTY
        // (no color chosen, no badges, no emotes) — this is what Twitch
        // actually sends, as opposed to omitting the tags.
        let line = "@badge-info=;badges=;color=;display-name=Nick;emotes=;id=msg-1;login=nick;room-id=42;tmi-sent-ts=1594545155039;user-id=7 :nick!nick@nick.tmi.twitch.tv PRIVMSG #chan :hello";
        let cm = map_privmsg(parse_line_to_privmsg(line));

        assert_eq!(cm.color, None);
        assert!(cm.badges.is_empty());
        assert!(cm.emotes.is_empty());
        assert_eq!(cm.text, "hello");
        assert_eq!(cm.user_login, "nick");
    }

    #[tokio::test]
    async fn rejects_malformed_channel_login() {
        // join() validates channel names before any network activity.
        let err = IrcChatSource::connect_anonymous("BAD CHANNEL!".to_string());
        assert!(err.is_err(), "malformed channel login must be rejected");
    }
}
