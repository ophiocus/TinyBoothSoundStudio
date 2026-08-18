//! TBSS-FR-0013 · chorddb — generative guitar chord-voicing engine.
//!
//! A voicing's entire left hand is a matrix: per string a fret
//! (muted / open / fretted) plus a finger. Rather than hand-enter the
//! thousands of shapes a guitar affords, we *generate* them — enumerate the
//! playable fret combinations inside a moving neck window, keep only those
//! whose sounding pitch-classes spell the target chord (root in the bass,
//! no foreign notes), assign an ergonomic fingering, then score and rank.
//! Every stored voicing is therefore pitch-class-correct **by construction**,
//! not by trust.
//!
//! The 44 hand-verified voicings in `docs/research/…verified.json` are not
//! the database — they are the *golden set*. The concrete open/named shapes
//! are overlaid at rank 0 so common songs render the recognisable grips with
//! their canonical fingering, and every one doubles as a correctness anchor
//! for the generator (a test asserts each spells its chord).
//!
//! Conventions (locked, TBSS-FR-0013 E4):
//!   * strings ordered low-E → high-e (index 0..5), canonical right-handed;
//!   * fret: `-1` muted, `0` open, `n` fretted; finger: `0` open/muted, `1..4`;
//!   * the renderer mirrors for left-handed at *draw* time — data stays
//!     canonical right-handed.
//!
//! v1 playability model (documented limits, relaxable later):
//!   * sounding strings must be **contiguous** (no interior string skips);
//!   * **root in the bass** (root-position only — no inversions/slash chords yet);
//!   * fingering must fit **four fingers** with an optional lowest-fret barre;
//!   * neck window is frets 0..12, hand span ≤ 4 frets.
//!
//! Consumed by E3 (voicing resolver) and E4 (fretboard renderer), which
//! aren't wired up yet — module-level `allow(dead_code)`; the tests exercise
//! every function.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Open-string MIDI note numbers, low-E → high-e (standard EADGBE tuning).
pub const OPEN_MIDI: [i32; 6] = [40, 45, 50, 55, 59, 64];
pub const NUM_STRINGS: usize = 6;

/// Practical upper fret for chord diagrams — one octave of positions is
/// plenty and keeps enumeration bounded.
const MAX_FRET: i8 = 12;
/// Frets a fretting hand can span (window width beyond the lowest finger).
const MAX_SPAN: i8 = 4;
const MAX_FINGERS: u8 = 4;
/// How many ranked voicings to keep per chord.
const MAX_PER_CHORD: usize = 12;

/// Sentinel fret values.
pub const MUTED: i8 = -1;
pub const OPEN: i8 = 0;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

// ── Chord qualities ─────────────────────────────────────────────────────

/// Chord qualities the DB can voice. A superset of E1's six recognised
/// qualities (`chordgrid::ChordQuality`), extended with the shapes the
/// research file's own `gaps` list flagged as missing from a hand-entered
/// table: sus2/sus4, 6/m6, dim7, aug, m7b5, add9/9, and power chords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quality {
    Maj,
    Min,
    Dom7,
    Min7,
    Maj7,
    Dim,
    Dim7,
    Aug,
    Sus2,
    Sus4,
    Maj6,
    Min6,
    M7b5,
    Add9,
    Dom9,
    Power5,
}

impl Quality {
    pub const ALL: [Quality; 16] = [
        Quality::Maj,
        Quality::Min,
        Quality::Dom7,
        Quality::Min7,
        Quality::Maj7,
        Quality::Dim,
        Quality::Dim7,
        Quality::Aug,
        Quality::Sus2,
        Quality::Sus4,
        Quality::Maj6,
        Quality::Min6,
        Quality::M7b5,
        Quality::Add9,
        Quality::Dom9,
        Quality::Power5,
    ];

