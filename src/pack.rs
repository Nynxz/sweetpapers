//! Pack discovery and image selection.
//!
//! Two selection modes:
//!
//! - **Auto** (`defaults.auto = true`): each screen receives a randomly
//!   chosen image whose orientation matches the screen. Falls back to the
//!   opposite orientation only if the matching bucket is empty.
//! - **Alphapack** (`defaults.auto = false`): files are grouped by their
//!   numeric filename prefix (`1.jpg`, `2_3.png` → groups 1 and 2). The
//!   group key maps to the screen key in the config.
//!
//! Both modes return `Vec<(screen_key, image_path)>`. The daemon applies
//! them in `config.ordered_screen_keys()` order.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rand::seq::IndexedRandom;
use tracing::{debug, warn};

use crate::config::{Config, NextMode, Orientation};
use crate::image_ext::is_image;
use crate::orientation::orientation_of;

/// A loaded pack: its sorted subdirectories.
#[derive(Debug, Clone)]
pub struct Pack {
    pub directories: Vec<PathBuf>,
}

impl Pack {
    pub fn load(name: &str, root: PathBuf) -> Result<Self> {
        if !root.is_dir() {
            anyhow::bail!("pack directory does not exist: {}", root.display());
        }
        let mut directories: Vec<PathBuf> = fs::read_dir(&root)
            .with_context(|| format!("reading pack dir {}", root.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        directories.sort();
        if directories.is_empty() {
            anyhow::bail!("pack '{name}' has no subdirectories");
        }
        Ok(Self { directories })
    }

    /// Pick the directory to use for swap index `i`.
    ///
    /// - `Ordered`: directories sorted, advancing by `i`.
    /// - `Random`: a directory other than the current one if possible.
    pub fn directory_at(&self, mode: NextMode, i: usize) -> &Path {
        let n = self.directories.len();
        let current = i % n;
        match mode {
            NextMode::Ordered => &self.directories[current],
            NextMode::Random if n > 1 => {
                let candidates: Vec<&PathBuf> = self
                    .directories
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, p)| (idx != current).then_some(p))
                    .collect();
                candidates
                    .choose(&mut rand::rng())
                    .map(|p| p.as_path())
                    .unwrap_or(&self.directories[current])
            }
            NextMode::Random => &self.directories[current],
        }
    }
}

// ---- Orientation cache ----------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct OrientationBuckets {
    pub landscape: Vec<PathBuf>,
    pub portrait: Vec<PathBuf>,
}

impl OrientationBuckets {
    pub fn bucket(&self, orientation: Orientation) -> &[PathBuf] {
        match orientation {
            Orientation::Landscape => &self.landscape,
            Orientation::Portrait => &self.portrait,
        }
    }
}

#[derive(Debug, Default)]
pub struct OrientationCache {
    by_dir: HashMap<PathBuf, OrientationBuckets>,
}

impl OrientationCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate_all(&mut self) {
        self.by_dir.clear();
    }

    /// Returns (or computes) the orientation buckets for a directory.
    pub fn buckets_for(&mut self, dir: &Path) -> Result<&OrientationBuckets> {
        let key = dir.to_path_buf();
        if !self.by_dir.contains_key(&key) {
            debug!(dir = %dir.display(), "computing orientation buckets");
            let buckets = compute_buckets(dir)?;
            self.by_dir.insert(key.clone(), buckets);
        }
        Ok(&self.by_dir[&key])
    }
}

fn compute_buckets(dir: &Path) -> Result<OrientationBuckets> {
    let mut buckets = OrientationBuckets::default();
    for path in list_images(dir)? {
        match orientation_of(&path) {
            Ok(Orientation::Landscape) => buckets.landscape.push(path),
            Ok(Orientation::Portrait) => buckets.portrait.push(path),
            Err(e) => warn!(path = %path.display(), error = %e, "skipping unreadable image"),
        }
    }
    buckets.landscape.sort();
    buckets.portrait.sort();
    Ok(buckets)
}

// ---- Image selection ------------------------------------------------------

pub type Selection = Vec<(String, PathBuf)>;

