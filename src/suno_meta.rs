//! Read RIFF/INFO/ICMT comments from Suno-exported WAV files.
//!
//! Empirically (verified on real Suno output, see TBSS-FR-0004 follow-up):
//! every stem WAV ships with a `LIST/INFO/ICMT` chunk containing the
//! string "made with suno studio; created=<ISO 8601 UTC>".
//!
//! No song UUID, no track ID, no song title — but the timestamp is
//! distinct per export session and identical across all stems of one
//! render. We capture it as a Unix-epoch integer (per spec request:
//! "hash all timestamps to epoch integers"), which is sortable directly
//! and round-trips cleanly through JSON.

use chrono::{DateTime, Utc};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SunoSession {
    /// Unix seconds since epoch — sortable, integer, JSON-clean.
    pub epoch: i64,
    /// Original ISO 8601 string from the WAV (kept for human display).
    pub iso_timestamp: String,
    /// Provenance string from the same ICMT (e.g. "made with suno studio").
    pub provenance: String,
}

/// Open a WAV, walk its RIFF chunks, and return the parsed Suno session
/// info from `LIST/INFO/ICMT` if present and well-formed. Returns `None`
/// for non-Suno WAVs or anything we can't parse.
pub fn read_wav_session(path: &Path) -> Option<SunoSession> {
    let mut f = BufReader::new(File::open(path).ok()?);

    // RIFF/WAVE header.
    let mut header = [0u8; 12];
    f.read_exact(&mut header).ok()?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return None;
    }

    // Walk top-level chunks until we either find LIST/INFO/ICMT or hit
    // the end / the data chunk (after which scanning is pointless).
    loop {
        let mut chunk_hdr = [0u8; 8];
        if f.read_exact(&mut chunk_hdr).is_err() { return None; }
        let chunk_id = &chunk_hdr[0..4];
        let chunk_sz = u32::from_le_bytes([chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]]) as u64;
        let padded = chunk_sz + (chunk_sz & 1);

        if chunk_id == b"LIST" {
            let mut list_type = [0u8; 4];
            if f.read_exact(&mut list_type).is_err() { return None; }
            if &list_type == b"INFO" {
                if let Some(s) = scan_info(&mut f, chunk_sz - 4) {
                    return Some(s);
                }
                // ICMT not found in this LIST/INFO; keep walking — Suno
                // only stamps one but be tolerant of files with other
                // INFO chunks before the relevant one.
            } else {
                // Skip the rest of this LIST chunk.
                f.seek(SeekFrom::Current((padded - 4) as i64)).ok()?;
            }
        } else if chunk_id == b"data" {
            // Past metadata into PCM body — stop.
            return None;
        } else {
            f.seek(SeekFrom::Current(padded as i64)).ok()?;
        }
    }
}

fn scan_info<R: Read + Seek>(f: &mut R, info_body_size: u64) -> Option<SunoSession> {
    let mut consumed = 0u64;
    while consumed + 8 <= info_body_size {
        let mut sub_hdr = [0u8; 8];
        if f.read_exact(&mut sub_hdr).is_err() { return None; }
        let sub_id = &sub_hdr[0..4];
        let sub_sz = u32::from_le_bytes([sub_hdr[4], sub_hdr[5], sub_hdr[6], sub_hdr[7]]) as u64;
        let padded = sub_sz + (sub_sz & 1);
        consumed += 8 + padded;

        if sub_id == b"ICMT" {
            let mut payload = vec![0u8; sub_sz as usize];
            if f.read_exact(&mut payload).is_err() { return None; }
            // Burn the alignment pad if any.
            if padded > sub_sz {
                let _ = f.seek(SeekFrom::Current(1));
            }
            let text = strip_trailing_nul(&payload);
            return parse_icmt(&text);
        } else {
            f.seek(SeekFrom::Current(padded as i64)).ok()?;
        }
    }
    None
}

fn strip_trailing_nul(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.trim_end_matches(|c: char| c == '\0' || c.is_whitespace()).to_string()
}

fn parse_icmt(text: &str) -> Option<SunoSession> {
    // Format observed on real Suno output:
    //   "made with suno studio; created=2026-04-25T05:31:37Z"
    // Tolerate variants: missing semicolon, only the created= part,
    // extra whitespace.
    let mut iso: Option<String> = None;
    let mut provenance = String::new();
    for raw in text.split(';') {
        let part = raw.trim();
        if part.is_empty() { continue; }
        if let Some(rest) = part.strip_prefix("created=") {
            iso = Some(rest.trim().to_string());
        } else {
            if !provenance.is_empty() { provenance.push_str("; "); }
            provenance.push_str(part);
        }
    }
    let iso = iso?;
    let dt: DateTime<Utc> = iso.parse().ok()?;
    Some(SunoSession {
        epoch: dt.timestamp(),
        iso_timestamp: iso,
        provenance: if provenance.is_empty() { "made with suno studio".into() } else { provenance },
    })
}
