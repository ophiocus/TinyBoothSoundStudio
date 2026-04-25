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
| `source` | object | Tagged enum: `{kind: "Recorded"}` or `{kind: "SunoStem", role, original_filename}`. |

### Backward compatibility

All fields added since v0.1 (`stereo`, `profile`, `source`) are marked `#[serde(default)]`. Older manifests load cleanly with sensible defaults.

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
  "active_profile": "Guitar"
}
```

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
