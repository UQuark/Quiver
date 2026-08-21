//! quiver-chat — OBS-Twitch chat widget engine.
//!
//! Headless tool: reads a RON config, joins a Twitch channel anonymously,
//! serves the widget frontend + WebSocket feed for the OBS browser source.

use std::path::PathBuf;

use clap::Parser;
use quiver_chat::config::{ChatConfig, SAMPLE_CONFIG};
use quiver_config::{Validate, load_from_path, write_schema};

#[derive(Debug, Parser)]
#[command(name = "quiver-chat", about = "OBS-Twitch chat widget engine")]
struct Args {
    /// Path to the RON config file.
    #[arg(long, default_value = "chat.ron")]
    config: PathBuf,

    /// Print a fully commented sample config to stdout and exit.
    #[arg(long)]
    sample_config: bool,

    /// Print the JSON Schema of the config model to stdout and exit.
    /// This is the machine-readable contract the Quiver UI consumes.
    #[arg(long)]
    print_schema: bool,
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    init_tracing();

    if args.sample_config {
        print!("{SAMPLE_CONFIG}");
        return Ok(());
    }

    if args.print_schema {
        let mut stdout = std::io::stdout().lock();
        write_schema::<ChatConfig>(&mut stdout)?;
        return Ok(());
    }

    let cfg: ChatConfig = load_from_path(&args.config)?;

    let issues = cfg.validate();
    if !issues.is_empty() {
        eprintln!("config {} is invalid:", args.config.display());
        for issue in &issues {
            eprintln!("  - {issue}");
        }
        std::process::exit(2);
    }

    tracing::info!(config = %args.config.display(), "configuration loaded and validated");

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(quiver_chat::serve::run(cfg))
}
