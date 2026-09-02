//! Custom badge resolution, HTTP caching, and priority-merge logic.
//!
//! `file://` URIs are excluded from the caching model (YAGNI KISS) and
//! resolved synchronously from the canonical path. `http(s)://` URIs get
//! the full dual-hash cache: URL-hash for lookup, content-hash for dedup
//! + conditional revalidation (ETag / Last-Modified).
//!
//! On-disk layout under `cache_dir`:
//! ```text
//! {cache_dir}/
//! ├── .index.json          # url_hash → CacheEntry (loaded into memory)
//! └── {content_hash}.{ext} # body files, content-addressed

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::CustomBadgesConfig;

/// Resolved badge ready for the widget: local URL + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedBadge {
    pub id: String,
    pub url: String,
    pub priority: u32,
    pub height: u32,
    pub label: Option<String>,
}

/// Fully resolved custom badge state: definitions + per-role + per-user maps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedCustomBadges {
    pub definitions: HashMap<String, ResolvedBadge>,
    pub per_role: HashMap<String, Vec<String>>,
    pub per_user: HashMap<String, Vec<String>>,
}

// ---- cache internals ----------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CacheEntry {
    url: String,
    url_hash: String,
    content_hash: String,
    content_type: String,
    etag: Option<String>,
    last_modified: Option<String>,
    /// Epoch seconds of last successful resolution (freshness marker).
    resolved_at: u64,
}

#[derive(Debug, Default)]
struct Index(HashMap<String, CacheEntry>); // url_hash → entry

// ---- public types -------------------------------------------------------

/// Combined badge resolution + cache state, held behind an RwLock.
pub struct BadgeCacheState {
    pub resolved: ResolvedCustomBadges,
    index: Index,
    cache_dir: PathBuf,
    /// file:// badges: url_hash → canonical filesystem path. NO caching,
    /// no hashing, no ETag — pure passthrough (KISS per spec).
    pub file_badges: HashMap<String, PathBuf>,
}

/// Shared badge cache state behind RwLock for reload-safe access.
pub type SharedBadgeCache = std::sync::Arc<std::sync::RwLock<Option<BadgeCacheState>>>;

// ---- public resolution API ----------------------------------------------
// ---- hashing helpers ----------------------------------------------------

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn content_hash_to_path(cache_dir: &Path, content_hash: &str) -> PathBuf {
    cache_dir.join(format!("{content_hash}.bin"))
}

fn default_cache_dir() -> PathBuf {
    dirs_cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Quiver")
        .join("badges")
}

/// XDG_CACHE_HOME → .cache fallback.
fn dirs_cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join(".cache")))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ---- URI helpers --------------------------------------------------------

fn uri_hash(url: &str) -> String {
    sha256_hex(url.as_bytes())
}

fn detect_content_type(uri: &str, body: &[u8]) -> String {
    // HTTP Content-Type is authoritative; file fallback by extension.
    match uri.rsplit('.').next() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        _ => sniff_content_type(body),
    }
    .to_string()
}

pub fn sniff_content_type(body: &[u8]) -> &'static str {
    if body.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if body.starts_with(b"RIFF") {
        "image/webp"
    } else if body.starts_with(b"GIF8") {
        "image/gif"
    } else if body.starts_with(b"<svg") || body.starts_with(b"<?xml") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

// ---- index persistence --------------------------------------------------

const INDEX_FILENAME: &str = ".index.json";

fn load_index(cache_dir: &Path) -> Index {
    let path = cache_dir.join(INDEX_FILENAME);
    match std::fs::read_to_string(&path) {
        Ok(json) => Index(serde_json::from_str(&json).unwrap_or_default()),
        Err(_) => Index::default(),
    }
}

fn save_index(index: &Index, cache_dir: &Path) {
    if let Ok(json) = serde_json::to_string_pretty(&index.0) {
        let _ = std::fs::create_dir_all(cache_dir);
        let _ = std::fs::write(cache_dir.join(INDEX_FILENAME), json);
    }
}