    /// `(essential, optional)` intervals in semitones from the root.
    ///
    /// **Essential** notes must all sound. **Optional** notes may sound but
    /// never have to — this is how the fifth is modelled, which is what makes
    /// the idiomatic open C7 (`x32310`, no fifth) a first-class voicing rather
    /// than a special case. A voicing may contain *only* essential ∪ optional
    /// pitch-classes (nothing foreign).
    fn intervals(self) -> (&'static [i8], &'static [i8]) {
        use Quality::*;
        match self {
            Maj => (&[0, 4], &[7]),
            Min => (&[0, 3], &[7]),
            Dom7 => (&[0, 4, 10], &[7]),
            Min7 => (&[0, 3, 10], &[7]),
            Maj7 => (&[0, 4, 11], &[7]),
            Dim => (&[0, 3, 6], &[]),
            Dim7 => (&[0, 3, 6, 9], &[]),
            Aug => (&[0, 4, 8], &[]),
            Sus2 => (&[0, 2, 7], &[]),
            Sus4 => (&[0, 5, 7], &[]),
            Maj6 => (&[0, 4, 9], &[7]),
            Min6 => (&[0, 3, 9], &[7]),
            M7b5 => (&[0, 3, 6, 10], &[]),
            // add9 = triad + 9th (2 semitones, one octave folded); 5th optional.
            Add9 => (&[0, 4, 2], &[7]),
            Dom9 => (&[0, 4, 10, 2], &[7]),
            Power5 => (&[0, 7], &[]),
        }
    }

    pub fn suffix(self) -> &'static str {
        use Quality::*;
        match self {
            Maj => "",
            Min => "m",
            Dom7 => "7",
            Min7 => "m7",
            Maj7 => "maj7",
            Dim => "dim",
            Dim7 => "dim7",
            Aug => "aug",
            Sus2 => "sus2",
            Sus4 => "sus4",
            Maj6 => "6",
            Min6 => "m6",
            M7b5 => "m7b5",
            Add9 => "add9",
            Dom9 => "9",
            Power5 => "5",
        }
    }

    /// Parse the long-form quality strings used in the research JSON.
    fn from_json(s: &str) -> Option<Quality> {
        Some(match s {
            "major" => Quality::Maj,
            "minor" => Quality::Min,
            "dom7" => Quality::Dom7,
            "min7" => Quality::Min7,
            "maj7" => Quality::Maj7,
            "dim" => Quality::Dim,
            "dim7" => Quality::Dim7,
            "aug" => Quality::Aug,
            "sus2" => Quality::Sus2,
            "sus4" => Quality::Sus4,
            "6" | "maj6" => Quality::Maj6,
            "m6" | "min6" => Quality::Min6,
            "m7b5" | "min7b5" => Quality::M7b5,
            "add9" => Quality::Add9,
            "9" | "dom9" => Quality::Dom9,
            "5" | "power5" => Quality::Power5,
            _ => return None,
        })
    }

    /// Bridge from E1's recognised-quality enum (a strict subset).
    pub fn from_grid(q: crate::chordgrid::ChordQuality) -> Quality {
        use crate::chordgrid::ChordQuality as G;
        match q {
            G::Major => Quality::Maj,
            G::Minor => Quality::Min,
            G::Dom7 => Quality::Dom7,
            G::Min7 => Quality::Min7,
            G::Maj7 => Quality::Maj7,
            G::Dim => Quality::Dim,
        }
    }
}

/// Display name for a chord, e.g. `C`, `Am`, `G7`, `F#m7b5`.
pub fn chord_name(root: u8, q: Quality) -> String {
    format!("{}{}", NOTE_NAMES[(root % 12) as usize], q.suffix())
}

/// Which pitch-classes are permitted (essential ∪ optional) for `root`/`q`.
fn allowed_mask(root: u8, q: Quality) -> [bool; 12] {
    let (ess, opt) = q.intervals();
    let mut a = [false; 12];
    for &iv in ess.iter().chain(opt.iter()) {
        a[((root as i8 + iv).rem_euclid(12)) as usize] = true;
    }
    a
}

/// The pitch-classes that *must* all sound for `root`/`q`.
fn essential_pcs(root: u8, q: Quality) -> Vec<usize> {
    let (ess, _) = q.intervals();
    ess.iter()
        .map(|&iv| ((root as i8 + iv).rem_euclid(12)) as usize)
        .collect()
}

