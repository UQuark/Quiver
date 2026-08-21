//! Quiver's Twitch connectivity layer.
//!
//! The ONLY crate in the workspace allowed to speak Twitch. Built as a
//! facade over [`twitch-irc`](https://crates.io/crates/twitch-irc): the
//! library stays an implementation detail — tool crates see only
//! [`events::Event`] and friends, so the backend can be swapped without
//! touching them.

pub mod error;
pub mod events;
pub mod helix;
pub mod source;

pub use error::TwitchError;
pub use events::{Badge, ChatMessage, EmoteRef, Event};
pub use helix::{HelixClient, HelixError};
pub use source::IrcChatSource;
