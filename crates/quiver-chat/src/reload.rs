//! Config hot reload: file watching + SIGHUP → load/validate/diff/apply.
//!
//! Contract: a reload is all-or-nothing. Parse errors, validation issues,
//! or an invalid channel login REJECT the whole reload — the running
//! config stays untouched. There is no half-applied state, ever.

use notify::Watcher as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use quiver_config::Validate;

use crate::config::ChatConfig;
use crate::live::{Action, LiveConfig, SharedLive, planned_actions};
use crate::serve::SharedBadges;

const DEBOUNCE: Duration = Duration::from_millis(500);

pub(crate) struct ReloadCtx {
    pub live: SharedLive,
    pub messages: crate::engine::SharedState,
    pub tx: tokio::sync::broadcast::Sender<String>,
    pub badges: SharedBadges,
    pub custom_badges: crate::badges::SharedBadgeCache,
    pub emotes: crate::emotes::SharedEmotes,
    pub filters: crate::filters::SharedCompiled,
    pub feed_swap: mpsc::UnboundedSender<String>,
    pub fe_watch: mpsc::UnboundedSender<Option<PathBuf>>,
    pub rebind: Arc<Notify>,
}

/// Watch the config's parent directory (atomic-save editors REPLACE the
/// file; a directory watch survives that) filtered on the file name.
/// SIGHUP triggers the same pipeline manually.
pub(crate) fn spawn_watcher(config_path: PathBuf, ctx: ReloadCtx, quit: CancellationToken) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<()>();

    // Bridge thread: owns the debouncer (it stops on Drop) and forwards
    // matching events into async-land.
    {
        let watch_file = config_path.clone();
        let file_name = watch_file
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        let dir: PathBuf = watch_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::thread::spawn(move || {
            let (std_tx, std_rx) = std::sync::mpsc::channel();
            let mut debouncer =
                match notify_debouncer_full::new_debouncer(DEBOUNCE, None, move |res| {
                    let _ = std_tx.send(res);
                }) {
                    Ok(d) => d,
                    Err(e) => {
                        warn!(error = %e, "config watcher unavailable — SIGHUP still works");
                        return;
                    }
                };
            if let Err(e) = debouncer.watch(&dir, notify::RecursiveMode::NonRecursive) {
                warn!(dir = %dir.display(), error = %e, "cannot watch config directory");
                return;
            }
            debug!(path = %watch_file.display(), "watching for config changes");
            for res in std_rx {
                match res {
                    Ok(events) => {
                        if events.iter().any(|e| {
                            e.paths
                                .iter()
                                .any(|p| p.file_name() == Some(file_name.as_os_str()))
                                || e.paths.iter().any(|p| p == &watch_file)
                        }) {
                            let _ = event_tx.send(());
                        }
                    }
                    Err(e) => warn!(errors = ?e, "config watch error"),
                }
            }
        });
    }

    tokio::spawn(async move {
        // SIGHUP where available; absent platform = never-firing future.
        #[cfg(unix)]
        let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .map(Some)
            .unwrap_or_else(|e| {
                warn!(error = %e, "SIGHUP handler unavailable");
                None
            });

        loop {
            #[cfg(unix)]
            {
                let hup_fut = async {
                    match hangup.as_mut() {
                        Some(h) => {
                            let _ = h.recv().await;
                            info!("SIGHUP received");
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    _ = quit.cancelled() => return,
                    _ = event_rx.recv() => (),
                    _ = hup_fut => (),
                }
            }
            #[cfg(not(unix))]
            tokio::select! {
                _ = quit.cancelled() => return,
                _ = event_rx.recv() => (),
            }

            apply_reload(&config_path, &ctx).await;
        }
    });
}

async fn apply_reload(path: &Path, ctx: &ReloadCtx) {
    // 1. Load. Any failure rejects the whole reload.
    let new_cfg: ChatConfig = match quiver_config::load_from_path(path) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "config reload rejected (parse failed) — keeping current config");
            return;
        }
    };

    // 2. Semantic validation.
    let issues = new_cfg.validate();
    if !issues.is_empty() {
        let summary: Vec<String> = issues.iter().map(|i| i.to_string()).collect();
        warn!(
            issues = ?summary,
            "config reload rejected (validation failed) — keeping current config"
        );
        return;
    }

    crate::config::report_css_lint(
        new_cfg.theme.custom_css.as_deref(),
        new_cfg.theme.role_css.as_ref(),
    );

    // 3. Diff against what is running.
    let old_live = match ctx.live.read() {
        Ok(l) => l.clone(),
        Err(_) => return,
    };
    let new_live = LiveConfig::from(&new_cfg);
    let actions = planned_actions(&old_live, &new_live);
    if actions.is_empty() {
        debug!("config unchanged");
        return;
    }

    // 4. Pre-flight: reject invalid swap targets BEFORE touching anything.
    if let Some(Action::SwapChannel(ch)) =
        actions.iter().find(|a| matches!(a, Action::SwapChannel(_)))
        && let Err(e) = twitch_irc_validate_channel(ch)
    {
        warn!(
            channel = %ch,
            error = %e,
            "config reload rejected (invalid channel login) — keeping current config"
        );
        return;
    }
    // Same contract for filters: a pattern that cannot compile rejects the
    // whole reload — moderation must never silently stop working.
    if actions.contains(&Action::ApplyFilters)
        && let Err(e) = crate::filters::CompiledFilters::compile(&new_live.filters)
    {
        warn!(
            error = %e,
            "config reload rejected (filters failed to compile) — keeping current config"
        );
        return;
    }

    // 5. Apply in canonical order (see planned_actions docs).
    for action in &actions {
        apply_action(ctx, action, &new_live).await;
    }

    if let Ok(mut live) = ctx.live.write() {
        *live = new_live;
    }
    info!(actions = ?actions, "configuration reloaded");
}