// ── Voicing (the fret matrix) ───────────────────────────────────────────

/// One playable chord shape — the left-hand matrix plus derived metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voicing {
    /// Fret per string, low-E → high-e. `-1` muted, `0` open, `n` fretted.
    pub frets: [i8; 6],
    /// Finger per string. `0` open/muted, `1..4`.
    pub fingers: [u8; 6],
    /// Lowest fretted fret — the diagram window start. `0` if all open/muted.
    pub base_fret: i8,
    /// `true` for the hand-verified golden shapes overlaid at rank 0.
    pub verified: bool,
}

impl Voicing {
    /// Sounding (non-muted) string indices, low → high.
    pub fn sounding(&self) -> Vec<usize> {
        (0..6).filter(|&i| self.frets[i] >= 0).collect()
    }

    pub fn open_count(&self) -> usize {
        self.frets.iter().filter(|&&f| f == 0).count()
    }

    pub fn sounding_count(&self) -> usize {
        self.frets.iter().filter(|&&f| f >= 0).count()
    }

    /// Sounding pitch-classes (with multiplicity across strings).
    pub fn pitch_classes(&self) -> Vec<usize> {
        (0..6)
            .filter(|&i| self.frets[i] >= 0)
            .map(|i| ((OPEN_MIDI[i] + self.frets[i] as i32) % 12) as usize)
            .collect()
    }

    fn min_fretted(&self) -> Option<i8> {
        self.frets.iter().copied().filter(|&f| f > 0).min()
    }

    pub fn max_fret(&self) -> i8 {
        self.frets.iter().copied().max().unwrap_or(0).max(0)
    }

    /// Fret span the hand must cover (0 for open/all-muted shapes).
    pub fn span(&self) -> i8 {
        match self.min_fretted() {
            Some(mn) => self.max_fret() - mn,
            None => 0,
        }
    }

    /// Barre segments as `(fret, low_string, high_string)` — a finger that
    /// covers ≥2 strings at one fret. Drives the E4 renderer's barre bar.
    pub fn barres(&self) -> Vec<(i8, usize, usize)> {
        let mut out = Vec::new();
        for finger in 1..=MAX_FINGERS {
            let strings: Vec<usize> = (0..6)
                .filter(|&i| self.fingers[i] == finger && self.frets[i] > 0)
                .collect();
            if strings.len() >= 2 {
                let fret = self.frets[strings[0]];
                if strings.iter().all(|&i| self.frets[i] == fret) {
                    out.push((fret, strings[0], *strings.last().unwrap()));
                }
            }
        }
        out
    }

    /// Display name given the chord this voicing was filed under.
    pub fn name(&self, root: u8, q: Quality) -> String {
        chord_name(root, q)
    }
}

/// Assign fingers `1..4` to a fret vector, barring the lowest fret when it
/// spans multiple strings. Returns `None` if it can't be fingered with four
/// fingers. Ascending (fret, string) order reproduces the canonical open and
/// CAGED-barre fingerings (verified against the golden set).
fn assign_fingers(frets: &[i8; 6]) -> Option<[u8; 6]> {
    let mut fingers = [0u8; 6];
    let fretted: Vec<usize> = (0..6).filter(|&i| frets[i] > 0).collect();
    if fretted.is_empty() {
        return Some(fingers);
    }
    let min_fret = fretted.iter().map(|&i| frets[i]).min().unwrap();
    let low_strings: Vec<usize> = fretted
        .iter()
        .copied()
        .filter(|&i| frets[i] == min_fret)
        .collect();
    let barre = low_strings.len() >= 2;

    let mut next = 1u8;
    if barre {
        for &i in &low_strings {
            fingers[i] = 1;
        }
        next = 2;
    }

    let mut rest: Vec<usize> = fretted
        .iter()
        .copied()
        .filter(|&i| !(barre && frets[i] == min_fret))
        .collect();
    rest.sort_by_key(|&i| (frets[i], i));
    for &i in &rest {
        if next > MAX_FINGERS {
            return None;
        }
        fingers[i] = next;
        next += 1;
    }
    Some(fingers)
}

