//! TBSS-FR-0013 · E3 — voicing resolver (ChordGrid → guitar voicings).
//!
//! Bridges E1's beat-quantised [`ChordGrid`] to E4's renderer: collapse the
//! per-beat cells into **spans** (consecutive beats holding the same chord),
//! then choose one [`Voicing`] per span from [`ChordDb`], threading the
//! previous position so the diagrams don't leap up and down the neck between
//! chords. The output is the frame plan E5 muxes and the E2 panel previews.
//!
//! Pure data → data, no UI. Consumed by E2/E4/E5 (not wired yet) — module-level
//! `allow(dead_code)`; the tests exercise the surface.
#![allow(dead_code)]

use crate::chorddb::{ChordDb, Quality, Voicing};
use crate::chordgrid::{ChordGrid, ChordLabel};

/// One stretch of the timeline holding a single chord (or N.C.), with the
/// voicing chosen to render for it.
#[derive(Debug, Clone)]
pub struct VoicedSpan {
    pub start_secs: f32,
    pub end_secs: f32,
    /// `None` = no confident chord over this stretch (silence / ambiguous).
    pub chord: Option<ChordLabel>,
    /// The chosen diagram. `None` when N.C., or when the chord has no voicing
    /// in the DB (shouldn't happen for the recognised qualities, but the E2
    /// editor can introduce arbitrary labels).
    pub voicing: Option<Voicing>,
    /// Display label — the chord name, or `"N.C."`.
    pub name: String,
    /// Mean detection confidence over the beats this span merged.
    pub confidence: f32,
    /// Flagged for the E2 editor: the detection is weak enough to be worth a
    /// human check. Cleared once an operator edits the span — an edit is a
    /// decision, not a guess.
    pub low_confidence: bool,
}

/// Mean confidence below which a span is flagged for review in the editor.
pub const LOW_CONFIDENCE: f32 = 0.6;

impl VoicedSpan {
    pub fn duration(&self) -> f32 {
        (self.end_secs - self.start_secs).max(0.0)
    }
}

/// Pick a voicing for one chord label, preferring one near `prev_base_fret`.
pub fn voice_label(
    db: &ChordDb,
    label: &ChordLabel,
    prev_base_fret: Option<i8>,
) -> Option<Voicing> {
    db.best_near(
        label.root,
        Quality::from_grid(label.quality),
        prev_base_fret,
    )
    .cloned()
}