/// Auto mode: pick one image per screen matching its orientation, avoiding
/// `last_images` where the bucket has room.
pub fn pick_by_orientation(
    buckets: &OrientationBuckets,
    config: &Config,
    last_images: &[PathBuf],
) -> Result<Selection> {
    let mut out: Selection = Vec::with_capacity(config.screens.len());
    let mut rng = rand::rng();

    for (screen_key, screen) in &config.screens {
        // Prefer the matching bucket; fall back if empty.
        let primary = buckets.bucket(screen.orientation);
        let bucket: &[PathBuf] = if primary.is_empty() {
            warn!(
                screen = screen_key,
                orientation = ?screen.orientation,
                "no images for screen orientation; falling back"
            );
            buckets.bucket(opposite(screen.orientation))
        } else {
            primary
        };

        if bucket.is_empty() {
            anyhow::bail!("no images available for screen '{screen_key}'");
        }

        // Exclude last_images when possible; otherwise pick from the full bucket.
        let pool: Vec<&PathBuf> = if bucket.len() > 1 {
            let filtered: Vec<&PathBuf> =
                bucket.iter().filter(|p| !last_images.contains(p)).collect();
            if filtered.is_empty() {
                bucket.iter().collect()
            } else {
                filtered
            }
        } else {
            bucket.iter().collect()
        };

        let chosen = pool
            .choose(&mut rng)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("empty selection pool for screen '{screen_key}'"))?;
        out.push((screen_key.clone(), chosen.clone()));
    }

    Ok(out)
}

/// Alphapack mode: group images in `dir` by numeric prefix, pick one per
/// group, map group keys to screen keys.
pub fn pick_by_alphapack(dir: &Path, config: &Config) -> Result<Selection> {
    let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for path in list_images(dir)? {
        if let Some(prefix) = numeric_prefix(&path) {
            groups.entry(prefix).or_default().push(path);
        }
    }

    let mut out: Selection = Vec::new();
    let mut rng = rand::rng();
    for screen_key in config.screens.keys() {
        if let Some(group) = groups.get(screen_key) {
            if let Some(chosen) = group.choose(&mut rng) {
                out.push((screen_key.clone(), chosen.clone()));
            }
        } else {
            debug!(
                screen = screen_key,
                dir = %dir.display(),
                "no images with matching numeric prefix"
            );
        }
    }
    Ok(out)
}

// ---- Helpers --------------------------------------------------------------

fn list_images(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    out.sort();
    Ok(out)
}

fn numeric_prefix(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let end = stem
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(stem.len());
    if end == 0 {
        None
    } else {
        Some(stem[..end].to_string())
    }
}

fn opposite(o: Orientation) -> Orientation {
    match o {
        Orientation::Landscape => Orientation::Portrait,
        Orientation::Portrait => Orientation::Landscape,
    }
}

