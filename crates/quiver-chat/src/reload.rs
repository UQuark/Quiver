//! Config hot reload: file watching + SIGHUP → load/validate/diff/apply.
//!
//! Contract: a reload is all-or-nothing. Parse errors, validation issues,
//! or an invalid channel login REJECT the whole reload — the running
//! config stays untouched. There is no half-applied state, ever.

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
    pub feed_swap: mpsc::UnboundedSender<String>,
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

    crate::config::report_css_lint(new_cfg.theme.custom_css.as_deref());

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
        Action::BroadcastMeta => {
            let badges = ctx.badges.read().map(|b| b.clone()).unwrap_or_default();
            let live_now = ctx
                .live
                .read()
                .map(|l| l.clone())
                .unwrap_or_else(|_| new_live.clone());
            let frame = serde_json::json!({
                "type": "config",
                "meta": crate::serve::meta_value(&live_now, &badges),
            });
            let _ = ctx.tx.send(frame.to_string());
        }
        Action::Rebind => ctx.rebind.notify_one(),
    }
}
