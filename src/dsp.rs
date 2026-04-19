//! Recording-tone presets and the realtime filter chain.
//!
//! A `Profile` is a flat, human-readable bag of numbers (threshold dB,
//! time constants in ms, ratios, etc.) that the user edits in the Admin
//! window. At record-time we freeze the active profile into a
//! `FilterChain` whose state lives on the audio thread and whose `process`
//! runs per sample between "pick channel from interleaved buffer" and
//! "write to WAV".
//!
//! Chain, in order:
//!   1. High-pass (Butterworth biquad)        — removes rumble / DC
//!   2. Noise gate (peak envelope follower)   — mutes silence / breath
//!   3. Compressor (feedforward, peak-follower) — evens dynamics
//!   4. Makeup + input gain                    — trim levels

use anyhow::{Context, Result};
use biquad::{
    Biquad, Coefficients, DirectForm2Transposed, ToHertz, Type, Q_BUTTERWORTH_F32,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A recording-tone preset. Every numeric field is what the Admin window
/// shows and lets the user edit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    pub description: String,

    /// Input trim applied first (dB). Negative attenuates a hot mic.
    pub input_gain_db: f32,

    pub hpf_enabled: bool,
    pub hpf_hz: f32,

    pub gate_enabled: bool,
    pub gate_threshold_db: f32,
    pub gate_attack_ms: f32,
    pub gate_release_ms: f32,

    pub compressor_enabled: bool,
    pub compressor_threshold_db: f32,
    pub compressor_ratio: f32,
    pub compressor_attack_ms: f32,
    pub compressor_release_ms: f32,
    pub compressor_makeup_db: f32,
}

impl Profile {
    pub fn raw(name: &str) -> Self {
        Self {
            name: name.into(),
            description: "No processing — record exactly what the mic hears.".into(),
            input_gain_db: 0.0,
            hpf_enabled: false,
            hpf_hz: 60.0,
            gate_enabled: false,
            gate_threshold_db: -60.0,
            gate_attack_ms: 5.0,
            gate_release_ms: 80.0,
            compressor_enabled: false,
            compressor_threshold_db: -18.0,
            compressor_ratio: 2.0,
            compressor_attack_ms: 10.0,
            compressor_release_ms: 120.0,
            compressor_makeup_db: 0.0,
        }
    }
}

/// Built-in presets. The guitar profile is first (default).
pub fn builtin_profiles() -> Vec<Profile> {
    vec![
        Profile {
            name: "Guitar".into(),
            description: "Acoustic or lightly-overdriven electric into a single mic. \
                          Low rumble trim, no gate (keeps decay), light compression to even strums.".into(),
            input_gain_db: 0.0,
            hpf_enabled: true,
            hpf_hz: 60.0,
            gate_enabled: false,
            gate_threshold_db: -55.0,
            gate_attack_ms: 3.0,
            gate_release_ms: 150.0,
            compressor_enabled: true,
            compressor_threshold_db: -20.0,
            compressor_ratio: 2.5,
            compressor_attack_ms: 20.0,
            compressor_release_ms: 150.0,
            compressor_makeup_db: 3.0,
        },
        Profile {
            name: "Vocals".into(),
            description: "Spoken or sung vocals. Aggressive low cut, gate for breath, \
                          moderate compression for intelligibility.".into(),
            input_gain_db: 0.0,
            hpf_enabled: true,
            hpf_hz: 100.0,
            gate_enabled: true,
            gate_threshold_db: -42.0,
            gate_attack_ms: 3.0,
            gate_release_ms: 80.0,
            compressor_enabled: true,
            compressor_threshold_db: -18.0,
            compressor_ratio: 3.5,
            compressor_attack_ms: 8.0,
            compressor_release_ms: 120.0,
            compressor_makeup_db: 4.0,
        },
        Profile {
            name: "Wind / Brass".into(),
            description: "Sax, flute, trumpet, harmonica. Gentle HPF. No gate (breath IS the sound). \
                          Compression only catches peaks — keep dynamics.".into(),
            input_gain_db: -3.0,
            hpf_enabled: true,
            hpf_hz: 50.0,
            gate_enabled: false,
            gate_threshold_db: -60.0,
            gate_attack_ms: 5.0,
            gate_release_ms: 100.0,
            compressor_enabled: true,
            compressor_threshold_db: -10.0,
            compressor_ratio: 2.0,
            compressor_attack_ms: 15.0,
            compressor_release_ms: 180.0,
            compressor_makeup_db: 1.0,
        },
        Profile {
            name: "Drums / Percussion".into(),
            description: "Room mic or overhead on drums/hand percussion. HPF off (sub-bass matters). \
                          Fast compression tames transients without squashing.".into(),
            input_gain_db: -6.0,
            hpf_enabled: false,
            hpf_hz: 40.0,
            gate_enabled: false,
            gate_threshold_db: -50.0,
            gate_attack_ms: 2.0,
            gate_release_ms: 60.0,
            compressor_enabled: true,
            compressor_threshold_db: -8.0,
            compressor_ratio: 4.0,
            compressor_attack_ms: 3.0,
            compressor_release_ms: 80.0,
            compressor_makeup_db: 2.0,
        },
        Profile::raw("Raw / Clean"),
    ]
}