// ---- resolve logic (http) -----------------------------------------------

/// Build a cache entry from an HTTP response headers + body hash.
fn make_entry(
    url: &str,
    uhash: String,
    body: &[u8],
    ct: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> CacheEntry {
    CacheEntry {
        url: url.to_string(),
        url_hash: uhash,
        content_hash: sha256_hex(body),
        content_type: ct.to_string(),
        etag: etag.map(str::to_string),
        last_modified: last_modified.map(str::to_string),
        resolved_at: now_epoch_secs(),
    }
}

/// Grab revalidation headers from a response (before consuming the body).
fn revalidation_headers(resp: &reqwest::Response) -> (Option<String>, Option<String>) {
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let lm = resp
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    (etag, lm)
}

/// Fetch one HTTP URL and persist into cache. Returns the local serve path
/// (the content-addressed route). Uses conditional GET (ETag /
/// If-Modified-Since) when a stale entry exists; on fetch failure serves
/// the stale cache so the widget never blanks.
async fn resolve_http(
    url: &str,
    http: &reqwest::Client,
    index: &mut Index,
    cache_dir: &Path,
    refresh_interval: Duration,
) -> Result<String, String> {
    let uhash = uri_hash(url);
    let now = now_epoch_secs();
    let local_route = |chash: &str| format!("/badge-cache/{chash}");

    // Fast path: known URL, body file on disk, not yet stale.
    if let Some(entry) = index.0.get(&uhash) {
        let path = content_hash_to_path(cache_dir, &entry.content_hash);
        if path.exists() {
            if now.saturating_sub(entry.resolved_at) < refresh_interval.as_secs() {
                return Ok(local_route(&entry.content_hash));
            }
            // Stale — conditional GET.
            let mut req = http.get(url);
            if let Some(etag) = &entry.etag {
                req = req.header("If-None-Match", etag.as_str());
            }
            if let Some(lm) = &entry.last_modified {
                req = req.header("If-Modified-Since", lm.as_str());
            }
            let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
            match resp.status() {
                reqwest::StatusCode::NOT_MODIFIED => {
                    let chash = index.0.get(&uhash).map(|e| e.content_hash.clone());
                    index.0.get_mut(&uhash).unwrap().resolved_at = now;
                    save_index(index, cache_dir);
                    return Ok(local_route(chash.as_deref().unwrap_or_default()));
                }
                s if s.is_success() => {
                    let (etag, lm) = revalidation_headers(&resp);
                    let body = resp.bytes().await.map_err(|e| format!("{url}: {e}"))?;
                    let ct = detect_content_type(url, &body);
                    let chash = sha256_hex(&body);
                    persist_body(&body, &ct, &chash, cache_dir);
                    let new_entry = make_entry(
                        url,
                        uhash.clone(),
                        &body,
                        &ct,
                        etag.as_deref(),
                        lm.as_deref(),
                    );
                    index.0.insert(uhash, new_entry);
                    save_index(index, cache_dir);
                    return Ok(local_route(&chash));
                }
                s => {
                    tracing::warn!(%url, status = %s, "badge revalidation failed — serving stale");
                    return Ok(local_route(&entry.content_hash));
                }
            }
        }
        // Cache file gone — fall through to re-fetch below.
    }

    // Cold path: unconditional GET.
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("{url}: HTTP {status}"));
    }
    let (etag, lm) = revalidation_headers(&resp);
    let body = resp.bytes().await.map_err(|e| format!("{url}: {e}"))?;
    let ct = detect_content_type(url, &body);
    let chash = sha256_hex(&body);
    persist_body(&body, &ct, &chash, cache_dir);
    let entry = make_entry(url, uhash, &body, &ct, etag.as_deref(), lm.as_deref());
    index.0.insert(entry.url_hash.clone(), entry);
    save_index(index, cache_dir);
    Ok(local_route(&chash))
}

