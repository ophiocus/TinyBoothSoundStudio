# Getting started

A first-run walkthrough. Five minutes from "I just installed it" to "I have a saved take."

## 1. Confirm the input device

Plug in your microphone or audio interface before launching the app — TinyBooth enumerates input devices once at startup.

In the **Record** tab, the **Input device** dropdown lists every input the OS exposes. The default device appears first. If you plug something in after launch, hit **Refresh**.

If you don't see your interface at all, it's an OS problem — open Windows Sound settings and verify the device is enabled and not muted at the OS level.

## 2. Choose a source mode

The **Source** row controls how multi-channel devices are collapsed:

- **All (mixdown → mono)** — sums every input channel into one mono track. Default for a basic mic.
- **Ch N → mono** — picks a single channel from a multi-input interface.
- **Stereo (Ch 1 + Ch 2 → L/R)** — only shown if the device has at least 2 channels. Captures L and R as a true stereo track.

Most cheap USB mics are 1-channel and only the first option is meaningful.

## 3. Pick a recording tone

Above the device picker is **Recording tone**. The dropdown shows the active preset. It defaults to **Guitar**. Hover any name for a one-line description.

If you have no idea which one to pick, leave it on Guitar — it's intentionally the gentlest and won't actively damage anything you record. See the *Recording tones* chapter for the full set.

## 4. Make a take

- Click the **⏺ Record** button.
- The button changes to **⏹ Stop** while recording. Time elapsed and the WAV filename are shown next to it.
- Watch the live waveform and spectrum scroll. The peak meter at the bottom should sit between -12 and -6 dB on the loudest moments — basically, the bar should fill the middle to upper third without ever pinning red on the right.
- Click **⏹** to stop. The take is saved as a WAV under your project folder, and the manifest is written to disk.

## 5. Look at what you saved

Switch to the **Project** tab. Your take appears as a row with a name, source, sample rate, gain slider, duration, and a delete button. Each take = one row. Rename it inline if you want.

## 6. Export when ready

Switch to the **Export** tab. WAV is always available; FLAC / MP3 / Ogg / M4A appear if `ffmpeg.exe` is reachable (next to the app, in `./ffmpeg/bin/`, or on your PATH).

Pick a format, click **Export…**, choose a destination. That's it.
