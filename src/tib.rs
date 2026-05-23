//! TinyBooth `.tib` container — a ZIP with opinionated scaffolding.
//!
//! See `docs/feature-requests/TBSS-FR-0007`. A `.tib` is a plain ZIP
//! (rename → `.zip` opens in any archive tool). This module is the IO
//! layer: open / list / read one entry / append entries / compact. It
//! is deliberately **agnostic to "dirt"** — it never inspects entry
//! *meaning*, only moves bytes — so non-scaffolded files a user drops
//! in are read-through and preserved on [`TibContainer::compact`].
//!
//! Design choices that matter (TBSS-FR-0007 §1, §5):
//!   * **Audio entries are STORE'd** (uncompressed) so a stem is a
//!     contiguous byte range that reads with no decompression and
//!     near-zero extra RAM; **JSON is DEFLATE'd**.
//!   * **Append-on-save**: [`Self::append`] uses `ZipWriter::new_append`,
//!     which adds entries after the existing data and rewrites only the
//!     (small) central directory — kilobytes, not the whole archive.
//!   * **Append-only, unique names.** The ZIP writer rejects duplicate
//!     filenames (verified against `zip` 2.4), so there is *no* cheap
//!     "shadow overwrite": updating a logical slot uses a fresh
//!     *versioned* name (`rev-NNN.wav`, `manifest-NNNN.json`) and the
//!     manifest points at the current one. [`Self::compact`] reclaims
//!     superseded entries. (Phase 1 surfaced this; it corrects the
//!     RFC's original `latest.wav`-overwrite idea.)
//!
//! Crash-safety hardening (fsync ordering, recoverable EOCD) is
//! TBSS-FR-0007 phase 8; this module gives `compact`/`create` atomicity
//! via temp-file + rename today, and a plain in-place append otherwise.
//!
//! NOTE: this is TBSS-FR-0007 **phase 1** — the IO layer, exercised by
//! its own tests but not yet wired into the app (phase 2 replaces the
//! folder format's path resolution with this). The module-level
//! `allow(dead_code)` below is removed when phase 2 lands.
#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// How an entry's bytes are stored. `Audio` → STORE (uncompressed,
/// contiguous, cheap random-access); `Json` → DEFLATE (small, packs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Audio,
    Json,
}

impl EntryKind {
    fn method(self) -> CompressionMethod {
        match self {
            EntryKind::Audio => CompressionMethod::Stored,
            EntryKind::Json => CompressionMethod::Deflated,
        }
    }
}

/// A handle to a `.tib` file on disk. Cheap to construct (just the
/// path); every method opens the file fresh, so a `TibContainer` never
/// holds an OS file handle between calls.
#[derive(Debug, Clone)]
pub struct TibContainer {
    path: PathBuf,
}

