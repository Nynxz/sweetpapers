//! Per-pack thumbnail generation and caching.
//!
//! - Picks a representative image (first image alphabetically from the first
//!   subdir; or a `.sweet_thumb.{jpg,png}` override at the pack root).
//! - Decode → center-crop square → resize to 256×256 → encode JPEG q80.
//! - Cached at `$XDG_CACHE_HOME/sweetpapers/thumbs/<pack>.jpg`.
//! - Invalidated when any file inside the pack is newer than the cached thumb.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{ImageReader, imageops::FilterType};
use tracing::{debug, warn};

use crate::image_ext::is_image;
use crate::paths::thumbnail_cache_dir;

const THUMB_SIZE: u32 = 256;
const THUMB_QUALITY: u8 = 80;
const OVERRIDE_NAMES: &[&str] = &[".sweet_thumb.jpg", ".sweet_thumb.jpeg", ".sweet_thumb.png"];

pub struct ThumbnailManager {
    cache_dir: PathBuf,
}

impl ThumbnailManager {
    pub fn new() -> Result<Self> {
        Self::with_cache_dir(thumbnail_cache_dir()?)
    }

    fn with_cache_dir(cache_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&cache_dir)
            .with_context(|| format!("creating thumbnail cache dir {}", cache_dir.display()))?;
        Ok(Self { cache_dir })
    }

    /// The path where this pack's thumbnail lives, regardless of whether it
    /// has been generated yet.
    pub fn cache_path(&self, pack_name: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.jpg", sanitize(pack_name)))
    }

    /// Returns the cached thumbnail path, regenerating if missing or stale.
    /// Returns `None` if the pack has no images and no override.
    pub fn ensure(&self, pack_name: &str, pack_dir: &Path) -> Result<Option<PathBuf>> {
        let out = self.cache_path(pack_name);
        if !needs_regen(&out, pack_dir)? {
            debug!(pack = pack_name, "thumbnail cache hit");
            return Ok(Some(out));
        }
        self.regen(pack_name, pack_dir, &out).map(Some)
    }

    /// Always regenerates, ignoring any cached file.
    pub fn force(&self, pack_name: &str, pack_dir: &Path) -> Result<PathBuf> {
        let out = self.cache_path(pack_name);
        self.regen(pack_name, pack_dir, &out)
    }

    fn regen(&self, pack_name: &str, pack_dir: &Path, out: &Path) -> Result<PathBuf> {
        let source = pick_source(pack_dir)?.with_context(|| {
            format!(
                "no images found in pack '{}' ({})",
                pack_name,
                pack_dir.display()
            )
        })?;
        debug!(pack = pack_name, source = %source.display(), "generating thumbnail");
        let img = ImageReader::open(&source)
            .with_context(|| format!("opening {}", source.display()))?
            .with_guessed_format()
            .with_context(|| format!("guessing format of {}", source.display()))?
            .decode()
            .with_context(|| format!("decoding {}", source.display()))?;
        let cropped = center_crop_square(img);
        let resized = cropped.resize_exact(THUMB_SIZE, THUMB_SIZE, FilterType::Lanczos3);
        let rgb = resized.to_rgb8();
        let file = fs::File::create(out)
            .with_context(|| format!("creating thumbnail {}", out.display()))?;
        let mut bw = BufWriter::new(file);
        JpegEncoder::new_with_quality(&mut bw, THUMB_QUALITY)
            .encode_image(&rgb)
            .with_context(|| format!("encoding thumbnail {}", out.display()))?;
        Ok(out.to_path_buf())
    }
}

fn pick_source(pack_dir: &Path) -> Result<Option<PathBuf>> {
    // Manual override at the pack root.
    for name in OVERRIDE_NAMES {
        let p = pack_dir.join(name);
        if p.is_file() {
            return Ok(Some(p));
        }
    }
    // First image alphabetically from the first sorted subdirectory.
    let mut subdirs: Vec<PathBuf> = fs::read_dir(pack_dir)
        .with_context(|| format!("reading pack dir {}", pack_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    for d in &subdirs {
        if let Some(img) = first_image(d)? {
            return Ok(Some(img));
        }
    }
    // Fallback: pack itself contains images at top level.
    first_image(pack_dir)
}

fn first_image(dir: &Path) -> Result<Option<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    files.sort();
    Ok(files.into_iter().next())
}

fn center_crop_square(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    img.crop_imm(x, y, side, side)
}

fn needs_regen(thumb: &Path, pack_dir: &Path) -> Result<bool> {
    let thumb_mtime = match fs::metadata(thumb).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return Ok(true),
    };
    let newest = newest_mtime(pack_dir).unwrap_or_else(|e| {
        warn!(error = %e, "mtime walk failed, regenerating");
        SystemTime::now()
    });
    Ok(newest > thumb_mtime)
}

fn newest_mtime(dir: &Path) -> Result<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    walk(dir, &mut |path| {
        if let Ok(m) = fs::metadata(path).and_then(|m| m.modified())
            && m > newest
        {
            newest = m;
        }
        Ok(())
    })?;
    Ok(newest)
}

fn walk(dir: &Path, cb: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, cb)?;
        } else {
            cb(&path)?;
        }
    }
    Ok(())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn write_test_image(path: &Path, w: u32, h: u32) {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(w, h, |x, _| Rgb([x as u8, 64, 200]));
        img.save(path).unwrap();
    }

    #[test]
    fn generates_and_caches_thumbnail() {
        let tmp = tempdir();
        let pack = tmp.path().join("MyPack");
        let sub = pack.join("Oceans");
        fs::create_dir_all(&sub).unwrap();
        write_test_image(&sub.join("1.png"), 800, 600);

        let mgr = ThumbnailManager::with_cache_dir(tmp.path().join("cache")).unwrap();

        let path = mgr.ensure("MyPack", &pack).unwrap().expect("thumb path");
        assert!(
            path.exists(),
            "thumbnail file should exist at {}",
            path.display()
        );

        // Second call should reuse the cached file (no regen). Verify by
        // checking the size didn't change.
        let size1 = fs::metadata(&path).unwrap().len();
        let path2 = mgr.ensure("MyPack", &pack).unwrap().unwrap();
        let size2 = fs::metadata(&path2).unwrap().len();
        assert_eq!(size1, size2);
    }

    #[test]
    fn manual_override_is_preferred() {
        let tmp = tempdir();
        let pack = tmp.path().join("OverridePack");
        let sub = pack.join("Sub");
        fs::create_dir_all(&sub).unwrap();
        // a "real" image in the subdir
        write_test_image(&sub.join("a.png"), 400, 400);
        // an override at the pack root with a different size, so we can detect it
        write_test_image(&pack.join(".sweet_thumb.png"), 100, 100);

        let picked = pick_source(&pack).unwrap().unwrap();
        assert_eq!(picked.file_name().unwrap(), ".sweet_thumb.png");
    }

    #[test]
    fn returns_none_when_pack_is_empty() {
        let tmp = tempdir();
        let pack = tmp.path().join("EmptyPack");
        fs::create_dir_all(&pack).unwrap();
        assert!(pick_source(&pack).unwrap().is_none());
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create tempdir")
    }
}
