# Sound → Vision

A philosophical / mathematical / metaphysical deep dive into what it
means to transform sound into vision. Not a marketing piece. A serious
attempt to engage with the problem TinyBooth's visualizer is partly
trying to solve, partly failing to solve, and what could come next.

This document was written in response to a user's pet peeve:

> *I CAN NEVER SEE IN THE COMPUTED VIZ THE VOLUMES CADENCE AND COLORS
> I CAN NAVIGATE WHILE LISTENING TO MUSIC*

The complaint is correct. It identifies a real gap. This document
takes that gap seriously.

---

## I. The structure of musical experience

When a human listens to music, the brain parses information at **at
least five hierarchical timescales simultaneously**:

| Timescale | Approximate duration | What's perceived |
|---|---|---|
| Sample | < 1 ms | Phase, polarity, transient onset |
| Note | 50–500 ms | Pitch, timbre, micro-dynamics |
| Beat | 0.5–2 s | Pulse, groove, syncopation |
| Phrase | 2–10 s | Melodic contour, tension/release |
| Section | 10–60 s | Verse, chorus, bridge — large-scale structure |
| Song | 1–30 min | Form, narrative arc |

These are not redundant. **Each timescale carries information the
others do not.** A spectrogram shows note-level information across
time but cannot tell you "this is the bridge". A waveform shows
beat-level pulse but cannot tell you the timbre of any particular
moment. A self-similarity matrix shows section-level repetition but
loses note-level detail.

The musician's lived experience is **a constant, effortless
integration across all these timescales**. You hear a single note and
simultaneously locate it within the current beat, the current phrase,
the current section, the song's overall arc, the genre's conventions,
your own memory of the artist's previous work. That's what the user
means by "volumes, cadences, colors" — these are timescale-specific
features that the listener integrates without effort:

- **Colors** = timbre at the **note** timescale
- **Cadences** = rhythmic motion at the **beat / phrase** timescale
- **Volumes** = dynamic envelope at the **phrase / section** timescale

A visualization that captures only the sample / note timescale **by
mathematical necessity cannot show** what the listener is integrating.

## II. Why most visualization is sterile

Almost every audio visualizer in existence operates on the
instantaneous signal: an FFT of the last few hundred milliseconds, a
peak meter, a Lissajous figure of the current sample pair. These are
**derivatives of NOW**. They have no memory and no anticipation.

The result is well known: viewers look at a typical visualizer for a
few seconds and lose interest. The visual is not *about* the music
in any meaningful sense. It's reactive in a way that feels mechanical
rather than musical, because mechanical-reactive is exactly what it
is.

The four modes we shipped in v0.4.11 (Lissajous, Mandala, Lorenz,
Chladni) all live in this trap to varying degrees:

- **Lissajous** captures phase relationships at the sample timescale.
  Beautiful for what it shows, but it shows nothing above ~80 ms.
- **Mandala** is a circular FFT — note timescale, no temporal context.
- **Lorenz** has *some* memory through the integrator's state, but the
  audio coupling is parametric (σ/ρ/β driven by current scalars), so
  the attractor's evolution is dominated by the ODE dynamics rather
  than the music's structure. Pretty; not actually about the music.
- **Chladni** is the most "purely mathematical" — it visualizes a
  fixed eigenmode basis weighted by spectral bins. Memoryless;
  context-free.

None of them shows **trajectory through perceptual space over time**.
None shows **structure**. None shows **anticipation**.

## III. The onion-skin insight

Hand-drawn animators use onion-skin: when drawing frame N, the previous
frame and the next frame are visible underneath as faint guides. The
deep insight is **not** "see the past and future" — it's that **each
frame is contextualised by its neighbors across time**, and that
context guides the artist's hand toward coherent motion.

Apply this to audio visualization. The transposition is not "show old
frames in a panel beside the current one". The transposition is:

> **Every visualized moment should be contextualised by its neighbors
> across multiple timescales simultaneously, the way a listener
> contextualises a note within a phrase within a section.**

This is a *much* stronger claim than "show me the past". It says: the
visualization itself should be a multi-timescale object, where note-
timescale features and section-timescale features are visible in the
same gestalt.

There's a corollary: a multi-timescale visualization has to abandon
the idea that the current moment is the *subject* of the picture. The
current moment is one point in a *trajectory*. The picture is the
trajectory.

