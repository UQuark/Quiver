//! Widget JS syntax gate.
//!
//! The frontend has no build step, so nothing else parses main.js before
//! browsers do — and a single parse error blanks the whole widget
//! silently (this exact bug shipped once). If `node` is available,
//! refuse to pass tests on a file that doesn't parse.

use std::path::Path;
use std::process::Command;

#[test]
fn widget_main_js_parses() {
    let js = Path::new(env!("CARGO_MANIFEST_DIR")).join("widget/src/main.js");
    let Some(node) = which_node() else {
        eprintln!("node not found — skipping widget syntax gate");
        return;
    };

    // --check parses without executing (no DOM, no network needed).
    let status = Command::new(node)
        .arg("--check")
        .arg(&js)
        .status()
        .expect("spawn node");
    assert!(
        status.success(),
        "widget/src/main.js has a syntax error — the browser would show NOTHING"
    );
}

fn which_node() -> Option<String> {
    for dir in std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = dir.join("node");
        if candidate.is_file() {
            return candidate.to_str().map(str::to_string);
        }
    }
    None
}
