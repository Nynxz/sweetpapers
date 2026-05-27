//! Single-image orientation detection. Uses `imagesize` so we read only the
//! file header (no pixel decode) across PNG/JPEG/WebP/AVIF/GIF/BMP/TIFF/HEIF.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Orientation;

pub fn orientation_of(path: &Path) -> Result<Orientation> {
    let dim = imagesize::size(path)
        .with_context(|| format!("reading dimensions of {}", path.display()))?;
    Ok(if dim.height > dim.width {
        Orientation::Portrait
    } else {
        Orientation::Landscape
    })
}
