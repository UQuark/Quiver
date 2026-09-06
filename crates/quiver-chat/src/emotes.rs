//! Third-party emote support: 7TV, BetterTTV, FrankerFaceZ.
//!
//! The engine owns all platform knowledge; the widget gets a per-provider
//! `{name: url}` map served from `/emotes.json` and replaces bare tokens
//! in message text with images. Provider enable/disable is widget-facing
//! (via meta flags) so the map ALWAYS contains every provider — disabling
//! means the widget strips those tokens instead of imaging them.

use std::collections::HashMap;

/// provider tag -> {emote name -> image url}
pub(crate) type ProviderMaps = HashMap<String, HashMap<String, String>>;

/// Merged third-party emote maps shared with the HTTP layer.
pub(crate) type SharedEmotes = std::sync::Arc<std::sync::RwLock<ProviderMaps>>;

pub(crate) const PROVIDERS: [&str; 3] = ["seventv", "bttv", "ffz"];

const SEVENTV_API: &str = "https://7tv.io/v3";
const SEVENTV_SIZE_SUFFIX: &str = "/2x.webp";

const BTTV_API: &str = "https://api.betterttv.net/3/cached";
const BTTV_URL: &str = "https://cdn.betterttv.net/emote";

const FFZ_API: &str = "https://api.frankerfacez.com/v1";

// ---- shared serde shapes ---------------------------------------------------

#[derive(serde::Deserialize)]
struct SevenTvSet {
    #[serde(default)]
    emotes: Vec<SevenTvEmote>,
}

#[derive(serde::Deserialize)]
struct SevenTvEmote {
    name: String,
    #[serde(default)]
    data: Option<SevenTvEmoteData>,
}

#[derive(serde::Deserialize)]
struct SevenTvEmoteData {
    #[serde(default)]
    host: Option<SevenTvHost>,
}

#[derive(serde::Deserialize)]
struct SevenTvHost {
    url: String,
}

#[derive(serde::Deserialize)]
struct BttvEmote {
    id: String,
    code: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BttvUser {
    #[serde(default)]
    channel_emotes: Vec<BttvEmote>,
    #[serde(default)]
    shared_emotes: Vec<BttvEmote>,
}

#[derive(serde::Deserialize)]
struct FfzEmoticon {
    name: String,
    urls: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct FfzSet {
    #[serde(default)]
    emoticons: Vec<FfzEmoticon>,
}

#[derive(serde::Deserialize)]
struct FfzSetsBody {
    #[serde(default)]
    sets: HashMap<String, FfzSet>,
}

// ---- 7TV -------------------------------------------------------------------

fn flatten_seventv(set: &SevenTvSet) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for e in &set.emotes {
        let Some(url) = e
            .data
            .as_ref()
            .and_then(|d| d.host.as_ref())
            .map(|h| format!("https:{}{}", h.url, SEVENTV_SIZE_SUFFIX))
        else {
            continue;
        };
        out.insert(e.name.clone(), url);
    }
    out
}

async fn fetch_seventv_global(http: &reqwest::Client) -> anyhow::Result<HashMap<String, String>> {
    let resp = http
        .get(format!("{SEVENTV_API}/emote-sets/global"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("7tv global set returned {}", resp.status());
    }
    let set: SevenTvSet = resp.json().await?;
    Ok(flatten_seventv(&set))
}

async fn fetch_seventv_channel(
    http: &reqwest::Client,
    broadcaster_id: &str,
) -> anyhow::Result<HashMap<String, String>> {
    let resp = http
        .get(format!("{SEVENTV_API}/users/twitch/{broadcaster_id}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "7tv user lookup returned {} for {broadcaster_id}",
            resp.status()
        );
    }
    let body: serde_json::Value = resp.json().await?;
    // Top-level `emote_set` holds the ACTIVE set; absent/null when none linked.
    let Some(set_val) = body.get("emote_set").filter(|v| !v.is_null()) else {
        return Ok(HashMap::new());
    };
    let set: SevenTvSet = serde_json::from_value(set_val.clone())?;
    Ok(flatten_seventv(&set))
}

// ---- BTTV ------------------------------------------------------------------

async fn fetch_bttv(
    http: &reqwest::Client,
    broadcaster_id: Option<&str>,
) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();

    let global: Vec<BttvEmote> = reqwest_get(http, format!("{BTTV_API}/emotes/global")).await?;
    for e in global {
        out.insert(e.code, format!("{BTTV_URL}/{}/2x.webp", e.id));
    }

    if let Some(bid) = broadcaster_id {
        let user: BttvUser = reqwest_get(http, format!("{BTTV_API}/users/twitch/{bid}")).await?;
        for e in user.channel_emotes.into_iter().chain(user.shared_emotes) {
            out.insert(e.code, format!("{BTTV_URL}/{}/2x.webp", e.id));
        }
    }
    Ok(out)
}

// ---- FFZ -------------------------------------------------------------------

/// Pick the best URL variant: prefer 2x, fall back to anything present.
fn ffz_pick_url(urls: &HashMap<String, String>) -> Option<String> {
    ["2", "4", "1"]
        .iter()
        .find_map(|k| urls.get(*k).cloned())
        .or_else(|| urls.values().next().cloned())
}

fn flatten_ffz_sets(value: serde_json::Value, wanted_sets: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(body) = serde_json::from_value::<FfzSetsBody>(value) else {
        return out;
    };
    for sid in wanted_sets {
        if let Some(set) = body.sets.get(sid) {
            for e in &set.emoticons {
                if let Some(url) = ffz_pick_url(&e.urls) {
                    out.insert(e.name.clone(), url);
                }
            }
        }
    }
    out
}

async fn fetch_ffz(
    http: &reqwest::Client,
    broadcaster_id: Option<&str>,
) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();

