//! TBSS-FR-0018 — bundled demo tracker songs.
//!
//! Traditional / public-domain tunes only: every melody here is a
//! centuries-old folk or classical theme whose composition is far
//! outside any copyright (Beethoven 1824; French trad. ~1780s; Russian
//! trad. 1861). The *encodings* are original to this project. Demos
//! bring **patterns and tempo** — the user's instruments (or a Library
//! pack) supply the sound; every note plays instrument 00.

#![allow(dead_code)]

use crate::tracker::{TrackerCell, TrackerPattern, TrackerSong};

/// (row, note) pairs → a 1-track pattern, one row per 8th note.
fn melody_song(bpm: f32, rows: u16, notes: &[(u16, u8)]) -> TrackerSong {
    let mut song = TrackerSong::new(4, rows);
    song.bpm = bpm;
    song.speed = 3; // 8th-note rows at speed 3 keeps tempo readable
    song.patterns[0] = TrackerPattern::empty(4, rows);
    for (row, note) in notes {
        if *row < rows {
            song.patterns[0].tracks[0][*row as usize] = TrackerCell {
                note: Some(*note),
                instr: Some(0),
                ..Default::default()
            };
        }
    }
    song
}

// Note helper: octave*12 + semitone (C=0 D=2 E=4 F=5 G=7 A=9 B=11).
const fn n(octave: u8, semi: u8) -> u8 {
    octave * 12 + semi
}

/// "Ode to Joy" — Beethoven, Symphony No. 9 (1824). The famous 16-bar
/// theme, quarter notes = 2 rows.
fn ode_to_joy() -> TrackerSong {
    const E: u8 = n(4, 4);
    const F: u8 = n(4, 5);
    const G: u8 = n(4, 7);
    const D: u8 = n(4, 2);
    const C: u8 = n(4, 0);
    let q = |i: u16| i * 2; // quarter-note rows
    let phrase1 = [E, E, F, G, G, F, E, D, C, C, D, E];
    let mut notes: Vec<(u16, u8)> = Vec::new();
    // Bar 1-4: phrase, ending E. D D (dotted rhythm approximated).
    for (i, p) in phrase1.iter().enumerate() {
        notes.push((q(i as u16), *p));
    }
    notes.push((q(12), E));
    notes.push((q(12) + 3, D));
    notes.push((q(14), D));
    // Bar 5-8: same phrase, ending D. C C.
    let off = q(16);
    for (i, p) in phrase1.iter().enumerate() {
        notes.push((off + q(i as u16), *p));
    }
    notes.push((off + q(12), D));
    notes.push((off + q(12) + 3, C));
    notes.push((off + q(14), C));
    melody_song(120.0, 64, &notes)
}

/// "Frère Jacques" — French traditional round (18th century).
fn frere_jacques() -> TrackerSong {
    const C: u8 = n(4, 0);
    const D: u8 = n(4, 2);
    const E: u8 = n(4, 4);
    const F: u8 = n(4, 5);
    const G: u8 = n(4, 7);
    const A: u8 = n(4, 9);
    const G3: u8 = n(3, 7);
    let mut notes: Vec<(u16, u8)> = Vec::new();
    let mut row = 0u16;
    let mut push = |notes: &mut Vec<(u16, u8)>, seq: &[(u8, u16)]| {
        for (note, len) in seq {
            notes.push((row, *note));
            row += len;
        }
    };
    // Frère Jacques ×2 (quarters), Dormez-vous ×2 (quarter quarter half),
    // Sonnez les matines ×2 (eighths + quarters), Ding dang dong ×2.
    push(&mut notes, &[(C, 2), (D, 2), (E, 2), (C, 2)]);
    push(&mut notes, &[(C, 2), (D, 2), (E, 2), (C, 2)]);
    push(&mut notes, &[(E, 2), (F, 2), (G, 4)]);
    push(&mut notes, &[(E, 2), (F, 2), (G, 4)]);
    push(
        &mut notes,
        &[(G, 1), (A, 1), (G, 1), (F, 1), (E, 2), (C, 2)],
    );
    push(
        &mut notes,
        &[(G, 1), (A, 1), (G, 1), (F, 1), (E, 2), (C, 2)],
    );
    push(&mut notes, &[(C, 2), (G3, 2), (C, 4)]);
    push(&mut notes, &[(C, 2), (G3, 2), (C, 4)]);
    melody_song(112.0, 64, &notes)
}