fn twitch_irc_validate_channel(login: &str) -> Result<(), String> {
    quiver_twitch::IrcChatSource::validate_channel_login(login).map_err(|e| e.to_string())
}

/// Frontend-watcher bridge: re-target the watched directory.
enum BridgeMsg {
    SetDir(Option<PathBuf>),
}

/// Watch the widget frontend directory; any change broadcasts ONE
/// coalesced `{type:"reload"}` frame per interval so connected pages
/// refresh themselves. Re-targetable at runtime via control messages
/// (config hot reload may move `server.widget_dist`).
pub(crate) fn spawn_frontend_watcher(
    initial_dir: Option<PathBuf>,
    mut ctrl_rx: mpsc::UnboundedReceiver<Option<PathBuf>>,
    tx: tokio::sync::broadcast::Sender<String>,
) {
    const MIN_INTERVAL: Duration = Duration::from_secs(1);

    // Trigger channel from the bridge thread into async-land.
    let (trig_tx, mut trig_rx) = mpsc::unbounded_channel::<()>();
    let (bridge_tx, bridge_rx) = std::sync::mpsc::channel::<BridgeMsg>();

    // Std thread owns the PollWatchers for the CURRENT dir. Watchers are
    // dropped (and stop watching) whenever the target changes.
    //
    // Raw PollWatcher, no debouncer layer: polling coalesces naturally and
    // the async side rate-limits frames; Debouncer(PollWatcher)+cache proved
    // unreliable on macOS while raw PollWatcher+Recursive works.
    {
        let trig_tx = trig_tx.clone();
        std::thread::spawn(move || {
            // Alive == watching. Underscore: never read, lifetime matters.
            let mut _watchers: Vec<notify::PollWatcher> = Vec::new();
            loop {
                match bridge_rx.recv() {
                    Ok(BridgeMsg::SetDir(dir)) => {
                        _watchers.clear();
                        let Some(d) = dir.filter(|d| d.is_dir()) else {
                            continue;
                        };
                        let tx2 = trig_tx.clone();
                        // Deterministic sub-second latency beats battery here:
                        // the dir is a handful of files and devs want instant
                        // reloads. FSEvents latency measured at 3-7s on this box.
                        let cfg = notify::Config::default()
                            .with_poll_interval(Duration::from_millis(300));
                        match notify::PollWatcher::new(
                            move |res: std::result::Result<notify::Event, notify::Error>| {
                                if res.is_ok() {
                                    let _ = tx2.send(());
                                }
                            },
                            cfg,
                        ) {
                            Ok(mut w) => match w.watch(&d, notify::RecursiveMode::Recursive) {
                                Ok(()) => {
                                    debug!(dir = %d.display(), "watching widget frontend");
                                    _watchers.push(w);
                                }
                                Err(e) => {
                                    warn!(dir = %d.display(), error = %e, "cannot watch widget dir")
                                }
                            },
                            Err(e) => warn!(error = %e, "frontend watcher unavailable"),
                        }
                    }
                    Err(_) => return, // controller dropped: shutdown
                }
            }
        });
    }

    tokio::spawn(async move {
        // Initial target.
        let _ = bridge_tx.send(BridgeMsg::SetDir(initial_dir));

        // Coalescing loop: at most one reload frame per MIN_INTERVAL.
        let mut last_sent: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                msg = ctrl_rx.recv() => {
                    // None = all senders dropped (shutdown).
                    match msg {
                        Some(dir) => { let _ = bridge_tx.send(BridgeMsg::SetDir(dir)); }
                        None => return,
                    }
                }
                _ = trig_rx.recv() => {
                    let now = tokio::time::Instant::now();
                    let due = match last_sent {
                        None => true,
                        Some(t) => now.duration_since(t) >= MIN_INTERVAL,
                    };
                    if due {
                        last_sent = Some(now);
                        info!("widget frontend changed — reloading connected pages");
                        let _ = tx.send(r#"{"type":"reload"}"#.to_string());
                    }
                }
            }
        }
    });
}

