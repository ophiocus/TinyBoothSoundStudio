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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    let Ok(devices) = host.input_devices() else {
        return out;
    };
    for dev in devices {
        let name = match dev.name() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let Ok(cfg) = dev.default_input_config() else {
            continue;
        };
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

/// Shared live-audio state for the visualiser.
///
/// Carries two ring buffers (`left` always populated; `right` only in stereo
/// mode) plus per-side peak atomics and a `stereo` flag the UI reads to pick
/// its layout. Updated on the audio thread, read on the UI thread.
pub struct VizState {
    pub left: Mutex<VecDeque<f32>>,
    pub right: Mutex<VecDeque<f32>>,
    pub sample_rate: AtomicU32,
    pub stereo: AtomicBool,
    peak_l_x1000: AtomicU32,
    peak_r_x1000: AtomicU32,
}

impl VizState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            left: Mutex::new(VecDeque::with_capacity(VIZ_BUFFER_CAP)),
            right: Mutex::new(VecDeque::with_capacity(VIZ_BUFFER_CAP)),
            sample_rate: AtomicU32::new(48_000),
            stereo: AtomicBool::new(false),
            peak_l_x1000: AtomicU32::new(0),
            peak_r_x1000: AtomicU32::new(0),
        })
    }

    pub fn is_stereo(&self) -> bool {
        self.stereo.load(Ordering::Relaxed)
    }

    pub fn peak_left(&self) -> f32 {
        self.peak_l_x1000.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn peak_right(&self) -> f32 {
        self.peak_r_x1000.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Reset state for a new recording session.
    pub fn reset(&self, stereo: bool, sample_rate: u32) {
        self.stereo.store(stereo, Ordering::Relaxed);
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        self.left.lock().clear();
        self.right.lock().clear();
        self.peak_l_x1000.store(0, Ordering::Relaxed);
        self.peak_r_x1000.store(0, Ordering::Relaxed);
    }

    /// Push a mono frame — only the left buffer + left peak update.
    fn push_mono(&self, s: f32) {
        push_into(&self.left, s);
        update_peak(&self.peak_l_x1000, s);
    }

    /// Push a stereo frame — both buffers + both peaks update independently.
    fn push_stereo(&self, l: f32, r: f32) {
        push_into(&self.left, l);
        push_into(&self.right, r);
        update_peak(&self.peak_l_x1000, l);
        update_peak(&self.peak_r_x1000, r);
    }

    /// Snapshot the last `n` samples of the left (or only) channel.
    pub fn snapshot_left(&self, n: usize) -> Vec<f32> {
        snapshot_q(&self.left, n)
    }
    /// Snapshot the last `n` samples of the right channel (empty if mono).
    pub fn snapshot_right(&self, n: usize) -> Vec<f32> {
        snapshot_q(&self.right, n)
    }
}

fn push_into(q: &Mutex<VecDeque<f32>>, s: f32) {
    let mut q = q.lock();
    if q.len() == VIZ_BUFFER_CAP {
        q.pop_front();
    }
    q.push_back(s);
}

fn snapshot_q(q: &Mutex<VecDeque<f32>>, n: usize) -> Vec<f32> {
    let q = q.lock();
    let len = q.len();
    let start = len.saturating_sub(n);
    q.iter().skip(start).copied().collect()
}

fn update_peak(atomic: &AtomicU32, frame: f32) {
    let absf = frame.abs().min(1.0);
    let curr = atomic.load(Ordering::Relaxed);
    let new = (absf * 1000.0) as u32;
    // Fast attack, slow release.
    let next = if new > curr {
        new
    } else {
        curr.saturating_sub(4)
    };
    atomic.store(next, Ordering::Relaxed);
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

/// How the cpal interleaved input buffer should be collapsed into the
/// track(s) we actually write to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceMode {
    /// Sum all hardware channels to a single mono track.
    Mixdown,
    /// Pick one hardware channel (0-based) as a mono track.
    Channel(u16),
    /// Capture channels 0 and 1 as an L/R stereo track. Requires a device
    /// with at least 2 channels.
    Stereo,
}

impl SourceMode {
    pub fn is_stereo(self) -> bool {
        matches!(self, Self::Stereo)
    }
}

/// Start recording from the named input device with the chosen `SourceMode`.
/// The `profile` is frozen into a realtime filter chain that runs on the
/// audio thread — `FilterChain` for mono modes, `FilterChainStereo` for
/// stereo (with envelope-linked gate and compressor).
///
/// `error_tx` is a clone of the app's audio-error channel. cpal's err_fn
/// closure pushes any stream-level error through it; the UI thread drains
/// and surfaces the message in the status bar (no `eprintln!` from a
/// real-time-ish thread — survival-guide §3.3).
pub fn start_recording(
    device_name: &str,
    mode: SourceMode,
    wav_path: &Path,
    viz: Arc<VizState>,
    profile: crate::dsp::Profile,
    error_tx: std::sync::mpsc::Sender<String>,
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

    match mode {
        SourceMode::Channel(c) if c >= channels_in => {
            return Err(anyhow!(
                "device only has {channels_in} channel(s); index {c} out of range"
            ));
        }
        SourceMode::Stereo if channels_in < 2 => {
            return Err(anyhow!(
                "stereo mode needs at least 2 input channels; this device reports {channels_in}"
            ));
        }
        _ => {}
    }

    if let Some(parent) = wav_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let wav_channels: u16 = if mode.is_stereo() { 2 } else { 1 };
    let spec = WavSpec {
        channels: wav_channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: WavSf::Int,
    };
    let writer = Arc::new(Mutex::new(Some(
        WavWriter::create(wav_path, spec).context("creating WAV writer")?,
    )));
    let frames = Arc::new(std::sync::atomic::AtomicU64::new(0));

    viz.reset(mode.is_stereo(), sample_rate);

    // err_fn closes over a Sender so the UI thread surfaces the error
    // in the status bar instead of locking stderr from the audio thread.
    let err_fn = move |e: cpal::StreamError| {
        let _ = error_tx.send(format!("input stream error: {e}"));
    };

    let stream = if mode.is_stereo() {
        let chain = crate::dsp::FilterChainStereo::new(profile, sample_rate);
        match sample_format {
            SampleFormat::F32 => build_stream_stereo::<f32>(
                &dev,
                &config,
                channels_in,
                writer.clone(),
                frames.clone(),
                viz.clone(),
                chain,
                err_fn,
            )?,
            SampleFormat::I16 => build_stream_stereo::<i16>(
                &dev,
                &config,
                channels_in,
                writer.clone(),
                frames.clone(),
                viz.clone(),
                chain,
                err_fn,
            )?,
            SampleFormat::U16 => build_stream_stereo::<u16>(
                &dev,
                &config,
                channels_in,
                writer.clone(),
                frames.clone(),
                viz.clone(),
                chain,
                err_fn,
            )?,
            other => return Err(anyhow!("unsupported sample format {other:?}")),
        }
    } else {
        let channel = match mode {
            SourceMode::Channel(c) => Some(c),
            SourceMode::Mixdown => None,
            SourceMode::Stereo => unreachable!(),
        };
        let chain = crate::dsp::FilterChain::new(profile, sample_rate);
        match sample_format {
            SampleFormat::F32 => build_stream_mono::<f32>(
                &dev,
                &config,
                channels_in,
                channel,
                writer.clone(),
                frames.clone(),
                viz.clone(),
                chain,
                err_fn,
            )?,
            SampleFormat::I16 => build_stream_mono::<i16>(
                &dev,
                &config,
                channels_in,
                channel,
                writer.clone(),
                frames.clone(),
                viz.clone(),
                chain,
                err_fn,
            )?,
            SampleFormat::U16 => build_stream_mono::<u16>(
                &dev,
                &config,
                channels_in,
                channel,
                writer.clone(),
                frames.clone(),
                viz.clone(),
                chain,
                err_fn,
            )?,
            other => return Err(anyhow!("unsupported sample format {other:?}")),
        }
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

/// Mono hot path — unchanged from the pre-stereo implementation.
#[allow(clippy::too_many_arguments)]
fn build_stream_mono<T>(
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
            let Some(w) = writer_lock.as_mut() else {
                return;
            };
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
                viz.push_mono(clamped);
            }
            frames.fetch_add(frame_count as u64, Ordering::Relaxed);
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

/// Stereo hot path. Reads channels 0 and 1 from the interleaved buffer,
/// runs the stereo filter chain, writes interleaved L R L R to the WAV,
/// and feeds the visualiser a mono (L+R)/2 mix.
#[allow(clippy::too_many_arguments)]
fn build_stream_stereo<T>(
    dev: &cpal::Device,
    config: &StreamConfig,
    channels_in: u16,
    writer: Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>,
    frames: Arc<std::sync::atomic::AtomicU64>,
    viz: Arc<VizState>,
    mut chain: crate::dsp::FilterChainStereo,
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
            let Some(w) = writer_lock.as_mut() else {
                return;
            };
            for i in 0..frame_count {
                let frame_start = i * ch;
                let l_in = data[frame_start].to_f32();
                let r_in = data[frame_start + 1].to_f32();
                let (l, r) = chain.process(l_in, r_in);
                let l_c = l.clamp(-1.0, 1.0);
                let r_c = r.clamp(-1.0, 1.0);
                let _ = w.write_sample((l_c * i16::MAX as f32) as i16);
                let _ = w.write_sample((r_c * i16::MAX as f32) as i16);
                viz.push_stereo(l_c, r_c);
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
    fn to_f32(self) -> f32 {
        self
    }
}
impl ToF32 for i16 {
    fn to_f32(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
}
impl ToF32 for u16 {
    fn to_f32(self) -> f32 {
        (self as f32 - i16::MAX as f32) / i16::MAX as f32
    }
}