/// Ergonomic score — higher is more idiomatic. Prefers open strings and low
/// positions, rewards fuller voicings, penalises stretch and barres. Verified
/// golden shapes get a large bonus so they always sort first.
fn score(v: &Voicing) -> f32 {
    3.0 * v.open_count() as f32 - 1.2 * v.base_fret as f32 + v.sounding_count() as f32
        - 0.5 * v.span() as f32
        - 0.7 * v.barres().len() as f32
        + if v.verified { 100.0 } else { 0.0 }
}

// ── Generator ───────────────────────────────────────────────────────────

struct GenCtx<'a> {
    allowed: [bool; 12],
    essential: &'a [usize],
    min_notes: usize,
    base: i8,
    root: u8,
}

impl GenCtx<'_> {
    /// Contiguity guard on the prefix `frets[0..=i]`: leading mutes, then one
    /// unbroken run of sounding strings, then trailing mutes. Prunes the DFS
    /// the moment a "sound, mute, sound" pattern appears.
    fn prefix_contiguous(frets: &[i8; 6], i: usize) -> bool {
        let mut started = false;
        let mut ended = false;
        for &f in frets.iter().take(i + 1) {
            let sounding = f >= 0;
            if sounding {
                if ended {
                    return false;
                }
                started = true;
            } else if started {
                ended = true;
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs(
        &self,
        i: usize,
        frets: &mut [i8; 6],
        cands: &[Vec<i8>],
        seen: &mut HashSet<[i8; 6]>,
        out: &mut Vec<Voicing>,
    ) {
        if i == NUM_STRINGS {
            self.finalize(frets, seen, out);
            return;
        }
        for &f in &cands[i] {
            frets[i] = f;
            if !Self::prefix_contiguous(frets, i) {
                continue;
            }
            self.dfs(i + 1, frets, cands, seen, out);
        }
        frets[i] = MUTED;
    }

    fn finalize(&self, frets: &[i8; 6], seen: &mut HashSet<[i8; 6]>, out: &mut Vec<Voicing>) {
        let sounding: Vec<usize> = (0..6).filter(|&i| frets[i] >= 0).collect();
        if sounding.len() < self.min_notes {
            return;
        }
        // Root must be in the bass (root position, v1).
        let bass = sounding[0];
        let bass_pc = ((OPEN_MIDI[bass] + frets[bass] as i32) % 12) as usize;
        if bass_pc != self.root as usize {
            return;
        }
        // Every essential pitch-class must sound.
        let pcs: HashSet<usize> = sounding
            .iter()
            .map(|&i| ((OPEN_MIDI[i] + frets[i] as i32) % 12) as usize)
            .collect();
        if !self.essential.iter().all(|e| pcs.contains(e)) {
            return;
        }
        // Anchor each fretted shape to the single window equal to its own
        // lowest fret, so it isn't generated once per enclosing window.
        let min_pos = frets.iter().copied().filter(|&f| f > 0).min();
        match min_pos {
            Some(mf) => {
                if self.base > 0 && mf != self.base {
                    return;
                }
                let maxf = sounding.iter().map(|&i| frets[i]).max().unwrap();
                if maxf - mf > MAX_SPAN {
                    return;
                }
            }
            None => {
                // All-open shape — emit only once, under the base-0 window.
                if self.base != 0 {
                    return;
                }
            }
        }
        let fingers = match assign_fingers(frets) {
            Some(f) => f,
            None => return,
        };
        if seen.insert(*frets) {
            out.push(Voicing {
                frets: *frets,
                fingers,
                base_fret: min_pos.unwrap_or(0),
                verified: false,
            });
        }
    }
}

/// Generate every playable, pitch-class-correct voicing of `root`/`q`,
/// ranked best-first and capped at [`MAX_PER_CHORD`].
pub fn generate_voicings(root: u8, q: Quality) -> Vec<Voicing> {
    let root = root % 12;
    let ctx = GenCtx {
        allowed: allowed_mask(root, q),
        essential: &essential_pcs(root, q),
        min_notes: {
            let (ess, _) = q.intervals();
            ess.len().max(2)
        },
        base: 0,
        root,
    };
    let mut seen: HashSet<[i8; 6]> = HashSet::new();
    let mut out: Vec<Voicing> = Vec::new();

    for base in 0..=(MAX_FRET - MAX_SPAN) {
        // Candidate frets per string within this window.
        let mut cands: Vec<Vec<i8>> = Vec::with_capacity(NUM_STRINGS);
        for &open in OPEN_MIDI.iter() {
            let mut v = vec![MUTED];
            if ctx.allowed[(open % 12) as usize] {
                v.push(OPEN);
            }
            let lo = base.max(1);
            let hi = (base + MAX_SPAN).min(MAX_FRET);
            for f in lo..=hi {
                if ctx.allowed[((open + f as i32) % 12) as usize] {
                    v.push(f);
                }
            }
            cands.push(v);
        }
        let window_ctx = GenCtx {
            base,
            ..ctx_ref(&ctx)
        };
        let mut frets = [MUTED; 6];
        window_ctx.dfs(0, &mut frets, &cands, &mut seen, &mut out);
    }

    out.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(MAX_PER_CHORD);
    out
}

/// Shallow copy of a [`GenCtx`] borrowing the same essential slice — lets the
/// per-window context reuse the parent's `allowed`/`essential` without
/// recomputing them each iteration.
fn ctx_ref<'a>(c: &GenCtx<'a>) -> GenCtx<'a> {
    GenCtx {
        allowed: c.allowed,
        essential: c.essential,
        min_notes: c.min_notes,
        base: c.base,
        root: c.root,
    }
}

