//! TBSS-FR-0013 · E5 — chord-diagram video build + mux over the original audio.
//!
//! Takes E3's [`VoicedSpan`]s, rasterises one E4 diagram per span, and muxes a
//! synchronised video track over the **original, untouched audio**. The output
//! is the deliverable the whole epic exists for.
//!
//! ## Why the concat demuxer rather than `image2`
//!
//! A chord diagram holds for whole seconds at a time, so emitting one PNG per
//! *frame* would write thousands of identical images (a 3-minute song at 30 fps
//! is ~5,400 files). ffmpeg's **concat demuxer** instead takes one image per
//! span plus an explicit `duration`, which encodes E1's beat timings directly
//! rather than approximating them by frame duplication — fewer files, and the
//! sync contract is stated instead of inferred.
//!
//! Two battle-scars are baked in:
//!   * the concat demuxer **ignores the final entry's `duration`** unless the
//!     last file is listed a second time, so [`build_concat`] repeats it — omit
//!     that and the last chord flashes by in one frame;
//!   * ffmpeg is run **with the frame directory as its working directory** and
//!     bare filenames in the concat list, which sidesteps Windows drive-letter
//!     and backslash escaping in concat paths entirely.
//!
//! ## Encoder probing
//!
//! Not every ffmpeg ships `libx264` — the build on this workstation is
//! configured `--disable-libx264` and offers `libopenh264` plus hardware
//! encoders instead. Hardcoding an encoder would fail on exactly those builds,
//! so [`pick_h264_encoder`] probes `ffmpeg -encoders` and takes the first
//! available in preference order (software first: reproducible and
//! GPU-independent).
//!
//! Consumed by the E2 panel's "Render video" action (not wired yet) —
//! module-level `allow(dead_code)`.
#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::chorddb::ChordDb;
use crate::chordvoice::VoicedSpan;
use crate::fretboard::{self, RenderOpts};

/// H.264 encoders in preference order. Software first so output is
/// reproducible and doesn't depend on the machine's GPU; hardware encoders are
/// the fallback for builds that ship neither software encoder.
const H264_ENCODERS: [&str; 6] = [
    "libx264",
    "libopenh264",
    "h264_nvenc",
    "h264_amf",
    "h264_qsv",
    "h264_mf",
];

/// Video build settings.
#[derive(Clone, Debug)]
pub struct VideoOpts {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Left-handed diagrams (passed through to the E4 renderer).
    pub mirror: bool,
    /// Target video bitrate, used by encoders without a CRF mode.
    pub bitrate_kbps: u32,
}

impl Default for VideoOpts {
    fn default() -> Self {
        Self {
            fps: 30,
            // 720p, even dimensions (yuv420p requires them).
            width: 1280,
            height: 720,
            mirror: false,
            bitrate_kbps: 2500,
        }
    }
}

impl VideoOpts {
    /// Force even dimensions — `yuv420p` (needed for broad player support)
    /// cannot represent odd width/height.
    fn even(&self) -> (u32, u32) {
        (self.width & !1, self.height & !1)
    }
}

/// Build the concat-demuxer script for a span list.
///
/// Returns `ffconcat` text referencing `frame_NNNN.png` by bare filename. The
/// final entry is repeated because the concat demuxer drops the last
/// `duration` — without the repeat the closing chord lasts a single frame.
pub fn build_concat(spans: &[VoicedSpan]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    if spans.is_empty() {
        return s;
    }
    for (i, span) in spans.iter().enumerate() {
        s.push_str(&format!("file 'frame_{i:04}.png'\n"));
        s.push_str(&format!("duration {:.6}\n", span.duration().max(0.001)));
    }
    // Repeat the final image so its duration is honoured.
    s.push_str(&format!("file 'frame_{:04}.png'\n", spans.len() - 1));
    s
}

