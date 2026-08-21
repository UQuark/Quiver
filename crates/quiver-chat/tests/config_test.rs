//! Integration tests for quiver-chat's config model.
//! Ground truth is hand-written constants; no clever computation.

use quiver_chat::config::{ChatConfig, SAMPLE_CONFIG};
use quiver_config::{Validate, load_from_path, parse_str, to_string};

#[test]
fn valid_fixture_parses_and_validates_clean() {
    let cfg: ChatConfig =
        load_from_path(std::path::Path::new("tests/fixtures/chat.ron")).expect("must parse");

    // Hand-written ground truth matching tests/fixtures/chat.ron exactly.
    assert_eq!(cfg.server.listen, "127.0.0.1:4783");
    assert_eq!(cfg.server.widget_dist, None);
    assert_eq!(cfg.twitch.channel, "quiverdev");
    assert_eq!(cfg.theme.font_size_px, 18);
    assert_eq!(cfg.theme.max_messages, 30);
    assert_eq!(cfg.theme.message_lifetime_secs, 60);

    assert_eq!(cfg.validate(), Vec::new());
}

#[test]
fn sample_config_parses_and_validates_clean() {
    // The printed sample must be usable as-is (minus channel rename):
    // "your_channel_here" is a valid login shape, so zero issues expected.
    let cfg: ChatConfig = parse_str(SAMPLE_CONFIG).expect("sample must parse");
    assert_eq!(cfg.validate(), Vec::new());
}

#[test]
fn invalid_values_report_exact_issue_paths() {
    let raw = r#"
(
    server: ( listen: "", ),
    twitch: ( channel: "has space!", ),
    theme: (
        font_size_px: 500,
        max_messages: 0,
        message_lifetime_secs: 0,
    ),
)
"#;
    let cfg: ChatConfig = parse_str(raw).expect("must parse");
    let issues = cfg.validate();
    let mut paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
    paths.sort_unstable();
    // Exactly the five planted violations ("has space!" is non-empty, so
    // twitch.channel trips ONLY the charset rule).
    assert_eq!(
        paths,
        vec![
            "server.listen",
            "theme.font_size_px",
            "theme.max_messages",
            "theme.message_lifetime_secs",
            "twitch.channel",
        ]
    );
}

#[test]
fn ron_roundtrip_preserves_optional_field_semantics() {
    let cfg: ChatConfig = parse_str(SAMPLE_CONFIG).expect("sample must parse");
    let text = to_string(&cfg).expect("serialize");
    let again: ChatConfig = parse_str(&text).expect("re-parse");
    assert_eq!(cfg, again);
}
