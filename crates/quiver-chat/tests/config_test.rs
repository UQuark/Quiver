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

// ---- custom_css ----------------------------------------------------------

#[test]
fn raw_string_css_roundtrips_with_quotes_and_newlines() {
    // RON raw string: no escaping of quotes or newlines.
    // Outer Rust literal uses ## because the RON itself contains "#.
    let raw = r##"
(
    server: ( listen: "127.0.0.1:1", ),
    twitch: ( channel: "chan", ),
    theme: (
        font_size_px: 18,
        max_messages: 30,
        message_lifetime_secs: 60,
        custom_css: Some(r#".msg { content: "x"; opacity: 0.5; }"#),
    ),
)
"##;
    let cfg: ChatConfig = parse_str(raw).expect("raw string CSS must parse");
    let css = cfg.theme.custom_css.expect("css present");
    assert_eq!(css, r#".msg { content: "x"; opacity: 0.5; }"#);
}

#[test]
fn lint_accepts_valid_and_unknown_property_css() {
    assert_eq!(
        quiver_chat::config::lint_custom_css(".msg { opacity: 0.9; }"),
        Ok(())
    );
    // Unknown properties are ignored by browsers — parser must agree.
    assert_eq!(
        quiver_chat::config::lint_custom_css(".a { blah-blah: 12px; }"),
        Ok(())
    );
}

#[test]
fn lint_rejects_broken_css_with_position_info() {
    let err = quiver_chat::config::lint_custom_css("} { color: red; ")
        .expect_err("stray brace must be rejected");
    assert!(!err.is_empty());
}

#[test]
fn lint_tolerates_unclosed_final_block_like_browsers_do() {
    // Browsers apply a truncated final rule; the parser matches that
    // behavior, so the lint must not flag it.
    assert_eq!(
        quiver_chat::config::lint_custom_css(".msg { opacity: 0.9; "),
        Ok(())
    );
}

// ---- role_css -------------------------------------------------------------

// Ground truth by hand: three roles, exact strings preserved.
#[test]
fn role_css_map_parses_with_exact_snippets() {
    let raw = r##"
(
    server: ( listen: "127.0.0.1:1", ),
    twitch: ( channel: "chan", ),
    theme: (
        font_size_px: 18,
        max_messages: 30,
        message_lifetime_secs: 60,
        role_css: Some({
            "moderator": ".msg { background: rgba(0,0,0,.35); }",
            "broadcaster": ".msg { border-left: 3px solid red; }",
            "vip": ".user { text-shadow: 0 0 4px pink; }",
        }),
    ),
)
"##;
    let cfg: ChatConfig = parse_str(raw).expect("role_css must parse");
    let roles = cfg.theme.role_css.expect("map present");
    assert_eq!(roles.len(), 3);
    assert_eq!(
        roles.get("moderator").map(String::as_str),
        Some(".msg { background: rgba(0,0,0,.35); }")
    );
    assert_eq!(
        roles.get("broadcaster").map(String::as_str),
        Some(".msg { border-left: 3px solid red; }")
    );
    assert!(!roles.contains_key("subscriber"));
}