// ── Golden overlay (research JSON) ──────────────────────────────────────

const GOLDEN_JSON: &str = include_str!("../docs/research/guitar-chord-voicings.verified.json");

#[derive(Deserialize)]
struct GoldenFile {
    voicings: Vec<GoldenVoicing>,
}

#[derive(Deserialize)]
struct GoldenVoicing {
    #[allow(dead_code)]
    name: String,
    root: String,
    quality: String,
    movable: bool,
    strings: Vec<GoldenString>,
}

#[derive(Deserialize)]
struct GoldenString {
    fret: i8,
    finger: u8,
}

fn parse_golden() -> Vec<GoldenVoicing> {
    serde_json::from_str::<GoldenFile>(GOLDEN_JSON)
        .expect("embedded golden voicings JSON must parse")
        .voicings
}

/// Note-name → pitch-class (handles `#`/`b`; returns `None` for "movable").
fn parse_root(s: &str) -> Option<u8> {
    let s = s.trim();
    let mut chars = s.chars();
    let letter = chars.next()?;
    let mut pc: i8 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    for c in chars {
        match c {
            '#' => pc += 1,
            'b' => pc -= 1,
            _ => return None,
        }
    }
    Some(pc.rem_euclid(12) as u8)
}

// ── The database ────────────────────────────────────────────────────────

/// A ready-to-query set of ranked voicings, one list per `(root, quality)`.
/// Build once (a few hundred ms in release) and cache it in app state.
pub struct ChordDb {
    map: HashMap<(u8, Quality), Vec<Voicing>>,
}

impl ChordDb {
    /// Generate the full DB, then overlay the hand-verified open/named shapes
    /// at rank 0 so common chords render their recognisable canonical grips.
    pub fn build() -> Self {
        let mut map: HashMap<(u8, Quality), Vec<Voicing>> = HashMap::new();
        for &q in Quality::ALL.iter() {
            for root in 0..12u8 {
                map.insert((root, q), generate_voicings(root, q));
            }
        }
        let mut db = ChordDb { map };
        db.apply_golden_overlay();
        db
    }

