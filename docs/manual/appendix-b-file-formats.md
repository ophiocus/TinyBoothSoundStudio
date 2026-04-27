# Appendix B — File formats

## `project.tinybooth` (JSON)

The project manifest. Located at the root of every TinyBooth project folder.

```json
{
  "version": 1,
  "name": "My Session",
  "created": "2026-04-25T18:00:00Z",
  "tracks": [
    {
      "id": "track-001",
      "name": "Lead vocal take 3",
      "file": "tracks/track-001.wav",
      "mute": false,
      "gain_db": 0.0,
      "sample_rate": 48000,
      "channel_source": null,
      "duration_secs": 47.2,
      "profile": { /* recording-tone profile snapshot, see below */ },
      "stereo": false,
      "source": { "kind": "Recorded" }
    }
  ]
}
```

### Track fields

| Field | Type | Notes |
|---|---|---|
| `id` | string | Unique within the project; auto-assigned `track-001`, `track-002`, etc. |
| `name` | string | Display name; user-editable. |
| `file` | string | Relative path from the manifest, always Unix-style separators. |
| `mute` | bool | Excluded from export when true. |
| `gain_db` | float | Applied at mixdown; range −24 to +12. |
| `sample_rate` | int | Hz, frozen at recording time. |
| `channel_source` | int or null | Mono mode: which hardware channel; `null` for mixdown or stereo. |
| `duration_secs` | float | Captured length. |
| `profile` | object or null | Snapshot of the recording-tone profile. |
| `stereo` | bool | True iff the underlying WAV has 2 channels. |
| `source` | object | Tagged enum: `{kind: "Recorded"}` or `{kind: "SunoStem", role, original_filename, session_epoch, session_ordinal, provenance}`. |
| `gain_automation` | object or null | Recorded fader-gesture lane (Catmull-Rom replay). `{points: [{time_secs, gain_db}, …]}`. |
| `correction` | object or null | Per-track DSP correction profile (same shape as a recording-tone profile). |

### Suno session metadata (added v0.3.1)

When a track originates from a Suno stem bundle, its `source` carries the session metadata harvested from the WAV's `LIST/INFO/ICMT` RIFF chunk:

- **`session_epoch`** — Unix epoch seconds (i64). Identical across every stem in one Suno render; distinct between re-renders. Sortable directly for "newest first" / "oldest first" views.
- **`session_ordinal`** — project-relative monotonically-increasing import counter (u32). Every track from one import event shares the ordinal; the project's `next_suno_ordinal` field is bumped on each successful import. Use this when you want to order *imports* into the project rather than Suno *renders*.
- **`provenance`** — free-form string from the same ICMT chunk (typically "made with suno studio").

The project itself gains a `next_suno_ordinal: u32` field (default `1`) to source the next ordinal.

### Project-level correction state (added v0.3.4)

Two new fields on the `Project` itself (alongside the existing `master_gain_db`, `master_gain_automation`, `next_suno_ordinal`):

- **`corrections_disabled`** — `bool`, default `false`. When true, every track's correction chain is bypassed at playback and at export. Non-destructive: chain configs stay put. The Mix-tab **Disable** button toggles this.
- **`default_correction`** — `Option<Profile>`, default `None`. Project-level seed used by **Enable all** when a track has no chain yet. Cascade order:
  1. existing `track.correction` (kept if `Some`)
  2. this `Project.default_correction` (if `Some`)
  3. feature default — Suno-Clean from `builtin_profiles()`
  
  No GUI editor yet; edit the JSON directly until one lands.

### Backward compatibility

All fields added since v0.1 (`stereo`, `profile`, `source`, `gain_automation`, `correction`, plus the v0.3.1 Suno session fields and `next_suno_ordinal`, plus the v0.2 `master_gain_db` and `master_gain_automation`, plus the v0.3.4 `corrections_disabled` and `default_correction`) are marked `#[serde(default)]`. Older manifests load cleanly with sensible defaults.

## `profiles.json` (JSON)

User-editable recording-tone profile list. Located at `%APPDATA%\TinyBooth Sound Studio\profiles.json`.

```json
[
  {
    "name": "Guitar",
    "description": "...",
    "input_gain_db": 0.0,
    "hpf_enabled": true,
    "hpf_hz": 60.0,
    "gate_enabled": false,
    "gate_threshold_db": -55.0,
    "gate_attack_ms": 3.0,
    "gate_release_ms": 150.0,
    "compressor_enabled": true,
    "compressor_threshold_db": -20.0,
    "compressor_ratio": 2.5,
    "compressor_attack_ms": 20.0,
    "compressor_release_ms": 150.0,
    "compressor_makeup_db": 3.0
  }
]
```

Edit by hand or via the Admin window.

## `config.json` (JSON)

App-wide configuration. Located at `%APPDATA%\TinyBooth Sound Studio\config.json`.

```json
{
  "dark_mode": true,
  "zoom": 1.0,
  "active_profile": "Guitar",
  "last_project_path": "C:\\path\\to\\project.tinybooth",
  "recent_projects": [
    "C:\\path\\to\\project.tinybooth",
    "C:\\other\\session\\project.tinybooth"
  ]
}
```

`last_project_path` and `recent_projects` were added in v0.2.1 — older configs default to `null` and `[]` respectively.

## Track WAV files

- 16-bit PCM (`hound::SampleFormat::Int`).
- Sample rate: matches the input device at recording time.
- Channels: 1 for mono modes, 2 (interleaved L R) for stereo mode.

Read by `hound::WavReader` at export and ingest time. Externally editable with any WAV-aware tool — TinyBooth re-reads from disk every export.

## Stem-source filenames (Suno)

Suno's stem bundle filenames are **advisory hints**, not contractual. The ingester matches by case-insensitive substring against:

```
vocal (+back) → BackingVocals
vocal         → Vocals
drum          → Drums
bass          → Bass
electric+guitar → ElectricGuitar
acoustic+guitar → AcousticGuitar
guitar        → ElectricGuitar (fallback)
piano|key     → Keys
synth|lead    → Synth
pad|chord     → Pads
string        → Strings
brass|wood    → Brass
perc          → Percussion
fx|other      → FxOther
instrumental  → Instrumental
master|mix|final → Master
(else)        → Unknown
```

Tempo-Locked WAVs (`tempo*lock` in filename) are excluded from import.
