use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use tar::{Archive, Builder};
use time::macros::format_description;
use time::OffsetDateTime;

pub const BUNDLE_EXTENSION: &str = ".p2pshare-bundle.tar";

#[derive(Debug)]
pub struct BundleBuild {
    pub bundle_path: PathBuf,
    pub logical_name: String,
    pub item_count: u64,
    pub hash: String,
    cleanup: BundleCleanup,
}

impl BundleBuild {
    /// Transfer ownership of the archive to the caller, which must remove it.
    pub fn into_path(mut self) -> PathBuf {
        self.cleanup.0.take().expect("bundle owns its archive")
    }
}

#[derive(Debug)]
struct BundleCleanup(Option<PathBuf>);

impl Drop for BundleCleanup {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct CancellableReader<'a, R> {
    inner: R,
    canceled: &'a AtomicBool,
}

impl<R: Read> Read for CancellableReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.canceled.load(Ordering::Relaxed) {
            return Err(io::Error::other("bundle operation canceled"));
        }
        self.inner.read(bytes)
    }
}

struct ExtractedBundle {
    directory: PathBuf,
    item_count: u64,
}

impl Drop for ExtractedBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub async fn create_bundle(paths: &[PathBuf]) -> Result<BundleBuild> {
    let input_paths = paths.to_vec();
    let canceled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelOnDrop(canceled.clone());
    tokio::task::spawn_blocking(move || create_bundle_cancellable(&input_paths, &canceled)).await?
}

pub async fn extract_bundle(bundle_path: &Path, output_dir: &Path) -> Result<u64> {
    extract_staged(bundle_path.to_path_buf(), output_dir.to_path_buf(), false).await
}

/// Take ownership of a received archive, removing it even if extraction is canceled.
pub async fn extract_bundle_owned(bundle_path: PathBuf, output_dir: &Path) -> Result<u64> {
    extract_staged(bundle_path, output_dir.to_path_buf(), true).await
}

async fn extract_staged(
    bundle_path: PathBuf,
    output_dir: PathBuf,
    owns_archive: bool,
) -> Result<u64> {
    let canceled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelOnDrop(canceled.clone());
    let archive_cleanup = BundleCleanup(owns_archive.then(|| bundle_path.clone()));
    let parent = output_dir
        .parent()
        .context("output directory has no parent")?
        .to_path_buf();
    let extracted = tokio::task::spawn_blocking(move || -> Result<ExtractedBundle> {
        // This guard outlives the archive's file handle, including on Windows.
        let _archive_cleanup = archive_cleanup;
        std::fs::create_dir_all(&parent)?;
        let directory = parent.join(unique_temp_name(".p2pshare-extract"));
        std::fs::create_dir(&directory)?;
        let mut extracted = ExtractedBundle {
            directory,
            item_count: 0,
        };
        extracted.item_count =
            extract_bundle_cancellable(&bundle_path, &extracted.directory, &canceled)?;
        Ok(extracted)
    })
    .await??;
    if output_dir.exists() {
        bail!("output directory already exists: {}", output_dir.display());
    }
    // Publish only after the worker has finished. No await between rename and return.
    std::fs::rename(&extracted.directory, &output_dir)?;
    Ok(extracted.item_count)
}

