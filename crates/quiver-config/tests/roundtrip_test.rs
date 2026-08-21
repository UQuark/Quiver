//! Integration tests for the quiver-config subsystem.
//!
//! Ground truth here is HAND-WRITTEN constants matching the fixture file
//! byte-for-byte in intent. No clever computation: if a test value ever
//! needs calculating, it is wrong.

use std::path::Path;

use quiver_config::{Validate, load_from_path, parse_str, require, schema_for, to_string};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---- hand-written ground truth for tests/fixtures/sample.ron -------------
const EXPECTED_LISTEN: &str = "127.0.0.1:4783";
const EXPECTED_CHANNEL: &str = "quiverdev";
const EXPECTED_FONT_SIZE_PX: u32 = 18;
const EXPECTED_MAX_MESSAGES: u32 = 30;
const EXPECTED_LIFETIME_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct ThemeConfig {
    font_size_px: u32,
    max_messages: u32,
    message_lifetime_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct SampleConfig {
    listen: String,
    channel: String,
    theme: ThemeConfig,
}

impl Validate for SampleConfig {
    fn validate(&self) -> Vec<quiver_config::ValidationIssue> {
        let mut out = Vec::new();
        require(
            !self.channel.is_empty(),
            "channel",
            "must not be empty",
            &mut out,
        );
        require(
            self.theme.font_size_px >= 8 && self.theme.font_size_px <= 200,
            "theme.font_size_px",
            "must be within 8..=200",
            &mut out,
        );
        require(
            self.theme.max_messages >= 1 && self.theme.max_messages <= 1000,
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

fn fixture_path() -> &'static Path {
    Path::new("tests/fixtures/sample.ron")
}

#[test]
fn parses_fixture_from_disk_with_expected_values() {
    let cfg: SampleConfig = load_from_path(fixture_path()).expect("fixture must parse");

    // Field-by-field against hand-written ground truth.
    assert_eq!(cfg.listen, EXPECTED_LISTEN);
    assert_eq!(cfg.channel, EXPECTED_CHANNEL);
    assert_eq!(cfg.theme.font_size_px, EXPECTED_FONT_SIZE_PX);
    assert_eq!(cfg.theme.max_messages, EXPECTED_MAX_MESSAGES);
    assert_eq!(cfg.theme.message_lifetime_secs, EXPECTED_LIFETIME_SECS);
}

#[test]
fn ron_roundtrip_preserves_value() {
    let cfg: SampleConfig = load_from_path(fixture_path()).expect("fixture must parse");

    let serialized = to_string(&cfg).expect("serialize must work");
    let reparsed: SampleConfig = parse_str(&serialized).expect("re-parse must work");

    assert_eq!(cfg, reparsed);
}

#[test]
fn valid_fixture_has_no_validation_issues() {
    let cfg: SampleConfig = load_from_path(fixture_path()).expect("fixture must parse");
    assert_eq!(cfg.validate(), Vec::new());
}

#[test]
fn invalid_values_report_exact_issue_paths() {
    let cfg = SampleConfig {
        listen: "127.0.0.1:1".to_string(),
        channel: String::new(),
        theme: ThemeConfig {
            font_size_px: 0,
            max_messages: 5000,
            message_lifetime_secs: 0,
        },
    };

    let issues = cfg.validate();

    // Exactly the four violations we planted, no more, no less.
    let mut paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        vec![
            "channel",
            "theme.font_size_px",
            "theme.max_messages",
            "theme.message_lifetime_secs",
        ]
    );
}

#[test]
fn json_schema_exposes_all_properties() {
    let doc = schema_for::<SampleConfig>();

    let props = doc
        .get("properties")
        .and_then(|p| p.as_object())
        .expect("schema must have top-level properties");

    assert!(props.contains_key("listen"));
    assert!(props.contains_key("channel"));
    // Nested structs are emitted as $defs entries referenced via $ref
    // (schemars 1.x, draft 2020-12 style).
    assert_eq!(
        props
            .get("theme")
            .and_then(|t| t.get("$ref"))
            .and_then(|r| r.as_str()),
        Some("#/$defs/ThemeConfig")
    );

    let theme_props = doc
        .pointer("/$defs/ThemeConfig/properties")
        .and_then(|p| p.as_object())
        .expect("nested theme must expose its own properties under $defs");
    assert!(theme_props.contains_key("font_size_px"));
    assert!(theme_props.contains_key("max_messages"));
    assert!(theme_props.contains_key("message_lifetime_secs"));

    // Integer fields must be typed as integer, not number/string.
    let font_type = doc
        .pointer("/$defs/ThemeConfig/properties/font_size_px/type")
        .and_then(|t| t.as_str());
    assert_eq!(font_type, Some("integer"));
}
