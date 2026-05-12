//! Transparent TinyBooth-metadata injection into WAV files.
//!
//! TinyBooth-managed WAVs carry a small JSON blob inside a standard
//! `LIST/INFO/ICMT` chunk (the "comment" field of the RIFF INFO
//! metadata sub-spec). When the file is later moved, copied, or
//! pulled out of a `.tinybooth` project folder, the TBSS context
//! travels with it — name of the project that produced it, role
//! (if any), polarity flip, active correction profile, telemetry
//! analyzer version. A reader that doesn't know about TBSS just
//! sees a standard comment field.
//!
//! Format on disk (appended to the end of an existing WAV, padding
//! to even length per RIFF spec):
//!
//! ```text
//!   "LIST" <chunk_size:u32_le>     ; chunk_size = 4 + (8 + payload_len + pad)
//!   "INFO"                          ; LIST type
//!   "ICMT" <field_size:u32_le>      ; field_size = payload_len (text bytes)
//!   <utf8 payload>
//!   <pad byte if odd>
//! ```
//!
//! The leading `RIFF` chunk's total size (bytes 4..8 of the file)
//! gets patched after the append.
//!
//! Reading: existing callers use [`crate::suno_meta::read_wav_session`]
//! which also walks `LIST/INFO/ICMT`. This module's writer is
//! interoperable with that reader.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The schema version of the JSON payload. Bumped if we change
/// shape so future readers can detect old payloads.
pub const TBSS_META_VERSION: u32 = 1;

/// Marker string that opens every payload so a reader can tell
/// "this comment is TinyBooth's" from a random WAV comment. Search
/// for this substring before attempting to JSON-parse.
pub const TBSS_META_MARKER: &str = "TBSS::";

/// JSON blob written into the WAV's LIST/INFO/ICMT field.
/// Order of fields here is also the order they appear in the
/// pretty-printed JSON — names chosen short so the comment stays
/// readable in third-party metadata tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TbssWavMeta {
    pub v: u32,
    pub project: String,
    /// "Suno stem (Vocals)" / "Recorded" / "TinyDAW take".
    pub source: String,
    /// True when the WAV was written with polarity flipped.
    pub polarity_inverted: bool,
    /// Name of the active correction-chain profile at write time,
    /// or `None` if no correction was attached.
    pub correction_profile: Option<String>,
    /// Telemetry profile chosen by the user (`Auto`, `Guitar`, etc.).
    pub telemetry_profile: String,
    /// `tinybooth-sound-studio v0.4.20` — produced-by string.
    pub produced_by: String,
}

impl TbssWavMeta {
    pub fn marker_string(&self) -> Result<String> {
        let json = serde_json::to_string(self).context("serialising tbss wav meta")?;
        Ok(format!("{TBSS_META_MARKER}{json}"))
    }

