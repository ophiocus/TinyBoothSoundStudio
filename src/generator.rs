//! Generator-track DSP — synthesises the WAV bytes for a
//! `TrackSource::Generator` from its parameters. Pure functions:
//! input is the generator mode + duration + sample rate, output is a
//! 16-bit PCM WAV `Vec<u8>` ready to hand to the audio path
//! (`TibDb::commit_destructive_revision` for `.tib` projects, plain
//! file write for folder projects). See [TBSS-FR-0009].
//!
//! [TBSS-FR-0009]: ../../docs/feature-requests/TBSS-FR-0009-generator-track.md
//!
//! **No I/O.** The bake plumbing (resolving duration from the project,
//! storing the audio through the backing, writing the timestamped
//! export) lives in `app::bake_generator` — step 3 of the feature.
//!
//! ## Modes
//!
//! - **Binaural** — independent sine carriers per channel at
//!   `carrier_hz ± beat_hz/2`. Stereo output is mandatory; the
//!   entrainment IS the L–R difference. Continuous (no envelope
//!   clicks).
//! - **Isochronic** — single sine carrier × a smoothed pulse envelope
//!   at `pulse_hz`. The envelope is `sin²(π·phase/duty)` inside the
//!   on-portion and 0 outside — zero crossings at pulse edges, no
//!   square-wave clicks. Stereo-duplicated for consistency with the
//!   project's stereo-first convention.
//! - **Layered** — deferred. `bake` returns `Err`.
//!
//! NOTE: the production caller (`app::bake_generator`) lands in step 3
//! of TBSS-FR-0009. Until then, all of this is exercised only by the
//! module tests — hence the module-level dead-code allow.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::f32::consts::TAU;
use std::io::Cursor;

use crate::project::GeneratorMode;

/// Top-level dispatch — render `mode` to a 16-bit PCM stereo WAV.
/// Returned bytes are ready to feed into hound's reader or the audio
/// path. Errors on the `Layered` variant (not yet implemented) and on
/// any out-of-range parameters.
pub fn bake(mode: &GeneratorMode, duration_secs: f32, sample_rate: u32) -> Result<Vec<u8>> {
    if duration_secs <= 0.0 {
        return Err(anyhow!(
            "generator duration must be > 0 (got {duration_secs})"
        ));
    }
    if sample_rate == 0 {
        return Err(anyhow!("generator sample rate must be > 0"));
    }
    match mode {
        GeneratorMode::Binaural {
            carrier_hz,
            beat_hz,
            amplitude,
        } => bake_binaural(
            *carrier_hz,
            *beat_hz,
            *amplitude,
            duration_secs,
            sample_rate,
        ),
        GeneratorMode::Isochronic {
            tone_hz,
            pulse_hz,
            duty_cycle,
            amplitude,
        } => bake_isochronic(
            *tone_hz,
            *pulse_hz,
            *duty_cycle,
            *amplitude,
            duration_secs,
            sample_rate,
        ),
        GeneratorMode::Layered => Err(anyhow!(
            "the Layered generator mode is not yet implemented \
             (TBSS-FR-0009 §'Modes — modular, scope all three'). \
             The data-model slot exists so the design can land \
             without rework; the DSP is deferred."
        )),
    }
}