/// Resolve a file:// URI: canonicalize + read. No caching layer.
fn resolve_file(url: &str) -> Result<String, String> {
    let path_str = url
        .strip_prefix("file://")
        .ok_or_else(|| format!("{url}: must start with file://"))?;
    let canonical = std::fs::canonicalize(path_str).map_err(|e| format!("{url}: {e}"))?;
    if !canonical.exists() {
        return Err(format!("{url}: file not found"));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn persist_body(body: &[u8], _ct: &str, chash: &str, cache_dir: &Path) -> PathBuf {
    let _ = std::fs::create_dir_all(cache_dir);
    let path = content_hash_to_path(cache_dir, chash);
    let _ = std::fs::write(&path, body);
    path
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---- public resolution API ----------------------------------------------

/// Resolve all definitions and wrap with cache state.
pub async fn resolve_full(
    cfg: &CustomBadgesConfig,
    http: &reqwest::Client,
) -> Result<BadgeCacheState, String> {
    let cache_dir = cfg.cache_dir.clone().unwrap_or_else(default_cache_dir);
    let refresh_interval = Duration::from_secs(cfg.refresh_interval_secs);
    let mut index = load_index(&cache_dir);
    let mut definitions = HashMap::new();
    let mut file_badges = HashMap::new();

    for (id, defn) in &cfg.definitions {
        let local_url = if defn.uri.starts_with("file://") {
            let canonical = resolve_file(&defn.uri)?;
            let uh = uri_hash(&defn.uri);
            file_badges.insert(uh.clone(), PathBuf::from(&canonical));
            format!("/badge-file/{uh}")
        } else {
            resolve_http(&defn.uri, http, &mut index, &cache_dir, refresh_interval).await?
        };
        definitions.insert(
            id.clone(),
            ResolvedBadge {
                id: id.clone(),
                url: local_url,
                priority: defn.priority,
                height: defn.height,
                label: defn.label.clone(),
            },
        );
    }

    Ok(BadgeCacheState {
        resolved: ResolvedCustomBadges {
            definitions,
            per_role: cfg.per_role.clone(),
            per_user: cfg.per_user.clone(),
        },
        index,
        cache_dir,
        file_badges,
    })
}

/// Serve the cached badge file. Returns (bytes, content_type).
pub fn read_cached_file(state: &BadgeCacheState, content_hash: &str) -> Option<(Vec<u8>, String)> {
    let path = content_hash_to_path(&state.cache_dir, content_hash);
    let entry = state
        .index
        .0
        .values()
        .find(|e| e.content_hash == content_hash)?;
    let bytes = std::fs::read(&path).ok()?;
    Some((bytes, entry.content_type.clone()))
}

/// Compute the custom badges attached to a message, in render order.
///
/// Union of per-role (for each Twitch badge id the sender carries) and
/// per-user (by Twitch user id OR login), deduplicated, sorted by
/// `priority` ascending (ties broken by insertion order via definition
/// map order). Mirrors the widget's JS merge exactly.
pub fn merge_badges<'a>(
    resolved: &'a ResolvedCustomBadges,
    message_badge_ids: &[&str],
    user_id: &str,
    user_login: &str,
) -> Vec<&'a ResolvedBadge> {
    let mut candidates: Vec<&str> = Vec::new();
    for bid in message_badge_ids {
        if let Some(ids) = resolved.per_role.get(*bid) {
            candidates.extend(ids.iter().map(String::as_str));
        }
    }
    if let Some(ids) = resolved.per_user.get(user_id) {
        candidates.extend(ids.iter().map(String::as_str));
    } else if let Some(ids) = resolved.per_user.get(user_login) {
        candidates.extend(ids.iter().map(String::as_str));
    }

    // Dedup preserving first-seen order.
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|id| seen.insert(*id));

    // Stable sort by priority ascending.
    let mut result: Vec<&ResolvedBadge> = candidates
        .iter()
        .filter_map(|id| resolved.definitions.get(*id))
        .collect();
    result.sort_by_key(|b| b.priority);
    result
}

