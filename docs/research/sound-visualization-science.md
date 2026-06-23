# The Science & Math of Sound Visualization
### Toward breathtaking, self-dimensioning, insight-generating visuals

---

## 1. The Opening Map: From What the Signal *Is* to What the Sound *Means*

Sound visualization is a ladder of representations, and every rung trades raw fidelity for meaning. The ladder runs from the literal shape of the pressure wave up to learned manifolds that encode timbral *concepts*. Each level answers a different question.

| Level | The question it answers | Core math |
|---|---|---|
| **Time-domain / phase-space** | *What is the instantaneous geometry of the signal?* | $(L,R)$ scatter, Takens delay $v(t)=[x(t),x(t+\tau),\dots]$ |
| **Spectral** | *What frequencies are present, and how loud?* | $X(m,k)=\sum_n x(n{+}mH)w(n)e^{-j2\pi kn/N}$ |
| **Time-frequency** | *How does the spectrum evolve, and where exactly is each energy grain?* | CQT, CWT, reassignment $\hat t,\hat\omega$, synchrosqueezing |
| **Perceptual** | *What would a human actually hear?* | ERB/Bark/mel warps, sones, LUFS, masking thresholds |
| **Music-theoretic / structural** | *What is the harmony, the key, the form?* | chroma fold, Tonnetz torus, self-similarity matrix |
| **Nonlinear / topological** | *How many degrees of freedom; is it periodic, chaotic, or noisy?* | recurrence plots, correlation dimension $D_2$, persistent homology |
| **Physical** | *What spatial structure would this sound excite?* | Chladni $D\nabla^4 w=\rho h\omega^2 w$, Faraday Mathieu instability |
| **Spatial** | *Where in space does the sound live?* | mid/side rotation, spherical harmonics $Y_l^m$, DirAC intensity |
| **Generative** | *What does the sound's energy *become* when it drives a simulation?* | curl-noise $\nabla\times\psi$, Gray-Scott RD, Stable Fluids |
| **Learned / embedded** | *What are the sound's intrinsic perceptual coordinates?* | PCA eigen-axes, UMAP/t-SNE, VAE latents (RAVE), saliency gradients |

The progression is not merely "more processing." It is a steady migration of the *axis of meaning*: a waveform's axes are volts and seconds; a spectrogram's are hertz and decibels; a chromagram's are the twelve pitch classes; a self-similarity matrix's are *time vs. time*; a UMAP plot's axes are **discovered by the data itself**. By the top of the ladder, the display has stopped showing you the signal and started showing you the **structure of the sound's meaning** — and, crucially, has begun **choosing its own coordinate system**. That last property — autonomous dimensioning — is the connective tissue of this whole document.

---

## 2. The Ranked Shortlist: ~15 Techniques That Score Highest on Breathtaking × Insight × Autonomy

Ranked by the combination of visual impact, revealed structure, and self-scaling intelligence. Each entry follows a fixed substructure.

---

### 2.1 Reassigned Spectrogram (Time-Frequency Reassignment) — *the sharpening operator*

- **Math.** Compute three STFTs with windows $h$, $T_h(t){=}t\,h(t)$, $D_h{=}h'(t)$. Relocate each grain:
$$\hat t = t - \mathrm{Re}\!\left\{\tfrac{X_{Th}X^*}{|X|^2}\right\},\qquad \hat\omega = \omega + \mathrm{Im}\!\left\{\tfrac{X_{Dh}X^*}{|X|^2}\right\}$$
deposit $|X|^2$ at $(\hat t,\hat\omega)$ instead of $(t,\omega)$. These are the local group delay $\partial\phi/\partial\omega$ and channelized instantaneous frequency $\partial\phi/\partial t$ — the phase the magnitude spectrogram throws away.
- **Hidden structure.** True instantaneous-frequency trajectories: micro-vibrato, glissando curvature, exact onset times, partials closer than the main-lobe width.
- **Why stunning.** A soft watercolor smear snaps into an etched line drawing — same data, hyper-crystalline. The before/after focus toggle is genuinely startling.
- **Autonomy.** Every coefficient computes its own corrected location; resolution is allocated by the signal, not a fixed grid. A concentration test auto-prunes noise points so density tracks tonality.
- **Realtime verdict.** ✅ 3× FFT/frame + scatter-accumulate; easily realtime to N≤4096 with `rustfft`. GPU point-splatting in wgpu for large grids.

### 2.2 Synchrosqueezed CQT — *invertible razor ridges that auto-tile by octave*

- **Math.** Estimate IF from CWT phase, $\omega_s(a,b)=\mathrm{Re}\!\big(\partial_b W_s/(i2\pi W_s)\big)$; squeeze energy in **frequency only** onto $\omega_l$: $T_s(\omega_l,b)=\int_{A(\omega_l)}W_s(a,b)\,a^{-3/2}\,da$. On a CQT base ($f_k=f_{\min}2^{k/B}$), octaves become equal stripes.
- **Hidden structure.** Individual oscillatory modes (vibrato + tremolo, beating partials, formant tracks) collapse to clean ridges you can **integrate to resynthesize** — paint a ridge, solo that mode.
- **Why stunning.** Vector-graphic crispness plus interactive "peel apart the components" magic.
- **Autonomy.** The $\omega_s$ map is derived from the data's own phase; ridge-detection auto-discovers the number of modes — the representation reports its own intrinsic dimensionality.
- **Realtime verdict.** ⚠️ CWT + b-derivative + frequency-only scatter; realtime at moderate scale counts on CPU, wgpu for large.

### 2.3 Constant-Q / Log-Frequency Spectrogram — *music looks like music*

- **Math.** $f_k=f_{\min}2^{k/B}$, constant $Q=1/(2^{1/B}-1)$, per-bin window $N_k=Q\,f_s/f_k$. Efficient via Brown–Puckette sparse spectral kernel: one big FFT + sparse matvec.
- **Hidden structure.** Transposition = vertical shift; a chord is a fixed shape; harmonics sit at $\log_2 n$ offsets. Bass gets long windows (resolves adjacent low notes); treble stays crisp.
- **Why stunning.** Octaves stack as evenly-spaced glowing rails; a chromatic run climbs a perfect staircase.
- **Autonomy.** Resolution is allocated *per bin by frequency* — the time-frequency tradeoff varies continuously across the axis with no global window. $B$ scales to pixel height; $f_{\min}/f_{\max}$ snap to the active band.
- **Realtime verdict.** ✅ Build sparse kernel once at startup; runtime is one FFT + sparse matmul.

### 2.4 Self-Similarity Matrix + Foote Checkerboard Novelty — *the song's self-portrait*

- **Math.** $S(n,m)=\langle x_n,x_m\rangle$ (cosine on chroma/MFCC). Slide a Gaussian-tapered checkerboard kernel $K(l,m)=\mathrm{sign}(l)\mathrm{sign}(m)e^{-(l^2+m^2)/2\sigma^2}$ down the diagonal: $N(i)=\sum_{l,m}K(l,m)S(i{+}l,i{+}m)$. Peaks = section boundaries.
- **Hidden structure.** Macro-FORM: verse/chorus/bridge as glowing blocks, repeats as off-diagonal stripes, the exact bar where the drop hits.
- **Why stunning.** A symmetric, fractal-feeling tapestry with shimmering diagonals — the single highest-insight image in MIR, and gallery-beautiful.
- **Autonomy.** Auto-sizes to song length; kernel width $L$ sweeps to build hierarchical segmentation; transposition-invariant SSM (min over 12 cyclic shifts) auto-handles key-shifted repeats.
- **Realtime verdict.** ✅ $S=X^TX$ one matmul; stream a column/frame. Prime wgpu candidate (R32F texture + compute novelty).

