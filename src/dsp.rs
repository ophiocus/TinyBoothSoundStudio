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
use biquad::{Biquad, Coefficients, DirectForm2Transposed, ToHertz, Type, Q_BUTTERWORTH_F32};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A recording-tone preset. Every numeric field is what the Admin window
/// shows and lets the user edit.
/// One band of the parametric EQ block.
///
/// Each `Profile` carries a fixed array of four bands. A band with
/// `kind = Bypass` is a no-op — the audio passes through untouched.
/// `q` is only meaningful for `Peak` and the shelves' transition slope.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EqBand {
    pub kind: EqBandKind,
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl EqBand {
    pub const fn bypass() -> Self {
        Self {
            kind: EqBandKind::Bypass,
            hz: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EqBandKind {
    Bypass,
    Peak,
    LowShelf,
    HighShelf,
}

impl EqBandKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bypass => "Bypass",
            Self::Peak => "Peak",
            Self::LowShelf => "Low shelf",
            Self::HighShelf => "High shelf",
        }
    }
}

fn default_eq_bands() -> [EqBand; 4] {
    [EqBand::bypass(); 4]
}

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

    /// 4-band parametric EQ. Bands with `kind = Bypass` are skipped.
    /// Added in v0.1.6; older profiles default to four bypass bands.
    #[serde(default = "default_eq_bands")]
    pub eq_bands: [EqBand; 4],

    /// Sidechain-compressed band-pass de-esser.
    /// Added in v0.1.6; defaults to disabled on older profiles.
    #[serde(default)]
    pub deess_enabled: bool,
    #[serde(default = "default_deess_hz")]
    pub deess_hz: f32,
    #[serde(default = "default_deess_threshold")]
    pub deess_threshold_db: f32,
    #[serde(default = "default_deess_ratio")]
    pub deess_ratio: f32,

    /// **DC-offset removal**. A 5 Hz Butterworth high-pass that strips the
    /// few-millivolt DC drift AI generators occasionally bake into stems.
    /// Conceptually distinct from `hpf_enabled` (which is a musical rumble
    /// trim at 30–100 Hz); this one is below the audible band and just
    /// reclaims headroom you'd otherwise lose to asymmetric peaks.
    /// Added v0.4.0.
    #[serde(default)]
    pub dc_remove_enabled: bool,

    /// **Nyquist-region cleanup**. A gentle low-pass at the configured
    /// frequency (default 18 kHz) that suppresses the Suno-characteristic
    /// "shimmer" / aliasing artefacts in the top octave. Inaudible to most
    /// listeners on the dry signal but makes the mix sound less "AI".
    /// Added v0.4.0.
    #[serde(default)]
    pub nyquist_clean_enabled: bool,
    #[serde(default = "default_nyquist_clean_hz")]
    pub nyquist_clean_hz: f32,
}

fn default_deess_hz() -> f32 {
    6500.0
}
fn default_deess_threshold() -> f32 {
    -18.0
}
fn default_deess_ratio() -> f32 {
    3.0
}
fn default_nyquist_clean_hz() -> f32 {
    18_000.0
}

/// Fixed cutoff for the DC-remove HPF — below the audible band, no need
/// to expose. Bumping this would eat into the bass; lowering it does
/// nothing useful. 5 Hz is the consensus for "just trim DC drift".
const DC_REMOVE_HZ: f32 = 5.0;

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
            eq_bands: default_eq_bands(),
            deess_enabled: false,
            deess_hz: default_deess_hz(),
            deess_threshold_db: default_deess_threshold(),
            deess_ratio: default_deess_ratio(),
            dc_remove_enabled: false,
            nyquist_clean_enabled: false,
            nyquist_clean_hz: default_nyquist_clean_hz(),
        }
    }
}

