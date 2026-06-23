# Sound-Visualization Engines: principle → physics → engine ⊳ TinyOutput

For each research item: **(1)** the underlying principle (the edict), **(2)** the
physics/mathematics it models (the governing law), **(3)** the advanced math
engine that runs that law against TinyBooth's output to produce the visual.

Companion to [sound-visualization-science.md](sound-visualization-science.md)
(the survey) and [sound-visualization-findings.json](sound-visualization-findings.json)
(raw structured findings). This document is the *implementation contract*.

---

## The TinyOutput contract

Everything a visualizer consumes is one of these, derived once per frame from the
bounced `mix_run` (or the live master tap) and shared across engines:

| Symbol | Meaning |
|---|---|
| `xL[n], xR[n]` | stereo f32 PCM at sample rate `SR` |
| `x[n] = ½(xL+xR)` | mono sum |
| `m[n]=½(L+R)`, `s[n]=½(L−R)` | mid / side |
| `X[m,k] = Σₙ x[n+mH] w[n] e^{−j2πkn/N}` | STFT (rustfft), hop `H`, window `w` (Hann), size `N` |
| `\|X[m,k]\|`, `φ[m,k]=∠X[m,k]` | magnitude, phase |
| `f[m]` | feature vector per frame: chroma(12), MFCC(13), centroid, flatness, flux |

`SR`, `N`, `H` are the engine's tuning knobs. Nothing below needs data the app
doesn't already produce.

---

## 1. Time-domain & phase-space scope

- **Principle.** A signal's identity lives in how its trajectory folds back on
  itself, not in its amplitude-vs-time trace.
- **Physics it models.** *State-space reconstruction.* Takens' embedding theorem:
  a single scalar observable of a dynamical system reconstructs an attractor
  diffeomorphic to the true state space, provided embedding dimension
  `m ≥ 2d+1` for an attractor of box-dimension `d`.
- **Engine ⊳ TinyOutput.** Input: a window of `x[n]` (mono) or `(xL,xR)` (stereo).
  Compute delay coordinates `v(n) = [x(n), x(n+τ), x(n+2τ)]`. Choose `τ` as the
  first minimum of the average mutual information `I(τ) = Σ p(x_n,x_{n+τ})
  log[p(x_n,x_{n+τ})/(p(x_n)p(x_{n+τ}))]`; choose `m` by false-nearest-neighbors.
  Render the projected orbit with a woscope Gaussian-beam line integral
  `F(p)=\frac{1}{2l}e^{−p_y²/2σ²}[erf(\frac{p_x}{√2σ})−erf(\frac{p_x−l}{√2σ})]`,
  additive-blended → a glowing closed orbit for periodic tone, a knot for chaos.

## 2. Fourier / spectral

- **Principle.** Any sound is a sum of sinusoids; the analysis window trades time
  resolution for frequency resolution.
- **Physics it models.** *Harmonic decomposition* under the Gabor–Heisenberg
  uncertainty limit `Δt · Δf ≥ 1/4π`.
- **Engine ⊳ TinyOutput.** Input: windowed frames of `x[n]`. Engine: STFT via
  rustfft, render `20·log₁₀|X[m,k]|` on a log₂-frequency axis through a
  perceptual LUT. The window length `N` is the live knob: drag it and a transient
  morphs from a vertical spike (small `N`) to a horizontal ridge (large `N`) —
  the uncertainty principle made interactive.

## 3. Advanced time-frequency (reassignment / synchrosqueezing)

- **Principle.** Phase tells you where energy truly sits; the magnitude
  spectrogram throws that information away and blurs.
- **Physics it models.** *Instantaneous frequency* `ω̂ = ∂φ/∂t` and *group delay*
  `t̂ = −∂φ/∂ω` — the local center of gravity of the time-frequency energy.
- **Engine ⊳ TinyOutput.** Compute three STFTs of `x[n]` with windows `h`, `t·h(t)`,
  `h'(t)`. Reassign each grain:
  `t̂ = t − Re{X_{th}X*/|X|²}`, `ω̂ = ω + Im{X_{dh}X*/|X|²}`,
  and deposit `|X|²` at `(t̂,ω̂)` instead of `(t,ω)`. Output: a razor-sharp
  scatter map. A/B toggle vs the plain spectrogram is the whole argument.