### 2.5 Glowing-Beam XY Oscilloscope (woscope Gaussian line-integral) — *highest wow-per-line-of-code*

- **Math.** Each segment rendered as the analytic line integral of a 2-D Gaussian beam:
$$F(p)=\tfrac{1}{2l}e^{-p_y^2/2\sigma^2}\big[\mathrm{erf}(\tfrac{p_x}{\sqrt2\sigma})-\mathrm{erf}(\tfrac{p_x-l}{\sqrt2\sigma})\big]$$
Brightness $\propto 1/l$ — slow beam glows brighter (real CRT behavior). Additive blend accumulates overlaps.
- **Hidden structure.** The instantaneous stereo trajectory; phase, transient asymmetry, DC offsets invisible in a waveform.
- **Why stunning.** Real oscilloscope-music glow, anti-aliased for free by the `erf`, motion blur baked into the math. Looks like a physical instrument.
- **Autonomy.** $\sigma$ scales with DPI; XY extent auto-fits to a slow-release running peak so quiet passages don't shrink to a dot; persistence time-constant can track tempo.
- **Realtime verdict.** ✅ CPU builds instance buffer; wgpu draws instanced quads w/ additive blend + `erf` fragment shader. ~1–2k segments/frame trivial.

### 2.6 Delay-Embedding Phase Portrait with Auto-$(\tau,m)$ — *the soul of the sound, self-dimensioning*

- **Math.** $v(t)=[x(t),x(t{+}\tau),\dots,x(t{+}(m{-}1)\tau)]$. **$\tau$** = first minimum of average mutual information $I(\tau)=\sum p_{ij}\log_2\!\frac{p_{ij}}{p_ip_j}$ (Fraser–Swinney). **$m$** = false-nearest-neighbors threshold. Takens guarantees a diffeomorphic embedding for $m>2d_A$.
- **Hidden structure.** Number of independent oscillation modes; period-doubling/chaos in growls, vocal fry, distortion; tonal-vs-noisy as thin-loop-vs-space-filling-fuzz.
- **Why stunning.** A flute traces a clean glowing ellipse, an overdriven guitar a tangled knot, a snare an exploding cloud — calligraphy of periodicity.
- **Autonomy.** **The standout.** $\tau$ and $m$ are estimated from the signal itself, frame to frame — the scope literally re-dimensions and re-scales to whatever is playing.
- **Realtime verdict.** ✅ Embedding is a strided gather; AMI/FNN on a downsampled window every ~100 ms; m=3 cloud auto-rotating on wgpu.

### 2.7 DirAC / Active-Intensity Soundfield Vector Map — *wind-map of the room*

- **Math.** From B-format $W,X,Y,Z$: intensity $I=\mathrm{Re}\{W^*[X,Y,Z]\}$, DOA opposite the flow ($\mathrm{az}=\mathrm{atan2}(-I_y,-I_x)$), diffuseness $\psi=1-\|\langle I\rangle\|/(c\langle E\rangle)$.
- **Hidden structure.** The direction every frequency arrives from; direct sound vs. reverberant wash, per band.
- **Why stunning.** A swirling field of arrows, each pointing where its sound lives, fading to a diffuse cloud as reverb builds — a flow-viz of the soundfield.
- **Autonomy.** Arrow length auto-scales to per-bin energy, color to diffuseness, count auto-thins to significant bins; degrades to 2-D azimuth for plain stereo (W=mid, Y=side).
- **Realtime verdict.** ✅ STFT/channel + per-bin arithmetic; instanced arrows. ⚠️ True 3-D needs B-format input.

### 2.8 Tonnetz Lattice / Harte 6-D Tonal Centroid — *harmony as geometric flow*

- **Math.** Tonnetz: pitch class at lattice $7a+4b \bmod 12$; major triad = up-triangle, minor = down, neo-Riemannian P/L/R = edge-flips sharing 2 vertices. Harte: project chroma to a 6-D hypertorus via three circles (fifths, minor-thirds, major-thirds), $\zeta=\Phi c/\|c\|_1$; HCDF $=\|\zeta(n{+}1)-\zeta(n{-}1)\|$ peaks at chord changes.
- **Hidden structure.** Voice-leading parsimony — "distant" chords are one edge-flip apart; harmonic *rhythm* separated from surface rhythm.
- **Why stunning.** A living crystal lattice; a triad triangle pivoting edge-to-edge on a slowly rotating torus.
- **Autonomy.** Active triad = argmax over 24 templates against chroma; $L_1$ normalization makes the centroid loudness-invariant; online PCA auto-projects the 6-D point to display axes the music actually uses.
- **Realtime verdict.** ✅ $\Phi$ is a constant 6×12 matvec; HCDF a diff+norm. 2-D pure egui; torus on wgpu.

### 2.9 SW1PerS Persistence Barcode (Topological Periodicity) — *coordinate-free "is this a note?"*

- **Math.** Sphere-normalize sliding windows, compute Vietoris-Rips $H_1$ persistence, score $s=1-\mathrm{mp}/\sqrt3$ where $\mathrm{mp}=\max_j(d_j-b_j)$. Quasiperiodic chords fill a $d$-torus ($b_1{=}d$, $b_2{=}\binom{d}{2}$): **a tritone literally grows an extra hole.**
- **Hidden structure.** Periodicity independent of waveform shape; consonance/dissonance as *genus*; damping-robust (sphere norm).
- **Why stunning.** One long $H_1$ bar dwarfs a litter of noise bars and *breathes* as a note stabilizes and decays — you watch a hole in the geometry of sound open and close.
- **Autonomy.** **Apex of self-scaling:** persistence is scale-free, so $\varepsilon$ is *read off* the diagram (most persistent bar); window length self-tunes to ~one period; torus dimension = #persistent $H_1$ bars = #independent pitches, *discovered not assumed*.
- **Realtime verdict.** ⚠️ Subsample to N≈200–400; Ripser-style cohomology+clearing (Rust `lophat`/`teia`); ~1–5 Hz barcode refresh — a "note-quality" meter, not 60fps.

### 2.10 Rainbowgram (Magnitude→Luminance, Instantaneous-Frequency→Hue) — *gorgeous AND informative*

- **Math.** $L \propto 20\log_{10}|X|$; hue from per-bin phase advance $\Delta\phi=\mathrm{wrap}(\phi_t-\phi_{t-1}-2\pi fH/f_s)$, $\mathrm{IF}=f+\Delta\phi f_s/(2\pi H)$, mapped through a **cyclic** colormap (phase is $S^1$).
- **Hidden structure.** Two magnitude-identical spectrograms that *sound* different because of phase; pitch drift, inharmonicity, partial-locking below bin resolution.
- **Why stunning.** Glowing harmonic filaments tinted by micro-frequency, blooming into literal rainbows on vibrato.
- **Autonomy.** Hue scale normalizes IF deviation by local bin width; magnitude→L uses per-clip dB percentile AGC; CQT base self-fits the register.
- **Realtime verdict.** ✅ One subtract+wrap/bin/hop on top of `rustfft`. Needs a Kovesi-cyclic / iso-luminant Oklch hue ring.

