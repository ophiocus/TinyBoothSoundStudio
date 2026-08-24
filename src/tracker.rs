//! TBSS-FR-0014 · E1+E2 — MadTracker-lineage tracker: data model and the
//! tick-based render engine.
//!
//! Pure code: patterns/instruments/song in (serde), interleaved stereo
//! f32 out. No I/O, no cpal, no egui — the UI epics (E3+) sit on top,
//! and every timing/effect behavior here is pinned by frame-exact tests.
//!
//! Conventions (per the FR's MadTracker 2 reference, consciously shrunk):
//! * Cell = note · instrument · volume · panning · effect (cmd + 4-digit
//!   hex param preserved even when unimplemented).
//! * Tempo = classic `speed` (ticks per row) + BPM; one tick =
//!   `2.5 / BPM` seconds (the Amiga-lineage constant all trackers use).
//! * Pitch = variable-rate sample playback, FT2 *linear-frequency* style:
//!   a semitone is a rate factor of 2^(1/12); slides move x/16 semitone
//!   per tick. Aliasing is authentic and intentional.
//! * NNA: Cut (default) or Continue (old voice keeps ringing in a second
//!   per-track slot).
//! * Instruments carry a resonant low-pass (MT2's signature) — applied
//!   per voice via the `biquad` crate the DSP chain already depends on.

#![allow(dead_code)] // consumed by the E3+ UI epics; engine fully test-covered

use serde::{Deserialize, Serialize};

// ───────────────────────── model (E1) ─────────────────────────

/// Semitone index, C-0 = 0 … B-9 = 119. Middle C (C-4) = 48.
pub type Note = u8;

pub const NOTE_C4: Note = 48;

