//! Decode an arbitrary audio file to mono `f32` samples for analysis.
//!
//! TBSS-FR-0013 needs to run its chord analyser over whatever the user drops
//! in — mp3, flac, m4a, ogg, opus — not just WAV. `hound` only reads WAV, so
//! everything else is decoded by piping raw `f32le` out of ffmpeg.
//!
//! **This decode is for *analysis only*.** The chord-video muxer still hands
//! ffmpeg the user's original file, so the audio in the finished video is the
//! untouched source — nothing here is ever re-encoded back into a deliverable.
//! That's why forcing a fixed analysis sample rate is safe: it changes what the
//! analyser hears, never what the listener gets.
//!
//! WAV is read directly through `hound` rather than shelling out, so the common
//! case still works on a machine with no ffmpeg installed.

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Sample rate requested from ffmpeg. Fixing it means we know the rate without
/// a second `ffprobe` round-trip, and it keeps the STFT window (4096/1024)
/// covering a consistent slice of time across sources.
pub const ANALYSIS_SR: u32 = 44_100;

/// File extensions offered in the "load audio" dialog.
pub const SUPPORTED_EXTS: [&str; 8] = ["wav", "mp3", "flac", "m4a", "aac", "ogg", "opus", "wma"];

/// Decode `path` to mono `f32` in `[-1, 1]`, returning `(samples, sample_rate)`.
///
/// WAV goes through `hound`; anything else (or a WAV `hound` can't handle, such
/// as a 24-bit or extensible-format file) falls back to ffmpeg.
pub fn decode_audio_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let is_wav = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    if is_wav {
        match decode_wav_mono(path) {
            Ok(v) => return Ok(v),
            // A WAV hound rejects (24-bit, WAVE_FORMAT_EXTENSIBLE, …) is still
            // decodable by ffmpeg — fall through rather than failing outright.
            Err(e) => {
                if crate::export::find_ffmpeg().is_none() {
                    return Err(e);
                }
            }
        }
    }
    decode_via_ffmpeg(path)
}

/// Decode a WAV with `hound`. Handles int and float formats, any channel count.
pub fn decode_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(Result::ok)
                .map(|s| s as f32 * scale)
                .collect()
        }
    };
    let mono = downmix(&interleaved, ch);
    if mono.is_empty() {
        bail!("no audio samples decoded from {}", path.display());
    }
    Ok((mono, spec.sample_rate))
}

/// Pipe raw mono `f32le` out of ffmpeg and collect it.
fn decode_via_ffmpeg(path: &Path) -> Result<(Vec<f32>, u32)> {
    let ffmpeg = crate::export::find_ffmpeg().ok_or_else(|| {
        anyhow!(
            "ffmpeg not found — needed to read {} files. Drop ffmpeg.exe next to the app \
             (or into ./ffmpeg/bin/), or install it on your PATH.",
            path.extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_else(|| "this".into())
        )
    })?;

    let out = Command::new(&ffmpeg)
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(path)
        // mono f32 at a known rate, straight to stdout
        .args(["-f", "f32le", "-ac", "1", "-ar"])
        .arg(ANALYSIS_SR.to_string())
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawning ffmpeg to decode audio")?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("ffmpeg could not decode {}:\n{err}", path.display());
    }

    let samples = f32le_to_samples(&out.stdout);
    if samples.is_empty() {
        bail!("no audio samples decoded from {}", path.display());
    }
    Ok((samples, ANALYSIS_SR))
}

/// Decode any hound-readable WAV to interleaved i16 — the playback/waveform
/// currency of the app. Returns `(spec, samples, frame_count)`.
///
/// Generic over `Read` because half the callers feed an in-memory
/// `Cursor<Vec<u8>>` (`.tib` BLOBs), not a file.
///
/// This replaces four copy-pasted decode ladders (player, record-tab
/// thumbnails, crossfade loader, album clips) that all **clamped** raw
/// 24/32-bit integer samples into i16 range instead of scaling them — a
/// 24-bit sample at −40 dBFS is ±84k, so everything above ~−48 dBFS
/// clamped to full scale and played back as a square wave. Here the value
/// is shifted by the actual bit depth (arithmetic shift, sign-correct),
/// and sub-16-bit files are shifted up symmetrically.
pub fn decode_wav_i16<R: std::io::Read>(
    mut reader: hound::WavReader<R>,
) -> Result<(hound::WavSpec, Vec<i16>, u64)> {
    let spec = reader.spec();
    let frames = reader.duration() as u64;
    let bits = spec.bits_per_sample;
    let samples: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Int if bits <= 16 => {
            let up = 16 - bits;
            reader
                .samples::<i16>()
                .filter_map(Result::ok)
                .map(|s| s << up)
                .collect()
        }
        hound::SampleFormat::Int => {
            let down = (bits - 16) as u32;
            reader
                .samples::<i32>()
                .filter_map(Result::ok)
                .map(|s| (s >> down) as i16)
                .collect()
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(Result::ok)
            .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect(),
    };
    Ok((spec, samples, frames))
}

/// Expand interleaved i16 (any channel count) to interleaved stereo f32 in
/// `[-1, 1]`: channel 0 → L, channel 1 (or a mono duplicate) → R, extra
/// channels dropped. Shared by the crossfade and album loaders, which both
/// hand-rolled this loop.
pub fn wav_i16_to_stereo_f32(samples: &[i16], channels: usize, frames: usize) -> Vec<f32> {
    let ch = channels.max(1);
    let denom = i16::MAX as f32;
    let mut stereo = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let base = f * ch;
        if base + ch > samples.len() {
            break;
        }
        let l = samples[base] as f32 / denom;
        let r = if ch >= 2 {
            samples[base + 1] as f32 / denom
        } else {
            l
        };
        stereo.push(l);
        stereo.push(r);
    }
    stereo
}