### 2.11 Audio-Forced Strange Attractor (Lorenz/Rössler/Chua) — *bifurcation you can see*

- **Math.** Drive the ODE's parameters from features: $\rho(t)=\rho_0+k\cdot\mathrm{RMS}$, band energies → $(\sigma,\rho,\beta)$, onsets → forcing splats; integrate RK4. Pushing $\rho$ past the period-doubling cascade blossoms one loop into the full butterfly.
- **Hidden structure.** The dynamical "temperature" of the sound — quiet = tidy limit cycle, loud/bright = visible bifurcation into chaos.
- **Why stunning.** A continuously-redrawn 3-D sculpture that breathes, sprouts wings, and never repeats.
- **Autonomy.** Features normalized by running percentile auto-traverse the interesting bifurcation interval; the system can auto-select *which* attractor from the measured FNN/embedding dimension.
- **Realtime verdict.** ✅ 3 state vars, RK4 ~40 flops/step — nothing. The app already has basic Lorenz; the upgrade is audio→parameter mapping + auto-ranging.

### 2.12 Driven Chladni / Cymatics (Lorentzian Modal Superposition) — *causal physics of the sound*

- **Math.** Solve the biharmonic eigenproblem $D\nabla^4\phi_n=\rho h\omega_n^2\phi_n$ once; at runtime the live spectrum drives a Lorentzian-weighted modal sum
$$w(x)=\sum_f A(f)\sum_n \frac{\phi_n(x)\phi_n(x_0)}{\omega_n^2-(2\pi f)^2+2i\zeta_n\omega_n(2\pi f)}$$
Near resonance the denominator → $2i\zeta_n\omega_n^2$ and that mode blooms. Sand grains drift down $-\nabla|w|^2$ onto nodes.
- **Hidden structure.** Which physical modes a sound *excites*, beating between near-degenerate partials, Q-factor as how cleanly one pattern dominates.
- **Why stunning.** Nodal lines snap into crystalline symmetry then dissolve into shimmering interference; 100k sand particles cascade into pattern.
- **Autonomy.** Eigenbasis is fixed; the FFT magnitude vector *is* the input that auto-selects modal content, wavelength, and symmetry — zero tuning. Keep N modes by cumulative-energy threshold.
- **Realtime verdict.** ✅ Eigensolve offline (`faer`/`nalgebra`); runtime is N Lorentzian-weighted adds/pixel — a wgpu fragment shader. The app already has basic Chladni; this is the physically-honest free-edge upgrade.

### 2.13 UMAP / SOM of Timbre Features — *the galaxy that discovers its own axes*

- **Math.** UMAP: fuzzy k-NN graph $w_{ij}$ with per-point bandwidth $\sigma_i$, low-dim kernel $q_{ij}=1/(1+a\|y_i-y_j\|^{2b})$, optimize cross-entropy. SOM: BMU $u=\arg\min_v\|x-w_v\|$, update $w_v{+}{=}\theta(u,v)\alpha(x{-}w_v)$, U-matrix shows cluster ridges.
- **Hidden structure.** Cluster topology of a corpus — kicks vs. hats, vowels vs. fricatives — manifold structure no spectrogram exposes.
- **Why stunning.** The UMAP "galaxy" of grain-points self-organizing into filaments; the SOM U-matrix as an alien topographic relief map.
- **Autonomy.** $\sigma_i$ is solved per-point (adaptive local scale); SOM neighborhood $\sigma(s)$ shrinks global→local automatically and discovers cluster boundaries.
- **Realtime verdict.** SOM ✅ (online, 30×30 grid streams). UMAP ⚠️ batch — precompute offline, or Parametric UMAP encoder (tiny MLP via `candle`) for live O(1) projection.

### 2.14 Grad-CAM / Saliency Overlay — *what the model hears*

