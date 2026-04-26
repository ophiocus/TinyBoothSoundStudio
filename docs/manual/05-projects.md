# Projects

A TinyBooth project is a folder on disk containing:

```
my-session/
  project.tinybooth     # JSON manifest
  tracks/
    track-001.wav
    track-002.wav
    ...
```

The `.tinybooth` file is a plain JSON document; it carries the project name, creation timestamp, and one entry per track (name, file path, mute, gain, sample rate, source mode, profile snapshot, stereo flag, and — for Suno-imported tracks — the original filename and inferred role).

Tracks live as siblings in `tracks/`. They are referenced by **relative path** from the manifest, so you can move or rename the project folder without breaking anything.

## Creating a project

Three ways:

- **Just hit Record without picking a folder.** TinyBooth creates a scratch project under `%APPDATA%\TinyBooth Sound Studio\sessions\session-<timestamp>\`. Convenient for testing; not where you want to store anything important.
- **File → New project…** picks a destination folder before you start recording.
- **Project tab → Choose folder…** moves the current project to a new folder. Tracks are not moved automatically — pick this *before* recording.

## The Project tab

Each track row has:

- **Mute checkbox** — excludes the track from export mixdown but keeps the file.
- **Name** — editable inline.
- **Source** — what kind of track it is: `mix`, `Ch N`, `stereo`, or `Suno · <Role>` for imported stems.
- **Rate** — sample rate (Hz).
- **Gain (dB)** — slider, −24 to +12. Applied at mixdown time.
- **Duration** — playback length in seconds.
- **✖** — delete the track and its WAV file.

The header shows the project name (editable), the folder path, and the creation timestamp.

## Saving and dirtiness

- A bullet (`●`) appears before the project name in the top bar when there are unsaved changes.
- **File → Save** writes the manifest immediately.
- **Stopping a take** also auto-saves.
- Quitting with unsaved changes does **not** prompt — the auto-save on stop covers the common case; explicit project metadata edits (rename, gain, mute) need an explicit Save.

## Opening a project

**File → Open project…** picks a `.tinybooth` file. The manifest is parsed; track entries are validated against the WAV files on disk; missing files show an error in the status bar but the project still loads.

### Auto-restore on startup

TinyBooth remembers the last project you were working on and re-opens it on the next launch. The path lives at `config.last_project_path` in `config.json`; it's updated every time you save, open, or import. If the file's been moved or deleted, TinyBooth falls back to a fresh scratch session and quietly clears the stale path — no error popup, just a status-bar note.

If you want to start with a blank slate without losing the breadcrumb, **File → New project…** picks a fresh folder and replaces the active project; it'll be the new "last" the next time you launch.

### File → Open Recent

The eight most recently-opened projects appear under **File → Open Recent**, most recent first. Each entry shows the parent folder's name (typically the project name) with the full path on hover. Click to load. **Clear list** wipes the history without touching the projects themselves.

A project that fails to load when picked from the recent list is automatically dropped from it — over time the menu prunes itself of dead entries.

## Manifest schema versioning

The current schema is `version: 1`. New fields added since v0.1 — `stereo`, `profile`, `source` — are all marked `#[serde(default)]`, so older manifests load cleanly with sensible defaults.
