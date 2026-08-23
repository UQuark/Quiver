//! Chat source facade over `twitch-irc`.
//!
//! The library owns the hard parts: TLS, CAP negotiation, reconnect with
//! backoff, connection pooling, rate limiting, PING/PONG. This module maps
//! its message types onto Quiver's own [`Event`] model so tool crates never
//! depend on `twitch-irc` types directly.

use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::{PrivmsgMessage, ServerMessage, UserNoticeEvent, UserNoticeMessage};
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};

use crate::error::TwitchError;
use crate::events::{
    Badge, ChatMessage, EmoteRef, Event, GiftSubEvent, MysteryGiftEvent, RaidEvent, SubEvent,
};

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
                ServerMessage::UserNotice(un) => return Some(map_user_notice(un)),
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
        user_id: pm.sender.id,
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
        reply_parent: pm.reply_parent.map(|rp| crate::events::ReplyParent {
            message_id: rp.message_id,
            user_login: rp.reply_parent_user.login,
            display_name: rp.reply_parent_user.name,
            text: rp.message_text,
        }),
    }
}

/// Map a library USERNOTICE onto Quiver's event model.
///
/// Modeled: sub/resub, subgift/anonsubgift, submysterygift/
/// anonsubmysterygift, raid. Everything else falls through to
/// [`Event::Other`] (rituals, gift upgrades, announcements, ...).
fn map_user_notice(un: UserNoticeMessage) -> Event {
    match un.event {
        UserNoticeEvent::SubOrResub {
            is_resub,
            cumulative_months,
            streak_months,
            sub_plan,
            ..
        } => Event::Sub(SubEvent {
            user_login: un.sender.login,
            display_name: un.sender.name,
            is_resub,
            tier: sub_plan,
            cumulative_months,
            streak_months,
            message: un.message_text,
        }),
        UserNoticeEvent::SubGift {
            is_sender_anonymous,
            cumulative_months,
            recipient,
            sub_plan,
            num_gifted_months,
            ..
        } => Event::GiftSub(GiftSubEvent {
            gifter_login: (!is_sender_anonymous).then(|| un.sender.login.clone()),
            gifter_display_name: (!is_sender_anonymous).then(|| un.sender.name.clone()),
            recipient_login: recipient.login,
            recipient_display_name: recipient.name,
            tier: sub_plan,
            cumulative_months,
            num_gifted_months,
        }),
        UserNoticeEvent::SubMysteryGift {
            mass_gift_count,
            sender_total_gifts,
            sub_plan,
        } => {
            // Anonymous mystery gifts carry a dummy sender (see lib docs).
            let anon = un.sender.login.eq_ignore_ascii_case("ananonymousgifter");
            Event::MysteryGift(MysteryGiftEvent {
                gifter_login: (!anon).then(|| un.sender.login.clone()),
                gifter_display_name: (!anon).then(|| un.sender.name.clone()),
                mass_gift_count,
                sender_total_gifts,
                tier: sub_plan,
            })
        }
        UserNoticeEvent::Raid { viewer_count, .. } => Event::Raid(RaidEvent {
            from_login: un.sender.login,
            from_display_name: un.sender.name,
            viewers: viewer_count,
        }),
        _ => Event::Other,
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

    fn parse_line_to_event(line: &str) -> Event {
        let irc_message = IRCMessage::parse(line).expect("line must parse as IRC");
        let server = ServerMessage::try_from(irc_message).expect("must parse as ServerMessage");
        match server {
            ServerMessage::UserNotice(un) => map_user_notice(un),
            other => panic!("expected USERNOTICE, got {other:?}"),
        }
    }

    #[test]
    fn maps_reply_parent_tags() {
        // Ground truth BY HAND: reply to abc-123 ("original text") from Y.
        let line = format!(
            "@{base};reply-parent-msg-id=abc-123;reply-parent-user-id=99;reply-parent-user-login=y;reply-parent-display-name=Y;reply-parent-msg-body=original\\stext :x!x@x.tmi.twitch.tv PRIVMSG #chan :answering here",
            base = USERNOTICE_BASE_TAGS
        );
        let irc = IRCMessage::parse(&line).expect("parses");
        let pm = PrivmsgMessage::try_from(irc).expect("parses as privmsg");
        let cm = map_privmsg(pm);

        let rp = cm.reply_parent.expect("reply parent present");
        assert_eq!(rp.message_id, "abc-123");
        assert_eq!(rp.user_login, "y");
        assert_eq!(rp.display_name, "Y");
        // Tag unescaping turns \s into spaces.
        assert_eq!(rp.text, "original text");
        assert_eq!(cm.text, "answering here");

        // Non-reply messages must carry None.
        let plain = map_privmsg(parse_line_to_privmsg(LINE));
        assert!(plain.reply_parent.is_none());
    }

    #[test]
    fn maps_full_privmsg_with_hand_verified_ground_truth() {
        let cm = map_privmsg(parse_line_to_privmsg(LINE));

        assert_eq!(cm.id, "abc-123");
        assert_eq!(cm.channel_login, "quiverdev");
        assert_eq!(cm.user_id, "29803735");
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

    // ---- USERNOTICE mapping --------------------------------------------------
    // Lines follow Twitch's real wire format (mandatory tags included).

    const USERNOTICE_BASE_TAGS: &str = "badge-info=;badges=;color=;display-name=X;emotes=;id=n-1;login=x;mod=0;room-id=11148817;subscriber=0;system-msg=Something\\shappened.;tmi-sent-ts=1594545155039;turbo=0;user-id=42";

    /// Ground truth BY HAND: sub, 12 cumulative months, tier 1000.
    #[test]
    fn maps_sub() {
        let line = format!(
            "@{base};msg-id=sub;msg-param-cumulative-months=12;msg-param-sub-plan=1000;msg-param-sub-plan-name=ChanSub;msg-param-should-share-streak=0 :x!x@x.tmi.twitch.tv USERNOTICE #chan",
            base = USERNOTICE_BASE_TAGS
        );
        match parse_line_to_event(&line) {
            Event::Sub(s) => {
                assert_eq!(s.user_login, "x");
                assert_eq!(s.display_name, "X");
                assert!(!s.is_resub);
                assert_eq!(s.tier, "1000");
                assert_eq!(s.cumulative_months, 12);
                assert_eq!(s.streak_months, None);
                assert_eq!(s.message, None);
            }
            other => panic!("expected Sub, got {other:?}"),
        }
    }

    /// Ground truth BY HAND: resub with streak + user message.
    #[test]
    fn maps_resub_with_streak_and_message() {
        let line = format!(
            "@{base};msg-id=resub;msg-param-cumulative-months=24;msg-param-streak-months=7;msg-param-should-share-streak=1;msg-param-sub-plan=2000;msg-param-sub-plan-name=ChanSub :x!x@x.tmi.twitch.tv USERNOTICE #chan :love the streams!",
            base = USERNOTICE_BASE_TAGS
        );
        match parse_line_to_event(&line) {
            Event::Sub(s) => {
                assert!(s.is_resub);
                assert_eq!(s.cumulative_months, 24);
                assert_eq!(s.streak_months, Some(7));
                assert_eq!(s.tier, "2000");
                assert_eq!(s.message.as_deref(), Some("love the streams!"));
            }
            other => panic!("expected Sub(resub), got {other:?}"),
        }
    }

    /// Ground truth BY HAND: raid from RaidLeader with 42 viewers.
    #[test]
    fn maps_raid() {
        // Raider identity owns the prefix; base tags say "x" so override
        // the identity tags too — real raids carry consistent identity.
        let line = r"@badge-info=;badges=broadcaster/1;color=;display-name=RaidLeader;emotes=;id=r-1;login=raidleader;mod=0;room-id=11148817;subscriber=0;system-msg=RaidLeader\sis\traiding.;tmi-sent-ts=1594545155039;turbo=0;user-id=77;msg-id=raid;msg-param-display-name=RaidLeader;msg-param-login=raidleader;msg-param-viewerCount=42;msg-param-profileImageURL=https://static-cdn.jtvnw.net/jtv_user_pictures/prof.png :raidleader!raidleader@raidleader.tmi.twitch.tv USERNOTICE #chan";

        match parse_line_to_event(line) {
            Event::Raid(r) => {
                assert_eq!(r.from_login, "raidleader");
                assert_eq!(r.from_display_name, "RaidLeader");
                assert_eq!(r.viewers, 42);
            }
            other => panic!("expected Raid, got {other:?}"),
        }
    }

    /// Ground truth BY HAND: named gifter, recipient Y, 5 gift-months,
    /// recipient at 8 cumulative months, tier 3000.
    #[test]
    fn maps_subgift() {
        let line = format!(
            "@{base};msg-id=subgift;msg-param-gift-months=5;msg-param-months=8;msg-param-cumulative-months=8;msg-param-recipient-id=99;msg-param-recipient-user-name=y;msg-param-recipient-display-name=Y;msg-param-sub-plan=3000;msg-param-sub-plan-name=ChanSub :x!x@x.tmi.twitch.tv USERNOTICE #chan",
            base = USERNOTICE_BASE_TAGS
        );
        match parse_line_to_event(&line) {
            Event::GiftSub(g) => {
                assert_eq!(g.gifter_login.as_deref(), Some("x"));
                assert_eq!(g.recipient_login, "y");
                assert_eq!(g.recipient_display_name, "Y");
                assert_eq!(g.tier, "3000");
                assert_eq!(g.cumulative_months, 8);
                assert_eq!(g.num_gifted_months, 5);
            }
            other => panic!("expected GiftSub, got {other:?}"),
        }
    }

    /// Ground truth BY HAND: mystery gift of 10 subs, lifetime total 55.
    #[test]
    fn maps_mystery_gift() {
        let line = format!(
            "@{base};msg-id=submysterygift;msg-param-mass-gift-count=10;msg-param-sender-count=55;msg-param-sub-plan=1000;msg-param-sub-plan-name=ChanSub :x!x@x.tmi.twitch.tv USERNOTICE #chan",
            base = USERNOTICE_BASE_TAGS
        );
        match parse_line_to_event(&line) {
            Event::MysteryGift(m) => {
                assert_eq!(m.gifter_login.as_deref(), Some("x"));
                assert_eq!(m.mass_gift_count, 10);
                assert_eq!(m.sender_total_gifts, Some(55));
                assert_eq!(m.tier, "1000");
            }
            other => panic!("expected MysteryGift, got {other:?}"),
        }
    }

    /// Unmodeled USERNOTICE types (e.g. rituals) must not crash the feed.
    #[test]
    fn maps_unknown_usernotice_to_other() {
        let line = format!(
            "@{base};msg-id=ritual;msg-param-ritual-name=new_chatter :x!x@x.tmi.twitch.tv USERNOTICE #chan",
            base = USERNOTICE_BASE_TAGS
        );
        assert_eq!(parse_line_to_event(&line), Event::Other);
    }
}