- **Math.** $\alpha_k^c=\frac1Z\sum_{ij}\partial y^c/\partial A^k_{ij}$, $L^c=\mathrm{ReLU}(\sum_k\alpha_k^c A^k)$, upsampled over the spectrogram. Integrated Gradients for per-bin signed attribution with completeness $\sum_i \mathrm{IG}_i=F(x)-F(x')$.
- **Hidden structure.** *Model reasoning, not signal* — which formant identifies a vowel, which 8 kHz artifact betrays a deepfake. Provably absent from any signal-only transform.
- **Why stunning.** A warm attention cloud floating over the cool spectrogram, migrating as the predicted class changes — an MRI of listening.
- **Autonomy.** Heatmap self-normalizes per frame; chosen layer's receptive field auto-sets granularity; model-agnostic (works across CNN/transformer).
- **Realtime verdict.** ⚠️ Needs a trained CNN + autograd runtime (`candle`/`burn`); compute on a keyframe schedule, interpolate. Highest insight in the whole document.

### 2.15 Goniometer + Per-Band Correlation Spectrogram — *the phase X-ray*

- **Math.** Mid/side rotation $M=(L{+}R)/\sqrt2,\ S=(L{-}R)/\sqrt2$; running Pearson $\rho=\langle LR\rangle/\sqrt{\langle L^2\rangle\langle R^2\rangle}$. Per-band: pan $=(|R|^2-|L|^2)/(|R|^2+|L|^2)$, coherence $\gamma^2=|\langle LR^*\rangle|^2/(\langle|L|^2\rangle\langle|R|^2\rangle)$.
- **Hidden structure.** Mono-incompatibility and phase cancellation invisible to the ear and to mono meters — *which* frequencies are out of phase (only the sub? only the reverb tail?).
- **Why stunning.** The tangled-yarn figure is hypnotic; the panorama spectrogram glows blue-left/red-right with phase-trouble bins flashing.
- **Autonomy.** Auto-gains on running RMS; correlation-keyed color self-annotates (green mono-safe → red collapsing); coherence averaging window self-scales with bin frequency.
- **Realtime verdict.** ✅ Goniometer is trivial CPU; per-band spectrogram is two `rustfft` calls/hop + cheap arithmetic.

---

## 3. Deep Dive — Autonomous Dimensioning: A Display That Dimensions Itself to the Signal

A naïve visualizer is a fixed instrument: a hardcoded dB floor, a fixed FFT size, a chosen colormap range, a manual gain slider. An **autonomous** display measures itself against the signal and re-dimensions on the fly. There are five distinct self-scaling operators, and a complete theory composes all five.

### 3.1 The value axis dimensions itself — percentile auto-ranging

The single most robust idea: set color/intensity limits to **running percentiles** of the live magnitude distribution, not fixed dB floors. Map $v\mapsto\mathrm{clamp}((v-q_{lo})/(q_{hi}-q_{lo}),0,1)$ with $q_{lo},q_{hi}$ the 2nd/98th percentiles, estimated online by the **Jain–Chlamtac P-square algorithm** (5 markers per quantile, $O(1)$ memory, no sample storage). Smooth the two limits with a one-pole filter to avoid flicker. Result: a whisper and a wall of distortion both fill the colormap's full perceptual range. **Do the percentiles in the dB domain**, or $q_{lo}$ collapses to the noise floor.

### 3.2 The resolution axis dimensions itself — adaptive time-frequency

The Gabor floor $\sigma_t\sigma_f\ge1/4\pi$ (equality only for a Gaussian window) is unbeatable, but *where* you spend the budget can be chosen by the signal:

- **Per-frame window selection** by a concentration measure — minimize Rényi entropy $R_\alpha(S)=\frac{1}{1-\alpha}\log_2\sum(|S|^2/\sum|S|^2)^\alpha$ (or kurtosis) of the magnitude column; lower entropy = better-matched window. Cheap closed-form fallback: if PSD spread $\sigma>\beta$, use short CQT-style windows; else $W=3B_s f_s/\mu$ with $\mu$ the centroid.
- **Constant-Q** bakes adaptivity into the transform: resolution allocated per-bin by frequency.
- **Reassignment / synchrosqueezing** are the *grid-free* limit — every energy quantum reports its own true coordinate, so resolution is set by the data, not the window.

### 3.3 The frequency/value axes dimension to *perception* — fixed perceptual rulers

The cleanest source of content-independent scaling: perceptual scales are **fixed rulers**, so axes self-calibrate to *hearing*, not to the file.

- ERB-rate runs 0→~42.5 Cam: $\mathrm{ERBS}(f)=21.4\log_{10}(0.00437f+1)$; pick $N$ gammatone filters equally spaced in ERBS to **exactly fill the pixel height**.
- Bark spans ~24 critical bands: $z=26.81f/(1960{+}f)-0.53$ (Traunmüller) — a fixed 24-row canvas, the most aggressively self-dimensioning frequency axis.
- Loudness in **sones** is an absolute perceptual quantity; brightness $\propto N$ self-normalizes once anchored (e.g. to integrated LUFS).
- **Equal-loudness weighting is *level-dependent*** — the ISO 226 contour family auto-adapts per-bin: louder passages flatten (less bass suppression), quiet passages emphasize mids, exactly like real hearing. This is content-adaptive brightness with **no user knob**.

### 3.4 The *number* of axes dimensions itself — DR that discovers dimensionality

This is the purest expression of autonomy: the data decides how many dimensions it occupies.

- **PCA eigenvalue spectrum**: keep the smallest $k$ with $\rho_k=\sum_{j\le k}\lambda_j/\sum_j\lambda_j\ge0.95$; the spectral gap $\lambda_k/\lambda_{k+1}$ tells you intrinsic dimensionality. A pure tone collapses to ~1 dim; an orchestral mix spreads across many. **Oja's rule** ($w{+}{=}\eta y(x-yw)$) tracks the top eigenvector with no covariance stored — the cheapest realtime self-scaling axis tracker.
- **Trustworthiness/continuity-gated promotion**: monitor $T(k)$ on a sliding window; if 2-D fidelity drops below $\tau$, **grow a third axis** — the dimensionality of the *picture* breathes with the sound.
- **VAE β-KL pressure** (RAVE) prunes unused latent axes (posterior collapse); per-dim KL reports exactly how many axes the timbre needs.
- **SW1PerS** reports the torus dimension = #persistent $H_1$ bars = #independent pitches.
- **δ-hyperbolicity** (Gromov 4-point) decides whether the data even *wants* hyperbolic vs. Euclidean geometry.

### 3.5 The neighborhood/threshold dimensions itself — adaptive radii & thresholds

- **Fixed-recurrence-rate $\varepsilon$**: solve $\varepsilon$ so recurrence density ≈ 2% — the recurrence plot keeps constant contrast for any signal.
- **Adaptive-whitening** per-bin running peak $P[n,k]=\max(|X|,r,mP[n{-}1,k])$ flattens spectral tilt so every band fills its display range.
- **Median-adaptive novelty threshold**: onset iff $SF[n]\ge\lambda\cdot\mathrm{median}(SF[n{-}w..n{+}w])+\delta$ — sensitivity self-tunes to local activity.
- **Itti–Koch normalization** $N(\cdot)$: multiply each feature map by $(M-\bar m)^2$ — promotes peaky maps, suppresses cluttered ones, re-balancing the emphasis budget frame to frame.

### 3.6 The shared temporal primitive

All of the above need a self-tuning temporal filter. The one-pole/EMA recurrence $z_n=\alpha y_n+(1-\alpha)z_{n-1}$ with **frame-rate-correct** $\alpha=\Delta t/(\tau+\Delta t)$ (read $\Delta t$ from `ctx.input(|i| i.stable_dt)`) keeps smoothing perceptually constant at 30/60/144 fps. Asymmetric attack/release ($\alpha_{\text{attack}}$ when rising, else $\alpha_{\text{release}}$) gives meter ballistics and the "breathing" aesthetic.

**The unified theory:** a self-dimensioning display is one where *every* free parameter — color limits, window length, axis warp, axis count, neighborhood radius, smoothing constant — is a function of the signal's own running statistics rather than a hardcoded constant. Percentiles set range, entropy sets resolution, perceptual rulers set the frequency/brightness mapping, eigenvalue/trustworthiness decay sets the dimension count, fixed-rate solving sets thresholds, and a frame-rate-correct EMA ties it all to wall-clock time.

---

## 4. Deep Dive — Insight Generation: Revealing What You Cannot Hear

A visualization is *insightful* exactly when it makes a structure that is inaudible-as-a-percept into something **detectable in <200 ms without serial search**. The cognitive science (Healey/Ware preattentive features, Cleveland–McGill encoding-accuracy hierarchy, Tufte data-ink) gives the design law; the DSP gives the structure. The principle: **map the problem to a preattentive channel** (orientation, length, position, motion, hue-as-category) and reserve high-accuracy channels (position on a common scale > length > angle > area > color) for the magnitudes you most need to monitor.

Concrete reveals — each is something the ear conflates or cannot integrate:

- **Phase problems → goniometer line collapse.** A stereo mix that sounds wide on headphones can *vanish* on a mono club system. Anti-phase content ($\rho\to-1$, $M\to0$) collapses the Lissajous figure to a **horizontal smear** — orientation is preattentive, so the failure pops instantly. The per-band coherence spectrogram localizes *which* frequencies cancel.

- **Harmonic structure → chromagram / Tonnetz.** Two recordings of the same chord on piano vs. guitar yield nearly identical chroma — you *see harmony* where the spectrogram shows only timbre. The Tonnetz makes voice-leading parsimony visible: a Wagnerian P/L/R chain is a walk across adjacent triangles, exposing that "distant" chords share common tones.

- **Song form → self-similarity matrix.** No amount of single-pass listening gives you AABA, the recapitulation, or the loop point at a glance. The SSM renders all of it as geometry; a *structureless* SSM flags a through-composed track that never develops.

- **Dynamic-range collapse → converging peak/RMS envelopes + loudness histogram.** Over-compression "sounds loud but lifeless." The peak and RMS lines **squeezing shut** (crest factor → 0 dB) and a loudness histogram piling into one narrow bin make the loudness-war damage viscerally visible. K-weighted LUFS gives a content-absolute axis comparable across tracks.

- **AI-shimmer / codec artifacts → rainbowgram + saliency.** Synthetic voices and codec pre-echo leave phase/IF fingerprints invisible in magnitude. The rainbowgram tints them as hue anomalies; Integrated-Gradients saliency points the network's finger at the exact betraying bin — and with **completeness**, the attributions literally sum to the prediction.

- **Periodicity vs. roughness vs. beating → phase portrait + persistence barcode.** The ear conflates "rough" (chaos) with "beating" (a quasiperiodic torus). The Poincaré first-return map distinguishes them: a finite set of dots = period-$n$, a closed loop = torus, fractal dust = chaos. SW1PerS quantifies it as a single barcode bar; a tritone *grows an extra hole*.

- **Spectral monotony / noisiness → entropy & flatness fields.** Spectral entropy $\tilde H=H/\log_2 K\in[0,1]$ and flatness $\mathrm{SFM}=\text{geomean}/\text{mean}$ (scale-invariant) expose tonal-vs-noise character and "every frame looks the same" mixes the ear can't tally.

The deepest insight techniques are the ones whose axis is **orthogonal to energy**: recurrence (holes vs. scale), saliency (model evidence vs. signal), self-similarity (time vs. time), correlation dimension (degrees of freedom). They reveal not *more* of the spectrum, but a completely different question about the same sound.

---

## 5. Color & Perceptual Honesty

**The load-bearing fact:** the human eye reads *order* almost entirely through **luminance**, not hue or saturation. A truthful spectrogram therefore maps magnitude to a **monotonically increasing perceptual lightness**, and uses hue/chroma only as a secondary, decorative, or categorical channel.

### Why jet/rainbow lies

Parameterize jet's luminance $Y(t)\approx0.2126R+0.7152G+0.0722B$ along $t\in[0,1]$: it is **non-monotonic** — rising from blue (~0.07) to a peak near yellow then falling to red (~0.21). Consequences:
- $dY/dt$ changes sign → iso-luminant pairs (blue ≈ red), so the eye can't rank values.
- $|dY/dt|$ spikes at the yellow/cyan transitions → **manufactured false edges** that no data supports.
- A near-zero slope across the green plateau → a perceptual **flat spot** hiding ~1/10 of the data range.
- Critically, CIELAB/CAM02 uniformity holds only at **low spatial frequency**; at spectrogram detail scale chromatic acuity collapses and only luminance carries fine structure — so rainbow's hue-encoded detail is *literally invisible* at the pixel scale.

### The right way

1. **Default to a perceptually-uniform sequential map** (viridis/magma/inferno/cividis), designed so perceptual lightness $J'$ in CAM02-UCS increases at a constant rate ($dJ'/dt=\text{const}$). Equal dB steps → equal perceived contrast everywhere; survives grayscale and red-green colorblindness. Ship the 256×3 LUT as a const array — zero runtime color math.
2. **For two orthogonal acoustic dimensions, compute color at runtime in Oklab/Oklch** (two 3×3 matmuls + 3 cube roots — trivial on CPU or in a fragment shader). Drive **lightness $L$ from dB magnitude**, reserve **hue for a genuinely independent variable** (instantaneous frequency, source identity). Oklab fixes CIELAB's blue-hue twist so a phase→hue sweep won't kink.
3. **For circular quantities (phase, pitch-class), use a *cyclic* perceptually-uniform map** — Kovesi CET-C, or an iso-luminant Oklch hue ring sweeping $h=2\pi t$ at fixed $L,C$. The LUT must be C¹-continuous at index 0≡255 or you reintroduce a false seam.
4. **For categorical source coloring, use HSLuv/Oklch gamut-aware palettes** — N hues equally spaced at fixed $L,S$ so distinct stems get equally-salient, equally-bright colors (different category, not bigger value).

