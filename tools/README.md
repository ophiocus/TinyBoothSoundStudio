# tools/

External utilities that complement the TinyBooth Sound Studio app. Run from your shell; no compilation involved.

## `compare.py` — exported-mix quality comparator

Compare a corrected mixdown against a baseline (the project's raw stem-sum or any reference WAV) and report the metrics that matter for Suno-style cleanup work: integrated LUFS, peak / RMS / crest factor, per-band RMS for sub / low / mud / mid / sibilance / air / shimmer, stereo correlation, and DC offset.

### Install

```
pip install numpy soundfile pyloudnorm
```

(Three pure-Python deps — `numpy`, `soundfile`, `pyloudnorm`. No native build needed.)

### Usage

**Auto-baseline from a project folder** — sums every WAV in `<project>/tracks/` at unity gain (mono → centre-pan, soft-limit to [-1, 1]) and uses that as the baseline:

```
python tools/compare.py \
    --project "C:\Users\Carlos\Downloads\suno\wadumdun-piano-Soda Stems (TinyBooth)" \
    --corrected "C:\path\to\export.wav"
```

**Direct file-vs-file** — for any two WAVs:

```
python tools/compare.py --files baseline.wav corrected.wav
```

Add `--out report.md` to write the report to a file instead of stdout.

### What you get

A markdown table with three sections:

- **Top-line metrics** — LUFS, peak dBFS, RMS dBFS, crest factor, stereo correlation, DC offset.
- **Per-band RMS** — eight bands; `✓` when a band moved in the intended-cleanup direction (mud / sibilance / shimmer **down**), `⚠` when it moved the opposite direction by ≥1 dB.
- **Verdict** — pass/fail bullets on loudness preservation, headroom, crest-factor drop, mud / shimmer reduction, stereo image preservation.

Sample (abridged):

```
## Top-line metrics

| Metric             | Baseline | Corrected |     Δ |
|---|---:|---:|---:|
| Integrated LUFS    |  -10.42  |  -10.78   | -0.360 |
| Peak (dBFS)        |    0.00  |   -1.84   | -1.840 |
| RMS  (dBFS)        |  -16.12  |  -17.03   | -0.910 |
| Crest factor (dB)  |  +16.12  |  +15.19   | -0.930 |

## Per-band RMS (dBFS)

| Band                  | Baseline | Corrected |     Δ |
|---|---:|---:|---:|
| mud       200–500 Hz  |  -22.15  |  -25.43   | -3.28 ✓ |
| shimmer  10–16 kHz    |  -34.07  |  -36.90   | -2.83 ✓ |

## Verdict

- ✓ Loudness preserved within ±1 LU.
- ✓ Peak headroom ≥ 1 dBFS — codec-safe.
- ✓ Crest factor drop 0.93 dB — dynamics preserved.
- ✓ Mud band reduced -3.28 dB.
- ✓ Shimmer band reduced -2.83 dB.
```

### Caveats

- **Inter-sample true peak** is not computed (would need oversampling). Sample peak is reported, which catches 99% of practical clipping; if you ship to streaming services that flag inter-sample peaks specifically, run a dedicated tool over the final master.
- **Baseline = raw stem sum**, not the Suno master. The two aren't bit-equivalent (Suno applies mastering on the rendered master only). The comparison is "did your chain change the bare-stems balance in the intended direction?" rather than "did you beat Suno's master".
- **Length alignment** truncates both files to the shorter — fair when the corrected mix was trimmed, but check the durations before reading too much into the LUFS numbers if they differ wildly.

### Roadmap

If this becomes a routine workflow we'll bring it into the app proper — likely a "Compare to baseline" button on the Export tab that runs the same metrics in-process and shows the report in a modal. For now, the script lets you start using it today and keeps the binary lean.
