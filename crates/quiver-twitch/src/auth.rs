//! Channel-scoped OAuth (user access tokens) via Twitch Device Code Flow.
//!
//! DCF needs no redirect server: the tool prints a device code, the user
//! approves on any device, and we poll for the token pair. Tokens persist to
//! `$XDG_CONFIG_HOME/quiver/oauth.json` (or `~/.config/quiver/oauth.json`) —
//! NEVER in the RON config: refresh tokens outlive the config file.
//!
//! Secrets never leave this module; nothing here logs credentials.

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;

use crate::helix::HelixError;

const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
/// Refresh margin: treat access tokens expiring within this window as expired.
const TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The full scope set Quiver can consume once a channel token exists:
/// redemptions (read + manage), hype trains, predictions, polls,
/// subscriptions, follows, goals, charity, ads, moderation read, chat read.
pub const DEFAULT_SCOPES: &[&str] = &[
    "channel:read:redemptions",
    "channel:manage:redemptions",
    "channel:read:hype_train",
    "channel:read:predictions",
    "channel:read:polls",
    "channel:read:subscriptions",
    "moderator:read:followers",
    "channel:read:goals",
    "channel:read:charity",
    "channel:read:ads",
    "moderation:read",
    "chat:read",
    "user:read:chat",
];

/// Persisted token pair + metadata (JSON on disk).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredTokens {
    pub client_id: String,
    /// Login of the user that approved the flow, when resolvable.
    pub channel_login: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix epoch seconds when the access token expires (0 = unknown).
    pub expires_at: u64,
    pub scopes: Vec<String>,
}

/// The un-authenticated half of Device Code Flow.
pub struct DeviceFlow<'a> {
    http: &'a reqwest::Client,
    client_id: &'a str,
    client_secret: Option<&'a str>,
}

/// A device authorization challenge the user must approve.
#[derive(Debug)]
pub struct DeviceChallenge {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    /// Minimum seconds between token polls.
    pub interval: u64,
}

/// Poll outcome for the user's approval.
#[derive(Debug)]
pub enum TokenPoll {
    /// Tokens granted.
    Granted(StoredTokens),
    /// Still waiting for the user to approve — poll again after `interval`.
    Pending,
}

impl<'a> DeviceFlow<'a> {
    pub fn new(
        http: &'a reqwest::Client,
        client_id: &'a str,
        client_secret: Option<&'a str>,
    ) -> Self {
        Self {
            http,
            client_id,
            client_secret,
        }
    }