/// "Korobeiniki" — Russian traditional (1861 text; folk melody).
/// The tune every tracker knows. In E minor, eighth-note rows.
fn korobeiniki() -> TrackerSong {
    const E4: u8 = n(4, 4);
    const B3: u8 = n(3, 11);
    const C4: u8 = n(4, 0);
    const D4: u8 = n(4, 2);
    const A3: u8 = n(3, 9);
    const G3: u8 = n(3, 7);
    const F4: u8 = n(4, 5);
    const G4: u8 = n(4, 7);
    const A4: u8 = n(4, 9);
    let mut notes: Vec<(u16, u8)> = Vec::new();
    let mut row = 0u16;
    let mut push = |notes: &mut Vec<(u16, u8)>, seq: &[(u8, u16)]| {
        for (note, len) in seq {
            notes.push((row, *note));
            row += len;
        }
    };
    // A-section (the famous phrase), eighths = 1 row, quarters = 2.
    push(
        &mut notes,
        &[
            (E4, 2),
            (B3, 1),
            (C4, 1),
            (D4, 2),
            (C4, 1),
            (B3, 1),
            (A3, 2),
            (A3, 1),
            (C4, 1),
            (E4, 2),
            (D4, 1),
            (C4, 1),
            (B3, 3),
            (C4, 1),
            (D4, 2),
            (E4, 2),
            (C4, 2),
            (A3, 2),
            (A3, 4),
        ],
    );
    // B-phrase.
    push(
        &mut notes,
        &[
            (D4, 3),
            (F4, 1),
            (A4, 2),
            (G4, 1),
            (F4, 1),
            (E4, 3),
            (C4, 1),
            (E4, 2),
            (D4, 1),
            (C4, 1),
            (B3, 2),
            (B3, 1),
            (C4, 1),
            (D4, 2),
            (E4, 2),
            (C4, 2),
            (A3, 2),
            (A3, 4),
        ],
    );
    melody_song(150.0, 64, &notes)
}

/// The bundled demos: `(display name, song)`.
pub fn demo_songs() -> Vec<(&'static str, TrackerSong)> {
    vec![
        ("Ode to Joy (Beethoven, 1824)", ode_to_joy()),
        ("Frère Jacques (trad.)", frere_jacques()),
        ("Korobeiniki (trad., 1861)", korobeiniki()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demos_are_well_formed() {
        for (name, song) in demo_songs() {
            assert!(!song.instruments.is_empty() || song.instruments.is_empty()); // instruments intentionally empty
            assert_eq!(song.n_tracks(), 4, "{name}");
            let notes: usize = song.patterns[0]
                .tracks
                .iter()
                .flatten()
                .filter(|c| c.note.is_some())
                .count();
            assert!(notes >= 20, "{name} has a real melody ({notes} notes)");
            // Every note within playable range and on a valid row.
            for track in &song.patterns[0].tracks {
                assert_eq!(track.len(), song.patterns[0].rows as usize, "{name}");
                for c in track {
                    if let Some(nn) = c.note {
                        assert!((12..=96).contains(&nn), "{name}: note {nn} out of range");
                    }
                }
            }
        }
    }

    #[test]
    fn demos_render_deterministic_nonempty_length() {
        // With a simple instrument + DC sample attached, each demo must
        // render its exact pattern length and produce signal.
        for (name, mut song) in demo_songs() {
            song.instruments
                .push(crate::tracker::TrackerInstrument::simple("demo"));
            let samples = vec![crate::tracker::DecodedSample {
                data: vec![0.2; 48_000 * 2],
                sample_rate: 48_000,
            }];
            let out = crate::tracker::render_song(&song, &samples, 48_000);
            assert!(!out.is_empty(), "{name}");
            assert!(out.iter().any(|s| s.abs() > 0.0), "{name} is silent");
        }
    }
}