/// Resolve a whole grid into render-ready spans.
///
/// Consecutive cells with the *same* chord (equal `Some`, or both `None`) are
/// merged into one span. Each chorded span picks a voicing via
/// [`ChordDb::best_near`], carrying the last chosen `base_fret` forward so the
/// progression minimises hand travel. N.C. spans don't reset that memory —
/// the hand stays put across a rest.
pub fn resolve_spans(grid: &ChordGrid, db: &ChordDb) -> Vec<VoicedSpan> {
    let mut spans: Vec<VoicedSpan> = Vec::new();
    if grid.cells.is_empty() {
        return spans;
    }

    // Coalesce equal-chord runs into (chord, start, end, conf_sum, n_cells).
    let mut runs: Vec<(Option<ChordLabel>, f32, f32, f32, u32)> = Vec::new();
    for cell in &grid.cells {
        match runs.last_mut() {
            Some((chord, _, end, sum, n)) if *chord == cell.chord => {
                *end = cell.end_secs;
                *sum += cell.confidence;
                *n += 1;
            }
            _ => runs.push((
                cell.chord,
                cell.start_secs,
                cell.end_secs,
                cell.confidence,
                1,
            )),
        }
    }

    // Assign voicings, threading the previous fret position.
    let mut prev_base: Option<i8> = None;
    for (chord, start, end, conf_sum, n) in runs {
        let confidence = if n > 0 { conf_sum / n as f32 } else { 0.0 };
        let (voicing, name) = match chord {
            Some(label) => {
                let v = voice_label(db, &label, prev_base);
                if let Some(ref voicing) = v {
                    prev_base = Some(voicing.base_fret);
                }
                (v, label.name())
            }
            None => (None, "N.C.".to_string()),
        };
        spans.push(VoicedSpan {
            start_secs: start,
            end_secs: end,
            chord,
            voicing,
            name,
            confidence,
            // N.C. is a definite "no chord", not a shaky guess — don't flag it.
            low_confidence: chord.is_some() && confidence < LOW_CONFIDENCE,
        });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chordgrid::{ChordCell, ChordQuality};

    fn label(root: u8, q: ChordQuality) -> ChordLabel {
        ChordLabel { root, quality: q }
    }

    fn cell(beat: u32, chord: Option<ChordLabel>) -> ChordCell {
        ChordCell {
            start_secs: beat as f32 * 0.5,
            end_secs: (beat + 1) as f32 * 0.5,
            beat_index: beat,
            chord,
            confidence: if chord.is_some() { 0.9 } else { 0.0 },
        }
    }

    fn grid_of(cells: Vec<ChordCell>) -> ChordGrid {
        ChordGrid {
            bpm: 120.0,
            beat_times: cells.iter().map(|c| c.start_secs).collect(),
            cells,
            core_progression: Vec::new(),
            verb_span: None,
        }
    }

    #[test]
    fn coalesces_repeated_chords_into_spans() {
        use ChordQuality::*;
        // C C | G G | Am Am | F F  → four spans.
        let c = label(0, Major);
        let g = label(7, Major);
        let am = label(9, Minor);
        let f = label(5, Major);
        let cells = vec![
            cell(0, Some(c)),
            cell(1, Some(c)),
            cell(2, Some(g)),
            cell(3, Some(g)),
            cell(4, Some(am)),
            cell(5, Some(am)),
            cell(6, Some(f)),
            cell(7, Some(f)),
        ];
        let db = ChordDb::build();
        let spans = resolve_spans(&grid_of(cells), &db);
        assert_eq!(spans.len(), 4);
        assert_eq!(
            spans.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            vec!["C", "G", "Am", "F"]
        );
        // Each span merged two beats.
        assert!((spans[0].start_secs - 0.0).abs() < 1e-6);
        assert!((spans[0].end_secs - 1.0).abs() < 1e-6);
        // Every chord resolved to a voicing.
        for s in &spans {
            assert!(s.voicing.is_some(), "{} unresolved", s.name);
        }
    }

    #[test]
    fn nc_cells_become_nc_spans_without_a_voicing() {
        use ChordQuality::*;
        let c = label(0, Major);
        let cells = vec![
            cell(0, Some(c)),
            cell(1, None),
            cell(2, None),
            cell(3, Some(c)),
        ];
        let db = ChordDb::build();
        let spans = resolve_spans(&grid_of(cells), &db);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[1].name, "N.C.");
        assert!(spans[1].chord.is_none());
        assert!(spans[1].voicing.is_none());
    }

    #[test]
    fn empty_grid_yields_no_spans() {
        let db = ChordDb::build();
        assert!(resolve_spans(&grid_of(Vec::new()), &db).is_empty());
    }

    #[test]
    fn threads_position_to_limit_fret_travel() {
        use ChordQuality::*;
        // The invariant that proves the previous position is actually threaded
        // through: for every span after the first, no *other* voicing of that
        // chord sits strictly closer to the preceding span's position than the
        // one chosen. (A resolver that ignored history would fail this the
        // moment its default pick wasn't also the nearest.)
        let cells = vec![
            cell(0, Some(label(0, Major))), // C
            cell(1, Some(label(5, Major))), // F
            cell(2, Some(label(7, Major))), // G
            cell(3, Some(label(9, Minor))), // Am
            cell(4, Some(label(1, Major))), // C# — no open shape, forces a move
            cell(5, Some(label(6, Dom7))),  // F#7
        ];
        let db = ChordDb::build();
        let spans = resolve_spans(&grid_of(cells), &db);
        assert_eq!(spans.len(), 6);

        let mut prev: Option<i8> = None;
        for s in &spans {
            let (label, v) = match (s.chord, s.voicing.as_ref()) {
                (Some(l), Some(v)) => (l, v),
                _ => continue,
            };
            if let Some(p) = prev {
                let chosen = (v.base_fret - p).abs();
                let nearest = db
                    .voicings(label.root, Quality::from_grid(label.quality))
                    .iter()
                    .map(|c| (c.base_fret - p).abs())
                    .min()
                    .expect("chord has voicings");
                assert_eq!(
                    chosen, nearest,
                    "{}: chose base_fret {} (dist {}) from prev {}, but {} was reachable",
                    s.name, v.base_fret, chosen, p, nearest
                );
            }
            prev = Some(v.base_fret);
        }
    }
}
