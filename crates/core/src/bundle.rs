use std::collections::HashSet;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use tar::{Archive, Builder};
use time::macros::format_description;
use time::OffsetDateTime;

pub const BUNDLE_EXTENSION: &str = ".p2pshare-bundle.tar";

#[derive(Debug, Clone)]
pub struct BundleBuild {
    pub bundle_path: PathBuf,
    pub logical_name: String,
    pub item_count: u64,
}

pub async fn create_bundle(paths: &[PathBuf]) -> Result<BundleBuild> {
    let input_paths = paths.to_vec();
    tokio::task::spawn_blocking(move || create_bundle_blocking(&input_paths)).await?
}

pub async fn extract_bundle(bundle_path: &Path, output_dir: &Path) -> Result<u64> {
    let bundle_path = bundle_path.to_path_buf();
    let output_dir = output_dir.to_path_buf();
    tokio::task::spawn_blocking(move || extract_bundle_blocking(&bundle_path, &output_dir)).await?
}

pub fn logical_name_from_wire_name(name: &str) -> String {
    name.strip_suffix(BUNDLE_EXTENSION)
        .unwrap_or(name)
        .to_string()
}

fn create_bundle_blocking(paths: &[PathBuf]) -> Result<BundleBuild> {
    if paths.is_empty() {
        bail!("at least one file is required");
    }

    let logical_name = bundle_logical_name()?;
    let bundle_name = format!("{}{}", logical_name, BUNDLE_EXTENSION);
    let bundle_path = std::env::temp_dir().join(unique_temp_name(&bundle_name));

    let file = File::create(&bundle_path)
        .with_context(|| format!("failed to create bundle file {}", bundle_path.display()))?;
    let mut builder = Builder::new(file);
    let mut used_names = HashSet::new();

    for path in paths {
        let metadata =
            std::fs::metadata(path).with_context(|| format!("cannot access {}", path.display()))?;
        if !metadata.is_file() {
            bail!("{} is not a regular file", path.display());
        }

        let base_name = path
            .file_name()
            .context("path has no file name")?
            .to_string_lossy()
            .to_string();
        let archive_name = dedupe_file_name(&base_name, &mut used_names);

        builder
            .append_path_with_name(path, &archive_name)
            .with_context(|| format!("failed to add {} to bundle", path.display()))?;
    }

    builder
        .finish()
        .context("failed to finalize bundle archive")?;

    Ok(BundleBuild {
        bundle_path,
        logical_name,
        item_count: paths.len() as u64,
    })
}

fn extract_bundle_blocking(bundle_path: &Path, output_dir: &Path) -> Result<u64> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    let file = File::open(bundle_path)
        .with_context(|| format!("failed to open bundle {}", bundle_path.display()))?;
    let mut archive = Archive::new(file);
    let mut item_count = 0u64;
    let mut seen = HashSet::new();

    for entry in archive.entries().context("failed to read bundle entries")? {
        let mut entry = entry.context("failed to read bundle entry")?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            bail!("bundle contains unsupported entry type");
        }

        let path = entry.path().context("bundle entry path is invalid")?;
        let file_name = sanitize_bundle_entry_path(&path)?;
        if !seen.insert(file_name.clone()) {
            bail!("bundle contains duplicate entry {}", file_name);
        }

        let dest = output_dir.join(&file_name);
        entry
            .unpack(&dest)
            .with_context(|| format!("failed to unpack {}", dest.display()))?;
        item_count += 1;
    }

    Ok(item_count)
}

fn sanitize_bundle_entry_path(path: &Path) -> Result<String> {
    let mut components = path.components();
    let Some(first) = components.next() else {
        bail!("bundle contains empty path");
    };
    if components.next().is_some() {
        bail!("bundle contains nested path {}", path.display());
    }

    let Component::Normal(name) = first else {
        bail!("bundle contains invalid path {}", path.display());
    };
    let file_name = name.to_string_lossy().trim().to_string();
    if file_name.is_empty() || file_name == "." || file_name == ".." {
        bail!("bundle contains invalid file name");
    }
    Ok(file_name)
}

fn bundle_logical_name() -> Result<String> {
    let now = OffsetDateTime::now_utc();
    let format = format_description!("[year][month][day]-[hour][minute][second]");
    Ok(format!("p2p-share-{}", now.format(&format)?))
}

fn unique_temp_name(base_name: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), stamp, base_name)
}

fn dedupe_file_name(name: &str, used: &mut HashSet<String>) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }

    let stem = Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = Path::new(name)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();

    for index in 1u32.. {
        let candidate = format!("{} ({}){}", stem, index, ext);
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        create_bundle_blocking, dedupe_file_name, extract_bundle_blocking,
        logical_name_from_wire_name,
    };

    #[test]
    fn dedupe_file_name_adds_numeric_suffixes() {
        let mut used = std::collections::HashSet::new();
        assert_eq!(dedupe_file_name("demo.txt", &mut used), "demo.txt");
        assert_eq!(dedupe_file_name("demo.txt", &mut used), "demo (1).txt");
        assert_eq!(dedupe_file_name("demo.txt", &mut used), "demo (2).txt");
    }

    #[test]
    fn logical_name_is_derived_from_bundle_file_name() {
        assert_eq!(
            logical_name_from_wire_name("p2p-share-20260331-193000.p2pshare-bundle.tar"),
            "p2p-share-20260331-193000"
        );
    }

    #[test]
    fn bundle_round_trip_preserves_multiple_files() {
        let root = temp_test_dir("bundle-round-trip");
        let source_dir = root.join("src");
        let output_dir = root.join("out");
        fs::create_dir_all(&source_dir).expect("create source dir");
        fs::write(source_dir.join("a.txt"), "alpha").expect("write a");
        fs::write(source_dir.join("b.txt"), "beta").expect("write b");

        let build = create_bundle_blocking(&[source_dir.join("a.txt"), source_dir.join("b.txt")])
            .expect("create bundle");

        let count =
            extract_bundle_blocking(&build.bundle_path, &output_dir).expect("extract bundle");
        assert_eq!(count, 2);
        assert_eq!(
            fs::read_to_string(output_dir.join("a.txt")).expect("read a"),
            "alpha"
        );
        assert_eq!(
            fs::read_to_string(output_dir.join("b.txt")).expect("read b"),
            "beta"
        );

        let _ = fs::remove_file(build.bundle_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bundle_creation_renames_duplicate_basenames() {
        let root = temp_test_dir("bundle-dedupe");
        let left = root.join("left");
        let right = root.join("right");
        let output_dir = root.join("out");
        fs::create_dir_all(&left).expect("create left");
        fs::create_dir_all(&right).expect("create right");
        fs::write(left.join("same.txt"), "left").expect("write left");
        fs::write(right.join("same.txt"), "right").expect("write right");

        let build = create_bundle_blocking(&[left.join("same.txt"), right.join("same.txt")])
            .expect("create bundle");

        let count =
            extract_bundle_blocking(&build.bundle_path, &output_dir).expect("extract bundle");
        assert_eq!(count, 2);
        assert_eq!(
            fs::read_to_string(output_dir.join("same.txt")).expect("read first"),
            "left"
        );
        assert_eq!(
            fs::read_to_string(output_dir.join("same (1).txt")).expect("read second"),
            "right"
        );

        let _ = fs::remove_file(build.bundle_path);
        let _ = fs::remove_dir_all(root);
    }

    fn temp_test_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("p2p-share-{label}-{stamp}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[allow(dead_code)]
    fn _assert_path_exists(path: &Path) {
        assert!(path.exists(), "{} should exist", path.display());
    }
}
