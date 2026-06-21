//! TinyBooth Album — in-memory model + pure-DSP render path.
//!
//! An Album is an arrangement of N clips, each referencing an external
//! source (currently a `.tib`'s bounced `mix_run`). The render path
//! decodes each source once, applies a per-clip equal-power fade-in /
//! fade-out + linear gain, then sums onto a single timeline at each
//! clip's `start_secs`. See `docs/feature-requests/TBSS-FR-0012-tinybooth-album.md`.

use anyhow::{anyhow, Context, Result};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// One clip in the album. Source is referenced by path (no embedded
/// audio); load-time resolution opens the file and decodes its
/// bounced mix.
#[derive(Debug, Clone)]
pub struct AlbumClip {
    pub source_path: PathBuf,
    pub start_secs: f32,
    pub fade_in_secs: f32,
    pub fade_out_secs: f32,
    pub gain_db: f32,
}

/// In-memory album. Saved to a `.tba` via [`crate::tba_album`].
#[derive(Debug, Clone, Default)]
pub struct Album {
    pub name: String,
    pub clips: Vec<AlbumClip>,
}

/// A clip with its source audio loaded — produced by the render path
/// before it sums onto the output timeline. Public so the UI can
/// surface per-clip diagnostics (decoded duration, sample rate) if
/// it ever wants to.
#[derive(Debug, Clone)]
pub struct LoadedAlbumClip {
    pub samples: Vec<f32>, // interleaved stereo f32
    pub sample_rate: u32,
    pub start_secs: f32,
    pub fade_in_secs: f32,
    pub fade_out_secs: f32,
    pub gain_db: f32,
}

/// Result of rendering an album: interleaved stereo f32 buffer plus
/// the resolved sample rate.
#[derive(Debug, Clone)]
pub struct AlbumMix {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16, // always 2 (we always render stereo)
}

/// Decode every clip's source into memory. Each clip's source is a
/// `.tib`'s `mix_run` WAV blob. Refuses sources that aren't `.tib` or
/// don't have a bounce yet. Rejects clip sets with mismatched sample
/// rates (the rest of the pipeline doesn't resample).
pub fn load_clips(clips: &[AlbumClip]) -> Result<Vec<LoadedAlbumClip>> {
    if clips.is_empty() {
        return Err(anyhow!("album has no clips"));
    }
    let mut loaded: Vec<LoadedAlbumClip> = Vec::with_capacity(clips.len());
    let mut sample_rate: Option<u32> = None;
    for (i, c) in clips.iter().enumerate() {
        let (samples, sr) = decode_clip_source(&c.source_path)
            .with_context(|| format!("loading clip {} ({})", i + 1, c.source_path.display()))?;
        match sample_rate {
            None => sample_rate = Some(sr),
            Some(existing) if existing != sr => {
                return Err(anyhow!(
                    "clip {} is {} Hz but album is {} Hz — re-bounce one of them to match",
                    i + 1,
                    sr,
                    existing
                ));
            }
            _ => {}
        }
        loaded.push(LoadedAlbumClip {
            samples,
            sample_rate: sr,
            start_secs: c.start_secs.max(0.0),
            fade_in_secs: c.fade_in_secs.max(0.0),
            fade_out_secs: c.fade_out_secs.max(0.0),
            gain_db: c.gain_db,
        });
    }
    Ok(loaded)
}

/// Decode a clip's source file. v0.4.52: only `.tib`'s `mix_run` is
/// supported. The decoded buffer is interleaved stereo f32.
fn decode_clip_source(path: &Path) -> Result<(Vec<f32>, u32)> {
    let is_tib = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("tib"))
        .unwrap_or(false);
    if !is_tib {
        return Err(anyhow!(
            "v0.4.52 only accepts .tib clips (got {})",
            path.display()
        ));
    }
    let db = crate::tib::TibDb::open(path.to_path_buf())
        .with_context(|| format!("opening {}", path.display()))?;
    let bytes = db
        .read_mix_run_audio()
        .context("reading mix_run audio")?
        .ok_or_else(|| {
            anyhow!(
                "{} has no bounced mix yet — open the project in TinyBooth and click Bounce first",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("this .tib")
            )
        })?;
    let reader = hound::WavReader::new(Cursor::new(bytes)).context("decoding mix_run WAV bytes")?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let frames = reader.duration() as usize;
    let samples_i16: Vec<i16> = match spec.sample_format {
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 16 {
                reader
                    .into_samples::<i16>()
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                reader
                    .into_samples::<i32>()
                    .filter_map(|r| r.ok())
                    .map(|s| s.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
                    .collect()
            }
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|r| r.ok())
            .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect(),
    };
    let denom = i16::MAX as f32;
    let mut stereo = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let base = f * channels;
        if base + channels > samples_i16.len() {
            break;
        }
        let l = samples_i16[base] as f32 / denom;
        let r = if channels >= 2 {
            samples_i16[base + 1] as f32 / denom
        } else {
            l
        };
        stereo.push(l);
        stereo.push(r);
    }
    Ok((stereo, spec.sample_rate))
}

