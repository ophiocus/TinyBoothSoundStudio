//! Audio capture and live analysis.
//!
//! The cpal input stream runs on its own thread. Each buffer it delivers is:
//!  - written to a WAV file on disk (if a recording is active), and
//!  - pushed to a bounded ring buffer the UI drains each frame for the
//!    waveform and spectrum displays.
//!
//! Only one `RecordingSession` runs at a time. Dropping it stops the stream
//! and flushes the WAV writer.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use hound::{SampleFormat as WavSf, WavSpec, WavWriter};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Keep ~4 s of mono audio for the live visualizer at 48 kHz.
const VIZ_BUFFER_CAP: usize = 48_000 * 4;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
}

pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    let Ok(devices) = host.input_devices() else { return out };
    for dev in devices {
        let name = match dev.name() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let Ok(cfg) = dev.default_input_config() else { continue };
        out.push(DeviceInfo {
            name,
            channels: cfg.channels(),
            sample_rate: cfg.sample_rate().0,
        });
    }
    // Default device first for convenience.
    out.sort_by(|a, b| (a.name != default_name).cmp(&(b.name != default_name)));
    out
}

/// Shared live-audio state: a mono ring buffer for the visualizer, plus
/// peak-level atomics. Updated by the audio thread, read by the UI thread.
pub struct VizState {
    pub mono: Mutex<VecDeque<f32>>,
    pub sample_rate: AtomicU32,
    peak_times_1000: AtomicU32, // 0..=1000, scaled absolute peak
}

impl VizState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            mono: Mutex::new(VecDeque::with_capacity(VIZ_BUFFER_CAP)),
            sample_rate: AtomicU32::new(48_000),
            peak_times_1000: AtomicU32::new(0),
        })
    }

    pub fn peak(&self) -> f32 {
        self.peak_times_1000.load(Ordering::Relaxed) as f32 / 1000.0
    }

    fn push_and_peak(&self, frame: f32) {
        let mut q = self.mono.lock();
        if q.len() == VIZ_BUFFER_CAP {
            q.pop_front();
        }
        q.push_back(frame);
        let absf = frame.abs().min(1.0);
        let curr = self.peak_times_1000.load(Ordering::Relaxed);
        let new = (absf * 1000.0) as u32;
        // Fast attack, slow release.
        let next = if new > curr { new } else { curr.saturating_sub(4) };
        self.peak_times_1000.store(next, Ordering::Relaxed);
    }

    pub fn snapshot(&self, n: usize) -> Vec<f32> {
        let q = self.mono.lock();
        let len = q.len();
        let start = len.saturating_sub(n);
        q.iter().skip(start).copied().collect()
    }
}

/// Active recording — writes WAV on the audio thread, drops to stop.
pub struct RecordingSession {
    pub wav_path: PathBuf,
    pub sample_rate: u32,
    writer: Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>,
    frames_written: Arc<std::sync::atomic::AtomicU64>,
    _stream: Stream,
}

impl RecordingSession {
    pub fn frames(&self) -> u64 {
        self.frames_written.load(Ordering::Relaxed)
    }

    pub fn duration_secs(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.max(1) as f32
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        if let Some(w) = self.writer.lock().take() {
            let _ = w.finalize();
        }
    }
}

/// Start recording from the named input device. Channel selection is a
/// 0-based index, or `None` to mixdown all channels to mono. The `profile`
/// is frozen into a realtime filter chain that runs on the audio thread.
pub fn start_recording(
    device_name: &str,
    channel: Option<u16>,
    wav_path: &Path,
    viz: Arc<VizState>,
    profile: crate::dsp::Profile,
) -> Result<RecordingSession> {
    let host = cpal::default_host();
    let dev = host
        .input_devices()?
        .find(|d| d.name().map(|n| n == device_name).unwrap_or(false))
        .ok_or_else(|| anyhow!("input device '{device_name}' not found"))?;

    let supported = dev
        .default_input_config()
        .context("reading default input config")?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.clone().into();
    let channels_in = config.channels;
    let sample_rate = config.sample_rate.0;

    if let Some(c) = channel {
        if c >= channels_in {
            return Err(anyhow!(
                "device only has {channels_in} channel(s); index {c} out of range"
            ));
        }
    }

    if let Some(parent) = wav_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = WavSpec {
        channels: 1, // always write mono — selected channel or downmix
        sample_rate,
        bits_per_sample: 16,
        sample_format: WavSf::Int,
    };
    let writer = Arc::new(Mutex::new(Some(
        WavWriter::create(wav_path, spec).context("creating WAV writer")?,
    )));
    let frames = Arc::new(std::sync::atomic::AtomicU64::new(0));

    viz.sample_rate.store(sample_rate, Ordering::Relaxed);
    viz.mono.lock().clear();

    let err_fn = |e| eprintln!("cpal stream error: {e}");
    let chain = crate::dsp::FilterChain::new(profile, sample_rate);

    // Build the right input callback for the hardware's sample format.
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&dev, &config, channels_in, channel, writer.clone(), frames.clone(), viz.clone(), chain, err_fn)?,
        SampleFormat::I16 => build_stream::<i16>(&dev, &config, channels_in, channel, writer.clone(), frames.clone(), viz.clone(), chain, err_fn)?,
        SampleFormat::U16 => build_stream::<u16>(&dev, &config, channels_in, channel, writer.clone(), frames.clone(), viz.clone(), chain, err_fn)?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    stream.play()?;

    Ok(RecordingSession {
        wav_path: wav_path.to_path_buf(),
        sample_rate,
        writer,
        frames_written: frames,
        _stream: stream,
    })
}

fn build_stream<T>(
    dev: &cpal::Device,
    config: &StreamConfig,
    channels_in: u16,
    channel: Option<u16>,
    writer: Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>,
    frames: Arc<std::sync::atomic::AtomicU64>,
    viz: Arc<VizState>,
    mut chain: crate::dsp::FilterChain,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<Stream>
where
    T: cpal::Sample + cpal::SizedSample + ToF32 + Send + 'static,
{
    let ch = channels_in as usize;
    let stream = dev.build_input_stream(
        config,
        move |data: &[T], _| {
            let frame_count = data.len() / ch.max(1);
            let mut writer_lock = writer.lock();
            let Some(w) = writer_lock.as_mut() else { return };
            for i in 0..frame_count {
                let frame_start = i * ch;
                let mono_in = match channel {
                    Some(sel) => data[frame_start + sel as usize].to_f32(),
                    None => {
                        let mut acc = 0.0f32;
                        for c in 0..ch {
                            acc += data[frame_start + c].to_f32();
                        }
                        acc / ch.max(1) as f32
                    }
                };
                // Active recording tone is applied *before* the WAV writer
                // sees anything — what you record is what you hear back.
                let processed = chain.process(mono_in);
                let clamped = processed.clamp(-1.0, 1.0);
                let sample_i16 = (clamped * i16::MAX as f32) as i16;
                let _ = w.write_sample(sample_i16);
                viz.push_and_peak(clamped);
            }
            frames.fetch_add(frame_count as u64, Ordering::Relaxed);
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

/// Normalise every hardware sample type into an f32 in [-1, 1].
pub trait ToF32 {
    fn to_f32(self) -> f32;
}
impl ToF32 for f32 {
    fn to_f32(self) -> f32 { self }
}
impl ToF32 for i16 {
    fn to_f32(self) -> f32 { self as f32 / i16::MAX as f32 }
}
impl ToF32 for u16 {
    fn to_f32(self) -> f32 { (self as f32 - i16::MAX as f32) / i16::MAX as f32 }
}