    /// Build from a Track + its hosting Project. Centralises the
    /// derivation so the caller doesn't have to know the field names.
    pub fn from_track(project: &crate::project::Project, track: &crate::project::Track) -> Self {
        use crate::project::TrackSource;
        let source = match &track.source {
            TrackSource::Recorded => match project.kind {
                crate::project::ProjectKind::TinyDAW => "TinyDAW take".to_string(),
                _ => "Recorded".to_string(),
            },
            TrackSource::SunoStem { role, .. } => format!("Suno stem ({})", role.label()),
        };
        Self {
            v: TBSS_META_VERSION,
            project: project.name.clone(),
            source,
            polarity_inverted: track.polarity_inverted,
            correction_profile: track.correction.as_ref().map(|p| p.name.clone()),
            telemetry_profile: format!("{:?}", track.telemetry_profile),
            produced_by: format!("tinybooth-sound-studio v{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Inject a TBSS comment into the WAV at `path`. The comment is
/// appended via a standard RIFF `LIST/INFO/ICMT` chunk so any
/// metadata-aware reader (foobar2000, exiftool, our own
/// `suno_meta::read_wav_session`) can pick it up.
///
/// **Idempotent**: if the file already has a TBSS comment (substring
/// match on `TBSS_META_MARKER` in any `ICMT` field) the new comment
/// REPLACES the old by rewriting the LIST/INFO chunk. Simpler than
/// surgically patching the ICMT alone, and cheap — projects are
/// small files.
pub fn inject_tbss_meta(path: &Path, meta: &TbssWavMeta) -> Result<()> {
    let payload = meta.marker_string()?;
    write_or_replace_icmt(path, payload.as_bytes())
}

/// Lower-level entry — write an arbitrary comment string into the
/// WAV's LIST/INFO/ICMT field. If the file already carries a
/// LIST/INFO chunk we strip it and write a fresh one with our text.
fn write_or_replace_icmt(path: &Path, payload: &[u8]) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {} for metadata injection", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("{} is not a RIFF/WAVE file", path.display());
    }
    let riff_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    // The file size should be 8 + riff_size; if a previous tool
    // tagged it wrong we just truncate our view to what the header
    // says rather than trust the OS file length.
    let logical_end = (8 + riff_size).min(bytes.len());

    // Walk chunks within [12, logical_end), strip any existing
    // top-level LIST chunks so we don't keep stacking comments.
    let mut kept: Vec<u8> = Vec::with_capacity(bytes.len());
    kept.extend_from_slice(&bytes[0..12]); // RIFF + size + WAVE
    let mut cursor = 12;
    while cursor + 8 <= logical_end {
        let id = &bytes[cursor..cursor + 4];
        let chunk_size = u32::from_le_bytes([
            bytes[cursor + 4],
            bytes[cursor + 5],
            bytes[cursor + 6],
            bytes[cursor + 7],
        ]) as usize;
        let total = 8 + chunk_size + (chunk_size & 1); // RIFF pad
        let end = (cursor + total).min(logical_end);
        if id == b"LIST" {
            // Skip — we'll write a fresh LIST below.
            cursor = end;
            continue;
        }
        kept.extend_from_slice(&bytes[cursor..end.min(bytes.len())]);
        cursor = end;
    }

    // Build the fresh LIST/INFO/ICMT chunk.
    let mut icmt: Vec<u8> = Vec::with_capacity(payload.len() + 16);
    icmt.extend_from_slice(b"ICMT");
    icmt.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    icmt.extend_from_slice(payload);
    if icmt.len() & 1 != 0 {
        icmt.push(0); // RIFF pad to even length
    }

    let info_payload_len = 4 /* "INFO" */ + icmt.len();
    let mut list_chunk: Vec<u8> = Vec::with_capacity(info_payload_len + 8);
    list_chunk.extend_from_slice(b"LIST");
    list_chunk.extend_from_slice(&(info_payload_len as u32).to_le_bytes());
    list_chunk.extend_from_slice(b"INFO");
    list_chunk.extend_from_slice(&icmt);

    kept.extend_from_slice(&list_chunk);

    // Patch the RIFF size in the leading header.
    let new_riff_size = (kept.len() - 8) as u32;
    kept[4..8].copy_from_slice(&new_riff_size.to_le_bytes());

    // Atomic rewrite: write to a temp file then rename. The user's
    // copy of the file stays valid even if the process dies mid-write.
    let tmp = path.with_extension("wav.tbss-tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("creating temp {}", tmp.display()))?;
        f.write_all(&kept)
            .with_context(|| format!("writing temp {}", tmp.display()))?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("atomic rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Read back the TBSS metadata if present. Returns `Ok(None)` when
/// the file is a valid WAV but doesn't carry our marker.
///
/// Reserved for the upcoming "drop a WAV onto the Project tab → mint
/// a track preserving its TBSS context" feature. v0.4.20 is the
/// write side; the read side ships here so the public API is stable.
#[allow(dead_code)]
pub fn read_tbss_meta(path: &Path) -> Result<Option<TbssWavMeta>> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header)
        .with_context(|| format!("reading RIFF header of {}", path.display()))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Ok(None);
    }
    let riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;

    // Walk chunks looking for LIST/INFO/ICMT.
    let mut pos: u64 = 12;
    while pos + 8 <= riff_size + 8 {
        file.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if file.read_exact(&mut hdr).is_err() {
            break;
        }
        let id = &hdr[0..4];
        let sz = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
        let total = 8 + sz + (sz & 1);
        if id == b"LIST" {
            let mut list_type = [0u8; 4];
            if file.read_exact(&mut list_type).is_err() {
                break;
            }
            if &list_type == b"INFO" {
                let mut sub = 4u64;
                while sub + 8 <= sz {
                    let mut h2 = [0u8; 8];
                    if file.read_exact(&mut h2).is_err() {
                        break;
                    }
                    let sub_id = &h2[0..4];
                    let sub_sz = u32::from_le_bytes([h2[4], h2[5], h2[6], h2[7]]) as usize;
                    if sub_id == b"ICMT" {
                        let mut buf = vec![0u8; sub_sz];
                        if file.read_exact(&mut buf).is_err() {
                            break;
                        }
                        if let Ok(s) = std::str::from_utf8(&buf) {
                            if let Some(json) = s.strip_prefix(TBSS_META_MARKER) {
                                let trimmed = json.trim_end_matches('\0');
                                if let Ok(m) = serde_json::from_str::<TbssWavMeta>(trimmed) {
                                    return Ok(Some(m));
                                }
                            }
                        }
                        // Not ours — keep walking.
                        let pad = sub_sz & 1;
                        file.seek(SeekFrom::Current(pad as i64))?;
                    } else {
                        file.seek(SeekFrom::Current((sub_sz + (sub_sz & 1)) as i64))?;
                    }
                    sub += 8 + sub_sz as u64 + (sub_sz & 1) as u64;
                }
            }
        }
        pos += total;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_silent_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..1000 {
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn fixture_meta() -> TbssWavMeta {
        TbssWavMeta {
            v: TBSS_META_VERSION,
            project: "demo".into(),
            source: "Suno stem (Vocals)".into(),
            polarity_inverted: false,
            correction_profile: Some("Suno-Vocal".into()),
            telemetry_profile: "Auto".into(),
            produced_by: "tinybooth-sound-studio v0.4.20".into(),
        }
    }

    #[test]
    fn inject_and_read_back_roundtrips() {
        let dir = std::env::temp_dir();
        let path = dir.join("tbss_meta_roundtrip.wav");
        make_silent_wav(&path);
        let meta = fixture_meta();
        inject_tbss_meta(&path, &meta).unwrap();
        let back = read_tbss_meta(&path)
            .unwrap()
            .expect("meta should be readable");
        assert_eq!(back.project, "demo");
        assert_eq!(back.source, "Suno stem (Vocals)");
        assert_eq!(back.correction_profile.as_deref(), Some("Suno-Vocal"));
        // Should still be a valid WAV (hound can re-open without error).
        let r = hound::WavReader::open(&path).expect("WAV still valid after injection");
        assert_eq!(r.spec().sample_rate, 48_000);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn second_injection_replaces_first() {
        let dir = std::env::temp_dir();
        let path = dir.join("tbss_meta_replace.wav");
        make_silent_wav(&path);
        let mut meta = fixture_meta();
        inject_tbss_meta(&path, &meta).unwrap();
        meta.project = "second pass".into();
        meta.polarity_inverted = true;
        inject_tbss_meta(&path, &meta).unwrap();
        let back = read_tbss_meta(&path).unwrap().expect("meta present");
        assert_eq!(back.project, "second pass");
        assert!(back.polarity_inverted);
        // File should not have grown unboundedly — sanity check.
        let bytes = std::fs::metadata(&path).unwrap().len();
        assert!(bytes < 10_000, "file unexpectedly large: {bytes} bytes");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_returns_none_on_plain_wav() {
        let dir = std::env::temp_dir();
        let path = dir.join("tbss_meta_plain.wav");
        make_silent_wav(&path);
        assert!(read_tbss_meta(&path).unwrap().is_none());
        let _ = std::fs::remove_file(&path);
    }
}
