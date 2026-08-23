/// A chat badge attached to a user (e.g. `broadcaster/1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    pub id: String,
    pub version: String,
}

/// An emote reference inside a message.
///
/// `start`/`end` are CHARACTER indices into the message text with
/// INCLUSIVE start and EXCLUSIVE end (Rust `&text[start..end]` slices it).
///
/// GOTCHA: Twitch's wire format uses UTF-16 code units with inclusive end.
/// `twitch-irc` normalizes to char indices + exclusive end for us. Do not
/// "simplify" this back to wire format without a very good reason.
#[derive(Debug, Clone, PartialEq)]
pub struct EmoteRef {
    pub id: String,
    pub start: usize,
    pub end: usize,
}

/// One parsed chat message.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub channel_login: String,
    /// Twitch numeric user id (stable across renames).
    pub user_id: String,
    pub user_login: String,
    pub display_name: String,
    pub color: Option<String>,
    pub badges: Vec<Badge>,
    pub emotes: Vec<EmoteRef>,
    pub text: String,
}

/// A new (or renewed) subscription. From USERNOTICE `sub`/`resub`.
#[derive(Debug, Clone, PartialEq)]
pub struct SubEvent {
    pub user_login: String,
    pub display_name: String,
    pub is_resub: bool,
    /// `Prime`, `1000`, `2000` or `3000`.
    pub tier: String,
    pub cumulative_months: u64,
    /// Consecutive-month streak; None when the user opted to hide it.
    pub streak_months: Option<u64>,
    /// Resub message text, when the user attached one.
    pub message: Option<String>,
}

/// One gifted sub delivered to a specific recipient.
/// From USERNOTICE `subgift`/`anonsubgift`.
#[derive(Debug, Clone, PartialEq)]
pub struct GiftSubEvent {
    /// None = anonymous gifter.
    pub gifter_login: Option<String>,
    pub gifter_display_name: Option<String>,
    pub recipient_login: String,
    pub recipient_display_name: String,
    pub tier: String,
    pub cumulative_months: u64,
    /// Multi-month gifts deliver this many months at once.
    pub num_gifted_months: u64,
}

/// The announcement preceding a wave of gift subs.
/// From USERNOTICE `submysterygift`/`anonsubmysterygift`.
#[derive(Debug, Clone, PartialEq)]
pub struct MysteryGiftEvent {
    /// None = anonymous gifter.
    pub gifter_login: Option<String>,
    pub gifter_display_name: Option<String>,
    pub mass_gift_count: u64,
    /// Gifter's lifetime total in the channel; absent on some promos.
    pub sender_total_gifts: Option<u64>,
    pub tier: String,
}

/// An incoming raid. From USERNOTICE `raid`.
#[derive(Debug, Clone, PartialEq)]
pub struct RaidEvent {
    pub from_login: String,
    pub from_display_name: String,
    pub viewers: u64,
}

/// Events surfaced by a chat source.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    ChatMessage(ChatMessage),
    Sub(SubEvent),
    GiftSub(GiftSubEvent),
    MysteryGift(MysteryGiftEvent),
    Raid(RaidEvent),
    /// Server PING. Answered internally by the library; informational only.
    Ping(String),
    /// Anything not yet modeled (JOIN/PART/NOTICE/USERNOTICE/...).
    Other,
}
