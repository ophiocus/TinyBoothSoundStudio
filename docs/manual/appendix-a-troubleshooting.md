# Appendix A — Troubleshooting

## "Input device not found" or empty dropdown

Cause: the OS isn't exposing any input device, or one was unplugged after launch.

Fix: ensure the device is plugged in and not muted at the OS level (Windows Sound settings). Then click the **Refresh** button on the Record tab — TinyBooth re-enumerates devices on demand.

## "Stereo (Ch 1+Ch 2)" option doesn't appear

Cause: the selected device reports only one channel.

Most cheap USB mics are 1-channel even if you expected stereo. Check the device's configuration in Windows Sound → Device properties → Advanced. If the device truly is single-channel hardware (e.g. a Blue Yeti in cardioid mode), there is no stereo to capture; use Mixdown.

## Recording sounds quiet / hot / clipped

Open **Admin → Recording-tone profiles…** and check the active profile's **Input gain** field. Negative values attenuate; positive values boost. The peak meter on the Record tab is the truth — aim for it to sit between -12 and -6 dB on the loudest moments and never pin red.

If the input is hot before TinyBooth even sees it (the level is high before the gain stage), reduce the OS-level input volume in Windows Sound settings. TinyBooth respects OS gain.

## Export options are greyed out with "(ffmpeg missing)"

Cause: the FLAC/MP3/Ogg/M4A encoders need an `ffmpeg.exe` somewhere TinyBooth can find it.

Fix:
1. Drop `ffmpeg.exe` in the same folder as `tinybooth-sound-studio.exe`, OR
2. Drop the official ffmpeg release zip's `bin/` folder under `ffmpeg/bin/` next to the exe, OR
3. Install ffmpeg system-wide: `winget install Gyan.FFmpeg` (then reopen TinyBooth).

WAV export always works without ffmpeg.

## Self-update click does nothing

Cause: usually offline, occasionally GitHub is rate-limiting. Less commonly, your Windows Defender or AV is intercepting the elevated `msiexec` launch.

Fix: confirm internet, retry by clicking the version label again. If you're certain there's a new release on GitHub but the click silently fails, download the MSI manually from the releases page.

## The Manual window doesn't open with F1

Cause: another egui widget has captured keyboard focus and is consuming the F1 keypress (e.g. you're typing in a text field).

Fix: click outside the text field, then press F1. Alternatively, **Help → Manual…** is always available.

## Project file fails to load

Cause: corrupted manifest JSON, or a track WAV file moved/deleted since save.

The project will still load if you can fix the manifest by hand — it's plain JSON. Open `project.tinybooth` in a text editor; the schema is documented in *Appendix B — File formats*.

## Crashes / unhandled errors

These are bugs. The app is small enough that a crash is reproducible — note what you were doing and open an issue at `https://github.com/ophiocus/TinyBoothSoundStudio/issues`. Logs, if any, go to `stderr`; running TinyBooth from a terminal will surface them.