## IV. The mathematical infrastructure of music perception

To build a visualization that can capture multi-timescale structure,
we need feature extraction at each timescale. The mathematical tools
exist; they're well known to the music-information-retrieval (MIR)
community. The question is which ones to lift.

### Sample / note timescale

- **Spectrogram** (STFT): the canonical time-frequency representation.
- **Cepstrum**: the spectrum of the log-spectrum. Separates *pitch*
  from *timbre* in a way the spectrum alone cannot.
- **MFCC** (Mel-frequency cepstral coefficients): cepstral
  coefficients on a perceptually-warped scale. The standard timbre
  descriptor.
- **Chromagram**: 12-bin pitch-class energy. Folds octaves together;
  captures harmonic content invariant to register.

### Beat / phrase timescale

- **Onset detection**: where do new events begin? Spectral flux,
  energy derivative, complex-domain detection.
- **Tempogram**: a 2D representation of pulse strength × tempo, over
  time. Shows rhythmic structure as an image.
- **Beat tracking**: discrete sequence of beat positions. Once you
  have these, visual elements can be aligned to the actual pulse,
  not just the sample stream.

### Section / song timescale

- **Self-similarity matrix (SSM)**: pairwise distance between every
  pair of moments in the song. Repeats appear as off-diagonal blocks.
  Choruses, verses, bridges become geometrically visible.
- **Recurrence plots** (Eckmann et al. 1987): same idea, different
  formulation. Used in nonlinear dynamics to detect structure.
- **Novelty curves**: derived from the SSM by checkerboard kernel
  convolution. Peaks correspond to section boundaries.
- **Tonnetz / pitch-class space**: the harmonic content of each moment
  as a point in a 2D lattice of musical relationships. Tonal motion
  becomes a trajectory.

### Across all timescales (cross-cutting)

- **Spectral flux**: how fast the spectrum is changing. Distinguishes
  legato passages from staccato.
- **Harmonic / percussive separation** (HPSS): split a signal into
  its sustained-tonal vs transient-percussive components.
- **Tension curves**: derived from chord-change rate, dissonance, and
  loudness. Closely tracks a listener's felt tension.

### The category nobody is plotting

Beyond standard MIR, there's an interesting category of features that
audio visualization basically *ignores*:

- **Phase coherence across bands** as a function of time.
- **Information-theoretic surprise** (predictive coding residual).
- **Topological features** of the audio manifold (persistent homology
  on time-delay embeddings).
- **Multi-scale entropy** (sample entropy at varying sample rates).
- **Modulation spectrum** (the spectrum of the time-varying spectrum
  — captures the *rhythm of the timbre*).

Each of these is a mathematically rigorous descriptor of a
perceptually meaningful property. None of them shows up in standard
audio visualizers.

## V. The AI fingerprint

The user asked whether the jerkiness in our current visualizer might
be a hint about a missing cleanup mechanism. **The answer is yes,
and I think it points at a real DSP intervention worth building.**

### The signature

Acoustic recordings have a specific statistical property at the
modulation timescale: **frame-to-frame fluctuations are correlated
across frequency bands**. When a string vibrates, its overtones move
together. When a singer's vibrato cycles, every formant cycles in
concert. Mathematically:

$$\rho(\Delta S(t, f_1), \Delta S(t, f_2)) > 0$$

for nearby bands `f_1`, `f_2`, where `ΔS` is the per-bin frame-to-
frame derivative. This correlation is a fingerprint of a single
*physical* source: a vibrating object that drives every band.

AI-generated audio frequently has the *opposite* signature. Each
frequency bin emerges from the generative process with its own noise
component. Within-frame, the spectrum looks plausible. Across frames,
the per-band fluctuations are **uncorrelated** — each bin has
independent noise. To the eye watching a Mandala viz, this shows up
as **shimmer**: fast, fine-grained, random fluctuation that doesn't
breathe like real music.

### The repair

A mild repair: **adaptive cross-band coupling** in the modulation
spectrum.

1. Compute the per-bin frame-to-frame derivative `ΔS(t, f)`.
2. Compute the pairwise correlation `ρ(f₁, f₂)` over a moving
   window (~1–2 s).
3. Compare against a learned baseline of natural-recording
   correlations (could be hard-coded from corpus statistics or
   estimated per-stem from quiet sections).
