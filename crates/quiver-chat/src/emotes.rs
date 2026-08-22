//! Third-party emote support (7TV today).
//!
//! The engine owns all Twitch/3rd-party knowledge; the widget just gets a
//! flat `{name: url}` map served from `/emotes.json` and replaces bare
//! tokens in message text with images.

use std::collections::HashMap;

/// Merged third-party emote map shared with the HTTP layer.
pub(crate) type SharedEmotes = std::sync::Arc<std::sync::RwLock<HashMap<String, String>>>;

const SEVENTV_API: &str = "https://7tv.io/v3";
/// CDN file suffix appended to the host url. 2x webp is the sweet spot.
const SEVENTV_SIZE_SUFFIX: &str = "/2x.webp";

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

/// Flatten a set payload into `name -> full CDN url`.
/// Emotes without usable host data are skipped silently.
fn flatten_set(set: &SevenTvSet) -> HashMap<String, String> {
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

/// Fetch the global 7TV set.
async fn fetch_global(http: &reqwest::Client) -> anyhow::Result<HashMap<String, String>> {
    let resp = http
        .get(format!("{SEVENTV_API}/emote-sets/global"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("7tv global set returned {}", resp.status());
    }
    let set: SevenTvSet = resp.json().await?;
    Ok(flatten_set(&set))
}

/// Fetch a channel's 7TV emote set via the channel's Twitch user id.
async fn fetch_channel(
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
    // Top-level `emote_set` holds the ACTIVE set; absent when none linked.
    let Some(set_val) = body.get("emote_set") else {
        return Ok(HashMap::new());
    };
    if set_val.is_null() {
        return Ok(HashMap::new());
    }
    let set: SevenTvSet = serde_json::from_value(set_val.clone())?;
    Ok(flatten_set(&set))
}

/// Build the merged map: global first, then the channel's active set
/// overriding collisions. Channel lookup requires Twitch credentials;
/// without them only the global set is available.
pub(crate) async fn load_third_party_emotes(
    creds: Option<(&str, &str)>,
    channel_login: &str,
) -> HashMap<String, String> {
    let http = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "http client unavailable — no third-party emotes");
            return HashMap::new();
        }
    };

    let mut map = match fetch_global(&http).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "7tv global emotes failed — skipping");
            HashMap::new()
        }
    };

    let Some((client_id, client_secret)) = creds else {
        tracing::info!("no twitch api credentials — channel-specific 7TV emotes disabled");
        return map;
    };

    // Resolve the broadcaster's numeric id through Helix like badges do.
    let helix = match quiver_twitch::HelixClient::new(client_id, client_secret) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, "http client for helix unavailable — global 7TV only");
            return map;
        }
    };
    let broadcaster_id = match helix.user_id(channel_login).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(
                channel = %channel_login,
                error = %e,
                "channel id lookup failed — using global 7TV emotes only"
            );
            return map;
        }
    };

    match fetch_channel(&http, &broadcaster_id).await {
        Ok(m) => {
            tracing::info!(
                global = map.len(),
                channel = m.len(),
                "third-party emote map ready"
            );
            map.extend(m);
        }
        Err(e) => {
            tracing::warn!(error = %e, "channel 7TV emotes failed — global only");
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SET: &str = r#"{
        "id": "set1",
        "emotes": [
            { "name": "wideSpeedNod", "data": { "host": { "url": "//cdn.7tv.app/emote/A" } } },
            { "name": "Degloved",     "data": { "host": { "url": "//cdn.7tv.app/emote/B" } } },
            { "name": "BrokenNoData", "data": null },
            { "name": "MissingHost",  "data": { "other": true } }
        ]
    }"#;

    /// Ground truth BY HAND: two usable entries, broken ones skipped,
    /// url = https: + host.url + /2x.webp.
    #[test]
    fn flattens_seventv_set_with_full_urls() {
        let set: SevenTvSet = serde_json::from_str(SAMPLE_SET).expect("fixture parses");
        let map = flatten_set(&set);

        assert_eq!(map.len(), 2);
        assert_eq!(map["wideSpeedNod"], "https://cdn.7tv.app/emote/A/2x.webp");
        assert_eq!(map["Degloved"], "https://cdn.7tv.app/emote/B/2x.webp");
        assert!(!map.contains_key("BrokenNoData"));
        assert!(!map.contains_key("MissingHost"));
    }

    /// Ground truth BY HAND: channel entries override global on collision.
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