## 4. Psychoacoustics & perceptual scales

- **Principle.** Perception ≠ physics; the honest frequency axis and brightness
  scale are the cochlea's, not the FFT's.
- **Physics it models.** *Basilar-membrane mechanics* — frequency warping to the
  ERB/Bark scale `ERB(f)=24.7(4.37f/1000+1)`, and Stevens' loudness power law
  `loudness ∝ I^0.3` (sones).
- **Engine ⊳ TinyOutput.** Map `|X[m,k]|²` through an ERB/gammatone filterbank to
  specific loudness `N'(z)`; set brightness via the cube-root compression;
  overlay the spreading-function masking threshold. Render the warped spectrogram
  beside the linear one so they visibly disagree.

## 5. Music-theoretic structure

- **Principle.** Harmony and form are invariant structures hiding beneath timbre.
- **Physics it models.** *Octave equivalence* (the pitch helix): chroma is the
  quotient of log-frequency by the octave; musical form is a metric structure on
  the feature-trajectory.
- **Engine ⊳ TinyOutput.** Fold the spectrum to chroma `c_p = Σ_{k: pc(k)=p}|X[k]|`,
  `pc(k)=round(12·log₂(f_k/440)) mod 12`. Build the self-similarity matrix
  `S(i,j)=⟨c_i,c_j⟩/(|c_i||c_j|)`; convolve a Gaussian checkerboard kernel down
  the diagonal for the Foote novelty curve `N(i)=Σ K(l,m)S(i+l,i+m)`. Render the
  SSM (form) and, optionally, the Tonnetz walk (harmony).

## 6. Nonlinear dynamics & chaos

- **Principle.** A sound's degrees of freedom and whether it's periodic /
  quasiperiodic / chaotic are countable geometric facts.
- **Physics it models.** *Dissipative dynamical systems* — attractors, Poincaré
  return maps, and the correlation dimension `D₂` (Grassberger–Procaccia).
