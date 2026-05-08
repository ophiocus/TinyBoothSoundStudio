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

## Where ffmpeg comes from

The MSI installer ships a static-LGPL build of `ffmpeg.exe` (sourced from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds)) and drops it next to `tinybooth-sound-studio.exe` in the install dir. Nothing for the user to do. FLAC / MP3 / Ogg / M4A export works out of the box on a fresh install.

For dev / source builds, TinyBooth's discovery still falls back through the legacy paths in this order:

1. `ffmpeg.exe` next to the running TinyBooth exe (where the MSI install drops it).
2. `ffmpeg/bin/ffmpeg.exe` next to the running exe.
3. `ffmpeg` or `ffmpeg.exe` on the system `PATH`.

So `cargo run` from the repo will use whatever ffmpeg you have on `PATH` if any (or no formats beyond WAV if you don't).

### Licensing & attribution

The bundled ffmpeg is the LGPL v2.1+ build (no GPL components, no patent-encumbered codecs we don't need). TinyBooth invokes it as a separate executable via subprocess, which is the LGPL-compliant integration mode for non-free apps. FFmpeg source is available from the [FFmpeg project](https://ffmpeg.org/) and BtbN's [build pipeline](https://github.com/BtbN/FFmpeg-Builds) (the exact build this MSI bundles).

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
