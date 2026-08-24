//! TBSS-FR-0018 · E1+E2 — the tracker's sample library: curated pack
//! manifest, background downloader, and the pitch-from-filename parser
//! that turns a folder of per-note recordings into multisample zones.
//!
//! Licensing is architectural here: the manifest ships METADATA ONLY.
//! CC0 packs (VCSL) could legally be mirrored but aren't; Philharmonia-
//! class packs ("free to use, must not be redistributed as samples") are
//! downloaded by the *user's* machine from the *source's* own URL. No
//! sample audio ever enters the repo or installer.
//!
//! Download URLs verified live 2026-08-24 (Philharmonia S3 → HTTP 200
//! with the sizes recorded below; VCSL → github codeload redirect,
//! followed by reqwest automatically).

#![allow(dead_code)] // E4/E5 UI consumes progressively

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::tracker::Note;

// ───────────────────────── manifest (E1) ─────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackLicense {
    pub id: String,
    /// One-line summary shown in the UI before download.
    pub summary: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplePack {
    /// Stable id → extraction dir name.
    pub id: String,
    pub name: String,
    pub source: String,
    pub license: PackLicense,
    pub download_url: String,
    /// Approximate download size, for the pre-download display.
    pub approx_mb: u32,
    /// Filename-note convention hint (see [`parse_note_from_name`] —
    /// the parser is tolerant; this is display metadata).
    pub convention: String,
}

/// The curated starter manifest, shipped with the app.
pub fn builtin_packs() -> Vec<SamplePack> {
    let phil_license = PackLicense {
        id: "philharmonia-free".into(),
        summary: "Free for any use incl. commercial; must NOT be re-sold or \
                  redistributed as samples — downloaded from the source on demand."
            .into(),
        url: "https://philharmonia.co.uk/resources/sound-samples/".into(),
    };
    let s3 = "https://philharmonia-assets.s3-eu-west-1.amazonaws.com/uploads/2020/02/12112005";
    let mut packs: Vec<SamplePack> = [
        (
            "philharmonia-strings",
            "Philharmonia — Strings",
            "Strings.zip",
            163,
        ),
        (
            "philharmonia-brass",
            "Philharmonia — Brass",
            "Brass.zip",
            98,
        ),
        (
            "philharmonia-woodwind",
            "Philharmonia — Woodwind",
            "Woodwind.zip",
            260,
        ),
        (
            "philharmonia-percussion",
            "Philharmonia — Percussion",
            "Percussion.zip",
            6,
        ),
    ]
    .into_iter()
    .map(|(id, name, file, mb)| SamplePack {
        id: id.into(),
        name: name.into(),
        source: "Philharmonia Orchestra".into(),
        license: phil_license.clone(),
        download_url: format!("{s3}/{file}"),
        approx_mb: mb,
        convention: "instrument_A2_… (s = sharp)".into(),
    })
    .collect();
    packs.push(SamplePack {
        id: "vcsl-full".into(),
        name: "VCSL — Versilian Community Sample Library (full)".into(),
        source: "Versilian Studios".into(),
        license: PackLicense {
            id: "CC0".into(),
            summary: "Creative Commons 0 — public domain, no strings attached.".into(),
            url: "https://github.com/sgossner/VCSL".into(),
        },
        download_url: "https://github.com/sgossner/VCSL/archive/refs/heads/master.zip".into(),
        approx_mb: 2600,
        convention: "Instrument_Articulation_C4_vl1_rr1.wav".into(),
    });
    packs
}

/// Where downloaded packs live.
pub fn library_dir() -> PathBuf {
    crate::config::Config::dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("sample-library")
}

pub fn pack_dir(pack_id: &str) -> PathBuf {
    library_dir().join(pack_id)
}

pub fn pack_is_downloaded(pack_id: &str) -> bool {
    pack_dir(pack_id).is_dir()
}

// ───────────────────────── parser (E2, pure) ─────────────────────────

/// Extract the pitch a sample filename encodes, across the conventions
/// the research surfaced:
///
/// * Philharmonia: `cello_A2_1_forte_arco-normal.mp3`, sharps as `s`
///   (`cello_As2_…`)
/// * Univ. of Iowa: `Piano.ff.C4.aiff`, flats as `b` (`Db5`)
/// * VCSL: `Xylophone_hard_C4_vl2_rr1.wav`, sharps as `#` or `s`
///
/// Strategy: split on non-alphanumerics and scan tokens for
/// `<letter><accidental?><octave>`. Returns the FIRST match — in every
/// surveyed convention the note token precedes any velocity/round-robin
/// numerals that could false-positive.
pub fn parse_note_from_name(name: &str) -> Option<Note> {
    for token in name.split(|c: char| !c.is_ascii_alphanumeric() && c != '#') {
        if let Some(n) = parse_note_token(token) {
            return Some(n);
        }
    }
    None
}

