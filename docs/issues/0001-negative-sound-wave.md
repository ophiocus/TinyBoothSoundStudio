# Issue #1 — Negative Sound Wave — AI-artifact suppression via subtractive common-mode rejection

> Mirrored locally from https://github.com/ophiocus/TinyBoothSoundStudio/issues/1
> Labels: `enhancement` · State: **OPEN** · Author: ophiocus (Carlos Santana) · Created: 2026-07-12

---

# Feature Request — TinyBooth · **Negative Sound Wave** (AI-artifact suppression)

> **Product:** TinyBooth (Sound Studio) · **Type:** correction item / DSP feature · **Status:** proposed
> **Origin:** operator seat (`D:\tecnocratica`) → routes to dev via `ophiocus/tecnocratica-ops` issue.
> **One-liner:** derive an *anti-artifact* signal from a track — the "negative wave" — that, subtracted,
> suppresses the generative (Suno/AI) residue and restores each track's separation and clarity.

---

## 1 · Problem (the "shrink-wrap" / underwater effect)
AI music (Suno et al.) is generated **from noise** by a diffusion/transformer decoder. The decoder never
perfectly reconstructs phase and fine time–frequency structure, so it leaves a **correlated residue baked
into every output** — an allegorical "shrink-wrap." Symptoms the operator hears:
- **"Underwatery"** — phase smearing / warble from imperfect vocoder phase reconstruction.
- **Loss of "singularity"** — when a song is split into stems, each stem carries **shared generative haze**
  and bleed from the others, so the tracks don't feel *distinct*; they sound fused, flat, smeared together.

**Key realization that makes this solvable:** these artifacts are **statistical outliers**, not music.
They are (a) **common across stems** (shared decoder residue), (b) **phase-incoherent** with real musical
partials, and (c) **spectrally diffuse** (a haze floor, not tonal content). Anything that satisfies all
three is almost certainly artifact — and can be estimated and subtracted without touching the music.

## 2 · Concept — the Negative Sound Wave
Estimate the artifact content Â of a track, invert it, and mix it back so it **destructively interferes**
with the residue (noise-cancellation logic, but targeted at *AI* artifacts rather than ambient noise).
The estimated artifact, phase-inverted, **is** the "negative wave" — and TinyBooth should expose it as a
**soloable, audible signal** so the operator can *hear what is being removed* and confirm it's residue,
not music, before committing. A single **Depth** knob controls how much is subtracted.

## 3 · How to find the outliers — the sound logic & math
Work in the STFT domain. Let `X(t,f)` be a track's complex spectrogram (magnitude `|X|`, phase `∠X`).
Estimate an artifact magnitude profile `Â(t,f)` from **as many of these estimators as apply**, then combine:

**(A) Cross-stem common-mode rejection — the flagship (needs stems).**
The shared decoder residue is the part of each stem *predictable from the others* — i.e. the **common mode**.
With `K` stems `X_k(t,f)`, estimate the common component per bin (robust: `C = median_k |X_k|`, or the
dominant cross-stem singular vector via a per-bin rank-1 SVD across the stem matrix), **gated by cross-stem
phase coherence** (only bins where stems agree count as shared). Subtract `C` from each stem. This is a
**differential amplifier rejecting common-mode noise** — it *decorrelates the stems*, which is literally the
"restore singularity" the operator wants: less shared haze ⇒ more separation between tracks.

**(B) Phase-incoherence mask (single-track).** Real partials evolve with coherent instantaneous frequency;
vocoder warble does not. Compute a per-bin phase-coherence / group-delay stability metric; **low-coherence
bins are artifact** → include in `Â`. Targets the "underwatery" smear directly.

**(C) Minimum-statistics noise floor (single-track).** Track the spectral floor over time (Martin's minimum
statistics). The persistent floor beneath the music is the diffuse generative haze.

**(D) HPSS residual (single-track).** Median-filter the spectrogram along time (→ harmonic) and along
frequency (→ percussive); the **leftover third component** (neither stable-tonal nor transient) is largely
the AI residual. Feed it into `Â`.

