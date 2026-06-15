//! Minimal cpal playback session for the Crossfade tab — plays a
//! pre-computed stereo f32 buffer through the default output device
//! until it's exhausted or the session is dropped. No coupling to
//! the Mix-tab player, no project state, no effects chain. See
//! [TBSS-FR-0010].
//!
//! [TBSS-FR-0010]: ../../docs/feature-requests/TBSS-FR-0010-crossfade-tab.md

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Audio buffer + ownership of a cpal output stream. Dropping the
/// session stops the stream. Built per ▶ press; replaced wholesale on
/// the next ▶ (no global state machine).
pub struct CrossfadePreviewSession {
    pub samples: Arc<Vec<f32>>,
    /// Source sample rate — kept for diagnostics / status display.
    #[allow(dead_code)]
    pub sample_rate: u32,
    pub channels: u16,
    pub position: Arc<AtomicU64>,
    /// The `!Send` cpal `Stream` — its Drop stops the device.
    _stream: Stream,
}

impl CrossfadePreviewSession {
    /// Start playback of `samples` (interleaved per `channels`,
    /// nominally stereo) at `start_frame`. Returns the live session;
    /// drop it to stop. Pass `start_frame = 0` to play from the head;
    /// pass a non-zero value to seek (the position atomic is
    /// initialised there, so the cpal callback resumes from that
    /// frame and `is_finished` continues to work).
    pub fn play(
        samples: Vec<f32>,
        sample_rate: u32,
        channels: u16,
        start_frame: u64,
    ) -> Result<Self> {
        let dev = crate::audio::output_device_by_name(None)
            .ok_or_else(|| anyhow!("no audio output device available"))?;

        // Negotiate a config that matches the source rate when the
        // device supports it; fall back to default config otherwise.
        // Same approach as the Mix-tab player.
        let supported = dev
            .supported_output_configs()
            .context("listing output configs")?
            .filter(|c| c.channels() >= channels.max(2))
            .find_map(|c| {
                if c.min_sample_rate().0 <= sample_rate && c.max_sample_rate().0 >= sample_rate {
                    Some(c.with_sample_rate(cpal::SampleRate(sample_rate)))
                } else {
                    None
                }
            });
        let config: StreamConfig = match supported {
            Some(s) => s.into(),
            None => dev
                .default_output_config()
                .context("default output config")?
                .into(),
        };
        let out_channels = config.channels as usize;

        let samples = Arc::new(samples);
        let position = Arc::new(AtomicU64::new(start_frame));
        let samples_cb = samples.clone();
        let position_cb = position.clone();
        let in_channels = channels as usize;

        let err_fn = |e: cpal::StreamError| {
            // Crossfade preview: surface stream errors via stderr —
            // there's no UI error channel here.
            eprintln!("crossfade preview stream error: {e}");
        };

        let stream = dev
            .build_output_stream(
                &config,
                move |out: &mut [f32], _| {
                    // Number of output frames in this buffer.
                    let out_frames = out.len() / out_channels.max(1);
                    let mut pos = position_cb.load(Ordering::Relaxed);
                    let in_frames = (samples_cb.len() / in_channels.max(1)) as u64;
                    for f in 0..out_frames {
                        let (l, r) = if pos < in_frames {
                            // Read interleaved input. For stereo input,
                            // pull two samples; for mono, duplicate.
                            let base = (pos as usize) * in_channels;
                            let l = samples_cb[base];
                            let r = if in_channels >= 2 {
                                samples_cb[base + 1]
                            } else {
                                l
                            };
                            pos += 1;
                            (l, r)
                        } else {
                            (0.0, 0.0)
                        };
                        let dst = f * out_channels;
                        // Write to first two output channels; zero the rest
                        // (e.g. 7.1 device, we only fill L/R).
                        if out_channels >= 1 {
                            out[dst] = l;
                        }
                        if out_channels >= 2 {
                            out[dst + 1] = r;
                        }
                        for ch in 2..out_channels {
                            out[dst + ch] = 0.0;
                        }
                    }
                    position_cb.store(pos, Ordering::Relaxed);
                },
                err_fn,
                None,
            )
            .context("building crossfade preview output stream")?;
        stream.play().context("starting crossfade preview stream")?;

        Ok(Self {
            samples,
            sample_rate,
            channels,
            position,
            _stream: stream,
        })
    }

    /// True when playback has reached the end of the buffer. The UI
    /// polls this each frame to clear its play-state flag.
    pub fn is_finished(&self) -> bool {
        let frames = (self.samples.len() / self.channels.max(1) as usize) as u64;
        self.position.load(Ordering::Relaxed) >= frames
    }
}