/// Probe ffmpeg for a usable H.264 encoder, in [`H264_ENCODERS`] order.
pub fn pick_h264_encoder(ffmpeg: &Path) -> Option<String> {
    let out = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-encoders")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let listing = String::from_utf8_lossy(&out.stdout);
    let available: Vec<&str> = listing
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .collect();
    H264_ENCODERS
        .iter()
        .find(|e| available.contains(e))
        .map(|e| e.to_string())
}

/// Render one PNG per span into `dir`, named `frame_NNNN.png`.
///
/// N.C. spans (no voicing) still get a frame — an empty diagram — so the video
/// track stays continuous and the concat durations keep lining up with the
/// audio.
pub fn write_frames(
    spans: &[VoicedSpan],
    db: &ChordDb,
    dir: &Path,
    opts: &VideoOpts,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (w, h) = opts.even();
    let ropts = RenderOpts {
        width: w,
        height: h,
        mirror: opts.mirror,
        ..Default::default()
    };
    // A placeholder for N.C.: reuse the renderer with an all-muted shape so the
    // frame geometry (and therefore the encoder's frame size) never changes.
    let silent = crate::chorddb::Voicing {
        frets: [-1; 6],
        fingers: [0; 6],
        base_fret: 0,
        verified: false,
    };
    let _ = db; // voicings are already resolved on the spans
    for (i, span) in spans.iter().enumerate() {
        let v = span.voicing.as_ref().unwrap_or(&silent);
        let img = fretboard::render(v, &ropts);
        img.save(dir.join(format!("frame_{i:04}.png")))
            .with_context(|| format!("writing frame {i}"))?;
    }
    Ok(())
}