fn parse_note_token(token: &str) -> Option<Note> {
    let bytes = token.as_bytes();
    if bytes.len() < 2 || bytes.len() > 4 {
        return None;
    }
    let letter = bytes[0].to_ascii_uppercase();
    if !(b'A'..=b'G').contains(&letter) {
        return None;
    }
    let mut semis: i32 = match letter {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let mut idx = 1;
    match bytes.get(1) {
        Some(b'#') | Some(b's') => {
            semis += 1;
            idx = 2;
        }
        // Lower-case b = flat. Upper-case B would be ambiguous with the
        // note letter but can't appear at index 1 of a valid token.
        Some(b'b') => {
            semis -= 1;
            idx = 2;
        }
        _ => {}
    }
    let octave_part = &token[idx..];
    if octave_part.is_empty() || octave_part.len() > 1 {
        return None; // single-digit octaves only — matches all conventions
    }
    let octave: i32 = octave_part.parse().ok()?;
    let note = octave * 12 + semis;
    // Philharmonia uses "s" for sharp but also words like "As" would
    // need the digit; range-check seals validity.
    (0..=119).contains(&note).then_some(note as Note)
}

/// Scan an extracted pack directory for audio files with parseable
/// pitches. Returns `(matched, unparseable_count)` — unparseable files
/// are counted, not silently dropped (audit rule).
pub fn scan_pack(dir: &Path) -> (Vec<(PathBuf, Note)>, usize) {
    let mut matched = Vec::new();
    let mut unparsed = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext_ok = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    crate::audiodecode::SUPPORTED_EXTS
                        .iter()
                        .any(|s| e.eq_ignore_ascii_case(s))
                        || e.eq_ignore_ascii_case("aiff")
                        || e.eq_ignore_ascii_case("aif")
                })
                .unwrap_or(false);
            if !ext_ok {
                continue;
            }
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            match parse_note_from_name(&name) {
                Some(n) => matched.push((p, n)),
                None => unparsed += 1,
            }
        }
    }
    matched.sort_by_key(|(_, n)| *n);
    (matched, unparsed)
}

/// Group a pack scan by instrument prefix (the token before the first
/// separator), so one downloaded category zip (e.g. "Strings") yields
/// one buildable instrument per actual instrument (cello, violin, …).
pub fn group_by_instrument(matched: &[(PathBuf, Note)]) -> Vec<(String, Vec<(PathBuf, Note)>)> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<(PathBuf, Note)>> = BTreeMap::new();
    for (p, n) in matched {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let prefix = stem
            .split(['_', '.', '-'])
            .next()
            .unwrap_or("sample")
            .to_ascii_lowercase();
        groups.entry(prefix).or_default().push((p.clone(), *n));
    }
    groups.into_iter().collect()
}

// ───────────────────────── downloader (E1) ─────────────────────────

/// Shared progress the UI polls: (downloaded bytes, total bytes or 0).
pub type Progress = std::sync::Arc<(std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64)>;

/// Download `pack` and extract it under [`pack_dir`]. Blocking — run on
/// a background thread. Zip-slip guarded like the Suno importer.
pub fn download_and_extract(pack: &SamplePack, progress: &Progress) -> Result<PathBuf> {
    use std::io::Read as _;
    use std::sync::atomic::Ordering;

    let dest = pack_dir(&pack.id);
    let staging = library_dir().join(format!("{}.part.zip", pack.id));
    std::fs::create_dir_all(library_dir())?;

    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("{}/{}", crate::APP_NAME, env!("APP_VERSION")))
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .context("building HTTP client")?;
    let mut resp = client
        .get(&pack.download_url)
        .send()
        .and_then(|r| r.error_for_status())
        .context("starting download")?;
    let total = resp.content_length().unwrap_or(0);
    progress.1.store(total, Ordering::Relaxed);

    {
        let mut out = std::io::BufWriter::new(
            std::fs::File::create(&staging).context("creating staging file")?,
        );
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = resp.read(&mut buf).context("reading download")?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n])?;
            progress.0.fetch_add(n as u64, Ordering::Relaxed);
        }
        std::io::Write::flush(&mut out)?;
    }

    // Extract.
    let file = std::fs::File::open(&staging)?;
    let mut zip = zip::ZipArchive::new(file).context("opening downloaded zip")?;
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // Zip-slip guard: only extract entries whose normalized path
        // stays inside the destination.
        let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let out_path = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&out_path)
            .with_context(|| format!("creating {}", out_path.display()))?;
        std::io::copy(&mut entry, &mut out)?;
    }
    let _ = std::fs::remove_file(&staging);
    Ok(dest)
}