    /// Start the flow: returns the code to print + URI for the user.
    pub async fn start(&self, scopes: &[&str]) -> Result<DeviceChallenge, HelixError> {
        #[derive(serde::Deserialize)]
        struct DeviceResponse {
            device_code: String,
            user_code: String,
            verification_uri: String,
            expires_in: u64,
            interval: u64,
        }

        // Twitch docs are inconsistent on the scopes parameter name (table:
        // `scopes`, example: `scope`) — send both; unknown params are ignored.
        let joined = scopes.join(" ");
        let mut form: Vec<(&str, &str)> = vec![
            ("client_id", self.client_id),
            ("scopes", &joined),
            ("scope", &joined),
        ];
        if let Some(secret) = self.client_secret {
            form.push(("client_secret", secret));
        }

        let resp = self.http.post(DEVICE_URL).form(&form).send().await?;
        let status = resp.status();
        if !status.is_success() {
            // Surface Twitch's actual reason (invalid_client, invalid scope,
            // device flow not enabled for this app, ...) — a bare 400 hides it.
            let body = resp.text().await.unwrap_or_default();
            return Err(HelixError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let resp: DeviceResponse = resp.json().await?;
        Ok(DeviceChallenge {
            device_code: resp.device_code,
            user_code: resp.user_code,
            verification_uri: resp.verification_uri,
            expires_in: resp.expires_in,
            interval: resp.interval,
        })
    }

    /// Poll for the user's approval.
    pub async fn poll(&self, device_code: &str) -> Result<TokenPoll, HelixError> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: u64,
            /// Twitch returns scope as a JSON ARRAY of strings.
            #[serde(default)]
            scope: Vec<String>,
        }

        #[derive(serde::Deserialize)]
        struct ErrorBody {
            message: Option<String>,
        }

        // NOTE: one .form() call — a second call would REPLACE the body,
        // silently dropping client_id/device_code (the "missing client id"
        // 400). Secret rides in the same form.
        let mut form: Vec<(&str, &str)> = vec![
            ("client_id", self.client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ];
        if let Some(secret) = self.client_secret {
            form.push(("client_secret", secret));
        }

        let resp = self.http.post(TOKEN_URL).form(&form).send().await?;
        let status = resp.status();
        if status.is_success() {
            let t: TokenResponse = resp.json().await?;
            return Ok(TokenPoll::Granted(StoredTokens {
                client_id: self.client_id.to_string(),
                channel_login: None,
                access_token: t.access_token,
                refresh_token: t.refresh_token,
                expires_at: now_epoch() + t.expires_in,
                scopes: t.scope,
            }));
        }

        let body: ErrorBody = resp.json().await.unwrap_or(ErrorBody { message: None });
        let message = body.message.unwrap_or_default();
        match message.as_str() {
            // Still awaiting approval / polled too fast — keep polling.
            "authorization_pending" | "slow_down" => Ok(TokenPoll::Pending),
            // Terminal failures: surface them.
            "access_denied" => Err(HelixError::Api {
                status: status.as_u16(),
                body: "user denied authorization".to_string(),
            }),
            "expired_token" => Err(HelixError::Api {
                status: status.as_u16(),
                body: "device code expired — restart `--auth`".to_string(),
            }),
            _ => Err(HelixError::Api {
                status: status.as_u16(),
                body: message,
            }),
        }
    }
}

/// A user-scoped token with refresh + persistence, loaded from (and written
/// back to) the on-disk OAuth store. Cloneable so call sites can hold a
/// handle without keeping the parent client's lock alive.
pub struct ChannelToken {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    store_path: PathBuf,
    state: RwLock<Option<StoredTokens>>,
}

impl Clone for ChannelToken {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            store_path: self.store_path.clone(),
            state: RwLock::new(self.state.read().unwrap().clone()),
        }
    }
}

impl ChannelToken {
    /// Path of the OAuth store for the configured client identity.
    pub fn store_path() -> PathBuf {
        config_dir().join("quiver").join("oauth.json")
    }

    /// Load an existing store; `None` when absent, invalid, or for another
    /// client_id.
    pub fn load(
        http: reqwest::Client,
        client_id: &str,
        client_secret: &str,
    ) -> Option<Self> {
        let store_path = Self::store_path();
        let text = std::fs::read_to_string(&store_path).ok()?;
        let store: StoredTokens = serde_json::from_str(&text).ok()?;
        if store.client_id != client_id {
            return None;
        }
        Some(Self::from_store(http, client_id, client_secret, store))
    }

    /// Wrap an already-obtained token pair (e.g. right after `--auth`).
    pub fn from_store(
        http: reqwest::Client,
        client_id: &str,
        client_secret: &str,
        store: StoredTokens,
    ) -> Self {
        Self {
            http,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            store_path: Self::store_path(),
            state: RwLock::new(Some(store)),
        }
    }

    pub fn is_present(&self) -> bool {
        self.state.read().unwrap().is_some()
    }

    pub fn channel_login(&self) -> Option<String> {
        self.state
            .read()
            .unwrap()
            .as_ref()
            .and_then(|s| s.channel_login.clone())
    }

    pub fn scopes(&self) -> Vec<String> {
        self.state
            .read()
            .unwrap()
            .as_ref()
            .map(|s| s.scopes.clone())
            .unwrap_or_default()
    }