### The non-physics of pitch→hue

There is **no perceptually valid pitch→hue law.** Newton's 7-color/diatonic analogy is numerology (he forced indigo to make 7 match the scale; the visible spectrum spans <1 octave of light). Large-N cross-modal studies show hue has essentially *no* reliable acoustic mapping, while the robust, replicable couplings are **pitch/centroid → lightness** (≈3–4% fewer errors, ~64–83 ms faster RT) and **loudness → saturation**. Genuine chromesthetes are internally consistent but disagree across individuals on hue — confirming hue carries no universal signal. **Design rule:** spend the reliable perceptual budget (lightness, then saturation) on the dimensions humans share; treat any note→hue palette as decorative convention and make it **user-remappable and iso-luminant** so it doesn't fight the magnitude channel.

The honest move everywhere: **map magnitude through perceptual loudness (sones / cube-root, $\propto I^{0.3}$) to lightness, reserve hue for orthogonal or categorical data, and auto-anchor the brightness scale to the clip's integrated loudness.**

---

## 6. TinyBooth Roadmap

The app already ships audio-driven **Lissajous**, **Mandala** (generative), **Lorenz** (chaos), and **Chladni** (cymatics) modes plus `rustfft` and a wgpu pipeline. The recommendations below are tiered, and each new mode is chosen to (a) reuse the existing FFT/wgpu plumbing and (b) earn its place by revealing something the current four modes don't.

### Foundation first: the shared analysis & rendering bus (do this before any mode)

These are not visible modes but the substrate every mode below consumes:

- **STFT ring-buffer texture** (R32F, `queue.write_texture` one column/frame, modulo-1.0 scroll in the shader) — constant per-frame cost regardless of history length.
- **Onset/feature engine** off the existing FFT: half-wave-rectified log-spectral-flux novelty $SF(n)=\sum_k H(\log(1{+}\gamma|X_{n,k}|)-\log(1{+}\gamma|X_{n-1,k}|))$ with median-adaptive threshold; smoothed band energies, spectral centroid, flatness. This is the **autonomous coupling engine** — it feeds Mandala symmetry, Lorenz parameters, and everything new.
- **Perceptual color core**: viridis/magma LUTs + an Oklab runtime path + a cyclic Oklch ring, with **percentile (P-square) dB auto-ranging**.
- **Frame-rate-correct EMA** primitive for all smoothing.

---

### Tier 1 — Quick wins (days; CPU + existing FFT)

| Mode | Technique & math | Data needed | Cost | Why it earns its place |
|---|---|---|---|---|
| **Glowing-beam upgrade to Lissajous** | woscope `erf`-difference line integral $F(p)=\frac{1}{2l}e^{-p_y^2/2\sigma^2}[\mathrm{erf}(\frac{p_x}{\sqrt2\sigma})-\mathrm{erf}(\frac{p_x-l}{\sqrt2\sigma})]$, additive blend; rotate to mid/side | L/R waveform | wgpu instanced quads, ~1–2k/frame | Turns the existing Lissajous from jagged polyline into a glowing CRT — highest wow-per-line-of-code; also doubles as goniometer |
| **Goniometer + correlation meter** | M/S rotation + running Pearson $\rho$; per-band coherence reuses FFT | L/R + 2 FFTs | ~free CPU | Phase-problem X-ray; pro-metering credibility for a "sound studio" |
| **Log/dB perceptual spectrogram** | $|STFT|$ on a log2-f axis via warp-LUT, dB color through viridis, percentile AGC | FFT bins | ✅ trivial | The iconic baseline the app is currently missing; substrate for everything |
| **Spectrum analyzer (Welch)** | Exponential-averaged PSD $\hat S_t=\alpha P_t+(1-\alpha)\hat S_{t-1}$, peak-hold | FFT bins | ✅ free | Clean, calm, expected studio readout; anchors loudness |
| **LUFS / crest-factor panel** | BS.1770 K-weighting biquads + gated mean-square; peak vs RMS converging envelopes + loudness histogram | waveform | ✅ trivial | Reveals dynamic-range collapse; content-absolute axis |
| **Entropy / flatness ribbon** | $\tilde H=H/\log_2K$, $\mathrm{SFM}=$ geomean/mean | FFT bins | ✅ $O(K)$ | Self-normalizing tonal-vs-noise meter under the spectrogram |