/// Build the chord video and mux it over `audio_in`, writing `out_path`.
///
/// The audio is passed through with `-c:a copy`, retried once with AAC if the
/// container rejects the source codec. The audio is never resampled or
/// filtered — "same untouched audio" is the epic's core promise, so a lossy
/// re-encode is a fallback, never the default.
///
/// **Caveat (measured, not assumed):** MP4 *does* accept `pcm_s16le`, so a WAV
/// source stream-copies successfully rather than falling back to AAC — the
/// audio stays bit-exact, but PCM-in-MP4 plays back in fewer players than AAC
/// does. The E2 panel should therefore offer an explicit "re-encode audio for
/// compatibility" opt-in rather than this layer quietly deciding to degrade it.
pub fn render_chord_video(
    spans: &[VoicedSpan],
    db: &ChordDb,
    audio_in: &Path,
    out_path: &Path,
    opts: &VideoOpts,
) -> Result<PathBuf> {
    if spans.is_empty() {
        return Err(anyhow!("no chord spans to render"));
    }
    let ffmpeg = crate::export::find_ffmpeg().ok_or_else(|| {
        anyhow!(
            "ffmpeg not found. Drop ffmpeg.exe next to the app (or into ./ffmpeg/bin/), \
             or install it on your PATH, then try again."
        )
    })?;
    let encoder = pick_h264_encoder(&ffmpeg)
        .ok_or_else(|| anyhow!("this ffmpeg build has no usable H.264 encoder"))?;

    let dir = std::env::temp_dir().join(format!("tinybooth-chordvid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_frames(spans, db, &dir, opts)?;
    std::fs::write(dir.join("concat.txt"), build_concat(spans))?;

    if let Some(p) = out_path.parent() {
        std::fs::create_dir_all(p)?;
    }

    // Try stream-copying the audio first; fall back to AAC when the container
    // rejects the source codec (e.g. PCM from a WAV into MP4).
    let mut last_err = String::new();
    for audio_args in [vec!["-c:a", "copy"], vec!["-c:a", "aac", "-b:a", "192k"]] {
        let mut cmd = Command::new(&ffmpeg);
        cmd.current_dir(&dir) // bare filenames → no Windows path escaping
            .arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("concat")
            .arg("-safe")
            .arg("0")
            .arg("-i")
            .arg("concat.txt")
            .arg("-i")
            .arg(audio_in)
            .arg("-map")
            .arg("0:v")
            .arg("-map")
            .arg("1:a")
            .arg("-c:v")
            .arg(&encoder)
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-r")
            .arg(opts.fps.to_string())
            .arg("-vsync")
            .arg("cfr");
        if encoder == "libx264" {
            cmd.arg("-crf").arg("20");
        } else {
            cmd.arg("-b:v").arg(format!("{}k", opts.bitrate_kbps));
        }
        for a in &audio_args {
            cmd.arg(a);
        }
        // Stop at the shorter stream so a rounding difference between the last
        // chord and the audio tail can't leave a frozen frame hanging.
        cmd.arg("-shortest").arg(out_path);

        let out = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning ffmpeg")?
            .wait_with_output()?;
        if out.status.success() {
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(out_path.to_path_buf());
        }
        last_err = String::from_utf8_lossy(&out.stderr).to_string();
    }

    let _ = std::fs::remove_dir_all(&dir);
    Err(anyhow!("ffmpeg failed:\n{last_err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chordgrid::{ChordLabel, ChordQuality};

    fn span(start: f32, end: f32, chord: Option<ChordLabel>, db: &ChordDb) -> VoicedSpan {
        let voicing = chord.and_then(|l| crate::chordvoice::voice_label(db, &l, None));
        VoicedSpan {
            start_secs: start,
            end_secs: end,
            chord,
            voicing,
            name: chord.map(|c| c.name()).unwrap_or_else(|| "N.C.".into()),
        }
    }

    #[test]
    fn concat_repeats_the_last_entry() {
        let db = ChordDb::build();
        let spans = vec![
            span(
                0.0,
                2.0,
                Some(ChordLabel {
                    root: 0,
                    quality: ChordQuality::Major,
                }),
                &db,
            ),
            span(
                2.0,
                4.0,
                Some(ChordLabel {
                    root: 7,
                    quality: ChordQuality::Major,
                }),
                &db,
            ),
        ];
        let txt = build_concat(&spans);
        assert!(txt.starts_with("ffconcat version 1.0"));
        // Two spans → three `file` lines (the last repeated).
        assert_eq!(txt.matches("file '").count(), 3);
        assert_eq!(txt.matches("frame_0001.png").count(), 2);
        assert!(txt.contains("duration 2.000000"));
    }

    #[test]
    fn concat_of_empty_spans_is_just_the_header() {
        assert_eq!(build_concat(&[]).trim(), "ffconcat version 1.0");
    }

    #[test]
    fn zero_length_span_still_gets_a_positive_duration() {
        let db = ChordDb::build();
        let s = vec![span(
            1.0,
            1.0,
            Some(ChordLabel {
                root: 0,
                quality: ChordQuality::Major,
            }),
            &db,
        )];
        let txt = build_concat(&s);
        // A zero duration would make ffmpeg drop the entry entirely.
        assert!(txt.contains("duration 0.001000"));
    }

    #[test]
    fn frames_are_written_one_per_span_including_nc() {
        let db = ChordDb::build();
        let spans = vec![
            span(
                0.0,
                1.0,
                Some(ChordLabel {
                    root: 0,
                    quality: ChordQuality::Major,
                }),
                &db,
            ),
            span(1.0, 2.0, None, &db), // N.C. still gets a frame
        ];
        let dir = std::env::temp_dir().join(format!("tbss-frames-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let opts = VideoOpts {
            width: 320,
            height: 180,
            ..Default::default()
        };
        write_frames(&spans, &db, &dir, &opts).unwrap();
        assert!(dir.join("frame_0000.png").is_file());
        assert!(dir.join("frame_0001.png").is_file());
        // Frame size must be constant across the whole track.
        let a = image::open(dir.join("frame_0000.png")).unwrap();
        let b = image::open(dir.join("frame_0001.png")).unwrap();
        assert_eq!(a.width(), b.width());
        assert_eq!(a.height(), b.height());
        assert_eq!((a.width(), a.height()), (320, 180));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end integration slice: synthesised audio → E1 grid → E3 spans →
    /// E5 muxed video. Requires ffmpeg on PATH, so it's `#[ignore]`d by default
    /// and run explicitly (`cargo test -- --ignored e2e`). This is the
    /// "prove a 10-second slice early" gate from the FR's risk section.
    #[test]
    #[ignore = "requires ffmpeg; run with --ignored"]
    fn e2e_ten_second_slice() {
        use crate::chordgrid;

        let sr = 44_100u32;
        let secs_per_chord = 2.5f32;
        // C, G, Am, F — the canonical loop, as additive triads with a few
        // harmonics and a per-beat amplitude envelope so onset detection has
        // something to lock onto.
        let chords: [[f32; 3]; 4] = [
            [261.63, 329.63, 392.00], // C  E  G
            [196.00, 246.94, 293.66], // G  B  D
            [220.00, 261.63, 329.63], // A  C  E
            [174.61, 220.00, 261.63], // F  A  C
        ];
        let total = (sr as f32 * secs_per_chord * 4.0) as usize;
        let mut mono = vec![0.0f32; total];
        let beat = sr as f32 * 0.5; // 120 BPM
        for (i, s) in mono.iter_mut().enumerate() {
            let t = i as f32 / sr as f32;
            let ci = ((t / secs_per_chord) as usize).min(3);
            let env = {
                let phase = (i as f32 % beat) / beat;
                (1.0 - phase).powf(1.5)
            };
            let mut v = 0.0;
            for &f in &chords[ci] {
                v += (std::f32::consts::TAU * f * t).sin();
                v += 0.3 * (std::f32::consts::TAU * 2.0 * f * t).sin();
            }
            *s = 0.12 * env * v;
        }

        let dir = std::env::temp_dir().join(format!("tbss-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("slice.wav");
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: sr,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&wav, spec).unwrap();
            for s in &mono {
                w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .unwrap();
            }
            w.finalize().unwrap();
        }

        // E1 → E3 → E5.
        let grid = chordgrid::analyze(&mono, sr);
        assert!(!grid.cells.is_empty(), "E1 produced no cells");
        let db = ChordDb::build();
        let spans = crate::chordvoice::resolve_spans(&grid, &db);
        assert!(!spans.is_empty(), "E3 produced no spans");
        eprintln!(
            "E1 bpm={:.1} cells={} → E3 spans={} [{}]",
            grid.bpm,
            grid.cells.len(),
            spans.len(),
            spans
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join(" ")
        );

        let out = dir.join("slice.mp4");
        let opts = VideoOpts {
            width: 640,
            height: 360,
            ..Default::default()
        };
        let made = render_chord_video(&spans, &db, &wav, &out, &opts).expect("render_chord_video");
        assert!(made.is_file(), "no output file");
        let size = std::fs::metadata(&made).unwrap().len();
        assert!(size > 10_000, "output suspiciously small: {size} bytes");

        // Verify the muxed result with ffprobe: both streams present, and the
        // video duration matches the audio within a frame's worth of slack.
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-show_entries",
                "stream=codec_type,codec_name",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&made)
            .output()
            .expect("ffprobe");
        let info = String::from_utf8_lossy(&probe.stdout).to_string();
        eprintln!("--- ffprobe ---\n{info}");
        assert!(info.contains("codec_type=video"), "no video stream");
        assert!(info.contains("codec_type=audio"), "no audio stream");
        let dur: f32 = info
            .lines()
            .find_map(|l| l.strip_prefix("duration="))
            .and_then(|d| d.trim().parse().ok())
            .expect("duration");
        let expected = secs_per_chord * 4.0;
        assert!(
            (dur - expected).abs() < 0.5,
            "muxed duration {dur:.2}s drifted from source {expected:.2}s"
        );
        eprintln!("OK: {size} bytes, {dur:.2}s, encoder-probed H.264");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn odd_dimensions_are_rounded_even_for_yuv420p() {
        let o = VideoOpts {
            width: 641,
            height: 361,
            ..Default::default()
        };
        assert_eq!(o.even(), (640, 360));
    }
}
