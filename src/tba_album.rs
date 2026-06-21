//! Album ↔ `.tba` mapping. Thin shuttle layer — the `.tba` storage
//! lives in [`crate::tba`], the in-memory model + render DSP in
//! [`crate::album`]. This module is the seam.

use anyhow::Result;
use std::path::Path;

use crate::album::{Album, AlbumClip};
use crate::tba::{ClipRow, TbaDb};

/// Load an `Album` from an open `.tba`.
pub fn load_album(db: &TbaDb) -> Result<Album> {
    let name = db.name()?;
    let rows = db.list_clips()?;
    let clips = rows
        .into_iter()
        .map(|r| AlbumClip {
            source_path: Path::new(&r.source_path).to_path_buf(),
            start_secs: r.start_secs,
            fade_in_secs: r.fade_in_secs,
            fade_out_secs: r.fade_out_secs,
            gain_db: r.gain_db,
        })
        .collect();
    Ok(Album { name, clips })
}

/// Save an `Album` to an open `.tba`. Replaces every `clips` row in
/// one transaction; updates the `meta.name` column.
pub fn save_album(db: &mut TbaDb, album: &Album) -> Result<()> {
    db.set_name(&album.name)?;
    let rows: Vec<ClipRow> = album
        .clips
        .iter()
        .enumerate()
        .map(|(i, c)| ClipRow {
            id: 0,
            ord: i as i64,
            source_path: c.source_path.to_string_lossy().to_string(),
            start_secs: c.start_secs,
            fade_in_secs: c.fade_in_secs,
            fade_out_secs: c.fade_out_secs,
            gain_db: c.gain_db,
        })
        .collect();
    db.replace_clips(&rows)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "tbss-tba-album-{}-{}.tba",
            name,
            std::process::id()
        ));
        for suffix in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
        p
    }

    #[test]
    fn album_round_trip_through_tba() {
        let p = scratch("rt");
        let original = Album {
            name: "My Album".into(),
            clips: vec![
                AlbumClip {
                    source_path: "/stems/A.tib".into(),
                    start_secs: 0.0,
                    fade_in_secs: 1.0,
                    fade_out_secs: 2.0,
                    gain_db: -1.5,
                },
                AlbumClip {
                    source_path: "/stems/B.tib".into(),
                    start_secs: 30.0,
                    fade_in_secs: 2.0,
                    fade_out_secs: 1.0,
                    gain_db: 0.0,
                },
            ],
        };
        let mut db = TbaDb::create(&p, &original.name).unwrap();
        save_album(&mut db, &original).unwrap();
        drop(db);

        let db2 = TbaDb::open(&p).unwrap();
        let back = load_album(&db2).unwrap();
        assert_eq!(back.name, original.name);
        assert_eq!(back.clips.len(), 2);
        assert_eq!(back.clips[0].start_secs, 0.0);
        assert_eq!(back.clips[1].source_path.to_string_lossy(), "/stems/B.tib");
        assert!((back.clips[0].gain_db - (-1.5)).abs() < 1e-6);

        for suffix in ["", "-wal", "-shm"] {
            let mut q = p.as_os_str().to_os_string();
            q.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(q));
        }
    }
}
