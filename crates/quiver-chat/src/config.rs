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

/// Sample config printed by `--sample-config`.
pub const SAMPLE_CONFIG: &str = r#"// quiver-chat configuration. RON format, verbose on purpose.
(
    server: (
        // Address the widget server listens on. OBS browser source points here.
        listen: "127.0.0.1:4783",
        // Optional: absolute path to the widget frontend directory
        // (crates/quiver-chat/widget). No build step needed.
        // widget_dist: "/absolute/path/to/crates/quiver-chat/widget",
    ),
    twitch: (
        // Channel login to read chat from (anonymous, read-only).
        channel: "your_channel_here",
        // Optional Twitch API credentials — enable badge image URLs.
        // Keep secrets out of version control (*.local.ron is git-ignored).
        // client_id: "your_client_id",
        // client_secret: "your_client_secret",
    ),
    theme: (
        font_size_px: 18,
        max_messages: 30,
        message_lifetime_secs: 60,
        // Optional: your own CSS on top of the built-in styles.
        // Author it as a RON RAW STRING so quotes and newlines need no
        // escaping (raw strings open with r followed by hashes and a
        // double-quote; see ThemeConfig docs).
        // custom_css: Some("/* example */ .msg { opacity: 0.9; }"),
        // Optional per-role CSS keyed by Twitch badge id (a RON map:
        // brace braces, quoted keys). Message rows get role-<id> classes;
        // snippets inject before custom_css.
        // role_css: Some({
        //     "moderator": ".msg { background: rgba(0,0,0,.35); }",
        //     "broadcaster": ".msg { border-left: 3px solid red; }",
        // }),
    ),
)
"#;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatConfig {
    pub server: ServerConfig,
    pub twitch: TwitchConfig,
    pub theme: ThemeConfig,
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
    pub font_size_px: u32,
    pub max_messages: u32,
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