/// Bake a stereo binaural-beats WAV. Left channel sine at `carrier_hz
/// − beat_hz/2`, right at `carrier_hz + beat_hz/2`. Continuous sine
/// — no envelope, no clicks. The brain perceives the L–R difference
/// as a beat at `beat_hz`. Headphones required to perceive the effect.
pub fn bake_binaural(
    carrier_hz: f32,
    beat_hz: f32,
    amplitude: f32,
    duration_secs: f32,
    sample_rate: u32,
) -> Result<Vec<u8>> {
    validate_amplitude(amplitude)?;
    if !carrier_hz.is_finite() || carrier_hz <= 0.0 {
        return Err(anyhow!(
            "binaural carrier must be > 0 Hz (got {carrier_hz})"
        ));
    }
    if !beat_hz.is_finite() || beat_hz < 0.0 {
        return Err(anyhow!("binaural beat must be ≥ 0 Hz (got {beat_hz})"));
    }

    let f_l = (carrier_hz - beat_hz * 0.5).max(0.0);
    let f_r = carrier_hz + beat_hz * 0.5;
    let total_frames = ((duration_secs * sample_rate as f32).round() as u64).max(1);
    let sr_f = sample_rate as f32;
    let scale = amplitude * i16::MAX as f32;

    let mut buf = Vec::with_capacity((total_frames as usize) * 4 + 64); // 4 B per stereo i16 frame + WAV header
    {
        let mut w = WavWriter::new(Cursor::new(&mut buf), stereo_spec(sample_rate))?;
        for n in 0..total_frames {
            let t = n as f32 / sr_f;
            let l = (TAU * f_l * t).sin() * scale;
            let r = (TAU * f_r * t).sin() * scale;
            w.write_sample(l as i16)?;
            w.write_sample(r as i16)?;
        }
        w.finalize()?;
    }
    Ok(buf)
}

/// Bake an isochronic-tone WAV: a sine carrier at `tone_hz` modulated
/// by a smoothed pulse envelope at `pulse_hz`. The envelope is
/// `sin²(π · pulse_phase / duty_cycle)` inside the on-portion of each
/// pulse cycle and 0 outside — smooth ramps in and out of every pulse
/// (no clicks). `duty_cycle` is the fraction of one pulse period that's
/// "on". Stereo output (mono duplicated to L/R).
pub fn bake_isochronic(
    tone_hz: f32,
    pulse_hz: f32,
    duty_cycle: f32,
    amplitude: f32,
    duration_secs: f32,
    sample_rate: u32,
) -> Result<Vec<u8>> {
    validate_amplitude(amplitude)?;
    if !tone_hz.is_finite() || tone_hz <= 0.0 {
        return Err(anyhow!("isochronic tone must be > 0 Hz (got {tone_hz})"));
    }
    if !pulse_hz.is_finite() || pulse_hz <= 0.0 {
        return Err(anyhow!("isochronic pulse must be > 0 Hz (got {pulse_hz})"));
    }
    if !duty_cycle.is_finite() || !(0.0..=1.0).contains(&duty_cycle) {
        return Err(anyhow!(
            "isochronic duty_cycle must be in [0, 1] (got {duty_cycle})"
        ));
    }

    let total_frames = ((duration_secs * sample_rate as f32).round() as u64).max(1);
    let sr_f = sample_rate as f32;
    let scale = amplitude * i16::MAX as f32;

    let mut buf = Vec::with_capacity((total_frames as usize) * 4 + 64);
    {
        let mut w = WavWriter::new(Cursor::new(&mut buf), stereo_spec(sample_rate))?;
        for n in 0..total_frames {
            let t = n as f32 / sr_f;
            // Phase within one pulse period, [0, 1).
            let pulse_phase = (t * pulse_hz).fract();
            let env = if pulse_phase < duty_cycle && duty_cycle > 0.0 {
                let p = std::f32::consts::PI * pulse_phase / duty_cycle;
                p.sin() * p.sin()
            } else {
                0.0
            };
            let s = (TAU * tone_hz * t).sin() * env * scale;
            let v = s as i16;
            w.write_sample(v)?;
            w.write_sample(v)?;
        }
        w.finalize()?;
    }
    Ok(buf)
}

fn validate_amplitude(a: f32) -> Result<()> {
    if !a.is_finite() || !(0.0..=1.0).contains(&a) {
        return Err(anyhow!("amplitude must be in [0, 1] (got {a})"));
    }
    Ok(())
}

