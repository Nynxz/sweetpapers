//! File-extension predicate for the image formats we support.

use std::path::Path;

pub const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];

pub fn is_image(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) => EXTENSIONS.iter().any(|e| *e == ext),
        None => false,
    }
}