**Combine → subtract → reconstruct:**
```
Â(t,f)   = weighted blend of the estimators above (coherence-gated)
|Ŝ|      = max( |X| − α·Â ,  β·|X| )         # α = Depth (over-subtraction); β = spectral floor
Ŝ        = |Ŝ| · e^{j·∠X}   →  iSTFT          # cleaned track (keep original phase)
negWave  = iSTFT( α·Â · e^{j·∠X} )            # the audible "negative wave": Ŝ = X − negWave
```

**Anti-"musical-noise" safeguard (mandatory).** Naïve spectral subtraction introduces warbly "musical
noise" — worse than the disease. Use a **decision-directed / log-MMSE (Ephraim–Malah) gain** instead of raw
subtraction, plus temporal smoothing and the spectral floor `β`, so removal is smooth and gated by an a-priori
SNR estimate rather than punching holes. This is the difference between a real feature and a toy.

## 4 · UX / controls
- **Negative Wave: solo / mute / gain** — hear the residue being removed; A/B the cleaned track.
- **Depth** (`α`) — global amount. **Floor** (`β`) — protect against over-scrubbing.
- **Estimator mix** — advanced: weight common-mode vs phase vs floor vs HPSS (sensible defaults hidden).
- **Presets** — "Suno stem," "Suno full mix," "gentle de-haze," "aggressive separate."
- **Null-test button** — invert cleaned against original to audition exactly what changed.

## 5 · Phasing
- **MVP:** single-track de-haze — estimators (B)+(C)+(D), log-MMSE subtraction, **soloable negative wave**,
  Depth + Floor knobs, one "Suno" preset. Ships the core value on any single track.
- **Full:** **multi-stem common-mode rejection (A)** with coherence gating (the "restore singularity"
  headline), estimator-mix panel, per-source presets, batch across a session's stems.

## 6 · Non-goals / risks
- **Not** a general denoiser, mastering chain, or upsampler; it removes residue, it does **not** hallucinate
  back lost detail.
- **Risk:** stripping *intended* airy/ambient/reverb content — mitigated by the audible negative wave +
  conservative defaults + Floor. **Risk:** musical noise — mitigated by log-MMSE (see §3). **Risk:** CPU on
  long stems — offline/render-time acceptable; realtime is a stretch goal.

## 7 · Acceptance criteria
- Negative wave is **soloable** and, at tuned defaults, contains **no clearly audible musical content** on a
  reference set (it's residue).
- **Measurable separation gain:** inter-stem correlation **drops** after common-mode rejection; noise-floor
  **spectral flatness improves**; A/B null test isolates only residue.
- **No audible musical noise** introduced on the reference set at default Depth.
- Cleaned tracks judged **clearer / less "underwatery"** in blind A/B by the operator.

---

## 🇨🇴 Resumen
**Onda Sonora Negativa** para TinyBooth: deriva de una pista una señal **anti-artefacto** (la "onda negativa")
que, al **restarse**, suprime el residuo generativo de la IA (Suno) — ese "shrink-wrap" que deja las pistas
**"submarinas"** y **fundidas entre sí** (sin singularidad). **La clave:** los artefactos son **outliers
estadísticos** — (a) **comunes entre stems**, (b) **incoherentes en fase** con los parciales musicales, y
(c) **difusos** (un piso de ruido, no tonos). Lo que cumple los tres es artefacto y se puede estimar y restar
sin tocar la música. **Método (dominio STFT):** el estimador estrella es el **rechazo de modo común entre
stems** (un amplificador diferencial: quita lo compartido ⇒ **decorrelaciona las pistas ⇒ recupera la
singularidad**), apoyado por máscara de **incoherencia de fase**, **piso de ruido de mínima estadística** y
**residuo HPSS**. Se resta con ganancia **log-MMSE (Ephraim–Malah)** — no resta cruda — para evitar el "ruido
musical" warbly. **UX:** la onda negativa es **audible/soloable** (oír qué se quita antes de aplicar) + perilla
de **Profundidad** y **Piso** + presets Suno. **MVP:** de-haze de pista única; **Full:** rechazo de modo común
multi-stem. **No es** un denoiser general ni reconstruye detalle perdido.