4. Where the observed correlation is anomalously low for adjacent
   bands, apply a 2D smoothing kernel in (time, frequency) with
   strength inversely proportional to the local correlation deficit.
   The smoothing pulls the per-band micro-fluctuations into mutual
   agreement — restoring the cross-band coupling that natural
   recordings exhibit.
5. Reconstruct via inverse STFT.

This is a Wiener-style filter in the modulation domain, informed by
a perceptual baseline. The output should sound less "AI-shimmery"
because the post-processed signal carries the cross-band coupling
that real recordings have and AI does not.

This wants to live as a v0.5+ feature filed as
`docs/feature-requests/TBSS-FR-coherence-restoration.md`. The hook
into the existing chain is straightforward: it sits between
`Nyquist clean` and the compressor. The visualizer can surface a
"Cross-band coherence" gauge so the user sees when the input signal
is shimmery and when the gate has caught it.

### Why it matters

This isn't theoretical. The user's whole project is about reducing
the AI-ness of Suno output. The current toolkit attacks the *audible*
artefacts (top-octave shimmer via Nyquist clean, DC offset, weird
EQ shapes via per-role presets). It does not attack the *structural*
artefact (band-decorrelated micro-flicker), because that's not where
the obvious symptoms live. But that's where the *fingerprint* lives,
and removing it would take Suno output meaningfully closer to "this
sounds like a recording" rather than "this sounds like a generation".

The visualizer revealed it. Worth following the lead.

## VI. The onion-skin visualization

Now to design.

The visualization that addresses the user's pet peeve has to:

1. Show information at **multiple timescales simultaneously**.
2. Make the **trajectory through perceptual space** the primary
   visual subject — not the current moment.
3. Layer **temporal context** the way onion-skin layers frames in
   animation.

### Mode design: "Onion Skin" — multi-timescale trajectory in feature space

**Two-axis feature plot.** Pick two perceptually-meaningful features
extracted from the audio:

- X axis: **spectral centroid** (timbral brightness — the "color")
- Y axis: **loudness** (RMS or LUFS — the "volume")

The current moment becomes a single point `(centroid_now, loudness_now)`.

**Layered temporal memory.** The view is not the current point. The
view is *all of the recent past* drawn with timescale-graded alpha:

- **Last 2 seconds** (note / beat scale): bright glowing trail.
  Latest sample is a bright dot at the head; alpha decays linearly
  toward the tail.
- **Last 30 seconds** (phrase scale): fainter ghost trail behind the
  bright trail. Same data, decimated; fades toward background.
- **Whole session** (section / song scale): a 2D heatmap "watermark"
  showing where in feature-space the music has *spent time*. Each
  cell of a 64×64 grid accumulates time-weighted residency. The
  watermark builds up over the course of a song, revealing the
  music's "home zone" and the regions it visits.

**Anticipated future** (optional, parametric). Autocorrelate the
recent trail's direction vector. If the trajectory has a clear
heading, project a dashed line forward as a predicted continuation.
Decays when the autocorrelation is weak (transition moments,
breakdowns, dynamic changes).

**Background grid.** Faint axes labeled "soft / loud" on Y and
"dark / bright" on X, so the listener orients immediately.

### What this captures

- **Volumes**: the trail's vertical motion. Phrases that swell rise
  upward; quiet sections drop. Visible at a glance.
- **Cadences**: the trail's *shape*. Steady grooves draw tight
  circles; dramatic phrases draw long diagonal sweeps; transients
  draw sharp corners.
- **Colors**: the trail's horizontal motion. Bright passages drift
  right; dark passages drift left. The hue of the trail can be
  modulated by spectral spread (a third feature) for additional
  timbral information.

The watermark layer specifically captures **section structure**:
verses concentrate in one zone, choruses in another, bridges
elsewhere. Over a song, you see the *song's regions*.

### What it leaves on the table

- Beat-level rhythmic detail (would need a tempogram overlay).
- Harmonic / pitch-class content (would need a Tonnetz overlay).
- Self-similarity structure (would need an SSM in a separate panel).

These are real limitations. Onion Skin is one mode, not the answer
to the whole question. The answer to the whole question is **a
constellation of timescale-targeted modes** that the listener can
switch between (or, eventually, see simultaneously in a multi-pane
layout).

The roadmap from here:

