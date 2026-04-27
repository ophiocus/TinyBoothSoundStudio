#!/usr/bin/env python3
"""
TinyBooth Sound Studio — exported-mix comparator.

Compare a corrected mix-down against a baseline (either an automatically-
summed project's stems, or a user-supplied baseline WAV) and report the
metrics that matter for Suno-style cleanup work:

  * integrated LUFS (BS.1770)
  * peak dBFS
  * RMS dBFS
  * crest factor (peak − RMS)  — over-compression guard
  * per-band RMS dBFS — sub / low / mud / mid / sibilance / air / shimmer
  * stereo correlation         — image-collapse guard
  * DC offset

Usage
-----

# Auto-baseline from a project's tracks/ folder:
python compare.py --project "C:\\path\\to\\<name> (TinyBooth)" --corrected "C:\\path\\to\\export.wav"

# Direct file-vs-file:
python compare.py --files "raw.wav" "corrected.wav"

Output is a markdown-friendly table you can paste into a doc or pipe to a file.

Dependencies
------------
    pip install numpy soundfile pyloudnorm

Notes
-----
* The auto-baseline path SUMS every WAV in `<project>/tracks/` at unity gain
  with mono → centre-pan, then soft-limits identically to the in-app
  mixdown. It does NOT apply per-track corrections — that's the whole
  point of comparing.
* "Improvement" thresholds are advisory; tune to taste. The script flags
  bands that moved by ≥1 dB so you can spot the chain's intended edits at
  a glance.
* Inter-sample true peak isn't computed (would need oversampling). Sample
  peak is what's reported; that's good enough for "did anything clip?"
  in 99% of practical cases.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import Iterable

import numpy as np
import soundfile as sf

try:
    import pyloudnorm
except ImportError:
    print("ERROR: pyloudnorm not installed. Run: pip install pyloudnorm", file=sys.stderr)
    sys.exit(2)


# ─── per-band edges (Hz) ──────────────────────────────────────────────────
BANDS = [
    ("sub        20–100 Hz",      20,    100),
    ("low       100–200 Hz",     100,    200),
    ("mud       200–500 Hz",     200,    500),
    ("mid       500 Hz–2 kHz",   500,   2000),
    ("upper-mid   2–5 kHz",     2000,   5000),
    ("sibilance   5–8 kHz",     5000,   8000),
    ("air        8–10 kHz",     8000,  10000),
    ("shimmer  10–16 kHz",     10000,  16000),
]


def _load_audio(path: Path) -> tuple[np.ndarray, int]:
    """Return (samples [N, 2], sample_rate). Mono → duplicated to L=R."""
    data, sr = sf.read(str(path), dtype="float32", always_2d=True)
    if data.shape[1] == 1:
        data = np.repeat(data, 2, axis=1)
    return data, sr


def _zero_pad(arr: np.ndarray, n: int) -> np.ndarray:
    if arr.shape[0] >= n:
        return arr[:n]
    pad = np.zeros((n - arr.shape[0], arr.shape[1]), dtype=arr.dtype)
    return np.concatenate([arr, pad], axis=0)


def make_raw_mix_from_project(project_dir: Path) -> tuple[np.ndarray, int]:
    """Sum every WAV in <project_dir>/tracks/ at unity gain, soft-limit."""
    tracks_dir = project_dir / "tracks"
    if not tracks_dir.is_dir():
        sys.exit(f"ERROR: no tracks/ folder under {project_dir}")
    wavs = sorted(tracks_dir.glob("*.wav"))
    if not wavs:
        sys.exit(f"ERROR: no WAVs in {tracks_dir}")

    streams = []
    sr_ref = None
    for w in wavs:
        a, sr = _load_audio(w)
        if sr_ref is None:
            sr_ref = sr
        elif sr != sr_ref:
            sys.exit(
                f"ERROR: sample-rate mismatch — {w.name} is {sr} Hz "
                f"but the first track was {sr_ref} Hz."
            )
        streams.append(a)

    longest = max(s.shape[0] for s in streams)
    mix = np.zeros((longest, 2), dtype=np.float32)
    for s in streams:
        mix += _zero_pad(s, longest)

    peak = float(np.max(np.abs(mix)))
    if peak > 1.0:
        mix /= peak
    return mix, sr_ref


def _band_rms_db(audio: np.ndarray, sr: int) -> dict[str, float]:
    """Per-band RMS in dBFS via single FFT on the mono sum."""
    mono = audio.mean(axis=1)
    n = len(mono)
    if n == 0:
        return {name: -np.inf for name, _, _ in BANDS}
    spec = np.abs(np.fft.rfft(mono))
    freqs = np.fft.rfftfreq(n, 1.0 / sr)
    out: dict[str, float] = {}
    for name, lo, hi in BANDS:
        mask = (freqs >= lo) & (freqs < hi)
        if not mask.any():
            out[name] = -np.inf
            continue
        # Parseval-equivalent RMS in this band — energy_in_band / N
        energy = (spec[mask] ** 2).sum() / (n * n / 2.0)
        out[name] = 10.0 * np.log10(max(energy, 1e-30))
    return out


def _stereo_correlation(audio: np.ndarray) -> float:
    if audio.shape[1] < 2:
        return 1.0
    L = audio[:, 0]
    R = audio[:, 1]
    if np.std(L) < 1e-9 or np.std(R) < 1e-9:
        return 1.0
    return float(np.clip(np.corrcoef(L, R)[0, 1], -1.0, 1.0))


def metrics(audio: np.ndarray, sr: int, label: str) -> dict:
    """Compute the full metric set for one signal."""
    if audio.shape[0] == 0:
        sys.exit(f"ERROR: '{label}' has zero samples.")

    meter = pyloudnorm.Meter(sr)
    lufs = float(meter.integrated_loudness(audio))

    peak = float(np.max(np.abs(audio)))
    peak_db = 20.0 * np.log10(max(peak, 1e-30))

    rms = float(np.sqrt(np.mean(audio ** 2)))
    rms_db = 20.0 * np.log10(max(rms, 1e-30))

    crest_db = peak_db - rms_db
    dc = float(np.mean(audio))
    correlation = _stereo_correlation(audio)
    bands = _band_rms_db(audio, sr)

    return dict(
        label=label, lufs=lufs, peak_db=peak_db, rms_db=rms_db,
        crest_db=crest_db, dc=dc, correlation=correlation, bands=bands,
    )


def _delta_marker(d: float, good_negative: bool, threshold: float = 1.0) -> str:
    """Annotate a delta with ✓ (intended-direction change ≥ threshold), ⚠ (wrong direction ≥ threshold), or blank."""
    if abs(d) < threshold:
        return " "
    if good_negative:
        return "✓" if d < 0 else "⚠"
    return " "


def report(orig: dict, corrected: dict) -> str:
    lines: list[str] = []
    lines.append(f"# TinyBooth comparison report\n")
    lines.append(f"- Baseline: **{orig['label']}**")
    lines.append(f"- Corrected: **{corrected['label']}**\n")

    lines.append("## Top-line metrics\n")
    lines.append("| Metric | Baseline | Corrected | Δ |")
    lines.append("|---|---:|---:|---:|")
    for key, fmt, label in (
        ("lufs",        "{:+.2f}", "Integrated LUFS"),
        ("peak_db",     "{:+.2f}", "Peak (dBFS)"),
        ("rms_db",      "{:+.2f}", "RMS  (dBFS)"),
        ("crest_db",    "{:+.2f}", "Crest factor (dB)"),
        ("correlation", "{:+.3f}", "Stereo correlation"),
        ("dc",          "{:+.5f}", "DC offset"),
    ):
        ov, cv = orig[key], corrected[key]
        lines.append(f"| {label} | {fmt.format(ov)} | {fmt.format(cv)} | {(cv - ov):+.3f} |")
    lines.append("")

    lines.append("## Per-band RMS (dBFS)\n")
    lines.append("| Band | Baseline | Corrected | Δ |")
    lines.append("|---|---:|---:|---:|")
    # Mark the bands where Suno-Clean intends a reduction.
    intent_negative = {"mud       200–500 Hz", "sibilance   5–8 kHz", "shimmer  10–16 kHz"}
    for name in (n for n, _, _ in BANDS):
        ov, cv = orig["bands"][name], corrected["bands"][name]
        d = cv - ov
        marker = _delta_marker(d, good_negative=(name in intent_negative))
        lines.append(f"| {name} | {ov:+.2f} | {cv:+.2f} | {d:+.2f} {marker} |")
    lines.append("")

    lines.append("## Verdict\n")
    verdict: list[str] = []

    if abs(corrected["lufs"] - orig["lufs"]) <= 1.0:
        verdict.append("- ✓ Loudness preserved within ±1 LU.")
    else:
        verdict.append(f"- ⚠ Loudness drifted by {corrected['lufs'] - orig['lufs']:+.2f} LU.")

    if corrected["peak_db"] <= -1.0:
        verdict.append("- ✓ Peak headroom ≥ 1 dBFS — codec-safe.")
    elif corrected["peak_db"] > 0.0:
        verdict.append("- ⚠ Sample-peak clipped (≥ 0 dBFS). Check the chain.")
    else:
        verdict.append(f"- ⚠ Peak {corrected['peak_db']:+.2f} dBFS — under 1 dB headroom.")

    cf_drop = orig["crest_db"] - corrected["crest_db"]
    if cf_drop <= 2.0:
        verdict.append(f"- ✓ Crest factor drop {cf_drop:.2f} dB — dynamics preserved.")
    else:
        verdict.append(f"- ⚠ Crest factor dropped {cf_drop:.2f} dB — over-compressed?")

    mud_d  = corrected["bands"]["mud       200–500 Hz"] - orig["bands"]["mud       200–500 Hz"]
    shim_d = corrected["bands"]["shimmer  10–16 kHz"]   - orig["bands"]["shimmer  10–16 kHz"]
    if mud_d <= -1.0:
        verdict.append(f"- ✓ Mud band reduced {mud_d:+.2f} dB.")
    if shim_d <= -1.0:
        verdict.append(f"- ✓ Shimmer band reduced {shim_d:+.2f} dB.")

    corr_change = abs(corrected["correlation"] - orig["correlation"])
    if corr_change > 0.05:
        verdict.append(f"- ⚠ Stereo correlation drifted by {corr_change:+.3f} — image may have collapsed/widened.")

    lines.extend(verdict)
    return "\n".join(lines)


def main() -> int:
    p = argparse.ArgumentParser(
        description="Compare a TinyBooth-corrected mix against a baseline.",
    )
    sub = p.add_mutually_exclusive_group(required=True)
    sub.add_argument("--project", help="Path to a TinyBooth project folder (auto-baselines from tracks/).")
    sub.add_argument("--files", nargs=2, metavar=("BASELINE_WAV", "CORRECTED_WAV"),
                     help="Direct WAV-vs-WAV comparison.")
    p.add_argument("--corrected", help="Path to the corrected/exported WAV (required with --project).")
    p.add_argument("--out", help="Optional path to write the report; default is stdout.")
    args = p.parse_args()

    if args.project:
        if not args.corrected:
            p.error("--corrected is required with --project")
        proj = Path(args.project)
        corrected_path = Path(args.corrected)
        baseline, sr_b = make_raw_mix_from_project(proj)
        corrected_audio, sr_c = _load_audio(corrected_path)
        baseline_label = f"{proj.name} / tracks/ (raw sum)"
        corrected_label = corrected_path.name
    else:
        bp = Path(args.files[0])
        cp = Path(args.files[1])
        baseline, sr_b = _load_audio(bp)
        corrected_audio, sr_c = _load_audio(cp)
        baseline_label = bp.name
        corrected_label = cp.name

    if sr_b != sr_c:
        sys.exit(f"ERROR: sample-rate mismatch — baseline {sr_b} Hz, corrected {sr_c} Hz.")

    # Length-align by truncating to the shorter — fair when the corrected
    # mix may have been trimmed.
    n = min(baseline.shape[0], corrected_audio.shape[0])
    baseline = baseline[:n]
    corrected_audio = corrected_audio[:n]

    text = report(
        metrics(baseline, sr_b, baseline_label),
        metrics(corrected_audio, sr_c, corrected_label),
    )

    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
