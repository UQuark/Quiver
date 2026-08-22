//! quiver-chat's RON configuration model.
//!
//! Verbose and explicit on purpose: this file IS the tool's interface.
//! The future Quiver UI generates configs of this shape from the JSON
//! Schema exported via `quiver-chat --print-schema`.

use std::collections::HashMap;
use std::path::PathBuf;

use quiver_config::{Validate, ValidationIssue, require};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// quiver-chat configuration. Verbose on purpose: this file IS the
/// tool's interface. The future Quiver UI generates files of this shape
/// from the JSON Schema exported via `quiver-chat --print-schema`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatConfig {
    /// HTTP server and widget frontend location.
    pub server: ServerConfig,
    /// Twitch chat source and optional API credentials.
    pub twitch: TwitchConfig,
    /// Visual appearance of rendered messages.
    pub theme: ThemeConfig,
    /// Emote source toggles. All on by default; disabling a provider makes
    /// its emojis vanish from rendered messages entirely.
    #[serde(default)]
    pub emotes: EmotesConfig,
}

fn default_true() -> bool {
    true
}

/// Per-provider emote switches, all enabled by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EmotesConfig {
    /// First-party Twitch emotes (rendered from per-message ranges).
    #[serde(default = "default_true")]
    pub twitch: bool,
    /// Native unicode emoji characters. `false` strips them from text.
    #[serde(default = "default_true")]
    pub unicode: bool,
    /// 7TV global + channel sets.
    #[serde(default = "default_true")]
    pub seventv: bool,
    /// BetterTTV global + channel/shared sets.
    #[serde(default = "default_true")]
    pub bttv: bool,
    /// FrankerFaceZ global + room sets.
    #[serde(default = "default_true")]
    pub ffz: bool,
}

impl Default for EmotesConfig {
    fn default() -> Self {
        Self {
            twitch: true,
            unicode: true,
            seventv: true,
            bttv: true,
            ffz: true,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:4783".to_string(),
            // Relative to where quiver-chat runs — works out of the box
            // from the monorepo root.
            widget_dist: Some(PathBuf::from("crates/quiver-chat/widget")),
        }
    }
}

impl Default for TwitchConfig {
    fn default() -> Self {
        Self {
            channel: "your_channel_here".to_string(),
            client_id: None,
            client_secret: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerConfig {
    /// `host:port` the widget HTTP+WS server binds to.
    pub listen: String,
    /// Optional absolute path to the built widget frontend directory.
    /// Unset → placeholder page is served at `/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget_dist: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TwitchConfig {
    /// Twitch channel login to read chat from (read-only anonymous join).
    pub channel: String,
    /// Twitch API client id — enables badge image URLs (Helix lookup).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Twitch API client secret. Keep ONLY in git-ignored local configs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThemeConfig {
    /// Base font size for messages, in CSS pixels.
    pub font_size_px: u32,
    /// Maximum number of messages kept in the visible history.
    pub max_messages: u32,
    /// Seconds a message stays visible before fading out.
    pub message_lifetime_secs: u64,
    /// Your own CSS, applied after the built-in widget styles. Authored as
    /// a RON raw string so newlines and quotes stay untouched.
    /// Parsed with lightningcss at load/reload; a parse error is reported
    /// as a warning but never blocks the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_css: Option<String>,
    /// Per-role CSS: Twitch badge set id -> CSS snippet applied to messages
    /// whose sender carries that badge. Message rows get `role-<id>` classes,
    /// snippets are injected before custom_css. Common ids: broadcaster,
    /// moderator, vip, subscriber, founder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_css: Option<HashMap<String, String>>,
}

/// Lint user CSS through a real parser (lightningcss).
/// Advisory only: Ok means "browser will almost certainly accept it".
pub fn lint_custom_css(css: &str) -> Result<(), String> {
    let options = lightningcss::stylesheet::ParserOptions::default();
    lightningcss::stylesheet::StyleSheet::parse(css, options)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Log a warning when user CSS fails the lint. Never fatal.
pub(crate) fn report_css_lint(css: Option<&str>, role_css: Option<&HashMap<String, String>>) {
    if let Some(css) = css
        && let Err(e) = lint_custom_css(css)
    {
        tracing::warn!(
            error = %e,
            "custom_css looks broken — browsers will ignore invalid rules"
        );
    }
    if let Some(map) = role_css {
        for (role, snippet) in map {
            if let Err(e) = lint_custom_css(snippet) {
                tracing::warn!(
                    role = %role,
                    error = %e,
                    "role_css snippet looks broken — browsers will ignore invalid rules"
                );
            }
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            font_size_px: 18,
            max_messages: 30,
            message_lifetime_secs: 60,
            custom_css: None,
            role_css: None,
        }
    }
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            twitch: TwitchConfig::default(),
            theme: ThemeConfig::default(),
            emotes: EmotesConfig::default(),
        }
    }
}

impl Validate for ChatConfig {
    fn validate(&self) -> Vec<ValidationIssue> {
        let mut out = Vec::new();

        require(
            !self.server.listen.is_empty(),
            "server.listen",
            "must not be empty",
            &mut out,
        );
        require(
            !self.twitch.channel.is_empty(),
            "twitch.channel",
            "must not be empty",
            &mut out,
        );
        require(
            self.twitch
                .channel
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "twitch.channel",
            "must contain only ASCII letters, digits and underscores (a Twitch login)",
            &mut out,
        );
        // Credentials are only meaningful as a pair.
        require(
            self.twitch.client_id.is_some() == self.twitch.client_secret.is_some(),
            if self.twitch.client_id.is_none() {
                "twitch.client_id"
            } else {
                "twitch.client_secret"
            },
            "client_id and client_secret must be set together",
            &mut out,
        );
        require(
            (8..=200).contains(&self.theme.font_size_px),
            "theme.font_size_px",
            "must be within 8..=200",
            &mut out,
        );
        require(
            (1..=1000).contains(&self.theme.max_messages),
            "theme.max_messages",
            "must be within 1..=1000",
            &mut out,
        );
        require(
            self.theme.message_lifetime_secs >= 1,
            "theme.message_lifetime_secs",
            "must be at least 1",
            &mut out,
        );

        out
    }
}