/// Reinterpret a little-endian `f32` byte stream as samples. A trailing partial
/// frame (a truncated pipe) is dropped rather than producing a garbage sample.
fn f32le_to_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Average interleaved frames down to mono.
fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let ch = channels.max(1);
    if ch == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a WAV of `secs` seconds of a 440 Hz tone.
    fn write_tone_wav(path: &Path, secs: f32, channels: u16, sr: u32) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let n = (sr as f32 * secs) as usize;
        for i in 0..n {
            let t = i as f32 / sr as f32;
            let v = (std::f32::consts::TAU * 440.0 * t).sin() * 0.5;
            for _ in 0..channels {
                w.write_sample((v * i16::MAX as f32) as i16).unwrap();
            }
        }
        w.finalize().unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tbss-decode-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn decodes_mono_wav_natively() {
        let p = scratch("mono.wav");
        write_tone_wav(&p, 0.5, 1, 44_100);
        let (s, sr) = decode_audio_mono(&p).unwrap();
        assert_eq!(sr, 44_100);
        assert!(
            (s.len() as i64 - 22_050).abs() < 64,
            "got {} samples",
            s.len()
        );
        assert!(s.iter().any(|v| v.abs() > 0.2), "signal looks silent");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn downmixes_stereo_to_mono() {
        let p = scratch("stereo.wav");
        write_tone_wav(&p, 0.5, 2, 44_100);
        let (s, _) = decode_audio_mono(&p).unwrap();
        // Two channels of the same tone → half as many mono samples, same level.
        assert!(
            (s.len() as i64 - 22_050).abs() < 64,
            "got {} samples",
            s.len()
        );
        assert!(s.iter().any(|v| v.abs() > 0.2));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn downmix_averages_channels() {
        // L = +1, R = -1 interleaved → silence.
        let inter = vec![1.0, -1.0, 1.0, -1.0];
        assert_eq!(downmix(&inter, 2), vec![0.0, 0.0]);
        // Mono passes through untouched.
        assert_eq!(downmix(&[0.25, -0.5], 1), vec![0.25, -0.5]);
    }

    /// 24-bit samples must be *scaled* to i16, not clamped. Under the old
    /// per-site copies a −20 dBFS 24-bit sine (peak ±838,860) clamped to
    /// ±32,767 — full scale, i.e. a square wave. Correct scaling lands it
    /// at ~±3,277.
    #[test]
    fn decode_scales_24bit_instead_of_clamping() {
        let p = scratch("s24.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        let peak_24 = (0.1 * (1i32 << 23) as f32) as i32; // −20 dBFS
        for i in 0..4410 {
            let t = i as f32 / 44_100.0;
            let v = ((std::f32::consts::TAU * 440.0 * t).sin() * peak_24 as f32) as i32;
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();

        let reader = hound::WavReader::open(&p).unwrap();
        let (spec, samples, frames) = decode_wav_i16(reader).unwrap();
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(frames, 4410);
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        let expected = (peak_24 >> 8) as u16;
        assert!(
            (peak as i32 - expected as i32).abs() < 64,
            "peak {peak} should be ≈{expected} (scaled), not 32767 (clamped)"
        );
    }

    #[test]
    fn stereo_expansion_duplicates_mono_and_drops_extras() {
        // Mono duplicates into both channels.
        let mono = [16384i16, -16384];
        let st = wav_i16_to_stereo_f32(&mono, 1, 2);
        assert_eq!(st.len(), 4);
        assert!((st[0] - st[1]).abs() < 1e-6);
        // 3-channel keeps ch0/ch1, drops ch2.
        let tri = [100i16, 200, 300, 400, 500, 600];
        let st = wav_i16_to_stereo_f32(&tri, 3, 2);
        assert_eq!(st.len(), 4);
        assert!((st[2] * i16::MAX as f32 - 400.0).abs() < 1.0);
    }

    #[test]
    fn f32le_ignores_a_truncated_trailing_frame() {
        let mut b = 1.0f32.to_le_bytes().to_vec();
        b.extend_from_slice(&[0u8; 3]); // partial frame
        assert_eq!(f32le_to_samples(&b), vec![1.0]);
    }

    /// The whole point of this module: a real compressed source decodes.
    /// Needs ffmpeg, so it's `#[ignore]`d — run with `--ignored`.
    #[test]
    #[ignore = "requires ffmpeg; run with --ignored"]
    fn decodes_mp3_via_ffmpeg() {
        let wav = scratch("src.wav");
        let mp3 = scratch("src.mp3");
        write_tone_wav(&wav, 2.0, 2, 44_100);
        let ff = crate::export::find_ffmpeg().expect("ffmpeg");
        let st = Command::new(&ff)
            .args(["-y", "-v", "error", "-i"])
            .arg(&wav)
            .args(["-codec:a", "libmp3lame", "-b:a", "192k"])
            .arg(&mp3)
            .status()
            .unwrap();
        assert!(st.success(), "encoding the test mp3 failed");

        let (s, sr) = decode_audio_mono(&mp3).unwrap();
        assert_eq!(sr, ANALYSIS_SR);
        // mp3 adds encoder padding, so allow generous slack around 2 s.
        let secs = s.len() as f32 / sr as f32;
        assert!(
            (secs - 2.0).abs() < 0.2,
            "decoded {secs:.3}s from a 2s mp3 ({} samples)",
            s.len()
        );
        assert!(s.iter().any(|v| v.abs() > 0.2), "decoded mp3 looks silent");
        let _ = std::fs::remove_file(&wav);
        let _ = std::fs::remove_file(&mp3);
    }
}
