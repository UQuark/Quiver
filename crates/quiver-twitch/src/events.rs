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
    pub user_login: String,
    pub display_name: String,
    /// User-chosen name color as `#RRGGBB`, or None if unset.
    pub color: Option<String>,
    pub badges: Vec<Badge>,
    pub emotes: Vec<EmoteRef>,
    pub text: String,
}

/// Events surfaced by a chat source.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    ChatMessage(ChatMessage),
    /// Server PING. Answered internally by the library; informational only.
    Ping(String),
    /// Anything not yet modeled (JOIN/PART/NOTICE/USERNOTICE/...).
    Other,
}
