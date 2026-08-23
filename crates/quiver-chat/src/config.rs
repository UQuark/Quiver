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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
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
    /// Message filtering (allowlist/denylist per dimension). Absent = no
    /// filtering at all.
    #[serde(default)]
    pub filters: FiltersConfig,
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

/// Filter list direction. Allowlist = at least one item must match;
/// denylist = no item may match. An EMPTY item list makes the whole
/// dimension inactive (an empty allowlist meaning "drop everything"
/// is a footgun we refuse to arm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Allowlist,
    Denylist,
}

/// One filter dimension: a direction plus its string items.
/// Item interpretation depends on the dimension (regex / exact id /
/// kind name / badge id) — see `FiltersConfig` field docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ListFilter {
    pub mode: Mode,
    pub items: Vec<String>,
}

fn default_command_prefixes() -> Vec<String> {
    vec!["!".to_string()]
}

// MANUAL Default: derived Default would ignore the serde field default and
// produce empty prefixes — inconsistent with parsed configs AND with what
// generate_default emits.
impl Default for FiltersConfig {
    fn default() -> Self {
        Self {
            command_prefixes: default_command_prefixes(),
            display_name: None,
            username: None,
            user_id: None,
            content: None,
            message_type: None,
            role: None,
        }
    }
}

/// Message filtering rules. Dimensions combine with AND: a message is
/// rendered only when EVERY active dimension passes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FiltersConfig {
    /// Prefixes that mark a chat message as the `command` kind
    /// (first match wins). Default: ["!"].
    #[serde(default = "default_command_prefixes")]
    pub command_prefixes: Vec<String>,
    /// Regex matched against the sender's display name.
    pub display_name: Option<ListFilter>,
    /// Regex matched against the sender's login name (always lowercase).
    pub username: Option<ListFilter>,
    /// EXACT match against the sender's Twitch user id (numeric string).
    pub user_id: Option<ListFilter>,
    /// Regex matched against the message text. Events have empty text.
    pub content: Option<ListFilter>,
    /// Message kind names: message | command | sub | gift_sub |
    /// mystery_gift | raid. Unknown names are config errors.
    pub message_type: Option<ListFilter>,
    /// Badge ids carried by the sender — same vocabulary as role_css
    /// (moderator, vip, subscriber, ...). Events carry no badges in v1.
    pub role: Option<ListFilter>,
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

        // Filters: regex patterns must compile; kind names must be real.
        // A broken filter is functional, not cosmetic — these are hard
        // validation errors, unlike advisory CSS lint.
        let f = &self.filters;
        require(
            f.command_prefixes.iter().all(|p| !p.is_empty()),
            "filters.command_prefixes",
            "prefixes must not be empty strings",
            &mut out,
        );
        for (name, dim) in [
            ("display_name", &f.display_name),
            ("username", &f.username),
            ("content", &f.content),
        ] {
            if let Some(dim) = dim {
                for item in &dim.items {
                    if let Err(e) = regex::Regex::new(item) {
                        out.push(ValidationIssue {
                            path: format!("filters.{name}.items"),
                            message: format!("invalid regex {item:?}: {e}"),
                        });
                    }
                }
            }
        }
        if let Some(dim) = &f.message_type {
            for item in &dim.items {
                if crate::filters::MsgKind::parse(item).is_none() {
                    out.push(ValidationIssue {
                        path: "filters.message_type.items".to_string(),
                        message: format!(
                            "unknown message kind {item:?} — expected one of: \
                             message, command, sub, gift_sub, mystery_gift, raid"
                        ),
                    });
                }
            }
        }

        out
    }
}