pub fn logical_name_from_wire_name(name: &str) -> String {
    name.strip_suffix(BUNDLE_EXTENSION)
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
fn create_bundle_blocking(paths: &[PathBuf]) -> Result<BundleBuild> {
    create_bundle_cancellable(paths, &AtomicBool::new(false))
}

fn create_bundle_cancellable(paths: &[PathBuf], canceled: &AtomicBool) -> Result<BundleBuild> {
    if paths.is_empty() {
        bail!("at least one file is required");
    }

    let logical_name = bundle_logical_name()?;
    let bundle_name = format!("{}{}", logical_name, BUNDLE_EXTENSION);
    let bundle_path = std::env::temp_dir().join(unique_temp_name(&bundle_name));

    let file = File::create(&bundle_path)
        .with_context(|| format!("failed to create bundle file {}", bundle_path.display()))?;
    let cleanup = BundleCleanup(Some(bundle_path.clone()));
    let mut builder = Builder::new(HashingWriter {
        file,
        hasher: blake3::Hasher::new(),
    });
    let mut used_names = HashSet::new();

    for path in paths {
        if canceled.load(Ordering::Relaxed) {
            bail!("bundle operation canceled");
        }
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

        let file = File::open(path)?;
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&metadata);
        builder
            .append_data(
                &mut header,
                &archive_name,
                CancellableReader {
                    inner: file,
                    canceled,
                },
            )
            .with_context(|| format!("failed to add {} to bundle", path.display()))?;
    }

    builder
        .finish()
        .context("failed to finalize bundle archive")?;

    let writer = builder
        .into_inner()
        .context("failed to close bundle archive")?;
    let hash = writer.hasher.finalize().to_hex().to_string();

    Ok(BundleBuild {
        cleanup,
        hash,
        bundle_path,
        logical_name,
        item_count: paths.len() as u64,
    })
}

// Hash exactly the bytes written, including tar headers and final padding.
struct HashingWriter {
    file: File,
    hasher: blake3::Hasher,
}

impl Write for HashingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
fn extract_bundle_blocking(bundle_path: &Path, output_dir: &Path) -> Result<u64> {
    extract_bundle_cancellable(bundle_path, output_dir, &AtomicBool::new(false))
}

fn extract_bundle_cancellable(
    bundle_path: &Path,
    output_dir: &Path,
    canceled: &AtomicBool,
) -> Result<u64> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    let file = File::open(bundle_path)
        .with_context(|| format!("failed to open bundle {}", bundle_path.display()))?;
    let mut archive = Archive::new(CancellableReader {
        inner: file,
        canceled,
    });
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

        assert_eq!(
            build.hash,
            blake3::hash(&fs::read(&build.bundle_path).expect("read archive"))
                .to_hex()
                .to_string()
        );
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
    fn canceled_extraction_cleans_owned_archive_and_never_publishes() {
        use std::future::Future;
        use std::task::Poll;
        let root = temp_test_dir("cancel-extraction");
        let source = root.join("source.txt");
        fs::write(&source, "payload").unwrap();
        let archive = create_bundle_blocking(&[source]).unwrap().into_path();
        let output = root.join("output");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let (ready, started) = std::sync::mpsc::channel();
        runtime.spawn_blocking(move || {
            ready.send(()).unwrap();
            blocked.recv().unwrap();
        });
        started.recv().unwrap();
        runtime.block_on(async {
            let mut extraction = Box::pin(super::extract_bundle_owned(archive.clone(), &output));
            std::future::poll_fn(|context| {
                assert!(extraction.as_mut().poll(context).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(extraction);
        });
        release.send(()).unwrap();
        runtime.shutdown_timeout(std::time::Duration::from_secs(5));
        assert!(!archive.exists());
        assert!(!output.exists());
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "only original source remains"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn owned_extraction_publishes_verified_contents_and_removes_archive() {
        let root = temp_test_dir("owned-extraction");
        let source = root.join("source.txt");
        fs::write(&source, "payload").unwrap();
        let archive = create_bundle_blocking(&[source]).unwrap().into_path();
        let output = root.join("output");
        assert_eq!(
            super::extract_bundle_owned(archive.clone(), &output)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            fs::read_to_string(output.join("source.txt")).unwrap(),
            "payload"
        );
        assert!(!archive.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_ownership_controls_cleanup() {
        let root = temp_test_dir("bundle-ownership");
        let source = root.join("source.txt");
        fs::write(&source, "source").unwrap();
        let bundle = create_bundle_blocking(std::slice::from_ref(&source)).unwrap();
        let temporary = bundle.bundle_path.clone();
        drop(bundle);
        assert!(!temporary.exists());
        let transferred = create_bundle_blocking(&[source]).unwrap().into_path();
        assert!(transferred.exists());
        fs::remove_file(transferred).unwrap();
        fs::remove_dir_all(root).unwrap();
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