/// Built-in presets. The guitar profile is first (default).
pub fn builtin_profiles() -> Vec<Profile> {
    // Helper closure: take an existing profile-builder and stamp the
    // post-Phase-1 fields onto it as defaults so the existing presets
    // are unchanged in behaviour.
    #[allow(clippy::too_many_arguments)]
    fn rec(
        name: &str,
        description: &str,
        input_gain_db: f32,
        hpf_enabled: bool,
        hpf_hz: f32,
        gate_enabled: bool,
        gate_threshold_db: f32,
        gate_attack_ms: f32,
        gate_release_ms: f32,
        compressor_enabled: bool,
        compressor_threshold_db: f32,
        compressor_ratio: f32,
        compressor_attack_ms: f32,
        compressor_release_ms: f32,
        compressor_makeup_db: f32,
    ) -> Profile {
        Profile {
            name: name.into(),
            description: description.into(),
            input_gain_db,
            hpf_enabled,
            hpf_hz,
            gate_enabled,
            gate_threshold_db,
            gate_attack_ms,
            gate_release_ms,
            compressor_enabled,
            compressor_threshold_db,
            compressor_ratio,
            compressor_attack_ms,
            compressor_release_ms,
            compressor_makeup_db,
            eq_bands: default_eq_bands(),
            deess_enabled: false,
            deess_hz: default_deess_hz(),
            deess_threshold_db: default_deess_threshold(),
            deess_ratio: default_deess_ratio(),
            dc_remove_enabled: false,
            nyquist_clean_enabled: false,
            nyquist_clean_hz: default_nyquist_clean_hz(),
        }
    }

    vec![
        rec(
            "Guitar",
            "Acoustic or lightly-overdriven electric into a single mic. \
             Low rumble trim, no gate (keeps decay), light compression to even strums.",
            0.0,
            true,
            60.0,
            false,
            -55.0,
            3.0,
            150.0,
            true,
            -20.0,
            2.5,
            20.0,
            150.0,
            3.0,
        ),
        rec(
            "Vocals",
            "Spoken or sung vocals. Aggressive low cut, gate for breath, \
             moderate compression for intelligibility.",
            0.0,
            true,
            100.0,
            true,
            -42.0,
            3.0,
            80.0,
            true,
            -18.0,
            3.5,
            8.0,
            120.0,
            4.0,
        ),
        rec(
            "Wind / Brass",
            "Sax, flute, trumpet, harmonica. Gentle HPF. No gate (breath IS the sound). \
             Compression only catches peaks — keep dynamics.",
            -3.0,
            true,
            50.0,
            false,
            -60.0,
            5.0,
            100.0,
            true,
            -10.0,
            2.0,
            15.0,
            180.0,
            1.0,
        ),
        rec(
            "Drums / Percussion",
            "Room mic or overhead on drums/hand percussion. HPF off (sub-bass matters). \
             Fast compression tames transients without squashing.",
            -6.0,
            false,
            40.0,
            false,
            -50.0,
            2.0,
            60.0,
            true,
            -8.0,
            4.0,
            3.0,
            80.0,
            2.0,
        ),
        Profile::raw("Raw / Clean"),
        // ── Post-processing preset for Suno-imported stems (TBSS-FR-0001 §5).
        // Consensus-derived defaults — calibrate against real Suno tracks
        // before treating them as gospel.
        Profile {
            name: "Suno-Clean".into(),
            description: "Post-process a Suno export: trim mud, tame shimmer, \
                          add air, gentle glue. Apply per stem in the Mix tab \
                          (Phase 2) or as a recording-tone profile to capture \
                          along."
                .into(),
            input_gain_db: 0.0,
            hpf_enabled: true,
            hpf_hz: 30.0,
            gate_enabled: false,
            gate_threshold_db: -60.0,
            gate_attack_ms: 5.0,
            gate_release_ms: 100.0,
            compressor_enabled: true,
            compressor_threshold_db: -12.0,
            compressor_ratio: 2.0,
            compressor_attack_ms: 30.0,
            compressor_release_ms: 200.0,
            compressor_makeup_db: 1.5,
            eq_bands: [
                EqBand {
                    kind: EqBandKind::Peak,
                    hz: 300.0,
                    gain_db: -3.0,
                    q: 1.0,
                },
                EqBand {
                    kind: EqBandKind::HighShelf,
                    hz: 10_000.0,
                    gain_db: 2.0,
                    q: 0.7,
                },
                EqBand {
                    kind: EqBandKind::Peak,
                    hz: 13_000.0,
                    gain_db: -2.0,
                    q: 2.0,
                },
                EqBand::bypass(),
            ],
            deess_enabled: true,
            deess_hz: 6500.0,
            deess_threshold_db: -18.0,
            deess_ratio: 3.0,
            dc_remove_enabled: false,
            nyquist_clean_enabled: false,
            nyquist_clean_hz: default_nyquist_clean_hz(),
        },
        // ─────────────────────────────────────────────────────────────
        //  Per-role Suno-X library (v0.4.0).
        //  Each preset is auto-seeded onto stems of the matching role at
        //  import time (see `role_to_preset_name`). Tuning goal: better-
        //  than-Suno-Clean defaults for each role's typical artefacts.
        //  All Suno-X presets opt into Nyquist cleanup (the AI-shimmer
        //  suppression at the top octave); DC remove is on for percussive /
        //  low-frequency stems where DC drift wastes most headroom.
        // ─────────────────────────────────────────────────────────────
        suno_preset(
            "Suno-Vocal",
            "Suno lead vocal: HPF for sub-rumble, mud cut, presence + air, \
             de-essing, gentle compression for intelligibility, Nyquist clean.",
            SunoBuild {
                hpf_hz: 90.0,
                eq: [
                    band_peak(250.0, -2.0, 1.0),
                    band_peak(4_000.0, 2.0, 1.2),
                    band_high_shelf(12_000.0, 1.5, 0.7),
                    EqBand::bypass(),
                ],
                deess: Some((7_000.0, -20.0, 4.0)),
                comp: Some((-16.0, 3.0, 8.0, 100.0, 2.0)),
                dc: false,
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-BackingVocal",
            "Suno backing vocal: more HPF (sit behind the lead), tighter \
             compression for consistency, lighter de-essing, Nyquist clean.",
            SunoBuild {
                hpf_hz: 110.0,
                eq: [
                    band_peak(300.0, -1.5, 1.0),
                    band_peak(3_500.0, 1.0, 1.2),
                    band_high_shelf(11_000.0, 1.0, 0.7),
                    EqBand::bypass(),
                ],
                deess: Some((7_000.0, -22.0, 3.0)),
                comp: Some((-14.0, 4.0, 10.0, 120.0, 2.5)),
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-Drums",
            "Suno drum stem: no HPF (kick needs sub), box cut, stick attack \
             lift, transient-friendly compression, DC remove + Nyquist clean.",
            SunoBuild {
                hpf_enabled: false,
                hpf_hz: 40.0,
                eq: [
                    band_peak(250.0, -2.0, 1.2),
                    band_peak(5_000.0, 2.0, 1.5),
                    band_high_shelf(12_000.0, 1.0, 0.7),
                    EqBand::bypass(),
                ],
                comp: Some((-8.0, 4.0, 3.0, 80.0, 1.5)),
                dc: true,
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-Bass",
            "Suno bass stem: HPF at 30 Hz, mud scoop, note-definition lift, \
             slow-attack compression to preserve pluck, DC remove.",
            SunoBuild {
                hpf_hz: 30.0,
                eq: [
                    band_peak(200.0, -2.0, 1.0),
                    band_peak(700.0, 1.0, 1.2),
                    EqBand::bypass(),
                    EqBand::bypass(),
                ],
                comp: Some((-12.0, 3.0, 30.0, 180.0, 1.5)),
                dc: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-ElectricGuitar",
            "Suno electric guitar: HPF, low-mid cut, presence lift, \
             moderate compression, Nyquist clean.",
            SunoBuild {
                hpf_hz: 80.0,
                eq: [
                    band_peak(300.0, -2.0, 1.0),
                    band_peak(3_000.0, 2.0, 1.2),
                    EqBand::bypass(),
                    EqBand::bypass(),
                ],
                comp: Some((-14.0, 2.5, 15.0, 150.0, 1.5)),
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-AcousticGuitar",
            "Suno acoustic: HPF, gentle mud trim, body + air lifts, \
             light compression, Nyquist clean.",
            SunoBuild {
                hpf_hz: 80.0,
                eq: [
                    band_peak(250.0, -1.5, 1.0),
                    band_peak(6_000.0, 1.0, 1.0),
                    band_high_shelf(12_000.0, 1.0, 0.7),
                    EqBand::bypass(),
                ],
                comp: Some((-16.0, 2.0, 20.0, 180.0, 1.0)),
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-Keys",
            "Suno keys / piano: HPF, mud cut, presence lift, gentle glue, \
             Nyquist clean.",
            SunoBuild {
                hpf_hz: 60.0,
                eq: [
                    band_peak(250.0, -1.5, 1.0),
                    band_peak(4_000.0, 1.0, 1.2),
                    EqBand::bypass(),
                    EqBand::bypass(),
                ],
                comp: Some((-12.0, 2.0, 20.0, 200.0, 1.0)),
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-Synth",
            "Suno synth / lead: HPF, mud trim, mid-presence lift, AI-shimmer \
             notch at 14 kHz, harder Nyquist clean at 17 kHz.",
            SunoBuild {
                hpf_hz: 60.0,
                eq: [
                    band_peak(200.0, -1.0, 1.0),
                    band_peak(2_000.0, 1.0, 1.2),
                    band_peak(14_000.0, -2.0, 3.0),
                    EqBand::bypass(),
                ],
                comp: Some((-12.0, 2.5, 15.0, 150.0, 1.5)),
                nyquist: true,
                nyquist_hz: 17_000.0,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-Pads",
            "Suno pads / chords: HPF, cloud cut, gentle high-mid trim, \
             light comp, Nyquist clean at 17 kHz.",
            SunoBuild {
                hpf_hz: 80.0,
                eq: [
                    band_peak(300.0, -2.0, 1.0),
                    band_peak(14_000.0, -1.0, 2.0),
                    EqBand::bypass(),
                    EqBand::bypass(),
                ],
                comp: Some((-16.0, 2.0, 30.0, 250.0, 0.5)),
                nyquist: true,
                nyquist_hz: 17_000.0,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-Percussion",
            "Suno percussion: HPF at 40 Hz, snap lift, air shelf, \
             moderate transient-friendly compression, DC remove + Nyquist clean.",
            SunoBuild {
                hpf_hz: 40.0,
                eq: [
                    band_peak(5_000.0, 2.0, 1.2),
                    band_high_shelf(10_000.0, 1.0, 0.7),
                    EqBand::bypass(),
                    EqBand::bypass(),
                ],
                comp: Some((-10.0, 3.0, 5.0, 100.0, 1.5)),
                dc: true,
                nyquist: true,
                ..Default::default()
            },
        ),
        suno_preset(
            "Suno-FxOther",
            "Suno FX / other: HPF, light glue, Nyquist clean. \
             Conservative — FX stems are intentional, don't over-process.",
            SunoBuild {
                hpf_hz: 60.0,
                comp: Some((-14.0, 2.0, 20.0, 200.0, 0.5)),
                nyquist: true,
                ..Default::default()
            },
        ),
    ]
}

/// Builder bag for the Suno-X presets — keeps each entry in
/// `builtin_profiles` to a struct literal of the few fields that vary.
struct SunoBuild {
    hpf_enabled: bool,
    hpf_hz: f32,
    eq: [EqBand; 4],
    /// `(centre_hz, threshold_db, ratio)` for the de-esser when present.
    deess: Option<(f32, f32, f32)>,
    /// `(threshold_db, ratio, attack_ms, release_ms, makeup_db)` for the comp.
    comp: Option<(f32, f32, f32, f32, f32)>,
    dc: bool,
    nyquist: bool,
    nyquist_hz: f32,
}

impl Default for SunoBuild {
    fn default() -> Self {
        Self {
            hpf_enabled: true,
            hpf_hz: 60.0,
            eq: [EqBand::bypass(); 4],
            deess: None,
            comp: None,
            dc: false,
            nyquist: false,
            nyquist_hz: 18_000.0,
        }
    }
}

fn suno_preset(name: &str, description: &str, b: SunoBuild) -> Profile {
    let (deess_enabled, deess_hz, deess_threshold_db, deess_ratio) = match b.deess {
        Some((hz, th, ratio)) => (true, hz, th, ratio),
        None => (
            false,
            default_deess_hz(),
            default_deess_threshold(),
            default_deess_ratio(),
        ),
    };
    let (
        compressor_enabled,
        compressor_threshold_db,
        compressor_ratio,
        compressor_attack_ms,
        compressor_release_ms,
        compressor_makeup_db,
    ) = match b.comp {
        Some((th, ratio, atk, rel, makeup)) => (true, th, ratio, atk, rel, makeup),
        None => (false, -18.0, 2.0, 10.0, 120.0, 0.0),
    };
    Profile {
        name: name.into(),
        description: description.into(),
        input_gain_db: 0.0,
        hpf_enabled: b.hpf_enabled,
        hpf_hz: b.hpf_hz,
        gate_enabled: false,
        gate_threshold_db: -60.0,
        gate_attack_ms: 5.0,
        gate_release_ms: 100.0,
        compressor_enabled,
        compressor_threshold_db,
        compressor_ratio,
        compressor_attack_ms,
        compressor_release_ms,
        compressor_makeup_db,
        eq_bands: b.eq,
        deess_enabled,
        deess_hz,
        deess_threshold_db,
        deess_ratio,
        dc_remove_enabled: b.dc,
        nyquist_clean_enabled: b.nyquist,
        nyquist_clean_hz: b.nyquist_hz,
    }
}

fn band_peak(hz: f32, gain_db: f32, q: f32) -> EqBand {
    EqBand {
        kind: EqBandKind::Peak,
        hz,
        gain_db,
        q,
    }
}

fn band_high_shelf(hz: f32, gain_db: f32, q: f32) -> EqBand {
    EqBand {
        kind: EqBandKind::HighShelf,
        hz,
        gain_db,
        q,
    }
}

/// Map a [`StemRole`] to the name of the built-in preset that the Suno
/// import flow should auto-seed onto stems of that role. `None` means
/// "leave the track's correction at None — caller will fall back to the
/// project's `default_correction` or the user's manual choice."
///
/// Master and Unknown intentionally return `None` — they're not stems
/// and shouldn't get a Suno-X chain by default.
pub fn role_to_preset_name(role: crate::project::StemRole) -> Option<&'static str> {
    use crate::project::StemRole::*;
    Some(match role {
        Vocals => "Suno-Vocal",
        BackingVocals => "Suno-BackingVocal",
        Drums => "Suno-Drums",
        Bass => "Suno-Bass",
        ElectricGuitar => "Suno-ElectricGuitar",
        AcousticGuitar => "Suno-AcousticGuitar",
        Keys => "Suno-Keys",
        Synth => "Suno-Synth",
        Pads => "Suno-Pads",
        Strings => "Suno-Pads", // pads-like ensemble — pads chain is a sane stand-in
        Brass => "Suno-Synth",  // mid-presence band shape matches sax/horn texture
        Percussion => "Suno-Percussion",
        FxOther => "Suno-FxOther",
        Instrumental => "Suno-Clean", // legacy 2-stem export — generic chain
        Master | Unknown => return None,
    })
}

// ───────────────────── persistence ─────────────────────

pub fn profiles_path() -> Option<PathBuf> {
    crate::config::Config::dir().map(|d| d.join("profiles.json"))
}

/// Load profiles from disk, or seed the built-in set on first run.
///
/// Forward-migration: when the on-disk list is missing any built-in
/// preset by name (e.g. the v0.4.0 Suno-X library landed after the user
/// already had a profiles.json), the missing built-ins are appended in
/// place. The user's custom and edited entries are preserved verbatim;
/// only genuinely-absent built-ins get added. This avoids the v0.3.x
/// problem where new role presets would never show up unless the user
/// hit "Reset to defaults" and lost their tweaks.
pub fn load_or_seed() -> Vec<Profile> {
    let Some(path) = profiles_path() else {
        return builtin_profiles();
    };
    let mut loaded: Vec<Profile> = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if loaded.is_empty() {
        loaded = builtin_profiles();
        let _ = save_profiles(&loaded);
        return loaded;
    }
    let mut added_any = false;
    for builtin in builtin_profiles() {
        if !loaded.iter().any(|p| p.name == builtin.name) {
            loaded.push(builtin);
            added_any = true;
        }
    }
    if added_any {
        let _ = save_profiles(&loaded);
    }
    loaded
}

pub fn save_profiles(profiles: &[Profile]) -> Result<()> {
    let Some(path) = profiles_path() else {
        anyhow::bail!("no config dir")
    };
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let json = serde_json::to_string_pretty(profiles).context("serialising profiles")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ───────────────────── realtime chain ─────────────────────

/// Owned per-stream filter state. Not `Clone` — one chain per recording.
pub struct FilterChain {
    sample_rate: f32,
    profile: Profile,

    /// Sub-audible HPF at 5 Hz that strips DC drift. Chain order: applied
    /// first so it cleans up before any non-linear stage. Allocates only
    /// when `profile.dc_remove_enabled`.
    dc_remove: Option<DirectForm2Transposed<f32>>,
    hpf: Option<DirectForm2Transposed<f32>>,
    eq: [Option<DirectForm2Transposed<f32>>; 4],

    /// Band-pass biquad in front of the de-esser envelope follower.
    deess_bp: Option<DirectForm2Transposed<f32>>,
    deess_env: f32,

    // Envelope follower state (shared between gate + comp via separate instances).
    gate_env: f32,
    gate_gain: f32,

    comp_env: f32,
    comp_gain: f32,

    /// Top-octave LPF applied at the end of the chain (after compressor
    /// makeup) so it also cleans up any harmonics the compressor added.
    /// Allocates only when `profile.nyquist_clean_enabled`.
    nyquist_clean: Option<DirectForm2Transposed<f32>>,
}

impl FilterChain {
    pub fn new(profile: Profile, sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let dc_remove = build_dc_remove(&profile, sr);
        let hpf = build_hpf(&profile, sr);
        let eq = build_eq_bands(&profile.eq_bands, sr);
        let deess_bp = if profile.deess_enabled {
            build_deess_bandpass(profile.deess_hz, sr)
        } else {
            None
        };
        let nyquist_clean = build_nyquist_clean(&profile, sr);
        Self {
            sample_rate: sr,
            profile,
            dc_remove,
            hpf,
            eq,
            deess_bp,
            deess_env: 0.0,
            gate_env: 0.0,
            gate_gain: 1.0,
            comp_env: 0.0,
            comp_gain: 1.0,
            nyquist_clean,
        }
    }

    /// Process one sample. Called once per frame on the audio thread.
    /// Order: input gain → DC-remove → HPF → EQ → de-esser → gate →
    /// compressor → makeup → Nyquist-clean.
    pub fn process(&mut self, x: f32) -> f32 {
        let mut s = x * db_to_lin(self.profile.input_gain_db);

        if let Some(h) = self.dc_remove.as_mut() {
            s = h.run(s);
        }

        if let Some(h) = self.hpf.as_mut() {
            s = h.run(s);
        }

        for slot in self.eq.iter_mut() {
            if let Some(b) = slot.as_mut() {
                s = b.run(s);
            }
        }

        if self.profile.deess_enabled {
            s = self.apply_deess(s);
        }

        if self.profile.gate_enabled {
            s = self.apply_gate(s);
        }

        if self.profile.compressor_enabled {
            s = self.apply_compressor(s);
        }

        if let Some(h) = self.nyquist_clean.as_mut() {
            s = h.run(s);
        }

        s
    }

    fn apply_deess(&mut self, s: f32) -> f32 {
        // Detect on the band-passed signal; attenuate the dry signal when
        // the band envelope exceeds threshold. Single-channel mirror of
        // the stereo path's downward-only sidechain.
        let p = &self.profile;
        // Fixed fast attack / release for sibilance — we want it transparent.
        let attack = time_coef(2.0, self.sample_rate);
        let release = time_coef(40.0, self.sample_rate);
        let band = if let Some(b) = self.deess_bp.as_mut() {
            b.run(s)
        } else {
            s
        };
        let det = band.abs();
        self.deess_env = if det > self.deess_env {
            attack * self.deess_env + (1.0 - attack) * det
        } else {
            release * self.deess_env + (1.0 - release) * det
        };
        let env_db = lin_to_db(self.deess_env.max(1e-9));
        let excess = (env_db - p.deess_threshold_db).max(0.0);
        let reduction_db = excess * (1.0 - 1.0 / p.deess_ratio.max(1.0));
        s * db_to_lin(-reduction_db)
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
        let gain_smooth = if target > self.gate_gain {
            attack
        } else {
            release
        };
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
        let smooth = if target_gain < self.comp_gain {
            attack
        } else {
            release
        };
        self.comp_gain = smooth * self.comp_gain + (1.0 - smooth) * target_gain;
        s * self.comp_gain * db_to_lin(p.compressor_makeup_db)
    }
}

fn time_coef(ms: f32, sample_rate: f32) -> f32 {
    // Classic one-pole coefficient: alpha = exp(-1 / (sr * tau)), tau in seconds.
    let tau = (ms.max(0.1)) / 1000.0;
    (-1.0 / (sample_rate * tau)).exp()
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
fn lin_to_db(lin: f32) -> f32 {
    20.0 * lin.max(1e-9).log10()
}

// ───────────── biquad builders shared by mono + stereo chains ─────────────

fn build_hpf(profile: &Profile, sr: f32) -> Option<DirectForm2Transposed<f32>> {
    if !profile.hpf_enabled {
        return None;
    }
    Coefficients::<f32>::from_params(
        Type::HighPass,
        sr.hz(),
        profile.hpf_hz.max(10.0).hz(),
        Q_BUTTERWORTH_F32,
    )
    .ok()
    .map(DirectForm2Transposed::<f32>::new)
}

fn build_dc_remove(profile: &Profile, sr: f32) -> Option<DirectForm2Transposed<f32>> {
    if !profile.dc_remove_enabled {
        return None;
    }
    Coefficients::<f32>::from_params(
        Type::HighPass,
        sr.hz(),
        DC_REMOVE_HZ.hz(),
        Q_BUTTERWORTH_F32,
    )
    .ok()
    .map(DirectForm2Transposed::<f32>::new)
}

fn build_nyquist_clean(profile: &Profile, sr: f32) -> Option<DirectForm2Transposed<f32>> {
    if !profile.nyquist_clean_enabled {
        return None;
    }
    Coefficients::<f32>::from_params(
        Type::LowPass,
        sr.hz(),
        // Clamp below sr/2 with margin so the biquad is well-conditioned
        // even at 44.1k where 18 kHz sits close to Nyquist (22.05 kHz).
        profile.nyquist_clean_hz.clamp(8_000.0, sr * 0.45).hz(),
        Q_BUTTERWORTH_F32,
    )
    .ok()
    .map(DirectForm2Transposed::<f32>::new)
}

fn build_eq_band(band: &EqBand, sr: f32) -> Option<DirectForm2Transposed<f32>> {
    let kind = match band.kind {
        EqBandKind::Bypass => return None,
        EqBandKind::Peak => Type::PeakingEQ(band.gain_db),
        EqBandKind::LowShelf => Type::LowShelf(band.gain_db),
        EqBandKind::HighShelf => Type::HighShelf(band.gain_db),
    };
    Coefficients::<f32>::from_params(
        kind,
        sr.hz(),
        band.hz.clamp(20.0, sr * 0.45).hz(),
        band.q.max(0.1),
    )
    .ok()
    .map(DirectForm2Transposed::<f32>::new)
}

fn build_eq_bands(bands: &[EqBand; 4], sr: f32) -> [Option<DirectForm2Transposed<f32>>; 4] {
    [
        build_eq_band(&bands[0], sr),
        build_eq_band(&bands[1], sr),
        build_eq_band(&bands[2], sr),
        build_eq_band(&bands[3], sr),
    ]
}

fn build_deess_bandpass(centre_hz: f32, sr: f32) -> Option<DirectForm2Transposed<f32>> {
    Coefficients::<f32>::from_params(
        Type::BandPass,
        sr.hz(),
        centre_hz.clamp(2_000.0, sr * 0.45).hz(),
        2.0_f32, // narrow band — Q≈2 keeps the detector focused on sibilance
    )
    .ok()
    .map(DirectForm2Transposed::<f32>::new)
}

// ───────────────────── stereo chain ─────────────────────

/// Stereo sibling of `FilterChain`.
///
/// Design choices:
///   - HPF runs independently per channel (no cross-channel state).
///   - Gate and compressor run envelope detection on `max(|L|, |R|)`
///     and apply the same gain to both channels. This preserves stereo
///     image — a gate duck or compressor squish never ducks one side
///     while leaving the other open.
///   - Mono `FilterChain` is left untouched; recording into a mono WAV
///     still uses the mono hot path.
pub struct FilterChainStereo {
    sample_rate: f32,
    profile: Profile,

    dc_remove_l: Option<DirectForm2Transposed<f32>>,
    dc_remove_r: Option<DirectForm2Transposed<f32>>,

    hpf_l: Option<DirectForm2Transposed<f32>>,
    hpf_r: Option<DirectForm2Transposed<f32>>,

    eq_l: [Option<DirectForm2Transposed<f32>>; 4],
    eq_r: [Option<DirectForm2Transposed<f32>>; 4],

    /// One band-pass + envelope per channel. The detector runs
    /// independently per side; reduction is applied to the dry signal
    /// of that side only (so a sibilant on L doesn't pull R down).
    deess_bp_l: Option<DirectForm2Transposed<f32>>,
    deess_bp_r: Option<DirectForm2Transposed<f32>>,
    deess_env_l: f32,
    deess_env_r: f32,

    gate_env: f32,
    gate_gain: f32,

    comp_env: f32,
    comp_gain: f32,

    nyquist_clean_l: Option<DirectForm2Transposed<f32>>,
    nyquist_clean_r: Option<DirectForm2Transposed<f32>>,
}

impl FilterChainStereo {
    pub fn new(profile: Profile, sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let dc_remove_l = build_dc_remove(&profile, sr);
        let dc_remove_r = build_dc_remove(&profile, sr);
        let hpf_l = build_hpf(&profile, sr);
        let hpf_r = build_hpf(&profile, sr);
        let eq_l = build_eq_bands(&profile.eq_bands, sr);
        let eq_r = build_eq_bands(&profile.eq_bands, sr);
        let (deess_bp_l, deess_bp_r) = if profile.deess_enabled {
            (
                build_deess_bandpass(profile.deess_hz, sr),
                build_deess_bandpass(profile.deess_hz, sr),
            )
        } else {
            (None, None)
        };
        let nyquist_clean_l = build_nyquist_clean(&profile, sr);
        let nyquist_clean_r = build_nyquist_clean(&profile, sr);
        Self {
            sample_rate: sr,
            profile,
            dc_remove_l,
            dc_remove_r,
            hpf_l,
            hpf_r,
            eq_l,
            eq_r,
            deess_bp_l,
            deess_bp_r,
            deess_env_l: 0.0,
            deess_env_r: 0.0,
            gate_env: 0.0,
            gate_gain: 1.0,
            comp_env: 0.0,
            comp_gain: 1.0,
            nyquist_clean_l,
            nyquist_clean_r,
        }
    }

    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let ig = db_to_lin(self.profile.input_gain_db);
        let mut l = l * ig;
        let mut r = r * ig;

        if let Some(h) = self.dc_remove_l.as_mut() {
            l = h.run(l);
        }
        if let Some(h) = self.dc_remove_r.as_mut() {
            r = h.run(r);
        }

        if let Some(h) = self.hpf_l.as_mut() {
            l = h.run(l);
        }
        if let Some(h) = self.hpf_r.as_mut() {
            r = h.run(r);
        }

        for slot in self.eq_l.iter_mut() {
            if let Some(b) = slot.as_mut() {
                l = b.run(l);
            }
        }
        for slot in self.eq_r.iter_mut() {
            if let Some(b) = slot.as_mut() {
                r = b.run(r);
            }
        }

        if self.profile.deess_enabled {
            let (nl, nr) = self.apply_deess(l, r);
            l = nl;
            r = nr;
        }

        if self.profile.gate_enabled {
            let g = self.gate_gain_update(l, r);
            l *= g;
            r *= g;
        }

        if self.profile.compressor_enabled {
            let g = self.comp_gain_update(l, r);
            let makeup = db_to_lin(self.profile.compressor_makeup_db);
            l *= g * makeup;
            r *= g * makeup;
        }

        if let Some(h) = self.nyquist_clean_l.as_mut() {
            l = h.run(l);
        }
        if let Some(h) = self.nyquist_clean_r.as_mut() {
            r = h.run(r);
        }

        (l, r)
    }

    fn apply_deess(&mut self, l: f32, r: f32) -> (f32, f32) {
        let p = &self.profile;
        let attack = time_coef(2.0, self.sample_rate);
        let release = time_coef(40.0, self.sample_rate);
        let band_l = if let Some(b) = self.deess_bp_l.as_mut() {
            b.run(l)
        } else {
            l
        };
        let band_r = if let Some(b) = self.deess_bp_r.as_mut() {
            b.run(r)
        } else {
            r
        };
        let det_l = band_l.abs();
        let det_r = band_r.abs();
        self.deess_env_l = if det_l > self.deess_env_l {
            attack * self.deess_env_l + (1.0 - attack) * det_l
        } else {
            release * self.deess_env_l + (1.0 - release) * det_l
        };
        self.deess_env_r = if det_r > self.deess_env_r {
            attack * self.deess_env_r + (1.0 - attack) * det_r
        } else {
            release * self.deess_env_r + (1.0 - release) * det_r
        };
        let red_l = ((lin_to_db(self.deess_env_l.max(1e-9)) - p.deess_threshold_db).max(0.0))
            * (1.0 - 1.0 / p.deess_ratio.max(1.0));
        let red_r = ((lin_to_db(self.deess_env_r.max(1e-9)) - p.deess_threshold_db).max(0.0))
            * (1.0 - 1.0 / p.deess_ratio.max(1.0));
        (l * db_to_lin(-red_l), r * db_to_lin(-red_r))
    }

    fn gate_gain_update(&mut self, l: f32, r: f32) -> f32 {
        let p = &self.profile;
        let attack = time_coef(p.gate_attack_ms, self.sample_rate);
        let release = time_coef(p.gate_release_ms, self.sample_rate);
        let det = l.abs().max(r.abs());
        self.gate_env = if det > self.gate_env {
            attack * self.gate_env + (1.0 - attack) * det
        } else {
            release * self.gate_env + (1.0 - release) * det
        };
        let target = if lin_to_db(self.gate_env.max(1e-9)) < p.gate_threshold_db {
            0.0
        } else {
            1.0
        };
        let smooth = if target > self.gate_gain {
            attack
        } else {
            release
        };
        self.gate_gain = smooth * self.gate_gain + (1.0 - smooth) * target;
        self.gate_gain
    }

    fn comp_gain_update(&mut self, l: f32, r: f32) -> f32 {
        let p = &self.profile;
        let attack = time_coef(p.compressor_attack_ms, self.sample_rate);
        let release = time_coef(p.compressor_release_ms, self.sample_rate);
        let det = l.abs().max(r.abs());
        self.comp_env = if det > self.comp_env {
            attack * self.comp_env + (1.0 - attack) * det
        } else {
            release * self.comp_env + (1.0 - release) * det
        };
        let env_db = lin_to_db(self.comp_env.max(1e-9));
        let excess = (env_db - p.compressor_threshold_db).max(0.0);
        let reduction_db = excess * (1.0 - 1.0 / p.compressor_ratio.max(1.0));
        let target = db_to_lin(-reduction_db);
        let smooth = if target < self.comp_gain {
            attack
        } else {
            release
        };
        self.comp_gain = smooth * self.comp_gain + (1.0 - smooth) * target;
        self.comp_gain
    }
}