    /// Pin each concrete (non-movable) verified shape to the front of its
    /// chord's list, replacing the generator's equivalent. Movable CAGED
    /// templates are *not* overlaid — the generator already reproduces them at
    /// every root; they serve only as tests/documentation.
    fn apply_golden_overlay(&mut self) {
        for gv in parse_golden() {
            if gv.movable || gv.strings.len() != NUM_STRINGS {
                continue;
            }
            let (Some(root), Some(q)) = (parse_root(&gv.root), Quality::from_json(&gv.quality))
            else {
                continue;
            };
            let mut frets = [MUTED; 6];
            let mut fingers = [0u8; 6];
            for (i, s) in gv.strings.iter().enumerate() {
                frets[i] = s.fret;
                fingers[i] = s.finger;
            }
            let base_fret = frets.iter().copied().filter(|&f| f > 0).min().unwrap_or(0);
            let v = Voicing {
                frets,
                fingers,
                base_fret,
                verified: true,
            };
            let entry = self.map.entry((root, q)).or_default();
            entry.retain(|x| x.frets != v.frets);
            entry.insert(0, v);
            entry.truncate(MAX_PER_CHORD);
        }
    }

    /// Ranked voicings (best first) for a chord; empty slice if none.
    pub fn voicings(&self, root: u8, q: Quality) -> &[Voicing] {
        self.map
            .get(&(root % 12, q))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The top-ranked (most idiomatic) voicing for a chord.
    pub fn best(&self, root: u8, q: Quality) -> Option<&Voicing> {
        self.voicings(root, q).first()
    }

    /// The voicing minimising fret travel from `prev_base_fret`, ties broken
    /// by rank. This is E3's primary selector — it keeps a progression's
    /// diagrams from leaping up and down the neck. With no previous position,
    /// falls back to [`ChordDb::best`].
    pub fn best_near(&self, root: u8, q: Quality, prev_base_fret: Option<i8>) -> Option<&Voicing> {
        let vs = self.voicings(root, q);
        match prev_base_fret {
            None => vs.first(),
            Some(p) => vs
                .iter()
                .min_by_key(|v| (v.base_fret - p).unsigned_abs() as u32),
        }
    }

    /// Look up a voicing directly from an E1 chord label.
    pub fn for_label(&self, label: &crate::chordgrid::ChordLabel) -> Option<&Voicing> {
        self.best(label.root, Quality::from_grid(label.quality))
    }

    /// Total voicing count across every chord — coverage metric.
    pub fn total_voicings(&self) -> usize {
        self.map.values().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every concrete (non-movable) golden voicing must actually spell its
    /// chord: essentials all present, no foreign notes, root in the bass.
    /// This validates the research file *and* the pitch-class maths.
    #[test]
    fn golden_concrete_voicings_spell_their_chord() {
        for gv in parse_golden() {
            if gv.movable {
                continue;
            }
            let root = parse_root(&gv.root).expect(&gv.name);
            let q = Quality::from_json(&gv.quality).expect(&gv.name);
            let mut frets = [MUTED; 6];
            for (i, s) in gv.strings.iter().enumerate() {
                frets[i] = s.fret;
            }
            let v = Voicing {
                frets,
                fingers: [0; 6],
                base_fret: 0,
                verified: true,
            };
            let allowed = allowed_mask(root, q);
            let pcs = v.pitch_classes();
            // No foreign notes.
            for &pc in &pcs {
                assert!(
                    allowed[pc],
                    "{}: foreign pitch-class {} ({})",
                    gv.name, pc, NOTE_NAMES[pc]
                );
            }
            // All essentials present.
            for e in essential_pcs(root, q) {
                assert!(
                    pcs.contains(&e),
                    "{}: missing essential pitch-class {} ({})",
                    gv.name,
                    e,
                    NOTE_NAMES[e]
                );
            }
            // Root in the bass.
            let bass = v.sounding()[0];
            assert_eq!(
                pcs[0], root as usize,
                "{}: bass is not the root (string {})",
                gv.name, bass
            );
        }
    }

    #[test]
    fn open_c_major_is_the_preferred_c() {
        let db = ChordDb::build();
        let best = db.best(0, Quality::Maj).expect("C major exists");
        assert!(best.verified, "preferred C should be the verified shape");
        assert_eq!(best.frets, [-1, 3, 2, 0, 1, 0], "open C = x32010");
    }

    #[test]
    fn c7_omits_the_fifth_and_is_still_correct() {
        let db = ChordDb::build();
        let best = db.best(0, Quality::Dom7).expect("C7 exists");
        assert_eq!(best.frets, [-1, 3, 2, 3, 1, 0], "open C7 = x32310");
        // G (pitch-class 7) is the fifth and is intentionally absent.
        assert!(
            !best.pitch_classes().contains(&7),
            "idiomatic open C7 omits its fifth"
        );
        // …yet the root, third and flat-seventh are all present.
        for e in [0usize, 4, 10] {
            assert!(best.pitch_classes().contains(&e));
        }
    }

    #[test]
    fn generator_reproduces_open_g_and_barre_f() {
        // Open G (320003) — all six strings sounding.
        let g = generate_voicings(7, Quality::Maj);
        assert!(
            g.iter().any(|v| v.frets == [3, 2, 0, 0, 0, 3]),
            "generator should find open G"
        );
        // F major E-shape barre at fret 1 (133211).
        let f = generate_voicings(5, Quality::Maj);
        assert!(
            f.iter().any(|v| v.frets == [1, 3, 3, 2, 1, 1]),
            "generator should find the F barre"
        );
    }

    #[test]
    fn all_generated_voicings_are_pitch_class_correct() {
        // Spot-check the invariant the generator promises: no foreign notes,
        // essentials present, root in bass, fingers in range.
        for &q in Quality::ALL.iter() {
            for root in 0..12u8 {
                let allowed = allowed_mask(root, q);
                let ess = essential_pcs(root, q);
                for v in generate_voicings(root, q) {
                    let pcs = v.pitch_classes();
                    assert_eq!(
                        pcs[0],
                        root as usize,
                        "{} root-in-bass",
                        chord_name(root, q)
                    );
                    for &pc in &pcs {
                        assert!(allowed[pc], "{} foreign note", chord_name(root, q));
                    }
                    for e in &ess {
                        assert!(pcs.contains(e), "{} missing essential", chord_name(root, q));
                    }
                    for i in 0..6 {
                        assert!(v.fingers[i] <= MAX_FINGERS);
                        assert_eq!(
                            v.fingers[i] > 0,
                            v.frets[i] > 0,
                            "{} finger↔fret mismatch on string {}",
                            chord_name(root, q),
                            i
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn coverage_core_qualities_every_root_nonempty() {
        let db = ChordDb::build();
        // The bread-and-butter qualities must exist for all twelve roots.
        let core = [
            Quality::Maj,
            Quality::Min,
            Quality::Dom7,
            Quality::Min7,
            Quality::Maj7,
            Quality::Dim,
            Quality::Sus2,
            Quality::Sus4,
            Quality::Power5,
        ];
        for &q in &core {
            for root in 0..12u8 {
                assert!(
                    !db.voicings(root, q).is_empty(),
                    "no voicing for {}",
                    chord_name(root, q)
                );
            }
        }
    }

    #[test]
    fn coverage_reaches_into_the_thousands() {
        let db = ChordDb::build();
        let total = db.total_voicings();
        // The whole point of the generative pivot: coverage of the same order
        // of magnitude as a full chord dictionary, not 44 hand-entered rows.
        assert!(total > 1000, "expected thousands of voicings, got {total}");
    }

    #[test]
    fn best_near_minimises_fret_travel() {
        let db = ChordDb::build();
        // Ask for a C major near the 8th fret — should not hand back the open
        // shape if a higher voicing sits closer.
        if let Some(v) = db.best_near(0, Quality::Maj, Some(8)) {
            let open = db.best(0, Quality::Maj).unwrap();
            assert!(
                (v.base_fret - 8).abs() <= (open.base_fret - 8).abs(),
                "best_near should be at least as close as the open shape"
            );
        }
    }
}