impl TibContainer {
    /// Wrap an existing `.tib` path. Does not touch disk.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create a fresh, empty `.tib` (an empty ZIP) at `path`, atomically
    /// (temp + rename). Overwrites any existing file at `path`.
    pub fn create_empty(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).context("creating .tib parent dir")?;
        }
        let tmp = tmp_sibling(&path);
        {
            let f = File::create(&tmp).context("creating temp .tib")?;
            let zw = ZipWriter::new(f);
            zw.finish().context("finishing empty .tib")?;
        }
        std::fs::rename(&tmp, &path).context("renaming temp .tib into place")?;
        Ok(Self { path })
    }

    /// Physical entry names, in central-directory order. Normally unique
    /// (we forbid duplicate appends); duplicates only appear if an
    /// external tool created them, in which case `read`/`live_names`
    /// resolve last-occurrence-wins defensively.
    pub fn entry_names(&self) -> Result<Vec<String>> {
        let mut archive = self.archive()?;
        let mut out = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            out.push(archive.by_index(i)?.name().to_string());
        }
        Ok(out)
    }

    /// Unique entry names (dedup of [`Self::entry_names`], first-seen
    /// order) — defensive against externally-created duplicates.
    pub fn live_names(&self) -> Result<Vec<String>> {
        let names = self.entry_names()?;
        let mut seen = std::collections::HashSet::new();
        // First pass: which names exist at all (preserve first-seen order).
        let mut order = Vec::new();
        for n in &names {
            if seen.insert(n.clone()) {
                order.push(n.clone());
            }
        }
        Ok(order)
    }

    /// True if `name` resolves to a live entry.
    pub fn contains(&self, name: &str) -> Result<bool> {
        Ok(self.last_index_of(name)?.is_some())
    }

    /// Read the **last** physical occurrence of `name` fully into memory.
    /// For STORE'd audio this is a windowed read of a contiguous range —
    /// no whole-archive decompression. Errors if the name is absent.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let mut archive = self.archive()?;
        let idx = last_index_of_in(&mut archive, name)?
            .ok_or_else(|| anyhow!("entry not found in .tib: {name}"))?;
        let mut entry = archive.by_index(idx)?;
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).context("reading .tib entry")?;
        Ok(buf)
    }

    /// Append one **new** entry. Uses `new_append` so only the central
    /// directory is rewritten — existing archive bytes are not
    /// recompressed or copied. The ZIP writer forbids duplicate names,
    /// so callers must use unique (versioned) names; appending an
    /// existing name is rejected with a clear error rather than the
    /// crate's opaque "Duplicate filename".
    pub fn append(&self, name: &str, bytes: &[u8], kind: EntryKind) -> Result<()> {
        if self.contains(name)? {
            return Err(anyhow!(
                "entry already exists in .tib: {name} — entries are append-only \
                 with unique names (use a versioned name like rev-NNN / \
                 manifest-NNNN, or compact first)"
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .with_context(|| format!("opening .tib for append: {}", self.path.display()))?;
        let mut zw = ZipWriter::new_append(file).context("opening .tib in append mode")?;
        let opts = SimpleFileOptions::default()
            .compression_method(kind.method())
            .large_file(bytes.len() as u64 >= u32::MAX as u64);
        zw.start_file(name, opts)
            .with_context(|| format!("starting .tib entry {name}"))?;
        zw.write_all(bytes)
            .with_context(|| format!("writing .tib entry {name}"))?;
        zw.finish().context("finalizing .tib append")?;
        Ok(())
    }

    /// Rewrite the archive keeping only entries whose name passes
    /// `keep` (using last-occurrence bytes), reclaiming dead/shadowed
    /// bytes. Unknown "dirt" entries are subject to the same predicate —
    /// pass a `keep` that returns `true` for names you don't recognise
    /// to preserve them. Atomic via temp + rename.
    pub fn compact(&self, keep: impl Fn(&str) -> bool) -> Result<()> {
        let live = self.live_names()?;
        let tmp = tmp_sibling(&self.path);
        {
            let out = File::create(&tmp).context("creating temp .tib for compaction")?;
            let mut zw = ZipWriter::new(out);
            let mut src = self.archive()?;
            for name in &live {
                if !keep(name) {
                    continue;
                }
                let idx = last_index_of_in(&mut src, name)?
                    .ok_or_else(|| anyhow!("compaction: name vanished: {name}"))?;
                // Preserve the original compression method per entry.
                let (method, bytes) = {
                    let mut e = src.by_index(idx)?;
                    let m = e.compression();
                    let mut b = Vec::with_capacity(e.size() as usize);
                    e.read_to_end(&mut b)?;
                    (m, b)
                };
                let opts = SimpleFileOptions::default()
                    .compression_method(method)
                    .large_file(bytes.len() as u64 >= u32::MAX as u64);
                zw.start_file(name.as_str(), opts)?;
                zw.write_all(&bytes)?;
            }
            zw.finish().context("finishing compacted .tib")?;
        }
        std::fs::rename(&tmp, &self.path).context("renaming compacted .tib into place")?;
        Ok(())
    }

    // ── internals ────────────────────────────────────────────────────

    fn archive(&self) -> Result<ZipArchive<File>> {
        let f = File::open(&self.path)
            .with_context(|| format!("opening .tib: {}", self.path.display()))?;
        ZipArchive::new(f).with_context(|| format!("reading .tib zip: {}", self.path.display()))
    }

    fn last_index_of(&self, name: &str) -> Result<Option<usize>> {
        let mut archive = self.archive()?;
        last_index_of_in(&mut archive, name)
    }
}

/// Last physical index whose name equals `name` (so appended shadows
/// win over the original).
fn last_index_of_in(archive: &mut ZipArchive<File>, name: &str) -> Result<Option<usize>> {
    let mut found = None;
    for i in 0..archive.len() {
        if archive.by_index(i)?.name() == name {
            found = Some(i);
        }
    }
    Ok(found)
}

/// A temp sibling path next to `path` (same dir, so rename is atomic on
/// the same filesystem).
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("tbss-tib-test-{}-{}.tib", name, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn empty_container_has_no_entries() {
        let p = scratch("empty");
        let c = TibContainer::create_empty(&p).unwrap();
        assert!(c.entry_names().unwrap().is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn round_trips_audio_and_json() {
        let p = scratch("roundtrip");
        let c = TibContainer::create_empty(&p).unwrap();
        let wav = [0u8, 1, 2, 3, 4, 5, 6, 7]; // stand-in PCM bytes
        let json = br#"{"version":2}"#.to_vec();
        c.append("stems/Vocals/Take 1/orig.wav", &wav, EntryKind::Audio)
            .unwrap();
        c.append("manifest.json", &json, EntryKind::Json).unwrap();

        assert_eq!(c.read("stems/Vocals/Take 1/orig.wav").unwrap(), wav);
        assert_eq!(c.read("manifest.json").unwrap(), json);
        assert!(c.contains("manifest.json").unwrap());
        assert!(!c.contains("nope").unwrap());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn append_rejects_duplicate_names() {
        // The ZIP writer forbids duplicate filenames, so an "overwrite"
        // must use a fresh versioned name (rev-NNN / manifest-NNNN).
        let p = scratch("dup");
        let c = TibContainer::create_empty(&p).unwrap();
        c.append("stems/V/T/rev-001.wav", &[1u8; 16], EntryKind::Audio)
            .unwrap();
        let err = c.append("stems/V/T/rev-001.wav", &[2u8; 16], EntryKind::Audio);
        assert!(err.is_err(), "duplicate name must be rejected");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn versioned_revisions_coexist() {
        let p = scratch("versioned");
        let c = TibContainer::create_empty(&p).unwrap();
        let r1 = [1u8; 16];
        let r2 = [2u8; 16];
        c.append("stems/V/T/rev-001.wav", &r1, EntryKind::Audio)
            .unwrap();
        c.append("stems/V/T/rev-002.wav", &r2, EntryKind::Audio)
            .unwrap();
        assert_eq!(c.read("stems/V/T/rev-001.wav").unwrap(), r1);
        assert_eq!(c.read("stems/V/T/rev-002.wav").unwrap(), r2);
        assert_eq!(c.entry_names().unwrap().len(), 2);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn audio_entries_are_stored_uncompressed() {
        let p = scratch("stored");
        let c = TibContainer::create_empty(&p).unwrap();
        c.append("a/latest.wav", &[9u8; 4096], EntryKind::Audio)
            .unwrap();
        c.append("manifest.json", &[9u8; 4096], EntryKind::Json)
            .unwrap();
        let mut ar = c.archive().unwrap();
        let methods: std::collections::HashMap<String, CompressionMethod> = (0..ar.len())
            .map(|i| {
                let e = ar.by_index(i).unwrap();
                (e.name().to_string(), e.compression())
            })
            .collect();
        assert_eq!(methods["a/latest.wav"], CompressionMethod::Stored);
        assert_eq!(methods["manifest.json"], CompressionMethod::Deflated);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn compact_drops_unkept_and_preserves_dirt() {
        let p = scratch("compact");
        let c = TibContainer::create_empty(&p).unwrap();
        c.append("stems/V/T/rev-001.wav", &[1u8; 32], EntryKind::Audio)
            .unwrap();
        c.append("stems/V/T/rev-002.wav", &[2u8; 32], EntryKind::Audio)
            .unwrap();
        c.append("notes/readme.txt", b"hand-dropped dirt", EntryKind::Json)
            .unwrap();
        assert_eq!(c.entry_names().unwrap().len(), 3);

        // Prune rev-001 (e.g. evicted by the FIFO); keep all else incl. dirt.
        c.compact(|name| name != "stems/V/T/rev-001.wav").unwrap();

        let names = c.entry_names().unwrap();
        assert_eq!(names.len(), 2);
        assert!(!c.contains("stems/V/T/rev-001.wav").unwrap());
        assert_eq!(c.read("stems/V/T/rev-002.wav").unwrap(), [2u8; 32]);
        assert_eq!(c.read("notes/readme.txt").unwrap(), b"hand-dropped dirt");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn tib_is_a_plain_zip() {
        // The whole point: a .tib opens as a ZIP (rename → .zip).
        let p = scratch("plainzip");
        let c = TibContainer::create_empty(&p).unwrap();
        c.append("manifest.json", br#"{"ok":true}"#, EntryKind::Json)
            .unwrap();
        let f = File::open(&p).unwrap();
        let mut ar = ZipArchive::new(f).expect("a .tib must be a valid zip");
        let mut s = String::new();
        ar.by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, r#"{"ok":true}"#);
        let _ = std::fs::remove_file(&p);
    }
}