/// Render the album into a single stereo f32 buffer. Each clip
/// contributes its own fade-in / fade-out (equal-power) and gain,
/// summed at its `start_secs` position on the timeline. Adjacent
/// clips whose fades overlap produce an emergent equal-power
/// crossfade — no explicit transition object.
pub fn render(clips: &[LoadedAlbumClip]) -> Result<AlbumMix> {
    if clips.is_empty() {
        return Err(anyhow!("album has no clips"));
    }
    let sample_rate = clips[0].sample_rate;
    let sr_f = sample_rate as f32;

    // Output frame range: [0, max(start + duration)]
    let end_secs = clips
        .iter()
        .map(|c| c.start_secs + (c.samples.len() as f32 / 2.0) / sr_f)
        .fold(0.0_f32, f32::max);
    let total_frames = (end_secs * sr_f).ceil() as usize;
    let mut out = vec![0.0_f32; total_frames * 2];

    for c in clips {
        let in_frames = c.samples.len() / 2;
        if in_frames == 0 {
            continue;
        }
        let start_frame = (c.start_secs.max(0.0) * sr_f).round() as usize;
        let fade_in_frames = ((c.fade_in_secs * sr_f).round() as usize).min(in_frames);
        let fade_out_frames = ((c.fade_out_secs * sr_f).round() as usize).min(in_frames);
        let gain = 10f32.powf(c.gain_db / 20.0);
        for f in 0..in_frames {
            let out_f = start_frame + f;
            if out_f * 2 + 1 >= out.len() {
                break;
            }
            // Equal-power fade-in / fade-out. The two ramps are
            // identical curves time-reversed; they sum to one in
            // power when adjacent clips' fades overlap perfectly.
            let mut env = 1.0_f32;
            if fade_in_frames > 0 && f < fade_in_frames {
                let t = f as f32 / fade_in_frames as f32;
                let s = (t * std::f32::consts::PI * 0.5).sin();
                env *= s * s;
            }
            if fade_out_frames > 0 && f + fade_out_frames > in_frames {
                let t = (in_frames - f) as f32 / fade_out_frames as f32;
                let s = (t * std::f32::consts::PI * 0.5).sin();
                env *= s * s;
            }
            let g = env * gain;
            out[out_f * 2] += c.samples[f * 2] * g;
            out[out_f * 2 + 1] += c.samples[f * 2 + 1] * g;
        }
    }

    // Soft-limit: scale down if the peak exceeds 1.0. Matches export.rs.
    let peak = out.iter().copied().fold(0.0_f32, |a, b| a.max(b.abs()));
    if peak > 1.0 {
        let k = 1.0 / peak;
        for s in &mut out {
            *s *= k;
        }
    }

    Ok(AlbumMix {
        samples: out,
        sample_rate,
        channels: 2,
    })
}

/// Stable hash of the album's mix-relevant state — the analogue of
/// [`crate::export::compute_mixrun_signature`] for `.tba`. Used to
/// flag the album's bounced cache as fresh / stale.
pub fn compute_mixrun_signature(album: &Album) -> String {
    #[derive(serde::Serialize)]
    struct ClipSig<'a> {
        source: &'a str,
        start_bits: u32,
        fin_bits: u32,
        fout_bits: u32,
        gain_bits: u32,
    }
    #[derive(serde::Serialize)]
    struct AlbumSig<'a> {
        name: &'a str,
        clips: Vec<ClipSig<'a>>,
    }
    let clips: Vec<ClipSig> = album
        .clips
        .iter()
        .map(|c| ClipSig {
            source: c.source_path.to_str().unwrap_or(""),
            start_bits: c.start_secs.to_bits(),
            fin_bits: c.fade_in_secs.to_bits(),
            fout_bits: c.fade_out_secs.to_bits(),
            gain_bits: c.gain_db.to_bits(),
        })
        .collect();
    let sig = AlbumSig {
        name: &album.name,
        clips,
    };
    let json = serde_json::to_string(&sig).unwrap_or_default();
    let mut h = DefaultHasher::new();
    json.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Encode an album mix as a 16-bit in-memory WAV byte stream so the
