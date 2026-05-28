//! XDG paths used by the daemon and clients.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// `$XDG_RUNTIME_DIR/sweetpapers.sock`, falling back to `/tmp/` if unset.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("sweetpapers.sock"),
        None => PathBuf::from("/tmp/sweetpapers.sock"),
    }
}

/// `$XDG_CACHE_HOME/sweetpapers/` (or `~/.cache/sweetpapers/`).
pub fn cache_dir() -> Result<PathBuf> {
    let base =
        dirs::cache_dir().context("could not resolve cache directory (XDG_CACHE_HOME or HOME)")?;
    Ok(base.join("sweetpapers"))
}

pub fn thumbnail_cache_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("thumbs"))
}

/// `$XDG_CONFIG_HOME/sweetpapers/config.jsonc` (or
/// `~/.config/sweetpapers/config.jsonc`). Used when `--config` is omitted.
pub fn default_config_path() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .context("could not resolve config directory (XDG_CONFIG_HOME or HOME)")?;
    Ok(base.join("sweetpapers").join("config.jsonc"))
}