// ---- tests --------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_deterministic() {
        let a = sha256_hex(b"hello");
        let b = sha256_hex(b"hello");
        assert_eq!(a, b);
        assert_ne!(a, sha256_hex(b"world"));
        assert_eq!(a.len(), 64); // hex-encoded 32 bytes
    }

    #[test]
    fn content_type_detects_common_formats() {
        assert_eq!(detect_content_type("img.png", b""), "image/png");
        assert_eq!(detect_content_type("img.jpg", b""), "image/jpeg");
        assert_eq!(detect_content_type("icon.svg", b""), "image/svg+xml");
        assert_eq!(detect_content_type("x.gif", b""), "image/gif");
    }

    #[test]
    fn sniff_falls_back_when_extension_missing() {
        assert_eq!(
            sniff_content_type(&[0x89, 0x50, 0x4E, 0x47, 0]),
            "image/png"
        );
        assert_eq!(sniff_content_type(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(sniff_content_type(b"RIFF....WEBP"), "image/webp");
        assert_eq!(sniff_content_type(b"<svg></svg>"), "image/svg+xml");
    }

    #[test]
    fn resolve_file_follows_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.png");
        std::fs::write(&real, b"fake png").unwrap();
        let link = tmp.path().join("link.png");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let url = format!("file://{}", link.display());
        let resolved = resolve_file(&url).unwrap();
        // Should follow symlink to canonical path.
        let canonical = std::fs::canonicalize(&link)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolved, canonical);
        assert!(Path::new(&resolved).exists());
    }

    #[test]
    fn file_not_found_returns_err() {
        assert!(resolve_file("file:///nonexistent/path.png").is_err());
    }

    #[test]
    fn priority_merge_order() {
        // Ground truth BY HAND: lower priority first; ties preserve definition order.
        let mut defs = HashMap::new();
        defs.insert(
            "a".to_string(),
            ResolvedBadge {
                id: "a".into(),
                url: "".into(),
                priority: 20,
                height: 24,
                label: None,
            },
        );
        defs.insert(
            "b".to_string(),
            ResolvedBadge {
                id: "b".into(),
                url: "".into(),
                priority: 10,
                height: 24,
                label: None,
            },
        );
        defs.insert(
            "c".to_string(),
            ResolvedBadge {
                id: "c".into(),
                url: "".into(),
                priority: 10,
                height: 24,
                label: None,
            },
        );

        let mut per_role = HashMap::new();
        per_role.insert(
            "moderator".to_string(),
            vec!["a".to_string(), "b".to_string()],
        );
        let mut per_user = HashMap::new();
        per_user.insert("42".to_string(), vec!["c".to_string()]);

        let badges = ResolvedCustomBadges {
            definitions: defs,
            per_role,
            per_user,
        };

        // Mod with user_id=42: badges from both role("a"p20 + "b"p10) and user("c"p10).
        let user_badges = merge_badges(&badges, &["moderator"], "42", "mod42");
        let ids: Vec<&str> = user_badges.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "c", "a"]); // p10s first (b then c in order), then p20
    }

    #[test]
    fn unknown_role_user_keys_are_ignored() {
        let badges = ResolvedCustomBadges::default();
        let merged = merge_badges(&badges, &["unknown_role"], "999", "unknown");
        assert!(merged.is_empty());
    }

    #[test]
    fn empty_role_user_keys_produce_empty_merge() {
        let mut defs = HashMap::new();
        defs.insert(
            "x".to_string(),
            ResolvedBadge {
                id: "x".into(),
                url: "".into(),
                priority: 1,
                height: 24,
                label: None,
            },
        );
        let badges = ResolvedCustomBadges {
            definitions: defs,
            per_role: HashMap::new(),
            per_user: HashMap::new(),
        };
        let merged = merge_badges(&badges, &[], "0", "user0");
        assert!(merged.is_empty());
    }
}