- **Engine ⊳ TinyOutput.** Delay-embed `x[n]` (engine #1). Take a Poincaré section
  on a hyperplane → first-return map; finite dots = period-n, a closed loop =
  torus/beating, dust = chaos. Build the recurrence matrix
  `R(i,j)=Θ(ε−|v_i−v_j|)`; estimate `D₂` from the correlation sum
  `C(ε) ~ ε^{D₂}`. Render the return map / recurrence plot.

## 7. Physical waves & cymatics

- **Principle.** A frequency is not just a number — it has a *spatial shape*, the
  eigenmode it would excite in a physical body.
- **Physics it models.** *Kirchhoff–Love plate vibration* — the biharmonic
  eigenproblem `D∇⁴w = ρh ω² w` with free edges; nodal lines are the zeros of an
  eigenmode; driven response is the modal sum
  `w = Σ_j a_j φ_j`, `a_j = F_j/(ω_j² − ω² + iγω)`.
- **Engine ⊳ TinyOutput.** Precompute the plate eigenmodes `φ_j` once (offline
  FEM/spectral solve). Per frame, drive modal amplitudes from the live spectrum:
  `a_j = Σ_k |X[k]| · resonance(ω_k, ω_j)`; form the displacement field
  `w(x,y)=Σ a_j φ_j(x,y)`; settle sand toward `∇|w| → 0` (nodal lines). Sweep
  pitch → patterns bloom and morph at resonances.

## 8. Color science & perceptual mapping

- **Principle.** The eye reads order through luminance; hue is for category, not
  magnitude — and the jet/rainbow colormap lies.
- **Physics it models.** *Human contrast sensitivity* — luminance-dominant at the
  high spatial frequency of spectrogram detail; CAM02-UCS perceptual uniformity
  (`dJ'/dt = const`).
- **Engine ⊳ TinyOutput.** Map dB magnitude → perceptual lightness via a
  viridis/magma 256-LUT (or `L` in Oklab from sones); reserve hue for a genuinely
  orthogonal variable (instantaneous frequency). Render the same spectrogram
  three ways — jet (annotated with its sign-flipping `dY/dt` false edges),
  grayscale-luminance, viridis — so the lie sits next to the fix.

## 9. Dimensionality reduction & embeddings

- **Principle.** A sound's meaningful axes can be *discovered by the data*, not
  imposed by the analyst.
- **Physics it models.** *The manifold hypothesis* — high-dimensional feature
  trajectories lie near a low-dimensional manifold; preserve local neighborhoods
  (UMAP's fuzzy simplicial sets + cross-entropy; SOM's competitive Hebbian update).
- **Engine ⊳ TinyOutput.** Per-frame feature vector `f[m]=[MFCC, chroma, centroid,
  flatness, …]`. Stream into an online SOM (best-matching-unit update
  `w_b ← w_b + η·h(·)·(f − w_b)`) or a parametric-UMAP encoder; project to 2-D.
  Render the self-organizing "timbre galaxy" with the playhead as a comet —
  axes that have no a-priori label, discovered live.

## 10. Spatial / stereo / ambisonics

- **Principle.** Sound has a *where*, encodable as directions / spherical
  harmonics, and stereo defects are directional failures.
- **Physics it models.** *Acoustic intensity* (active power flow) `I = p·u`;
  ambisonic encoding onto the spherical-harmonic basis `Y_l^m(θ,φ)`;
  diffuseness `ψ = 1 − |⟨I⟩| / ⟨|I|⟩`.
- **Engine ⊳ TinyOutput.** From stereo build a pseudo-B-format (`W=m`, `Y=s`).
  Per band, intensity from the cross-spectrum `Re{W*·Y}` → direction-of-arrival;
  inter-channel coherence `γ²=|S_LR|²/(S_LL·S_RR)`. Render the DOA arrow field +
  a per-band coherence ring; anti-phase bands (`γ→` with negative correlation)
  flare red — mono-incompatibility at a glance.

## 11. Generative / particle / fluid

- **Principle.** Sound's energy, when it drives a simulation's forces, becomes
  emergent form — beauty as a direct readout of dynamics.
- **Physics it models.** *Incompressible Navier–Stokes*
  `∂u/∂t + (u·∇)u = −∇p/ρ + ν∇²u`, `∇·u = 0` (Stam's stable fluids), or
  curl-noise flow, or Gray–Scott reaction-diffusion.
- **Engine ⊳ TinyOutput.** Band energies → inflow forcing; onset (spectral flux
  `SF=Σ_k H(|X_{m,k}|−|X_{m−1,k}|)`) → vortex injection (a curl impulse); spectral
  centroid → particle hue. Advect particles semi-Lagrangian through the field.
  Render the living particle flow.

## 12. Adaptive / autonomous display

- **Principle.** Every free parameter should be a function of the signal's own
  running statistics, not a hardcoded constant.
- **Physics it models.** *Robust online estimation* — order statistics (the P²
  running-percentile algorithm), adaptive windowing, exponential forgetting.
- **Engine ⊳ TinyOutput.** Maintain running `P5`/`P99` of dB via P² → colormap
  floor/ceiling; per-band adaptive window `N_b ∝ 1/f_b`; frame-rate-correct EMA
  `α = Δt/(τ+Δt)` (read `Δt` from the frame clock). Render the self-ranging
  spectrogram with the percentile gauges exposed so you watch it dimension itself.

## 13. Information-theoretic & insight

- **Principle.** Insight = mapping an inaudible structure to a preattentive
  channel so a defect pops in under 200 ms.
- **Physics it models.** *Shannon information* — spectral entropy
  `H = −Σ_k p_k log p_k` (`p_k=|X_k|²/Σ|X|²`); crest factor `peak/RMS`;
  BS.1770 K-weighted loudness.
- **Engine ⊳ TinyOutput.** Compute normalized entropy `H̃=H/log K`, flatness
  `SFM = geomean/mean`, crest from peak vs RMS envelopes, integrated LUFS via the
  K-filter + gating. Render "health" ribbons whose thresholds are tuned so
  over-compression / monotony / mono-death jump out before you read a number.

## 14. Rendering & realtime systems

- **Principle.** The substrate — ring-buffer textures, LUTs, log-f resampling,
  frame-rate-correct smoothing — is what makes everything else live.
- **Models.** Not physics but *computational geometry & signal-rate budgeting*:
  the ring buffer as modular addressing, texture sampling as resampling
  (sinc/bilinear), EMA as a one-pole IIR.
- **Engine ⊳ TinyOutput.** Write each STFT column into an `R32F` texture ring
  (`queue.write_texture`, modulo-1.0 scroll in the shader → O(1) per frame
  regardless of history). Log-f resample via a precomputed bin→pixel LUT with an
  antialiased gather. Render the scrolling spectrogram + an "engine cam" HUD
  (write-column highlighted, FPS/cost readout).

## 15. Topological data analysis (SW1PerS)

- **Principle.** Periodicity is a *hole* in the right space; "is this a note?" is
  a topological question.
- **Physics it models.** *Persistent homology* — a sliding-window embedding of a
  periodic signal traces a loop, i.e. a nontrivial first homology class `H₁`;
  periodicity = the persistence (lifetime) of the longest `H₁` bar.
- **Engine ⊳ TinyOutput.** Sliding-window embed
  `SW_{M,τ}x(t)=[x(t),x(t+τ),…,x(t+Mτ)]`, mean-center and sphere-normalize, build a
  Vietoris–Rips complex, compute `H₁` persistence (Rust `lophat`/`teia`); score
  `s = 1 − mp/√3`. Render the point cloud + the persistence barcode — a clean
  tone shows one long bar, a tritone grows an extra hole.

## 16. Optical-flow / motion-field spectrogram

- **Principle.** Time-frequency energy doesn't merely sit — it *moves*; vibrato
  and glissando are velocity fields.
- **Physics it models.** *Optical flow* under brightness constancy
  `I_t + ∇I·v = 0`; here the field is analytic from the reassignment derivatives
  `v = (−∂φ/∂ω, ∂φ/∂t − ω)`.
- **Engine ⊳ TinyOutput.** Take the same phase-derivative field as engine #3;
  integrate Line-Integral-Convolution noise along its streamlines
  `D(r)=∫ k(s) N(σ_r(s)) ds`; color by Middlebury HSV flow direction/magnitude.
  Render the spectrogram as a flowing fluid — motion, not static energy.

## 17. Differentiable / neural & feature-inversion

- **Principle.** A model *hears* features; you can see what it attends to and walk
  its latent space.
- **Physics it models.** *Gradient attribution* — Grad-CAM
  (`α_k^c = GAP(∂y^c/∂A_k)`) and Integrated Gradients (path integral of input
  gradients with the completeness axiom `Σ attributions = f(x) − f(x')`).
- **Engine ⊳ TinyOutput.** Feed log-mel of `x[n]` to a small bundled CNN
  (`candle`/`ort`); backprop the class score to the input bins; overlay the
  saliency heatmap on the spectrogram ("what the model hears"). Slerp the latent
  for a morph. Render the saliency overlay + latent walk.

## 18. Hyperbolic / non-Euclidean & graph layouts

- **Principle.** Musical/timbral structure is hierarchical, and hierarchies fit
  naturally in negatively-curved space (exponential room for the branches).
- **Physics it models.** *Hyperbolic geometry* — the Poincaré disk (curvature −1);
  trees embed with arbitrarily low distortion (Sarkar); isometries are Möbius
  transforms `z ↦ e^{iα}(z−p)/(1−p̄z)`.
- **Engine ⊳ TinyOutput.** Build a similarity graph of sections/grains from the
  SSM (engine #5); hierarchical-cluster it; Sarkar-embed the tree into the disk;
  Möbius pan/zoom for infinite focus-plus-context. Render the song-form browser.

---

## How they compose

The engines form a dependency DAG over the TinyOutput contract, so most reuse the
same STFT:

- **STFT (#2)** feeds #3, #4, #5, #10, #13, #16, #17 and the foundation render bus (#14).
- **Reassignment (#3)** is reused verbatim by the optical-flow field (#16).
- **Chroma/SSM (#5)** feeds the hyperbolic browser (#18).
- **Delay embedding (#1)** feeds the chaos maps (#6) and shares mathematics with TDA (#15).
- **Features (#9, #13)** are computed once per frame and shared.

Build order is therefore: the render bus + STFT + feature engine first, then each
engine is a thin consumer. See the tiered roadmap in
[sound-visualization-science.md](sound-visualization-science.md) §6.