    /// A valid user access token, refreshing (and persisting) when stale.
    ///
    /// Guards are never held across `.await` (std RwLock guards are !Send);
    /// the refresh happens between short critical sections.
    pub async fn access_token(&self) -> Result<String, HelixError> {
        // Fast path: cached token still valid past the refresh margin.
        {
            let guard = self.state.read().unwrap();
            if let Some(store) = guard.as_ref()
                && token_is_fresh(store.expires_at)
            {
                return Ok(store.access_token.clone());
            }
        }

        // Refresh: extract the refresh token under a short lock, drop the
        // guard, then do the network round-trip.
        let refresh_token = {
            let mut guard = self.state.write().unwrap();
            match guard.as_mut() {
                None => {
                    return Err(HelixError::ChannelAuthRequired(
                        "channel oauth not configured — run `quiver-chat --auth`".to_string(),
                    ))
                }
                Some(store) => {
                    if token_is_fresh(store.expires_at) {
                        // Another task refreshed it in the meantime.
                        return Ok(store.access_token.clone());
                    }
                    if store.refresh_token.is_empty() {
                        return Err(HelixError::ChannelAuthRequired(
                            "refresh token missing — re-run `quiver-chat --auth`".to_string(),
                        ));
                    }
                    store.refresh_token.clone()
                }
            }
        };

        #[derive(serde::Deserialize)]
        struct RefreshResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            expires_in: u64,
        }

        let resp = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
            ])
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(HelixError::ChannelAuthRequired(format!(
                "token refresh failed (status {status}) — re-run `quiver-chat --auth`: {body}"
            )));
        }
        let t: RefreshResponse = resp.json().await?;

        // Store the fresh pair (rotation-safe) and persist.
        {
            let mut guard = self.state.write().unwrap();
            if let Some(store) = guard.as_mut() {
                store.access_token = t.access_token.clone();
                if let Some(rt) = t.refresh_token {
                    store.refresh_token = rt;
                }
                store.expires_at = now_epoch() + t.expires_in;
            }
        }
        self.persist().ok();
        Ok(t.access_token)
    }

    /// Persist current tokens to the store, atomically (tmp + rename).
    pub fn persist(&self) -> std::io::Result<()> {
        let Some(store) = self.state.read().unwrap().clone() else {
            return Ok(());
        };
        if let Some(parent) = self.store_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&store)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let tmp = self.store_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        // oauth.json is a credential file: 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.store_path)
    }

    /// Set the channel login post-authorization (resolves via /helix/users).
    pub fn set_channel_login(&self, login: String) {
        if let Some(store) = self.state.write().unwrap().as_mut() {
            store.channel_login = Some(login);
        }
    }
}

fn token_is_fresh(expires_at_epoch: u64) -> bool {
    expires_at_epoch > 0 && expires_at_epoch.saturating_sub(now_epoch()) > TOKEN_EXPIRY_MARGIN.as_secs()
}

// Token store helpers ------------------------------------------------------

/// Persist a token pair to the store, atomically (tmp + rename), 0600.
pub fn persist_tokens(store: &StoredTokens) -> std::io::Result<()> {
    let path = ChannelToken::store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path)
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_tokens_roundtrip_json() {
        let t = StoredTokens {
            client_id: "cid".into(),
            channel_login: Some("likh_tar".into()),
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: 12345,
            scopes: vec!["channel:read:redemptions".into()],
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: StoredTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, "at");
        assert_eq!(back.channel_login.as_deref(), Some("likh_tar"));
        assert_eq!(back.scopes.len(), 1);
    }

    #[test]
    fn freshness_window() {
        let now = now_epoch();
        assert!(token_is_fresh(now + 1000), "far future is fresh");
        assert!(!token_is_fresh(now), "expiring now is stale");
        assert!(!token_is_fresh(0), "unknown expiry is treated stale (forces refresh)");
        assert!(!token_is_fresh(now + 30), "inside the 60s margin is stale");
    }

    #[test]
    fn default_scopes_are_unique_and_sorted() {
        let mut scopes = DEFAULT_SCOPES.to_vec();
        let unique = scopes.clone();
        scopes.sort_unstable();
        scopes.dedup();
        assert_eq!(scopes.len(), unique.len(), "DEFAULT_SCOPES has duplicates");
    }
}