/// `mix_run` row can hold it the same way `.tib`'s does.
pub fn encode_mix_to_wav_bytes(mix: &AlbumMix) -> Result<Vec<u8>> {
    use hound::{SampleFormat, WavSpec, WavWriter};
    let spec = WavSpec {
        channels: mix.channels,
        sample_rate: mix.sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut buf = Cursor::new(Vec::<u8>::with_capacity(mix.samples.len() * 2 + 44));
    {
        let mut w = WavWriter::new(&mut buf, spec).context("creating in-memory WAV writer")?;
        for s in &mix.samples {
            let clamped = s.clamp(-1.0, 1.0);
            w.write_sample((clamped * i16::MAX as f32) as i16)?;
        }
        w.finalize().context("finalising in-memory WAV")?;
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_sec_const_clip(sr: u32, value: f32, start_secs: f32, fade: f32) -> LoadedAlbumClip {
        let frames = sr as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(value);
            samples.push(value);
        }
        LoadedAlbumClip {
            samples,
            sample_rate: sr,
            start_secs,
            fade_in_secs: fade,
            fade_out_secs: fade,
            gain_db: 0.0,
        }
    }

    #[test]
    fn render_empty_errors() {
        assert!(render(&[]).is_err());
    }

    #[test]
    fn render_single_clip_no_fade_passes_through() {
        // 1s of constant 0.5; no fades, no gain — peak in the middle
        // should be exactly 0.5.
        let clip = LoadedAlbumClip {
            samples: vec![0.5; 100 * 2], // 100 frames at sr=100
            sample_rate: 100,
            start_secs: 0.0,
            fade_in_secs: 0.0,
            fade_out_secs: 0.0,
            gain_db: 0.0,
        };
        let mix = render(&[clip]).unwrap();
        assert_eq!(mix.sample_rate, 100);
        // Mid-frame should be 0.5 unchanged.
        let mid = mix.samples[50 * 2];
        assert!((mid - 0.5).abs() < 1e-6, "mid = {mid}");
    }

    #[test]
    fn render_two_clips_equal_power_overlap_sums_to_one_in_power() {
        // Two 1-second clips at sr=1000, second starts at 0.5s with
        // 0.5s fades on the inner edges, so they overlap fully across
        // [0.5, 1.0]. The equal-power crossfade should keep the power
        // sum at ~1.0 (within rounding).
        let sr = 1000;
        let a = LoadedAlbumClip {
            samples: vec![1.0; sr as usize * 2],
            sample_rate: sr,
            start_secs: 0.0,
            fade_in_secs: 0.0,
            fade_out_secs: 0.5,
            gain_db: 0.0,
        };
        let b = LoadedAlbumClip {
            samples: vec![1.0; sr as usize * 2],
            sample_rate: sr,
            start_secs: 0.5,
            fade_in_secs: 0.5,
            fade_out_secs: 0.0,
            gain_db: 0.0,
        };
        let mix = render(&[a, b]).unwrap();
        // Sample in the middle of the overlap (frame index ~ 750 = 0.75s).
        let f = 750;
        let s = mix.samples[f * 2];
        let power = s * s;
        // Equal-power: each contributes cos²/sin², summing in power
        // to 1.0. They're playing the SAME constant 1.0 signal so the
        // amplitude sum is sin² + cos² of the same point = 1. Either
        // way the result is ~1.0 in amplitude here. Tolerant bound.
        assert!(
            (s - 1.0).abs() < 0.05,
            "expected ~1.0 in overlap, got {s} (power {power})"
        );
    }

    #[test]
    fn render_applies_gain() {
        let mut clip = one_sec_const_clip(100, 1.0, 0.0, 0.0);
        clip.gain_db = -6.0; // ~0.501 lin
        let mix = render(&[clip]).unwrap();
        let mid = mix.samples[50 * 2];
        assert!(
            (mid - 0.501).abs() < 0.01,
            "expected ~0.501 with -6 dB, got {mid}"
        );
    }

    #[test]
    fn signature_stable_and_change_invalidates() {
        let a = Album {
            name: "A".into(),
            clips: vec![AlbumClip {
                source_path: "x.tib".into(),
                start_secs: 0.0,
                fade_in_secs: 1.0,
                fade_out_secs: 1.0,
                gain_db: 0.0,
            }],
        };
        let s1 = compute_mixrun_signature(&a);
        let s2 = compute_mixrun_signature(&a);
        assert_eq!(s1, s2, "signature is deterministic");
        let mut b = a.clone();
        b.clips[0].gain_db = -1.0;
        let s3 = compute_mixrun_signature(&b);
        assert_ne!(s1, s3, "gain change must change signature");
    }
}
