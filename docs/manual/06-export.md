# Export

Mixes all unmuted tracks in the current project to a single audio file. The output is stereo if any input track is stereo; mono otherwise.

## Formats

| Format | Extension | Backend | Notes |
|---|---|---|---|
| WAV (16-bit) | `.wav` | hound (native) | Always available. |
| FLAC | `.flac` | ffmpeg | Lossless. |
| MP3 | `.mp3` | ffmpeg / libmp3lame | Bitrate slider 64–320 kbps. |
| Ogg Vorbis | `.ogg` | ffmpeg / libvorbis | Bitrate slider 64–320 kbps. |
| Ogg Opus | `.opus` | ffmpeg / libopus | Bitrate slider 64–320 kbps. |
| M4A (AAC) | `.m4a` | ffmpeg / aac | Bitrate slider 64–320 kbps. |

WAV is built in and always works. Everything else is dispatched to a bundled ffmpeg subprocess. If ffmpeg isn't found, the dropdown still shows those formats but they're disabled with an `(ffmpeg missing)` suffix.

## Where ffmpeg is searched for

In order:

1. `ffmpeg.exe` next to the running TinyBooth exe.
2. `ffmpeg/bin/ffmpeg.exe` next to the running exe (matches the Suno-style bundled layout).
3. `ffmpeg` or `ffmpeg.exe` on the system `PATH`.

If you install ffmpeg via `winget install Gyan.FFmpeg` and reopen TinyBooth, all formats become available. No restart needed beyond a fresh process.

## Mixdown algorithm

1. Read every unmuted track's WAV via hound.
2. Apply the per-track gain (dB → linear).
3. If any source track is stereo, the output is stereo. Mono inputs are centre-panned (same sample to L and R). Stereo inputs in a mono export are averaged to `(L+R)/2`.
4. Sum samples across all tracks.
5. **Soft-limit** to [-1, 1] — if the summed peak exceeds unity, the entire mix is scaled down by a single factor so the loudest moment lands at exactly 1.0. This prevents clipping but does NOT add perceived loudness; that's a separate mastering concern.
6. Write WAV, or pipe via a temporary WAV through ffmpeg with the requested codec.

## Sample-rate handling

All input tracks must share the same sample rate. If they differ, export errors with a clear message. Resampling is **not** currently supported — if you need a different output rate, take a detour through ffmpeg manually after the WAV export.

## File-naming default

The save dialog pre-fills with `<project_name>.<extension>`, sanitising any non-alphanumeric characters in the project name to underscores. Override at will.