/// List the subdirectories of `packs_root`. Each one is an available pack name.
pub fn discover_packs(packs_root: &Path) -> Result<Vec<String>> {
    if !packs_root.is_dir() {
        return Ok(vec![]);
    }
    let mut names: Vec<String> = fs::read_dir(packs_root)
        .with_context(|| format!("reading packs root {}", packs_root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Screen;
    use image::{ImageBuffer, Rgb};
    use std::collections::BTreeMap;

    fn write_img(path: &Path, w: u32, h: u32) {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(w, h);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        img.save(path).unwrap();
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create tempdir")
    }

    fn config_with_screens(screens: &[(&str, &str, Orientation)]) -> Config {
        let mut map = BTreeMap::new();
        for (k, name, o) in screens {
            map.insert(
                k.to_string(),
                Screen {
                    name: name.to_string(),
                    orientation: *o,
                },
            );
        }
        let raw = serde_json::json!({
            "screens": map,
            "defaults": { "packs_location": "~/x" }
        });
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn numeric_prefix_picks_leading_digits_only() {
        assert_eq!(numeric_prefix(Path::new("1.jpg")).as_deref(), Some("1"));
        assert_eq!(numeric_prefix(Path::new("2_3.png")).as_deref(), Some("2"));
        assert_eq!(
            numeric_prefix(Path::new("42abc.jpg")).as_deref(),
            Some("42")
        );
        assert_eq!(numeric_prefix(Path::new("abc.jpg")).as_deref(), None);
    }

    #[test]
    fn directory_at_ordered_advances_modulo_len() {
        let tmp = tempdir();
        for name in &["A", "B", "C"] {
            let d = tmp.path().join(name);
            fs::create_dir_all(&d).unwrap();
        }
        let pack = Pack::load("p", tmp.path().to_path_buf()).unwrap();
        assert_eq!(
            pack.directory_at(NextMode::Ordered, 0).file_name().unwrap(),
            "A"
        );
        assert_eq!(
            pack.directory_at(NextMode::Ordered, 1).file_name().unwrap(),
            "B"
        );
        assert_eq!(
            pack.directory_at(NextMode::Ordered, 2).file_name().unwrap(),
            "C"
        );
        assert_eq!(
            pack.directory_at(NextMode::Ordered, 3).file_name().unwrap(),
            "A"
        );
    }

    #[test]
    fn pick_by_orientation_falls_back_when_bucket_empty() {
        let buckets = OrientationBuckets {
            landscape: vec![PathBuf::from("/l/a.jpg"), PathBuf::from("/l/b.jpg")],
            portrait: vec![],
        };
        let config = config_with_screens(&[
            ("1", "DP-3", Orientation::Landscape),
            ("2", "HDMI-A-1", Orientation::Portrait),
        ]);
        let selection = pick_by_orientation(&buckets, &config, &[]).unwrap();
        assert_eq!(selection.len(), 2);
        // Both screens pick from landscape since portrait is empty.
        for (_, p) in &selection {
            assert!(
                buckets.landscape.contains(p),
                "{} not in landscape",
                p.display()
            );
        }
    }

    #[test]
    fn pick_by_orientation_avoids_last_images_when_possible() {
        let last = PathBuf::from("/l/a.jpg");
        let other = PathBuf::from("/l/b.jpg");
        let buckets = OrientationBuckets {
            landscape: vec![last.clone(), other.clone()],
            portrait: vec![],
        };
        let config = config_with_screens(&[("1", "DP-3", Orientation::Landscape)]);
        // With last_images = [a], picker should always select b.
        for _ in 0..20 {
            let s = pick_by_orientation(&buckets, &config, std::slice::from_ref(&last)).unwrap();
            assert_eq!(s[0].1, other);
        }
    }

    #[test]
    fn pick_by_orientation_returns_only_image_when_bucket_size_is_one() {
        let only = PathBuf::from("/l/a.jpg");
        let buckets = OrientationBuckets {
            landscape: vec![only.clone()],
            portrait: vec![],
        };
        let config = config_with_screens(&[("1", "DP-3", Orientation::Landscape)]);
        let s = pick_by_orientation(&buckets, &config, std::slice::from_ref(&only)).unwrap();
        assert_eq!(s[0].1, only);
    }

    #[test]
    fn pick_by_alphapack_maps_prefix_to_screen() {
        let tmp = tempdir();
        let dir = tmp.path().join("alpha");
        fs::create_dir_all(&dir).unwrap();
        write_img(&dir.join("1.png"), 100, 100);
        write_img(&dir.join("2.png"), 100, 100);
        write_img(&dir.join("2_extra.png"), 100, 100);
        let config = config_with_screens(&[
            ("1", "A", Orientation::Landscape),
            ("2", "B", Orientation::Landscape),
        ]);
        let selection = pick_by_alphapack(&dir, &config).unwrap();
        let by_screen: HashMap<_, _> = selection.into_iter().collect();
        assert_eq!(by_screen.get("1").unwrap().file_name().unwrap(), "1.png");
        let two = by_screen.get("2").unwrap();
        assert!(["2.png", "2_extra.png"].contains(&two.file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn discover_packs_lists_subdirs_sorted() {
        let tmp = tempdir();
        for name in &["Mountain", "Ocean", ".hidden"] {
            fs::create_dir_all(tmp.path().join(name)).unwrap();
        }
        let packs = discover_packs(tmp.path()).unwrap();
        assert_eq!(packs, vec!["Mountain".to_string(), "Ocean".to_string()]);
    }

    #[test]
    fn orientation_cache_computes_then_caches() {
        let tmp = tempdir();
        let dir = tmp.path().join("d");
        fs::create_dir_all(&dir).unwrap();
        write_img(&dir.join("wide.png"), 200, 100);
        write_img(&dir.join("tall.png"), 100, 200);

        let mut cache = OrientationCache::new();
        let b = cache.buckets_for(&dir).unwrap();
        assert_eq!(b.landscape.len(), 1);
        assert_eq!(b.portrait.len(), 1);

        // Second call should be cached (no panic from re-read of deleted file).
        fs::remove_dir_all(&dir).unwrap();
        let b2 = cache.buckets_for(&dir).unwrap();
        assert_eq!(b2.landscape.len(), 1);
        assert_eq!(b2.portrait.len(), 1);
    }
}
