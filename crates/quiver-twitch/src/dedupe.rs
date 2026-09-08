//! Cross-source redemption dedupe.
//!
//! A single channel-point redemption can arrive via THREE producers: IRC
//! (custom-reward-id PRIVMSG, instant, no redemption id), the Helix
//! redemption poller (has redemption id), and EventSub (redemption.add,
//! has redemption id). Exactly one render must win:
//!
//! - IRC always renders (instant, and refunds nothing downstream) and
//!   marks `(user, reward)` — the poller suppresses that match inside the
//!   window.
//! - Poller and EventSub dedupe against each other by the UNIQUE
//!   redemption id, and skip when IRC already rendered that (user, reward)
//!   moments ago.
//!
//! User-visible contract: EVERY redeem the broadcaster performs renders —
//! including rapid repeats of the same reward. The windows only exist to
//! collapse duplicate deliveries of ONE redemption across sources.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Shared dedupe ring used by engine (IRC), poller, and EventSub.
pub type SharedRedeemDeduper = Arc<Mutex<RedeemDeduper>>;

#[derive(Default)]
pub struct RedeemDeduper {
    /// Redemption ids the poller/EventSub already delivered.
    seen_ids: VecDeque<(String, Instant)>,
    /// `(user_login, user_id, reward_id)` triples rendered from IRC — used
    /// ONLY to suppress later duplicate deliveries of the same redemption.
    recent_irc: VecDeque<(String, String, String, Instant)>,
}

impl RedeemDeduper {
    const IRC_WINDOW: Duration = Duration::from_secs(30);
    const ID_WINDOW: Duration = Duration::from_secs(60);

    /// IRC-side: remember that this (user, reward) was just rendered. Never
    /// suppresses IRC emissions — each IRC message is its own redemption.
    pub fn mark_irc(&mut self, user_login: &str, user_id: &str, reward_id: &str) {
        let now = std::time::Instant::now();
        self.recent_irc
            .retain(|(_, _, _, seen)| now.duration_since(*seen) < Self::IRC_WINDOW);
        self.recent_irc.push_back((
            user_login.to_string(),
            user_id.to_string(),
            reward_id.to_string(),
            now,
        ));
    }

    /// Poller/EventSub side: was this redemption id already delivered?
    /// Marks it when new.
    pub fn is_new_redemption_id(&mut self, redemption_id: &str) -> bool {
        let now = std::time::Instant::now();
        self.seen_ids
            .retain(|(_, seen)| now.duration_since(*seen) < Self::ID_WINDOW);
        if self.seen_ids.iter().any(|(id, _)| id == redemption_id) {
            return false;
        }
        self.seen_ids.push_back((redemption_id.to_string(), now));
        true
    }

    /// Poller/EventSub side: did IRC already render this redemption moments
    /// ago (same user, same reward)? True → skip, IRC already covered it.
    pub fn irc_already_rendered(
        &mut self,
        user_login: &str,
        user_id: &str,
        reward_id: &str,
    ) -> bool {
        let now = std::time::Instant::now();
        self.recent_irc
            .retain(|(_, _, _, seen)| now.duration_since(*seen) < Self::IRC_WINDOW);
        self.recent_irc
            .iter()
            .any(|(u, uid, r, _)| u == user_login && (uid.is_empty() || uid == user_id) && r == reward_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> RedeemDeduper {
        RedeemDeduper::default()
    }

    #[test]
    fn irc_repeats_always_render() {
        let mut d = RedeemDeduper::default();
        // Repeated IRC arrivals of the SAME reward by the SAME user are
        // legitimate separate redemptions — mark_irc never blocks.
        d.mark_irc("u", "1", "r");
        d.mark_irc("u", "1", "r");
        assert_eq!(d.recent_irc.len(), 2, "irc marks are additive");
    }

    #[test]
    fn poller_fullfills_suppressed_after_irc() {
        let mut d = RedeemDeduper::default();
        d.mark_irc("u", "1", "r");
        assert!(
            d.irc_already_rendered("u", "1", "r"),
            "poller suppresses what IRC just rendered"
        );
        // A different user with the same reward still renders.
        assert!(!d.irc_already_rendered("other", "2", "r"));
    }

    #[test]
    fn redemption_ids_dedupe_without_irc_match() {
        let mut d = RedeemDeduper::default();
        assert!(d.is_new_redemption_id("aaa"));
        assert!(!d.is_new_redemption_id("aaa"), "same id suppressed");
        assert!(d.is_new_redemption_id("bbb"));
        // A different redemption id for the same user/reward still renders —
        // rapid repeats are NOT collapsed by the id ring.
        assert!(d.is_new_redemption_id("ccc"));
    }

    #[test]
    fn irc_window_expires() {
        let mut d = RedeemDeduper::default();
        d.mark_irc("u", "1", "r");
        // Simulate expiry by aging the entries past the window.
        for e in d.recent_irc.iter_mut() {
            e.3 = std::time::Instant::now() - Duration::from_secs(31);
        }
        assert!(!d.irc_already_rendered("u", "1", "r"), "window expired");
    }
}