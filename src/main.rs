mod awww;
mod client;
mod config;
mod daemon;
mod image_ext;
mod orientation;
mod pack;
mod paths;
mod proto;
mod thumbnail;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::proto::Request;

#[derive(Parser, Debug)]
#[command(
    name = "sweetpapers",
    version,
    about = "Per-monitor wallpaper rotator with a control socket"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,

    /// Print response as raw JSON (client subcommands only).
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the daemon: rotate wallpapers and serve the control socket.
    Daemon {
        /// Path to the config file (JSON / JSONC / JSON5).
        /// Defaults to $XDG_CONFIG_HOME/sweetpapers/config.jsonc.
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Pack to start on.
        /// Defaults to `defaults.pack` from the config if omitted.
        #[arg(short, long)]
        pack: Option<String>,
    },
    /// Show current daemon state.
    Status,
    /// List available packs and their thumbnails.
    List,
    /// Switch the active pack.
    Pack {
        /// Pack name (subdirectory of `defaults.packs_location`).
        name: String,
    },
    /// Advance to the next directory and swap now.
    Next,
    /// Go to the previous directory and swap now.
    Prev,
    /// Pause automatic rotation.
    Pause,
    /// Resume automatic rotation.
    Resume,
    /// Re-read the config file (keeps current pack).
    Reload,
    /// Set the rotation interval in seconds at runtime.
    Interval {
        /// New interval in seconds.
        secs: u64,
    },
    /// Get or regenerate a pack's thumbnail.
    Thumbnail {
        /// Pack name.
        name: String,
        /// Force regeneration, ignoring the cache.
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();
    match cli.command {
        Cmd::Daemon { config, pack } => {
            let config_path = match resolve_config_path(config) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("config error: {:#}", e);
                    return ExitCode::FAILURE;
                }
            };
            match daemon::run(config_path, pack) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("daemon error: {:#}", e);
                    ExitCode::FAILURE
                }
            }
        }
        Cmd::Status => client::dispatch(Request::Status, cli.json),
        Cmd::List => client::dispatch(Request::List, cli.json),
        Cmd::Pack { name } => client::dispatch(Request::Pack { name }, cli.json),
        Cmd::Next => client::dispatch(Request::Next, cli.json),
        Cmd::Prev => client::dispatch(Request::Prev, cli.json),
        Cmd::Pause => client::dispatch(Request::Pause, cli.json),
        Cmd::Resume => client::dispatch(Request::Resume, cli.json),
        Cmd::Reload => client::dispatch(Request::Reload, cli.json),
        Cmd::Interval { secs } => client::dispatch(Request::Interval { secs }, cli.json),
        Cmd::Thumbnail { name, force } => {
            client::dispatch(Request::Thumbnail { name, force }, cli.json)
        }
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn resolve_config_path(arg: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(p) = arg {
        if !p.exists() {
            anyhow::bail!("config file not found: {}", p.display());
        }
        return Ok(p);
    }
    let default = paths::default_config_path()?;
    if !default.exists() {
        anyhow::bail!(
            "no config file at {} (pass --config to use a different path, \
             or copy config.example.jsonc there)",
            default.display()
        );
    }
    Ok(default)
}