---

### Tier 2 — Flagships (weeks; the signature additions)

| Mode | Technique & math | Data needed | Cost | Why it earns its place |
|---|---|---|---|---|
| **Constant-Q spectrogram** | Brown–Puckette sparse kernel: one FFT + sparse matvec; $f_k=f_{\min}2^{k/B}$ | FFT + precomputed kernel | ✅ runtime cheap | "Music looks like music" — octaves as equal rails; the musician's spectrogram |
| **Reassigned spectrogram** | 3-FFT $\hat t,\hat\omega$ scatter; toggle blurry↔razor | 3 FFTs/frame | ✅ realtime to N≤4096; wgpu splat | The jaw-drop focus-snap; highest sharpness-per-cost in the spectral family |
| **Rainbowgram** | Mag→L, IF (phase-derivative)→hue via cyclic Oklch ring | FFT magnitude + phase | ✅ one subtract+wrap/bin | Gorgeous *and* reveals phase/timbre invisible in magnitude — the canonical "breathtaking + insightful" image |
| **Chromagram + harmony HUD** | Octave-fold to 12 bins; circle-of-fifths bloom $z=\sum_b c_b e^{i\theta_b}$; Krumhansl 24-key correlation; Tonnetz triad | FFT→chroma | ✅ $O(12)$ + 24 dot-products | Key, chord, modulation as a glowing compass — a whole new *meaning* axis the app lacks |
| **Self-similarity matrix + Foote novelty** | $S=X^TX$ on chroma/MFCC; checkerboard kernel novelty | feature sequence | ✅ wgpu texture + compute | The song's self-portrait — macro-form no current mode shows |
| **Delay-embedding phase portrait (auto-$\tau,m$)** | AMI for $\tau$, FNN for $m$; render m=3 ribbon | mono waveform | ✅ gather + periodic AMI/FNN | The autonomous-dimensioning showpiece; a chaos mode that *self-tunes* (complements the existing forced Lorenz) |
| **Driven Chladni upgrade** | Replace idealized cos·cos with Lorentzian modal superposition over a numerically-solved free-edge eigenbasis; spectrum drives mode weights; sand particles down $-\nabla\|w\|^2$ | FFT magnitude + offline eigensolve | ✅ runtime fragment shader; ⚠️ eigensolve once | Makes the *existing* Chladni mode physically honest and spectrum-reactive — modes bloom at resonance, beat, and morph with pitch |
| **DirAC / soundfield arrow map** | $I=\mathrm{Re}\{W^*[X,Y,Z]\}$, DOA + diffuseness; 2-D pseudo-version for stereo (W=mid, Y=side) | L/R (→ B-format if available) | ✅ STFT/channel + arrows | A flow-viz of *where* sound lives; degrades gracefully for stereo input |

---

### Tier 3 — Moonshots (research-grade; high ceiling, heavier deps)

| Mode | Technique & math | Data needed | Cost | Why it earns its place |
|---|---|---|---|---|
| **Synchrosqueezed CQT** | IF reassignment in frequency only; invertible → paint-a-ridge-to-solo | CWT + b-derivative | ⚠️ wgpu | Razor ridges + analysis-resynthesis; a visualizer that's also an instrument |
| **Optical-flow / LIC on the spectrogram** | Analytic reassignment vector field $v=(-\partial\phi/\partial\omega,\partial\phi/\partial t-\omega)$; Line Integral Convolution $D(r)=\int k(s)N(\sigma_r(s))ds$ + Middlebury HSV | 3 FFTs (shares reassignment) | ⚠️ fragment shader | Spectrogram as a flowing fluid — glissandi/vibrato as visible *motion*, the rare measured (not decorative) fluid look |
| **SW1PerS persistence barcode** | Sphere-norm sliding windows, Rips $H_1$, $s=1-\mathrm{mp}/\sqrt3$ (Rust `lophat`/`teia`) | mono waveform | ⚠️ N≈300, ~1–5 Hz | Coordinate-free "is this a note?"; tritone grows a hole — nothing else shows topology of sound |
| **UMAP/SOM timbre galaxy** | SOM online (BMU + U-matrix) live; Parametric-UMAP encoder (`candle`) for streaming | MFCC/feature vectors | SOM ✅ / UMAP ⚠️ | The constellation that discovers its own axes — sample-browser killer feature |
| **RAVE latent-walk** | Encode live audio → slerp through VAE latent → decode + render spectrogram | pretrained ONNX model | ⚠️ `ort`/`candle` | A generative mirror of the performance; faster-than-realtime on CPU |
| **Grad-CAM / IG saliency** | $\alpha_k^c$ GAP-of-gradients + ReLU over a bundled audio CNN | spectrogram + autograd model | ⚠️ keyframe schedule | "What the model hears" — the single highest-insight overlay, model reasoning made visible |
| **Faraday parametric waves** | Swift–Hohenberg surrogate $\partial_t u=[\varepsilon-(\nabla^2+k_c^2)^2]u-u^3$, $k_c$ from dispersion, $\varepsilon\propto$ loudness−threshold | RMS + dominant freq | ⚠️ wgpu RD | The most spectacular cymatics class — self-organizing 8/12-fold quasicrystal patterns that erupt at a loudness threshold; a far richer "Mandala" successor |
| **Hyperbolic song-form browser** | Sarkar tree embedding into Poincaré disk, Möbius $z\mapsto e^{i\alpha}(z-p)/(1-\bar p z)$ navigation | form/grain similarity tree | ⚠️ layout once | Infinite-zoom focus+context map of the whole song's structure |