/// Display form, tracker style: "C-4", "A#3".
pub fn note_name(n: Note) -> String {
    const NAMES: [&str; 12] = [
        "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
    ];
    format!("{}{}", NAMES[(n % 12) as usize], n / 12)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerCell {
    pub note: Option<Note>,
    /// Instrument index into `TrackerSong::instruments`.
    pub instr: Option<u8>,
    /// Volume column, 0..=64 (tracker convention).
    pub vol: Option<u8>,
    /// Panning column, 0 = hard left … 255 = hard right (128 centre).
    pub pan: Option<u8>,
    /// Effect command (letter) + 4-digit hex parameter, MT2-style.
    /// Unknown commands round-trip untouched and no-op in playback.
    pub fx: Option<(u8, u16)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerPattern {
    pub rows: u16,
    /// `tracks[t][r]` — column-major like the editor renders.
    pub tracks: Vec<Vec<TrackerCell>>,
}

impl TrackerPattern {
    pub fn empty(tracks: usize, rows: u16) -> Self {
        Self {
            rows,
            tracks: vec![vec![TrackerCell::default(); rows as usize]; tracks],
        }
    }
    pub fn cell(&self, track: usize, row: u16) -> TrackerCell {
        self.tracks
            .get(track)
            .and_then(|t| t.get(row as usize))
            .copied()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopMode {
    Off,
    Forward,
    PingPong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nna {
    Cut,
    Continue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCfg {
    pub cutoff_hz: f32,
    pub q: f32,
}

/// One multisample zone (TBSS-FR-0018): a sample assigned to a root
/// note. The engine picks the zone whose root is NEAREST to the played
/// note and pitches relative to it — so a C-4 recording never has to
/// stretch five octaves when a C-6 recording exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleZone {
    pub root: Note,
    /// Index into the flat decoded-sample pool the engine receives.
    pub sample: usize,
    /// Trim window in source frames (`end == 0` = to the end).
    #[serde(default)]
    pub start: u64,
    #[serde(default)]
    pub end: u64,
    /// Loop window (used when the instrument's loop_mode != Off;
    /// `loop_end == 0` = trim end).
    #[serde(default)]
    pub loop_start: u64,
    #[serde(default)]
    pub loop_end: u64,
}

/// An instrument: playback configuration over one or more sample zones.
/// The sample *data* is not stored here (sources decode through
/// `audiodecode` at load time); the engine receives decoded samples
/// side-by-side in a flat pool that `SampleZone::sample` indexes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerInstrument {
    pub name: String,
    /// Which note plays the sample at its native rate.
    pub base_note: Note,
    /// Multisample zones (FR-0018). EMPTY = legacy single-sample mode:
    /// one implicit zone at `base_note` whose sample index equals the
    /// instrument's own index — exactly the pre-zone behavior, so every
    /// existing song deserializes and renders unchanged.
    #[serde(default)]
    pub zones: Vec<SampleZone>,
    pub gain_db: f32,
    pub loop_mode: LoopMode,
    /// Loop window in sample frames (used when `loop_mode != Off`).
    pub loop_start: u64,
    pub loop_end: u64,
    /// MT2-signature resonant low-pass, optional.
    pub filter: Option<FilterCfg>,
    pub nna: Nna,
}

impl TrackerInstrument {
    pub fn simple(name: &str) -> Self {
        Self {
            name: name.into(),
            base_note: NOTE_C4,
            zones: Vec::new(),
            gain_db: 0.0,
            loop_mode: LoopMode::Off,
            loop_start: 0,
            loop_end: 0,
            filter: None,
            nna: Nna::Cut,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerSong {
    pub bpm: f32,
    /// Ticks per row.
    pub speed: u8,
    pub instruments: Vec<TrackerInstrument>,
    pub patterns: Vec<TrackerPattern>,
    /// Pattern order; empty = play pattern 0.
    pub order: Vec<u8>,
    /// Tracks whose lane renders as a drum step-grid in the editor.
    pub drum_view_tracks: Vec<bool>,
}

impl TrackerSong {
    pub fn new(tracks: usize, rows: u16) -> Self {
        Self {
            bpm: 125.0,
            speed: 6,
            instruments: Vec::new(),
            patterns: vec![TrackerPattern::empty(tracks, rows)],
            order: vec![0],
            drum_view_tracks: vec![false; tracks],
        }
    }
    pub fn n_tracks(&self) -> usize {
        self.patterns.first().map(|p| p.tracks.len()).unwrap_or(0)
    }
}

/// Decoded audio for one instrument: mono f32 at a known rate. Parallel
/// to `TrackerSong::instruments`.
#[derive(Debug, Clone, Default)]
pub struct DecodedSample {
    pub data: Vec<f32>,
    pub sample_rate: u32,
}

// ───────────────────────── engine (E2) ─────────────────────────

/// One tick's length in output frames. The 2.5/BPM constant is the
/// classic tracker tick (125 BPM · speed 6 = 50 ticks/s = one PAL frame).
fn tick_frames(out_rate: u32, bpm: f32) -> usize {
    ((out_rate as f64) * 2.5 / (bpm.max(1.0) as f64)).round() as usize
}

const MAX_VOL: f32 = 64.0;

#[derive(Clone)]
struct Voice {
    instr: usize,
    /// Which entry of the decoded-sample pool this voice reads.
    sample_idx: usize,
    /// Zone bounds in source frames (end = exclusive; loop in frames).
    zone_start: f64,
    zone_end: f64,
    zloop_start: f64,
    zloop_end: f64,
    /// Position in source frames (fractional — variable-rate).
    pos: f64,
    /// Frames advanced per output frame (rate ratio × note factor).
    step: f64,
    /// Base step for the triggered note (slides/vibrato modulate around it).
    base_step: f64,
    /// Semitone offset accumulated by slides (1xx/2xx).
    slide_semis: f64,
    vol: f32, // 0..=64
    pan: f32, // 0 = L, 1 = R
    /// Ping-pong direction.
    reverse: bool,
    active: bool,
    /// Per-voice filter state (biquad DF2T), rebuilt on trigger.
    filter: Option<biquad::DirectForm2Transposed<f32>>,
    /// Vibrato LFO phase (radians), advanced per tick.
    vib_phase: f64,
}

impl Voice {
    fn silent() -> Self {
        Self {
            instr: 0,
            sample_idx: 0,
            zone_start: 0.0,
            zone_end: 0.0,
            zloop_start: 0.0,
            zloop_end: 0.0,
            pos: 0.0,
            step: 0.0,
            base_step: 0.0,
            slide_semis: 0.0,
            vol: 0.0,
            pan: 0.5,
            reverse: false,
            active: false,
            filter: None,
            vib_phase: 0.0,
        }
    }
}

fn semis_to_factor(semis: f64) -> f64 {
    (semis / 12.0).exp2()
}

fn make_filter(cfg: &FilterCfg, out_rate: u32) -> Option<biquad::DirectForm2Transposed<f32>> {
    use biquad::{Biquad as _, Coefficients, ToHertz, Type};
    let f0 = cfg.cutoff_hz.clamp(20.0, out_rate as f32 * 0.45);
    let coeffs = Coefficients::<f32>::from_params(
        Type::LowPass,
        (out_rate as f32).hz(),
        f0.hz(),
        cfg.q.max(0.05),
    )
    .ok()?;
    let mut f = biquad::DirectForm2Transposed::<f32>::new(coeffs);
    let _ = f.run(0.0);
    Some(f)
}

/// Render the whole song (its order chain) to interleaved stereo f32 at
/// `out_rate`. Deterministic; pure.
pub fn render_song(song: &TrackerSong, samples: &[DecodedSample], out_rate: u32) -> Vec<f32> {
    let order: Vec<u8> = if song.order.is_empty() {
        vec![0]
    } else {
        song.order.clone()
    };
    let n_tracks = song.n_tracks();
    // Two voice slots per track: current + NNA-continue.
    let mut voices: Vec<[Voice; 2]> = vec![[Voice::silent(), Voice::silent()]; n_tracks];
    let mut out: Vec<f32> = Vec::new();
    let mut bpm = song.bpm;
    let mut speed = song.speed.max(1);

    let mut order_pos = 0usize;
    while order_pos < order.len() {
        let Some(pattern) = song.patterns.get(order[order_pos] as usize) else {
            order_pos += 1;
            continue;
        };
        let mut row: u16 = 0;
        let mut break_to_next = false;
        while row < pattern.rows && !break_to_next {
            // Row-scope effect state, latched from the cells.
            struct RowFx {
                delay_ticks: u8,
                cut_tick: Option<u8>,
                arp: Option<(u8, u8)>,
                slide_per_tick: f64, // semitones (+up / −down)
                vib: Option<(u8, u8)>,
                vol_slide: f32, // per tick, ±(x or −y)
                pending: Option<TrackerCell>,
            }
            let mut row_fx: Vec<RowFx> = Vec::with_capacity(n_tracks);
            for t in 0..n_tracks {
                let cell = pattern.cell(t, row);
                let mut fx = RowFx {
                    delay_ticks: 0,
                    cut_tick: None,
                    arp: None,
                    slide_per_tick: 0.0,
                    vib: None,
                    vol_slide: 0.0,
                    pending: Some(cell),
                };
                if let Some((cmd, param)) = cell.fx {
                    let byte = (param & 0xFF) as u8;
                    let (x, y) = ((byte >> 4) & 0xF, byte & 0xF);
                    match cmd {
                        b'0' if byte != 0 => fx.arp = Some((x, y)),
                        b'1' => fx.slide_per_tick = byte as f64 / 16.0,
                        b'2' => fx.slide_per_tick = -(byte as f64) / 16.0,
                        b'4' => fx.vib = Some((x, y)),
                        b'A' => {
                            fx.vol_slide = if x > 0 { x as f32 } else { -(y as f32) };
                        }
                        b'D' => break_to_next = true,
                        b'F' => {
                            if byte >= 0x20 {
                                bpm = byte as f32;
                            } else if byte > 0 {
                                speed = byte;
                            }
                        }
                        b'E' => match x {
                            0xC => fx.cut_tick = Some(y),
                            0xD => fx.delay_ticks = y,
                            _ => {}
                        },
                        _ => {} // unknown: preserved, ignored
                    }
                }
                row_fx.push(fx);
            }

            let tf = tick_frames(out_rate, bpm);
            for tick in 0..speed {
                // Tick-start bookkeeping per track.
                for (t, fx) in row_fx.iter_mut().enumerate() {
                    // Delayed / immediate note trigger.
                    if tick == fx.delay_ticks {
                        if let Some(cell) = fx.pending.take() {
                            trigger_cell(song, samples, out_rate, &mut voices[t], &cell);
                        }
                    }
                    let v = &mut voices[t][0];
                    if !v.active {
                        continue;
                    }
                    // ECx note cut.
                    if fx.cut_tick == Some(tick) {
                        v.vol = 0.0;
                    }
                    // Axy volume slide (not on tick 0, classic).
                    if tick > 0 && fx.vol_slide != 0.0 {
                        v.vol = (v.vol + fx.vol_slide).clamp(0.0, MAX_VOL);
                    }
                    // 1xx/2xx pitch slide (not on tick 0).
                    if tick > 0 && fx.slide_per_tick != 0.0 {
                        v.slide_semis += fx.slide_per_tick;
                    }
                    // Per-tick pitch: base × slides × arp × vibrato.
                    let mut semis = v.slide_semis;
                    if let Some((ax, ay)) = fx.arp {
                        semis += match tick % 3 {
                            1 => ax as f64,
                            2 => ay as f64,
                            _ => 0.0,
                        };
                    }
                    if let Some((rate, depth)) = fx.vib {
                        v.vib_phase += (rate as f64) * std::f64::consts::TAU / 64.0;
                        semis += v.vib_phase.sin() * (depth as f64) / 16.0;
                    }
                    v.step = v.base_step * semis_to_factor(semis);
                }

                // Mix `tf` frames.
                let start = out.len();
                out.resize(start + tf * 2, 0.0);
                for slots in voices.iter_mut() {
                    for v in slots.iter_mut() {
                        if !v.active {
                            continue;
                        }
                        let (instr, sample) =
                            match (song.instruments.get(v.instr), samples.get(v.sample_idx)) {
                                (Some(i), Some(s)) if !s.data.is_empty() => (i, s),
                                _ => {
                                    v.active = false;
                                    continue;
                                }
                            };
                        let gain = (10.0f32).powf(instr.gain_db / 20.0) * (v.vol / MAX_VOL);
                        // Zone bounds resolved at trigger (FR-0018).
                        let n = if v.zone_end > 0.0 {
                            v.zone_end
                        } else {
                            sample.data.len() as f64
                        };
                        let (l_gain, r_gain) = ((1.0 - v.pan) * gain, v.pan * gain);
                        for f in 0..tf {
                            if !v.active {
                                break;
                            }
                            let idx = v.pos.floor() as usize;
                            let frac = (v.pos - v.pos.floor()) as f32;
                            let s0 = sample.data.get(idx).copied().unwrap_or(0.0);
                            let s1 = sample.data.get(idx + 1).copied().unwrap_or(s0);
                            let mut s = s0 + (s1 - s0) * frac;
                            if let Some(filt) = v.filter.as_mut() {
                                use biquad::Biquad as _;
                                s = filt.run(s);
                            }
                            let o = (start + f * 2).min(out.len().saturating_sub(2));
                            out[o] += s * l_gain;
                            out[o + 1] += s * r_gain;

                            // Advance with loop handling (zone-scoped).
                            let (ls, le) = (
                                v.zloop_start.min(n),
                                if v.zloop_end == 0.0 {
                                    n
                                } else {
                                    v.zloop_end.min(n)
                                },
                            );
                            match instr.loop_mode {
                                LoopMode::Off => {
                                    v.pos += v.step;
                                    if v.pos >= n {
                                        v.active = false;
                                    }
                                }
                                LoopMode::Forward => {
                                    v.pos += v.step;
                                    if v.pos >= le && le > ls {
                                        v.pos = ls + (v.pos - le) % (le - ls);
                                    } else if v.pos >= n {
                                        v.active = false;
                                    }
                                }
                                LoopMode::PingPong => {
                                    if v.reverse {
                                        v.pos -= v.step;
                                        if v.pos <= ls {
                                            v.pos = ls;
                                            v.reverse = false;
                                        }
                                    } else {
                                        v.pos += v.step;
                                        if v.pos >= le && le > ls {
                                            v.pos = le;
                                            v.reverse = true;
                                        } else if v.pos >= n {
                                            v.active = false;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            row += 1;
        }
        order_pos += 1;
    }
    // Soft clamp — tracker sums can exceed unity by design.
    for s in &mut out {
        *s = s.clamp(-1.0, 1.0);
    }
    out
}

/// Apply a cell's note/instrument/volume/pan to a track's voice slots,
/// honoring the instrument's NNA.
fn trigger_cell(
    song: &TrackerSong,
    samples: &[DecodedSample],
    out_rate: u32,
    slots: &mut [Voice; 2],
    cell: &TrackerCell,
) {
    // Volume/pan-only rows adjust the live voice without retriggering.
    if cell.note.is_none() {
        if let Some(vol) = cell.vol {
            slots[0].vol = (vol as f32).clamp(0.0, MAX_VOL);
        }
        if let Some(pan) = cell.pan {
            slots[0].pan = pan as f32 / 255.0;
        }
        return;
    }
    let note = cell.note.unwrap();
    let instr_idx = cell.instr.map(|i| i as usize).unwrap_or(slots[0].instr);
    let Some(instr) = song.instruments.get(instr_idx) else {
        return;
    };
    // Zone resolution (FR-0018): nearest root wins; legacy instruments
    // (no zones) behave as one implicit zone at base_note whose sample
    // index is the instrument index — the pre-zone contract.
    let zone = resolve_zone(instr, instr_idx, note);
    let Some(sample) = samples.get(zone.sample) else {
        return;
    };
    if sample.data.is_empty() {
        return;
    }

    // NNA: Continue moves the ringing voice to slot 1; Cut just replaces.
    if slots[0].active && instr.nna == Nna::Continue {
        slots[1] = slots[0].clone();
    }

    let n_frames = sample.data.len() as u64;
    let z_start = zone.start.min(n_frames);
    let z_end = if zone.end == 0 {
        n_frames
    } else {
        zone.end.min(n_frames)
    };
    let zl_start = zone.loop_start.clamp(z_start, z_end);
    let zl_end = if zone.loop_end == 0 {
        z_end
    } else {
        zone.loop_end.clamp(zl_start, z_end)
    };

    let rate_ratio = sample.sample_rate as f64 / out_rate.max(1) as f64;
    let note_factor = semis_to_factor(note as f64 - zone.root as f64);
    let base_step = rate_ratio * note_factor;

    // 9xx sample offset applies at trigger (relative to the zone start).
    let offset = match cell.fx {
        Some((b'9', p)) => (z_start + ((p & 0xFF) as u64) * 256).min(z_end) as f64,
        _ => z_start as f64,
    };

    slots[0] = Voice {
        instr: instr_idx,
        sample_idx: zone.sample,
        zone_start: z_start as f64,
        zone_end: z_end as f64,
        zloop_start: zl_start as f64,
        zloop_end: zl_end as f64,
        pos: offset,
        step: base_step,
        base_step,
        slide_semis: 0.0,
        vol: cell
            .vol
            .map(|v| v as f32)
            .unwrap_or(MAX_VOL)
            .clamp(0.0, MAX_VOL),
        pan: cell.pan.map(|p| p as f32 / 255.0).unwrap_or(0.5),
        reverse: false,
        active: true,
        filter: instr.filter.as_ref().and_then(|c| make_filter(c, out_rate)),
        vib_phase: 0.0,
    };
}

/// Render a single note of one instrument through the real engine —
/// the piano widget's audition path (FR-0018 E4). Same zones, filter,
/// gain, and loop behavior playback will use.
pub fn render_one_note(
    song: &TrackerSong,
    samples: &[DecodedSample],
    instr_idx: usize,
    note: Note,
    out_rate: u32,
    secs: f32,
) -> Vec<f32> {
    let mut one = TrackerSong::new(1, 1);
    one.instruments = song.instruments.clone();
    // One row stretched to ~secs: rows are speed·tick long; pick speed
    // so a single row covers the audition window (cap at the engine's
    // 31-tick ceiling and pad rows if needed).
    one.bpm = 125.0;
    let tick_secs = 2.5 / one.bpm as f64;
    let ticks = ((secs as f64 / tick_secs).ceil() as u64).max(1);
    let speed = ticks.min(31) as u8;
    let rows = ticks.div_ceil(31).max(1) as u16;
    one.speed = speed;
    one.patterns[0] = TrackerPattern::empty(1, rows);
    one.patterns[0].tracks[0][0] = TrackerCell {
        note: Some(note),
        instr: Some(instr_idx as u8),
        ..Default::default()
    };
    render_song(&one, samples, out_rate)
}

/// Pick the zone whose root is nearest the played note (ties → lower
/// root). Legacy no-zone instruments synthesize the implicit zone.
fn resolve_zone(instr: &TrackerInstrument, instr_idx: usize, note: Note) -> SampleZone {
    if instr.zones.is_empty() {
        return SampleZone {
            root: instr.base_note,
            sample: instr_idx,
            start: 0,
            end: 0,
            loop_start: instr.loop_start,
            loop_end: instr.loop_end,
        };
    }
    *instr
        .zones
        .iter()
        .min_by_key(|z| ((z.root as i32 - note as i32).abs(), z.root))
        .expect("non-empty zones")
}

/// Total frames one pattern occupies at the song's (initial) tempo —
/// used by the UI for row-position display. Fxx changes mid-pattern are
/// not reflected here (display-only helper).
pub fn pattern_frames(song: &TrackerSong, pattern: &TrackerPattern, out_rate: u32) -> usize {
    tick_frames(out_rate, song.bpm) * song.speed.max(1) as usize * pattern.rows as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn ramp_sample(len: usize) -> DecodedSample {
        DecodedSample {
            data: (0..len).map(|i| (i as f32 / len as f32) * 0.5).collect(),
            sample_rate: RATE,
        }
    }
    fn dc_sample(len: usize, level: f32) -> DecodedSample {
        DecodedSample {
            data: vec![level; len],
            sample_rate: RATE,
        }
    }

    fn one_note_song(rows: u16) -> (TrackerSong, Vec<DecodedSample>) {
        let mut song = TrackerSong::new(1, rows);
        song.instruments.push(TrackerInstrument::simple("s"));
        song.patterns[0].tracks[0][0] = TrackerCell {
            note: Some(NOTE_C4),
            instr: Some(0),
            ..Default::default()
        };
        (song, vec![dc_sample(RATE as usize * 4, 0.25)])
    }

    #[test]
    fn render_length_is_exact() {
        let (song, samples) = one_note_song(4);
        let out = render_song(&song, &samples, RATE);
        // 125 BPM → tick = 48000·2.5/125 = 960 frames; speed 6; 4 rows.
        let expected_frames = 960 * 6 * 4;
        assert_eq!(out.len(), expected_frames * 2, "stereo interleaved");
    }

    #[test]
    fn base_note_plays_at_native_rate() {
        let mut song = TrackerSong::new(1, 1);
        song.instruments.push(TrackerInstrument::simple("r"));
        song.patterns[0].tracks[0][0] = TrackerCell {
            note: Some(NOTE_C4),
            instr: Some(0),
            ..Default::default()
        };
        let ramp = ramp_sample(RATE as usize);
        let out = render_song(&song, std::slice::from_ref(&ramp), RATE);
        // Frame k of L channel should be ramp[k] × (pan 0.5) with vol 64.
        for k in [1usize, 100, 1000] {
            let expect = ramp.data[k] * 0.5;
            assert!(
                (out[k * 2] - expect).abs() < 1e-4,
                "frame {k}: {} vs {expect}",
                out[k * 2]
            );
        }
    }

    #[test]
    fn octave_up_doubles_the_step() {
        let mut song = TrackerSong::new(1, 1);
        song.instruments.push(TrackerInstrument::simple("r"));
        song.patterns[0].tracks[0][0] = TrackerCell {
            note: Some(NOTE_C4 + 12),
            instr: Some(0),
            ..Default::default()
        };
        let ramp = ramp_sample(RATE as usize);
        let out = render_song(&song, std::slice::from_ref(&ramp), RATE);
        // At +12 semitones the voice reads 2 source frames per output
        // frame: out[k] ≈ ramp[2k] · pan.
        for k in [10usize, 500] {
            let expect = ramp.data[2 * k] * 0.5;
            assert!(
                (out[k * 2] - expect).abs() < 1e-3,
                "frame {k}: {} vs {expect}",
                out[k * 2]
            );
        }
    }

    #[test]
    fn volume_column_scales_output() {
        let (mut song, samples) = one_note_song(1);
        song.patterns[0].tracks[0][0].vol = Some(32); // half
        let half = render_song(&song, &samples, RATE);
        song.patterns[0].tracks[0][0].vol = Some(64);
        let full = render_song(&song, &samples, RATE);
        assert!((half[100] * 2.0 - full[100]).abs() < 1e-5);
    }

    #[test]
    fn note_cut_silences_after_the_named_tick() {
        let (mut song, samples) = one_note_song(1);
        song.patterns[0].tracks[0][0].fx = Some((b'E', 0x00C2)); // ECx, x=2
        let out = render_song(&song, &samples, RATE);
        let tick = 960usize;
        assert!(out[(tick * 2 - 2) * 2].abs() > 0.0, "audible before cut");
        // First frame of tick 2 onward is silent.
        assert_eq!(out[(tick * 2 + 10) * 2], 0.0, "cut at tick 2");
    }

    #[test]
    fn note_delay_starts_at_the_named_tick() {
        let (mut song, samples) = one_note_song(1);
        song.patterns[0].tracks[0][0].fx = Some((b'E', 0x00D3)); // EDx, x=3
        let out = render_song(&song, &samples, RATE);
        let tick = 960usize;
        assert_eq!(out[(tick * 3 - 10) * 2], 0.0, "silent before delay");
        assert!(out[(tick * 3 + 10) * 2].abs() > 0.0, "audible after");
    }

    #[test]
    fn set_speed_changes_row_length() {
        let (mut song, samples) = one_note_song(2);
        // Row 0 sets speed 3 → rows are 3 ticks from then on.
        song.patterns[0].tracks[0][0].fx = Some((b'F', 0x0003));
        let out = render_song(&song, &samples, RATE);
        assert_eq!(out.len(), 960 * 3 * 2 * 2, "both rows at speed 3");
    }

    #[test]
    fn pattern_break_skips_the_rest() {
        let (mut song, samples) = one_note_song(8);
        song.patterns[0].tracks[0][1].fx = Some((b'D', 0x0000));
        let out = render_song(&song, &samples, RATE);
        assert_eq!(out.len(), 960 * 6 * 2 * 2, "only rows 0..=1 played");
    }

    #[test]
    fn order_chain_concatenates() {
        let (mut song, samples) = one_note_song(2);
        song.order = vec![0, 0, 0];
        let out = render_song(&song, &samples, RATE);
        assert_eq!(out.len(), 960 * 6 * 2 * 3 * 2);
    }

    #[test]
    fn forward_loop_keeps_ringing() {
        let mut song = TrackerSong::new(1, 4);
        let mut inst = TrackerInstrument::simple("loop");
        inst.loop_mode = LoopMode::Forward;
        inst.loop_start = 0;
        inst.loop_end = 100;
        song.instruments.push(inst);
        song.patterns[0].tracks[0][0] = TrackerCell {
            note: Some(NOTE_C4),
            instr: Some(0),
            ..Default::default()
        };
        let samples = vec![dc_sample(100, 0.25)];
        let out = render_song(&song, &samples, RATE);
        // Deep into the pattern (far past 100 frames) it still sounds.
        let deep = out.len() - 20;
        assert!(out[deep].abs() > 0.0, "loop sustains to the end");
    }

    #[test]
    fn nna_cut_vs_continue() {
        // Two notes a row apart; with Continue the first keeps ringing
        // (loop), so the summed level right after the second trigger is
        // higher than with Cut.
        let build = |nna: Nna| {
            let mut song = TrackerSong::new(1, 2);
            let mut inst = TrackerInstrument::simple("s");
            inst.loop_mode = LoopMode::Forward;
            inst.loop_end = 50;
            inst.nna = nna;
            song.instruments.push(inst);
            for r in 0..2 {
                song.patterns[0].tracks[0][r] = TrackerCell {
                    note: Some(NOTE_C4),
                    instr: Some(0),
                    ..Default::default()
                };
            }
            render_song(&song, &[dc_sample(50, 0.2)], RATE)
        };
        let cut = build(Nna::Cut);
        let cont = build(Nna::Continue);
        let row2 = 960 * 6 * 2 + 100; // shortly after row 1 triggers
        assert!(
            cont[row2] > cut[row2] + 0.05,
            "continue rings both voices: {} vs {}",
            cont[row2],
            cut[row2]
        );
    }

    #[test]
    fn unknown_commands_are_preserved_and_ignored() {
        let (mut song, samples) = one_note_song(1);
        song.patterns[0].tracks[0][0].fx = Some((b'Z', 0xBEEF));
        let json = serde_json::to_string(&song).unwrap();
        let back: TrackerSong = serde_json::from_str(&json).unwrap();
        assert_eq!(back.patterns[0].tracks[0][0].fx, Some((b'Z', 0xBEEF)));
        // And playback neither panics nor changes length.
        let out = render_song(&back, &samples, RATE);
        assert_eq!(out.len(), 960 * 6 * 2);
    }

    #[test]
    fn zones_pick_the_nearest_root() {
        // Two zones: C-3 (sample 0 = DC 0.1) and C-5 (sample 1 = DC 0.3).
        // Playing C-4 (equidistant) ties → lower root (sample 0).
        // Playing D-5 → C-5 zone (sample 1).
        let mut song = TrackerSong::new(1, 2);
        let mut inst = TrackerInstrument::simple("multi");
        inst.zones = vec![
            SampleZone {
                root: 36,
                sample: 0,
                start: 0,
                end: 0,
                loop_start: 0,
                loop_end: 0,
            },
            SampleZone {
                root: 60,
                sample: 1,
                start: 0,
                end: 0,
                loop_start: 0,
                loop_end: 0,
            },
        ];
        song.instruments.push(inst);
        song.patterns[0].tracks[0][0] = TrackerCell {
            note: Some(48),
            instr: Some(0),
            ..Default::default()
        };
        song.patterns[0].tracks[0][1] = TrackerCell {
            note: Some(62),
            instr: Some(0),
            ..Default::default()
        };
        let samples = vec![
            dc_sample(RATE as usize * 4, 0.1),
            dc_sample(RATE as usize * 4, 0.3),
        ];
        let out = render_song(&song, &samples, RATE);
        // Row 0: C-4 from the C-3 zone → +12 semis, DC value 0.1·pan.
        assert!((out[100 * 2] - 0.1 * 0.5).abs() < 1e-4, "row 0 uses zone 0");
        // Row 1: D-5 from the C-5 zone → DC value 0.3·pan.
        let row1 = 960 * 6 * 2 + 200;
        assert!((out[row1] - 0.3 * 0.5).abs() < 1e-4, "row 1 uses zone 1");
    }

    #[test]
    fn zone_trim_bounds_playback() {
        // Zone trimmed to frames 0..1000 at base pitch: the voice must
        // go silent once it crosses the trim end, long before the
        // sample's real end.
        let mut song = TrackerSong::new(1, 2);
        let mut inst = TrackerInstrument::simple("trim");
        inst.zones = vec![SampleZone {
            root: NOTE_C4,
            sample: 0,
            start: 0,
            end: 1000,
            loop_start: 0,
            loop_end: 0,
        }];
        song.instruments.push(inst);
        song.patterns[0].tracks[0][0] = TrackerCell {
            note: Some(NOTE_C4),
            instr: Some(0),
            ..Default::default()
        };
        let samples = vec![dc_sample(RATE as usize * 4, 0.25)];
        let out = render_song(&song, &samples, RATE);
        assert!(out[500 * 2].abs() > 0.0, "audible inside the trim");
        assert_eq!(out[2000 * 2], 0.0, "silent past the trim end");
    }

    #[test]
    fn legacy_instrument_equals_single_zone() {
        // A no-zones instrument and an explicit one-zone instrument at
        // the same root must render identically.
        let build = |zoned: bool| {
            let mut song = TrackerSong::new(1, 2);
            let mut inst = TrackerInstrument::simple("s");
            if zoned {
                inst.zones = vec![SampleZone {
                    root: NOTE_C4,
                    sample: 0,
                    start: 0,
                    end: 0,
                    loop_start: 0,
                    loop_end: 0,
                }];
            }
            song.instruments.push(inst);
            song.patterns[0].tracks[0][0] = TrackerCell {
                note: Some(NOTE_C4 + 5),
                instr: Some(0),
                ..Default::default()
            };
            render_song(&song, &[ramp_sample(RATE as usize)], RATE)
        };
        assert_eq!(build(false), build(true));
    }

    #[test]
    fn note_names_render_tracker_style() {
        assert_eq!(note_name(NOTE_C4), "C-4");
        assert_eq!(note_name(NOTE_C4 + 10), "A#4");
    }
}