/// Delete a downloaded pack.
pub fn remove_pack(pack_id: &str) -> Result<()> {
    let d = pack_dir(pack_id);
    if d.is_dir() {
        std::fs::remove_dir_all(&d)?;
    } else {
        return Err(anyhow!("pack '{pack_id}' is not downloaded"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::NOTE_C4;

    #[test]
    fn parses_philharmonia_names() {
        // Real convention: sharps as 's', note token early.
        assert_eq!(
            parse_note_from_name("cello_A2_1_forte_arco-normal"),
            Some(2 * 12 + 9)
        );
        assert_eq!(
            parse_note_from_name("cello_As2_05_forte_arco-normal"),
            Some(2 * 12 + 10)
        );
        assert_eq!(
            parse_note_from_name("trumpet_C4_025_pianissimo_normal"),
            Some(NOTE_C4)
        );
    }

    #[test]
    fn parses_iowa_names() {
        assert_eq!(parse_note_from_name("Piano.ff.C4"), Some(NOTE_C4));
        assert_eq!(parse_note_from_name("Piano.mf.Db5"), Some(5 * 12 + 1));
    }

    #[test]
    fn parses_vcsl_names() {
        assert_eq!(
            parse_note_from_name("Xylophone_hard_C4_vl2_rr1"),
            Some(NOTE_C4)
        );
        assert_eq!(parse_note_from_name("Tuba_sus_F#1_vl1_rr1"), Some(12 + 6));
    }

    #[test]
    fn ignores_lookalike_tokens() {
        // 'forte'/'arco'/velocity digits must not parse as notes; a name
        // with no real note token yields None.
        assert_eq!(parse_note_from_name("snare_hit_forte_rr12"), None);
        assert_eq!(parse_note_from_name("kick_09_hard"), None);
        // Out-of-range octave digits (two-digit) rejected.
        assert_eq!(parse_note_from_name("thing_C42_x"), None);
    }

    #[test]
    fn grouping_splits_instruments_in_a_category_zip() {
        let files = vec![
            (PathBuf::from("cello_A2_1_f.mp3"), 33),
            (PathBuf::from("cello_C3_1_f.mp3"), 36),
            (PathBuf::from("violin_A4_1_f.mp3"), 57),
        ];
        let groups = group_by_instrument(&files);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "cello");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "violin");
    }

    /// Real-world acid test: download the smallest Philharmonia pack
    /// (Percussion, ~6 MB) through the actual code path and scan it.
    /// Env-gated + #[ignore] — network + disk.
    /// Run: TBSS_SAMPLELIB_PROBE=1 cargo test ... samplelib_probe -- --ignored --nocapture
    #[test]
    #[ignore = "network; set TBSS_SAMPLELIB_PROBE=1"]
    fn samplelib_probe_real_pack() {
        if std::env::var("TBSS_SAMPLELIB_PROBE").is_err() {
            eprintln!("TBSS_SAMPLELIB_PROBE not set — skipping");
            return;
        }
        let dl = |id: &str| {
            let pack = builtin_packs().into_iter().find(|p| p.id == id).unwrap();
            let progress: Progress = std::sync::Arc::new((
                std::sync::atomic::AtomicU64::new(0),
                std::sync::atomic::AtomicU64::new(0),
            ));
            if !pack_is_downloaded(id) {
                download_and_extract(&pack, &progress).expect("download+extract");
            }
            let (matched, unparsed) = scan_pack(&pack_dir(id));
            let groups = group_by_instrument(&matched);
            eprintln!(
                "{id}: pitched {} · unparseable {} · instruments {:?}",
                matched.len(),
                unparsed,
                groups
                    .iter()
                    .map(|(n, f)| format!("{n}×{}", f.len()))
                    .collect::<Vec<_>>()
            );
            (matched, unparsed, groups)
        };

        // Percussion: genuinely unpitched (agogo/shaker/bass drum, note
        // slot empty as a double underscore) — the parser must claim
        // NOTHING rather than hallucinate pitches.
        let (m, u, _) = dl("philharmonia-percussion");
        assert_eq!(m.len(), 0, "percussion misparsed as pitched");
        assert!(u > 100, "percussion files present");

        // Brass: real pitched material — trumpet/trombone/tuba per-note
        // files must parse into multiple instruments with many notes.
        let (m, _, groups) = dl("philharmonia-brass");
        assert!(m.len() > 500, "expected hundreds of pitched brass files");
        assert!(groups.len() >= 3, "several brass instruments");
        for (path, note) in m.iter().take(5) {
            eprintln!(
                "  {} -> {}",
                path.file_name().unwrap().to_string_lossy(),
                crate::tracker::note_name(*note)
            );
        }
    }

    #[test]
    fn builtin_manifest_is_sane() {
        let packs = builtin_packs();
        assert!(packs.len() >= 5);
        for p in &packs {
            assert!(p.download_url.starts_with("https://"), "{}", p.id);
            assert!(!p.license.summary.is_empty());
            assert!(p.approx_mb > 0);
        }
    }
}