async fn apply_action(ctx: &ReloadCtx, action: &Action, new_live: &LiveConfig) {
    match action {
        Action::SetMax(n) => {
            let evicted = ctx
                .messages
                .lock()
                .map(|mut m| m.set_max(*n as usize))
                .unwrap_or_default();
            if !evicted.is_empty() {
                let frame = serde_json::json!({ "type": "expire", "ids": evicted });
                let _ = ctx.tx.send(frame.to_string());
            }
        }
        Action::SetWidgetDist(dir) => {
            // Re-target the frontend watcher; None disables it.
            let _ = ctx.fe_watch.send(dir.clone());
        }
        Action::SwapChannel(channel) => {
            // Supervisor parts/joins and broadcasts {"type":"clear"}.
            let _ = ctx.feed_swap.send(channel.clone());
        }
        Action::RefreshBadges => match &new_live.creds {
            Some((id, secret)) => {
                let map = crate::serve::load_badge_map(id, secret, &new_live.channel).await;
                info!(badge_count = map.len(), "badge map refreshed");
                if let Ok(mut b) = ctx.badges.write() {
                    *b = map;
                }
            }
            None => {
                if let Ok(mut b) = ctx.badges.write() {
                    b.clear();
                }
            }
        },
        Action::RefreshEmotes => {
            let map = crate::emotes::load_third_party_emotes(
                new_live
                    .creds
                    .as_ref()
                    .map(|(a, b)| (a.as_str(), b.as_str())),
                &new_live.channel,
            )
            .await;
            info!(emote_count = map.len(), "third-party emote map refreshed");
            if let Ok(mut e) = ctx.emotes.write() {
                *e = map;
            }
        }
        Action::ApplyFilters => match crate::filters::CompiledFilters::compile(&new_live.filters) {
            Ok(f) => {
                if let Ok(mut g) = ctx.filters.write() {
                    *g = Some(f);
                }
                info!("filters applied");
            }
            Err(e) => warn!(error = %e, "filter compile failed at apply — keeping old filters"),
        },
        Action::BroadcastMeta => {
            // IMPORTANT: build from new_live, NOT ctx.live — during apply,
            // the shared slot still holds the OLD config (it is swapped in
            // only after all actions ran). Reading it here broadcasts the
            // previous state, making every reload appear one-behind.
            let badges = ctx.badges.read().map(|b| b.clone()).unwrap_or_default();
            let custom = ctx
                .custom_badges
                .read()
                .ok()
                .and_then(|g| g.as_ref().map(|c| c.resolved.clone()));
            let frame = serde_json::json!({
                "type": "config",
                "meta": crate::serve::meta_value(new_live, &badges, custom.as_ref()),
            });
            let _ = ctx.tx.send(frame.to_string());
        }
        Action::ResolveBadges => match &new_live.badges {
            Some(badge_cfg) => {
                let http = reqwest::Client::new();
                match crate::badges::resolve_full(badge_cfg, &http).await {
                    Ok(state) => {
                        if let Ok(mut g) = ctx.custom_badges.write() {
                            *g = Some(state);
                        }
                        info!("custom badge cache re-resolved");
                    }
                    Err(e) => {
                        warn!(error = %e, "custom badge re-resolution failed — keeping old cache");
                    }
                }
            }
            None => {
                if let Ok(mut g) = ctx.custom_badges.write() {
                    *g = None;
                }
                info!("custom badges removed");
            }
        },
        Action::Rebind => ctx.rebind.notify_one(),
    }
}