| Mode | Timescale | Status |
|---|---|---|
| Lissajous | sample / note | shipped (v0.4.11) |
| Mandala | note | shipped (v0.4.11) |
| Lorenz | note | shipped (v0.4.11) |
| Chladni | note | shipped (v0.4.11) |
| **Onion Skin** | note / phrase / section | **shipping** in v0.4.12 |
| Self-Similarity Matrix | section / song | future |
| Tempogram | beat / phrase | future |
| Tonnetz trajectory | note / phrase (harmonic) | future |
| Cross-band coherence gauge | shimmer fingerprint | future |
| Modulation spectrum | rhythm-of-timbre | future |

## VII. The metaphysical bit

A note for the user, who specifically asked for the metaphysical
angle.

The reason existing audio visualization feels like it doesn't capture
the music is, I think, because it's stuck at the wrong level of
abstraction. The signal is real — sound *is* pressure waves — but
**the music is not in the signal**. The music is in the *listener's
construction* of the signal. A spectrogram is a faithful picture of
the pressure waves; it's a poor picture of the *listening*.

To make a faithful picture of the listening, you have to model
something about the listener: the timescales they integrate, the
features they parse, the structures they expect. There's a real
sense in which **better music visualization is more accurately
described as cognitive modeling than as signal processing**. The MIR
community knows this; the visualizer-toy community mostly doesn't.

This isn't a counsel of despair. We can ship better visualizations
*just* by lifting MIR features that already exist (chromagrams,
self-similarity matrices, tempograms) and rendering them well. We
don't have to build a perceptual model from scratch. We just have to
choose features that are *about* what the listener is hearing,
rather than features that are *about* the signal.

The user's pet peeve was, in its precise form: "I can navigate
volumes, cadences, colors when I listen, but I can't see them in the
viz." That's the gap between signal-as-such and signal-as-perceived.
Closing the gap is what this work is now about. The Onion Skin mode
is the first move in that direction. It will not, by itself, close
the gap. But it acknowledges the gap exists, and it's built around
the right idea: **show the trajectory, not the moment.**

---

## VIII. Open questions

- **Is the cross-band-coherence repair actually audible?** Needs
  empirical validation on real Suno output. Build a prototype, A/B
  test against unprocessed.
- **How to render the SSM live?** It's a triangle that grows over
  time. Ring buffer of feature vectors → on-the-fly distance matrix.
- **Beat tracking** in real time is a solved-but-fiddly problem
  (Ellis 2007, Krebs et al. 2015). The dependency cost would be
  meaningful; might be worth pulling.
- **Tonnetz** rendering: the chromagram → 2D-lattice mapping is
  trivial; the useful insight is in motion patterns. Animations of
  Tonnetz trajectories need careful smoothing to read well.
- **Predictive futures** for the Onion Skin: how reliable is short-
  horizon prediction from autocorrelation alone? Probably enough for
  steady-state passages, useless across phrase boundaries. Ship the
  feature off by default, parametric.

## IX. References / further reading

Audio MIR foundations:

- Müller, Meinard. *Fundamentals of Music Processing* (Springer,
  2015). The textbook on chroma, tempogram, SSM, novelty.
- Foote, Jonathan. "Visualizing Music and Audio using Self-
  Similarity" (1999). The original SSM paper for music.
- Bartsch, Mark; Wakefield, Gregory. "To Catch a Chorus" (2001).
  Chromagram-based structure analysis.

Cognitive grounding:

- Huron, David. *Sweet Anticipation: Music and the Psychology of
  Expectation* (MIT Press, 2006). The cognitive-prediction framework
  that motivates "anticipation" features.
- Bregman, Albert. *Auditory Scene Analysis* (MIT Press, 1990). The
  classic on perceptual stream segregation.

AI-audio fingerprint detection:

- Pang et al. "Detecting AI-generated speech via spectro-temporal
  modulation analysis" (2024). The cross-band-coherence idea has
  precedent in this literature.

Onion-skin in animation, for the curious:

- Williams, Richard. *The Animator's Survival Kit* (Faber, 2001).
  The animator's bible. Onion-skinning is throughout; the
  philosophical lesson — "every frame is contextualised by its
  neighbors" — generalises beyond animation.

---

*This document is intentionally a living design artefact, not a
spec. It captures the current intellectual state of the
visualization work. PRs welcome.*