**Sequencing recommendation:** Foundation bus → Tier 1 (immediately doubles the app's perceived polish and adds metering credibility) → CQT + Reassigned + Rainbowgram + Chromagram HUD (the four that define a *serious* music visualizer) → SSM and driven-Chladni upgrade → pick one moonshot (the LIC optical-flow or Faraday wave are the highest breathtaking-ceiling, both pure-wgpu and reuse existing analysis). The forced-Lorenz and Mandala modes the app already has are validated by the research (sections 2.11, generative facet) — the upgrade path for them is **auto-ranged feature coupling** (percentile-normalized features → bifurcation interval) and **chroma-driven symmetry order $N$** for the Mandala, both nearly free given the feature engine.

---

## 7. Reading List

### Foundational time-frequency & reassignment
- [Short-time Fourier transform — Wikipedia](https://en.wikipedia.org/wiki/Short-time_Fourier_transform)
- [Spectrum Analysis Windows — J.O. Smith (CCRMA)](https://www.dsprelated.com/freebooks/sasp/Spectrum_Analysis_Windows.html)
- [Spectral Interpolation: zero-padding = sinc interpolation — J.O. Smith](https://www.dsprelated.com/freebooks/sasp/Spectral_Interpolation.html)
- [Fulop & Fitz — A Unified Theory of Time-Frequency Reassignment (arXiv:0903.3080)](https://arxiv.org/abs/0903.3080)
- [Reassignment method — Wikipedia](https://en.wikipedia.org/wiki/Reassignment_method)
- [Auger, Flandrin et al. — Reassignment and Synchrosqueezing: An Overview](https://perso.ens-lyon.fr/patrick.flandrin/06633061.pdf)
- [Daubechies, Lu & Wu — Synchrosqueezed wavelet transforms (arXiv:1105.0010)](https://arxiv.org/pdf/1105.0010)
- [Fourier, Gabor, Morlet or Wigner — TF transform comparison (arXiv:2101.06707)](https://arxiv.org/pdf/2101.06707)

### Constant-Q, wavelets, multitaper
- [Brown — Calculation of a Constant Q Spectral Transform (JASA 1991)](https://www.ee.columbia.edu/~dpwe/papers/Brown91-cqt.pdf)
- [Schörkhuber & Klapuri — Constant-Q Transform Toolbox (DAFx-10)](https://www.researchgate.net/publication/228523955_Constant-Q_transform_toolbox_for_music_processing)
- [Morlet wavelet — Wikipedia](https://en.wikipedia.org/wiki/Morlet_wavelet)
- [Multitaper / Thomson's method revisited (arXiv:2103.11586)](https://arxiv.org/pdf/2103.11586)

### Psychoacoustics & perceptual scales
- [Equivalent Rectangular Bandwidth — J.O. Smith (CCRMA)](https://ccrma.stanford.edu/~jos/sasp/Equivalent_Rectangular_Bandwidth.html)
- [Auditory scales of frequency (Bark/mel/ERB) — Traunmüller](https://www2.ling.su.se/staff/hartmut/bark.htm)
- [Loudness Spectrogram Examples (sones, specific loudness) — J.O. Smith](https://www.dsprelated.com/freebooks/sasp/Loudness_Spectrogram_Examples.html)
- [Psychoacoustic Models for Perceptual Audio Coding — Tutorial (MDPI 2019)](https://www.mdpi.com/2076-3417/9/14/2854)
- [Recommendation ITU-R BS.1770-5 (K-weighting, LUFS, gating)](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf)
- [Revision of ISO 226 equal-loudness contours 2003→2023](https://www.jstage.jst.go.jp/article/ast/45/1/45_e23.66/_pdf/-char/en)
- [MFCC tutorial — Practical Cryptography](http://practicalcryptography.com/miscellaneous/machine-learning/guide-mel-frequency-cepstral-coefficients-mfccs/)

### Music-theoretic structure (MIR)
- [FMP — Fundamentals of Music Processing notebooks (Müller, AudioLabs)](https://www.audiolabs-erlangen.de/resources/MIR/FMP/landing.html)
- [Foote — Automatic Audio Segmentation Using a Measure of Audio Novelty (2000)](https://ccrma.stanford.edu/workshops/mir2009/references/Foote_00.pdf)
- [Harte, Sandler, Gasser — Detecting Harmonic Change in Musical Audio (2006)](https://www.ofai.at/~martin.gasser/papers/oefai-tr-2006-13.pdf)
- [Chew — Spiral Array model](https://en.wikipedia.org/wiki/Spiral_array_model)
- [Krumhansl key-finding profiles — Robert Hart worked example](http://rnhart.net/articles/key-finding/)
- [Tonnetz / Neo-Riemannian theory — Wikipedia](https://en.wikipedia.org/wiki/Tonnetz)
- [Grosche, Müller, Kurth — Cyclic Tempogram (ICASSP 2010)](https://resources.mpi-inf.mpg.de/MIR/tempogramtoolbox/2010_GroscheMuellerKurth_TempogramCyclic_ICASSP.pdf)

### Nonlinear dynamics, chaos, topology
- [Attractor reconstruction — Scholarpedia](http://www.scholarpedia.org/article/Attractor_reconstruction)
- [Takens's theorem — Wikipedia](https://en.wikipedia.org/wiki/Takens's_theorem)
- [Fraser & Swinney mutual information / Weeks AMI tutorial](http://www.physics.emory.edu/faculty/weeks/research/tseries3.html)
- [Recurrence quantification analysis — Wikipedia](https://en.wikipedia.org/wiki/Recurrence_quantification_analysis)
- [Marwan — Recurrence Plots 25 years later (arXiv:1306.0688)](https://arxiv.org/pdf/1306.0688)
- [Grassberger–Procaccia algorithm — Scholarpedia](http://www.scholarpedia.org/article/Grassberger-Procaccia_algorithm)
- [Rosenstein et al. — Largest Lyapunov exponents (PhysioNet)](https://physionet.org/files/lyapunov/1.0.0/RosensteinM93.pdf)
- [Perea & Harer — Sliding Windows and Persistence (arXiv:1307.6188)](https://arxiv.org/abs/1307.6188)
- [Perea — Sliding Window Persistence of Quasiperiodic Functions / dissonance (arXiv:2103.04540)](https://arxiv.org/abs/2103.04540)
- [Bauer — Ripser (arXiv:1908.02518)](https://arxiv.org/abs/1908.02518) · [LoPHAT — Rust persistence toolkit](https://github.com/tomchaplin/lophat)

### Physical waves & cymatics
- [Tseng et al. — Resonant vibration of thin plates / Chladni reconstruction (JASA 2015)](https://asa.scitation.org/doi/10.1121/1.4916704)
- [Vibration of plates (Kirchhoff–Love) — Wikipedia](https://en.wikipedia.org/wiki/Vibration_of_plates)
- [Vibration of a circular membrane (Bessel modes) — Wikipedia](https://en.wikipedia.org/wiki/Vibration_of_a_circular_membrane)
- [Chen & Viñals — Amplitude equations / Faraday pattern selection (arXiv:patt-sol/9702002)](https://arxiv.org/pdf/patt-sol/9702002)
- [The Fundamentals of Modal Testing — App Note 243-3 (Lorentzian modal sum)](https://rotorlab.tamu.edu/me459/APP%20Note%20243-3%20The%20Fundamentals%20of%20Modal%20Testing.pdf)
- [Cymatica — GPU Chladni simulator (100k particles, realtime)](https://www.cymatica.app/)

### Color science & perceptual mapping
- [matplotlib colormaps — viridis/magma design (Smith & van der Walt)](https://bids.github.io/colormap/)
- [Kovesi — Good Colour Maps: How to Design Them (arXiv:1509.03700)](https://arxiv.org/abs/1509.03700)
- [Ottosson — Oklab perceptual color space](https://bottosson.github.io/posts/oklab/) · [sRGB gamut clipping](https://bottosson.github.io/posts/gamutclipping/)
- [Anikin & Johansson — Implicit color↔sound associations (PMC6407832)](https://pmc.ncbi.nlm.nih.gov/articles/PMC6407832/)
- [Spence & Di Stefano — Coloured hearing, colour music, colour organs (2022)](https://journals.sagepub.com/doi/10.1177/20416695221092802)
- [Engel et al. — NSynth rainbowgrams](https://magenta.tensorflow.org/nsynth)
- [Turbo, an improved rainbow colormap — Google Research](https://research.google/blog/turbo-an-improved-rainbow-colormap-for-visualization/)

### Dimensionality reduction & embeddings
- [UMAP and its Variants: Tutorial and Survey (arXiv:2109.02508)](https://arxiv.org/abs/2109.02508)
- [t-SNE — Wikipedia](https://en.wikipedia.org/wiki/T-distributed_stochastic_neighbor_embedding)
- [Self-organizing map — Wikipedia](https://en.wikipedia.org/wiki/Self-organizing_map)
- [Oja learning rule (streaming PCA) — Scholarpedia](http://www.scholarpedia.org/article/Oja_learning_rule)
- [RAVE: realtime audio VAE (arXiv:2111.05011)](https://arxiv.org/abs/2111.05011)
- [Comparative Audio Analysis with MFCC/UMAP/t-SNE/PCA — Fedden](https://medium.com/@LeonFedden/comparative-audio-analysis-with-wavenet-mfccs-umap-t-sne-and-pca-cb8237bfce2f)

### Spatial / ambisonics
- [Goniometer (audio) — Wikipedia](https://en.wikipedia.org/wiki/Goniometer_(audio)) · [Ziemer & Schuller — Goniometers as MIR feature (arXiv:2302.01090)](https://arxiv.org/abs/2302.01090)
- [Pulkki et al. — Applications of Directional Audio Coding (DirAC)](http://decoy.iki.fi/dsound/ambisonic/motherlode/source/rba-15-002.pdf)
- [Ambisonic data exchange formats (ACN/SN3D) — Wikipedia](https://en.wikipedia.org/wiki/Ambisonic_data_exchange_formats)
- [Zotter & Frank — All-Round Ambisonic Panning and Decoding](https://www.researchgate.net/publication/262825495_All-Round_Ambisonic_Panning_and_Decoding)
- [Steered-response power (SRP-PHAT) — Wikipedia](https://en.wikipedia.org/wiki/Steered-response_power)

### Generative / particle / fluid
- [Bridson et al. — Curl-Noise for Procedural Fluid Flow (SIGGRAPH 2007)](https://www.cs.ubc.ca/~rbridson/docs/bridson-siggraph2007-curlnoise.pdf)
- [Jos Stam — Stable Fluids (annotated notes, Dan Morris)](https://dmorris.net/projects/summaries/dmorris.stable_fluids.notes.pdf)
- [Gray-Scott model — exact PDEs & parameter atlas (Biological Modeling)](https://biologicalmodeling.org/prologue/gray-scott) · [Karl Sims — Reaction-Diffusion tutorial](https://www.karlsims.com/rd.html)
- [Müller et al. — Particle-Based Fluid Simulation (SPH, 2003)](https://people.computing.clemson.edu/~dhouse/courses/881/papers/mueller03.pdf)
- [LYGIA kaleidoscope() — domain-fold reference](https://lygia.xyz/space/kaleidoscope)

### Adaptive display, novelty, information theory & cognition
- [Jain & Chlamtac — P-square streaming quantiles (CACM 1985)](https://www.researchgate.net/publication/255672978_The_P_2_algorithm_for_dynamic_calculation_of_quantiles_and_histograms_without_storing_observations)
- [Nisar et al. — Adaptive window-size selection for spectrograms (2016)](https://pmc.ncbi.nlm.nih.gov/articles/PMC5013242/)
- [Itti, Koch, Niebur — Saliency-Based Visual Attention (1998)](https://www.sciencedirect.com/science/article/pii/S0042698999001637)
- [Stowell & Plumbley — Adaptive whitening for onset detection (ICMC 2007)](https://www.researchgate.net/publication/250824858_Adaptive_whitening_for_improved_real-time_audio_onset_detection)
- [A Basic Tutorial on Novelty and Activation Functions (TISMIR)](https://transactions.ismir.net/articles/10.5334/tismir.202)
- [Bello & Duxbury — Complex-domain onset detection (DAFx 2003)](https://www.dafx.de/paper-archive/2003/pdfs/dafx81.pdf)
- [Spectral flatness — Wikipedia](https://en.wikipedia.org/wiki/Spectral_flatness)
- [Perception and Visualization — preattentive features, Cleveland–McGill (Iowa STAT4580)](https://homepage.divms.uiowa.edu/~luke/classes/STAT4580/percep.html)

### Rendering math & realtime systems (Rust/wgpu)
- [spectro — making-of (ring-buffer texture, warp-LUT, CIELAB color interp)](https://github.com/calebj0seph/spectro/blob/master/docs/making-of.md)
- [glspect — realtime OpenGL spectrogram (circular-queue scroll)](https://github.com/ahbarnett/glspect)
- [ChartGPU + WebGPU Charts at 120 FPS (min/max tiling, LTTB, instanced lines)](https://news.ycombinator.com/item?id=46706528)
- [EMA / one-pole IIR, α↔time-constant — mbedded.ninja](https://blog.mbedded.ninja/programming/signal-processing/digital-filters/exponential-moving-average-ema-filter/)
- [mina86 — sRGB↔Lab conversions (exact formulas)](https://mina86.com/2021/srgb-lab-lchab-conversions/)
- [Line integral convolution — Wikipedia](https://en.wikipedia.org/wiki/Line_integral_convolution)
- [egui-wgpu CallbackTrait (prepare/paint integration)](https://github.com/emilk/egui/discussions/4583)

### Differentiable / neural visualization
- [DDSP: Differentiable Digital Signal Processing (Engel et al., ICLR 2020)](https://ar5iv.labs.arxiv.org/html/2001.04643)
- [Grad-CAM (Selvaraju et al. 2017)](https://aiwiki.ai/wiki/grad_cam) · [Integrated Gradients / SmoothGrad baselines (Distill)](https://distill.pub/2020/attribution-baselines/)
- [GANSynth — Adversarial Neural Audio Synthesis (arXiv:1902.08710)](https://ar5iv.labs.arxiv.org/html/1902.08710)
- [Griffin-Lim phase recovery — reference implementation](https://github.com/bkvogel/griffin_lim)
- [ort — Fast ONNX inference for Rust](https://github.com/pykeio/ort)

### Hyperbolic / graph layouts
- [Lamping, Rao, Pirolli — Hyperbolic focus+context browser (CHI '95)](https://dl.acm.org/doi/fullHtml/10.1145/223904.223956)
- [Nickel & Kiela — Poincaré Embeddings (NeurIPS 2017)](https://arxiv.org/abs/1705.08039) · [Sarkar — Low-distortion tree embedding](https://homepages.inf.ed.ac.uk/rsarkar/papers/HyperbolicDelaunayFull.pdf)
- [Fruchterman & Reingold — Force-Directed Placement (1991)](https://onlinelibrary.wiley.com/doi/10.1002/spe.4380211102)
- [Analysis and Visualization of Musical Structure using Networks (arXiv:2404.15208)](https://arxiv.org/html/2404.15208v1)
- [Munzner — H3: large directed graphs in 3D hyperbolic space](https://graphics.stanford.edu/papers/munzner_thesis/html/node8.html)