    // Global sets are listed under default_sets.
    let global: serde_json::Value = reqwest_get(http, format!("{FFZ_API}/set/global")).await?;
    let default_sets: Vec<String> = global
        .get("default_sets")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    out.extend(flatten_ffz_sets(global, &default_sets));

    if let Some(bid) = broadcaster_id {
        let room: serde_json::Value = reqwest_get(http, format!("{FFZ_API}/room/id/{bid}")).await?;
        let active_set = room
            .pointer("/room/set")
            .and_then(|v| v.as_i64())
            .map(|n| n.to_string());
        out.extend(flatten_ffz_sets(
            room,
            &active_set.iter().cloned().collect::<Vec<_>>(),
        ));
    }
    Ok(out)
}

// ---- orchestration ---------------------------------------------------------

async fn reqwest_get<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: String,
) -> anyhow::Result<T> {
    let resp = http.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("upstream returned {}", resp.status());
    }
    Ok(resp.json().await?)
}

/// Build every provider's map. Channel-specific parts require Twitch
/// credentials (numeric id resolution); failures degrade that part only.
pub(crate) async fn load_third_party_emotes(
    creds: Option<(&str, &str)>,
    channel_login: &str,
) -> ProviderMaps {
    let mut providers: ProviderMaps = PROVIDERS
        .iter()
        .map(|p| ((*p).to_string(), HashMap::new()))
        .collect();

    let http = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "http client unavailable — no third-party emotes");
            return providers;
        }
    };

    // Resolve the broadcaster id once; shared by channel-specific lookups.
    let broadcaster_id: Option<String> = match creds {
        Some((client_id, client_secret)) => {
            let helix = quiver_twitch::HelixClient::new(client_id, client_secret);
            match helix {
                Ok(h) => match h.user_id(channel_login).await {
                    Ok(id) => Some(id),
                    Err(e) => {
                        tracing::warn!(
                            channel = %channel_login,
                            error = %e,
                            "channel id lookup failed — global third-party emotes only"
                        );
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "helix http unavailable");
                    None
                }
            }
        }
        None => {
            tracing::info!("no twitch api credentials — channel-specific emotes disabled");
            None
        }
    };

    macro_rules! fill {
        ($provider:literal, $fut:expr) => {{
            match $fut.await {
                Ok(m) => {
                    providers.insert($provider.to_string(), m);
                }
                Err(e) => {
                    tracing::warn!(provider = $provider, error = %e, "emote fetch failed");
                }
            }
        }};
    }

    fill!("seventv", async {
        let mut m = fetch_seventv_global(&http).await.unwrap_or_default();
        if let Some(bid) = &broadcaster_id
            && let Ok(c) = fetch_seventv_channel(&http, bid).await
        {
            m.extend(c);
        }
        Ok::<_, anyhow::Error>(m)
    });
    fill!("bttv", fetch_bttv(&http, broadcaster_id.as_deref()));
    fill!("ffz", fetch_ffz(&http, broadcaster_id.as_deref()));

    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground truth BY HAND: two usable entries, broken ones skipped,
    /// url = https: + host.url + /2x.webp.
    #[test]
    fn flattens_seventv_set_with_full_urls() {
        let sample = r#"{
            "emotes": [
                { "name": "wideSpeedNod", "data": { "host": { "url": "//cdn.7tv.app/emote/A" } } },
                { "name": "Degloved",     "data": { "host": { "url": "//cdn.7tv.app/emote/B" } } },
                { "name": "BrokenNoData", "data": null },
                { "name": "MissingHost",  "data": { "other": true } }
            ]
        }"#;
        let set: SevenTvSet = serde_json::from_str(sample).expect("fixture parses");
        let map = flatten_seventv(&set);

        assert_eq!(map.len(), 2);
        assert_eq!(map["wideSpeedNod"], "https://cdn.7tv.app/emote/A/2x.webp");
        assert_eq!(map["Degloved"], "https://cdn.7tv.app/emote/B/2x.webp");
        assert!(!map.contains_key("BrokenNoData"));
    }

    /// Ground truth BY HAND: bttv entries assemble CDN urls from ids.
    #[test]
    fn flattens_bttv_emotes() {
        let sample = r#"[
            { "id": "abc123", "code": ":tf:" },
            { "id": "def456", "code": "Sadge" }
        ]"#;
        let list: Vec<BttvEmote> = serde_json::from_str(sample).expect("fixture parses");
        let mut map = HashMap::new();
        for e in list {
            map.insert(e.code.clone(), format!("{BTTV_URL}/{}/2x.webp", e.id));
        }
        assert_eq!(
            map[":tf:"],
            "https://cdn.betterttv.net/emote/abc123/2x.webp"
        );
        assert_eq!(
            map["Sadge"],
            "https://cdn.betterttv.net/emote/def456/2x.webp"
        );
    }

    /// Regression for #33: the BTTV user endpoint returns camelCase keys
    /// (`channelEmotes` / `sharedEmotes`) — verified live against
    /// https://api.betterttv.net/3/cached/users/twitch/{id}. Without the
    /// rename_all, serde silently produced empty vecs for both and only
    /// global BTTV emotes ever worked.
    #[test]
    fn bttv_user_deserializes_camelcase_channel_and_shared_emotes() {
        let sample = r#"{
            "id": "5b1e6093da2d8b3f2c2f3e9a",
            "bots": [],
            "avatar": "",
            "channelEmotes": [
                { "id": "abc123", "code": "channelOnly" }
            ],
            "sharedEmotes": [
                { "id": "def456", "code": "sharedOnly" }
            ]
        }"#;
        let user: BttvUser = serde_json::from_str(sample).expect("camelCase fixture parses");
        assert_eq!(user.channel_emotes.len(), 1);
        assert_eq!(user.channel_emotes[0].code, "channelOnly");
        assert_eq!(user.channel_emotes[0].id, "abc123");
        assert_eq!(user.shared_emotes.len(), 1);
        assert_eq!(user.shared_emotes[0].code, "sharedOnly");
        assert_eq!(user.shared_emotes[0].id, "def456");
    }

    /// Ground truth BY HAND: ffz picks the "2" variant when present,
    /// falls back to any available variant.
    #[test]
    fn ffz_url_selection_prefers_2x() {
        let sample = r#"{
            "sets": {
                "166907": { "emoticons": [
                    { "name": "WideHard", "urls": {
                        "1": "https://cdn.frankerfacez.com/emote/246878/1",
                        "2": "https://cdn.frankerfacez.com/emote/246878/2" } },
                    { "name": "Only4x", "urls": { "4": "https://cdn.frankerfacez.com/emote/9/4" } }
                ]}
            }
        }"#;
        let value: serde_json::Value = serde_json::from_str(sample).expect("fixture parses");
        let map = flatten_ffz_sets(value, &["166907".to_string()]);

        assert_eq!(map.len(), 2);
        assert_eq!(
            map["WideHard"],
            "https://cdn.frankerfacez.com/emote/246878/2"
        );
        assert_eq!(map["Only4x"], "https://cdn.frankerfacez.com/emote/9/4");
    }

    /// Ground truth BY HAND: channel overrides global within one provider.
    #[test]
    fn channel_overrides_global() {
        let mut global = HashMap::new();
        global.insert("shared".to_string(), "global".to_string());
        global.insert("g_only".to_string(), "global".to_string());

        let mut channel = HashMap::new();
        channel.insert("shared".to_string(), "channel".to_string());
        channel.insert("c_only".to_string(), "channel".to_string());

        global.extend(channel);
        assert_eq!(global["shared"], "channel");
        assert_eq!(global.len(), 3);
    }
}
