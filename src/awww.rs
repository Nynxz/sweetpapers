//! Drives the `awww` CLI (via argv, no shell) to paint an image to a monitor.
//!
//! We talk to awww through its CLI, the same way every other tool does.
//! The IPC socket between `awww` and `awww-daemon` is an internal detail of
//! awww that changes between releases, so going through the CLI keeps us
//! version-stable.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, error, info};

use crate::config::Transition;

pub fn paint(transition: &Transition, monitor: &str, image: &Path) -> Result<()> {
    let mut cmd = Command::new("awww");
    cmd.arg("img")
        .arg("-o")
        .arg(monitor)
        .arg(image)
        .arg("--resize")
        .arg(&transition.fill_mode)
        .arg("--transition-type")
        .arg(&transition.transition_type)
        .arg("--transition-duration")
        .arg(transition.transition_duration.to_string())
        .arg("--transition-step")
        .arg(transition.transition_step.to_string())
        .arg("--transition-fps")
        .arg(transition.transition_fps.to_string());
    debug!(monitor, image = %image.display(), "awww img");

    let output = cmd
        .output()
        .with_context(|| "spawning `awww` (is it installed and on PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(
            monitor,
            status = ?output.status.code(),
            stderr = %stderr.trim(),
            "awww failed (is awww-daemon running?)",
        );
        anyhow::bail!("awww img failed for monitor {}: {}", monitor, stderr.trim());
    }

    info!(monitor, image = %image.display(), "painted");
    Ok(())
}