// ───────────────────── persistence ─────────────────────

pub fn profiles_path() -> Option<PathBuf> {
    crate::config::Config::dir().map(|d| d.join("profiles.json"))
}

/// Load profiles from disk, or seed the built-in set on first run.
pub fn load_or_seed() -> Vec<Profile> {
    let Some(path) = profiles_path() else { return builtin_profiles() };
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Vec<Profile>>(&s) {
            if !v.is_empty() { return v; }
        }
    }
    let defaults = builtin_profiles();
    let _ = save_profiles(&defaults);
    defaults
}

pub fn save_profiles(profiles: &[Profile]) -> Result<()> {
    let Some(path) = profiles_path() else { anyhow::bail!("no config dir") };
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let json = serde_json::to_string_pretty(profiles).context("serialising profiles")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ───────────────────── realtime chain ─────────────────────

/// Owned per-stream filter state. Not `Clone` — one chain per recording.
pub struct FilterChain {
    sample_rate: f32,
    profile: Profile,

    hpf: Option<DirectForm2Transposed<f32>>,

    // Envelope follower state (shared between gate + comp via separate instances).
    gate_env: f32,
    gate_gain: f32,

    comp_env: f32,
    comp_gain: f32,
}

impl FilterChain {
    pub fn new(profile: Profile, sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let hpf = if profile.hpf_enabled {
            Coefficients::<f32>::from_params(
                Type::HighPass,
                sr.hz(),
                profile.hpf_hz.max(10.0).hz(),
                Q_BUTTERWORTH_F32,
            )
            .ok()
            .map(DirectForm2Transposed::<f32>::new)
        } else {
            None
        };
        Self {
            sample_rate: sr,
            profile,
            hpf,
            gate_env: 0.0,
            gate_gain: 1.0,
            comp_env: 0.0,
            comp_gain: 1.0,
        }
    }

    /// Process one sample. Called once per frame on the audio thread.
    pub fn process(&mut self, x: f32) -> f32 {
        let mut s = x * db_to_lin(self.profile.input_gain_db);

        if let Some(h) = self.hpf.as_mut() {
            s = h.run(s);
        }

        if self.profile.gate_enabled {
            s = self.apply_gate(s);
        }

        if self.profile.compressor_enabled {
            s = self.apply_compressor(s);
        }

        s
    }

    fn apply_gate(&mut self, s: f32) -> f32 {
        let p = &self.profile;
        let attack = time_coef(p.gate_attack_ms, self.sample_rate);
        let release = time_coef(p.gate_release_ms, self.sample_rate);
        let abs_s = s.abs();
        // Envelope: fast up, slow down.
        self.gate_env = if abs_s > self.gate_env {
            attack * self.gate_env + (1.0 - attack) * abs_s
        } else {
            release * self.gate_env + (1.0 - release) * abs_s
        };
        let target = if lin_to_db(self.gate_env.max(1e-9)) < p.gate_threshold_db {
            0.0
        } else {
            1.0
        };
        // Smooth gain changes to avoid clicks.
        let gain_smooth = if target > self.gate_gain { attack } else { release };
        self.gate_gain = gain_smooth * self.gate_gain + (1.0 - gain_smooth) * target;
        s * self.gate_gain
    }

    fn apply_compressor(&mut self, s: f32) -> f32 {
        let p = &self.profile;
        let attack = time_coef(p.compressor_attack_ms, self.sample_rate);
        let release = time_coef(p.compressor_release_ms, self.sample_rate);
        let abs_s = s.abs();
        self.comp_env = if abs_s > self.comp_env {
            attack * self.comp_env + (1.0 - attack) * abs_s
        } else {
            release * self.comp_env + (1.0 - release) * abs_s
        };
        let env_db = lin_to_db(self.comp_env.max(1e-9));
        let excess = (env_db - p.compressor_threshold_db).max(0.0);
        let reduction_db = excess * (1.0 - 1.0 / p.compressor_ratio.max(1.0));
        // Target linear gain (before makeup).
        let target_gain = db_to_lin(-reduction_db);
        // Smooth the gain envelope.
        let smooth = if target_gain < self.comp_gain { attack } else { release };
        self.comp_gain = smooth * self.comp_gain + (1.0 - smooth) * target_gain;
        s * self.comp_gain * db_to_lin(p.compressor_makeup_db)
    }
}

fn time_coef(ms: f32, sample_rate: f32) -> f32 {
    // Classic one-pole coefficient: alpha = exp(-1 / (sr * tau)), tau in seconds.
    let tau = (ms.max(0.1)) / 1000.0;
    (-1.0 / (sample_rate * tau)).exp()
}

fn db_to_lin(db: f32) -> f32 { 10f32.powf(db / 20.0) }
fn lin_to_db(lin: f32) -> f32 { 20.0 * lin.max(1e-9).log10() }