fn stereo_spec(sample_rate: u32) -> WavSpec {
    WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_wav(bytes: &[u8]) -> hound::WavReader<Cursor<&[u8]>> {
        hound::WavReader::new(Cursor::new(bytes)).expect("valid WAV header")
    }

    #[test]
    fn binaural_produces_stereo_wav_of_expected_length() {
        let bytes = bake_binaural(200.0, 10.0, 0.3, 0.25, 8_000).unwrap();
        let reader = parse_wav(&bytes);
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 8_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(reader.duration(), 2_000, "0.25 s × 8 kHz = 2000 frames");
    }

    #[test]
    fn binaural_first_sample_is_zero() {
        // sin(0) = 0 — both channels start at 0 regardless of carrier/beat.
        let bytes = bake_binaural(440.0, 7.0, 0.5, 0.01, 48_000).unwrap();
        let mut reader = parse_wav(&bytes);
        let mut samples = reader.samples::<i16>();
        let l = samples.next().unwrap().unwrap();
        let r = samples.next().unwrap().unwrap();
        assert_eq!(l, 0);
        assert_eq!(r, 0);
    }

    #[test]
    fn binaural_left_right_differ_after_a_few_samples() {
        // 10 Hz beat → L and R sines drift apart; at 48 kHz, by sample 200
        // (~4 ms) the phases have diverged enough to produce different
        // values for any non-trivial carrier.
        let bytes = bake_binaural(200.0, 10.0, 0.5, 0.005, 48_000).unwrap();
        let mut reader = parse_wav(&bytes);
        let s: Vec<i16> = reader.samples::<i16>().filter_map(|r| r.ok()).collect();
        assert!(s.len() >= 400, "0.005 s × 48 kHz × 2 ch = 480 samples");
        // Sample 200: stereo frame 100 → indices 200 (L) and 201 (R).
        assert_ne!(
            s[200], s[201],
            "L and R should diverge with a non-zero beat"
        );
    }

    #[test]
    fn isochronic_produces_stereo_wav_of_expected_length() {
        let bytes = bake_isochronic(200.0, 10.0, 0.5, 0.3, 0.1, 48_000).unwrap();
        let reader = parse_wav(&bytes);
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(reader.duration(), 4_800, "0.1 s × 48 kHz = 4800 frames");
    }

    #[test]
    fn isochronic_envelope_is_zero_outside_duty() {
        // pulse_hz=10, duty_cycle=0.2 → on-portion for first 20 ms of each
        // 100 ms period. Sample at t=50 ms (well into the off-portion):
        // envelope is 0, so the sample value is 0.
        let bytes = bake_isochronic(440.0, 10.0, 0.2, 1.0, 0.1, 48_000).unwrap();
        let mut reader = parse_wav(&bytes);
        let samples: Vec<i16> = reader.samples::<i16>().filter_map(|r| r.ok()).collect();
        // 50 ms × 48 kHz × 2 ch = 4800 — sample at offset 4800 is t=50ms left ch.
        // Envelope is 0 there, so the value is 0.
        assert_eq!(samples[4800], 0, "off-portion of duty must be silent");
        assert_eq!(samples[4801], 0);
    }

    #[test]
    fn bake_dispatches_on_mode() {
        let bin = bake(
            &GeneratorMode::Binaural {
                carrier_hz: 200.0,
                beat_hz: 10.0,
                amplitude: 0.3,
            },
            0.01,
            8_000,
        )
        .unwrap();
        let iso = bake(
            &GeneratorMode::Isochronic {
                tone_hz: 200.0,
                pulse_hz: 10.0,
                duty_cycle: 0.5,
                amplitude: 0.3,
            },
            0.01,
            8_000,
        )
        .unwrap();
        // Different DSP — they produce different bytes (header may match
        // but sample data differs).
        assert_ne!(bin, iso);
    }

    #[test]
    fn layered_mode_is_unimplemented() {
        let err = bake(&GeneratorMode::Layered, 1.0, 8_000).unwrap_err();
        assert!(
            err.to_string().contains("not yet implemented"),
            "layered should be a clear unimplemented error, got: {err}"
        );
    }

    #[test]
    fn invalid_params_rejected() {
        // Negative duration.
        assert!(bake(
            &GeneratorMode::Binaural {
                carrier_hz: 200.0,
                beat_hz: 10.0,
                amplitude: 0.3,
            },
            -1.0,
            8_000,
        )
        .is_err());
        // Out-of-range amplitude.
        assert!(bake_binaural(200.0, 10.0, 1.5, 0.1, 8_000).is_err());
        // Out-of-range duty.
        assert!(bake_isochronic(200.0, 10.0, 1.5, 0.3, 0.1, 8_000).is_err());
    }
}
