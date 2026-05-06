# Design vibes — pretty, awesome, mesmerizing

A creative brief for what TinyBooth could become if we keep leaning
into its wedge. Not a roadmap, not a spec. **An idea-dump with
opinions** — meant to inform real tickets later, not to be ticked
off as-is.

The wedge: TinyBooth is **the bedroom-studio opposite of a corporate
DAW**. Pro Tools is an aircraft cockpit; TinyBooth should feel like
a candlelit booth at a 1970s analog studio. Warm, tactile, slightly
mystical, deeply respectful of the user's time. Ambitious about
*atmosphere* in a category that mostly gives you grids of sliders.

Three pillars:

1. **Frame of mind** — what does the app *feel* like to use? §I.
2. **Visualization** — sound → insight, with shape and beauty. §II.
3. **Tactile controls** — every knob and button should be a small
   pleasure to operate. §III.

Plus a sober "what would we actually ship first" section at the
end. §V.

---

## I. Frame of mind

The atmosphere is the product. A user mixing in TinyBooth should
feel, at a minimum, *not stressed*. At the best, *creatively
charged*.

### Visual language

- **Warm dim, not cold dim.** The default dark mode pulls toward
  amber and oxblood, not cool blue. Think the inside of a Leslie
  speaker or the glow of a 1962 Fender amp's pilot lamp.
- **Soft shadows, no hard edges.** Channel strips are subtly
  embossed, faders cast gentle shadows. Nothing should look
  laser-cut.
- **Grain.** A whisper of film grain on backgrounds and dark
  panels — barely perceptible at 1× zoom, slightly more visible
  at 2× zoom. Tells the eye "this is a place, not a screen."
- **Type with personality.** Headers in a slab serif (Source
  Serif Pro or similar), readouts in a vari-typewriter
  monospace, body in something humanist-warm. Avoid the
  default-egui sans for anything important.
- **Idle motion.** When nothing is playing, the master peak
  meter still draws breath — a slow 0.2 Hz sine pulse at
  -∞ dB, like the booth waiting for you.
- **Studio-cat presence.** Tiny sleeping cat curled up in an
  unobtrusive corner. Stretches when you start playback, paws
  at a fader when you adjust one (cosmetic — never blocks the
  control), purrs softly during long uninterrupted sessions.
  This sounds twee until you ship it; then it becomes the
  thing people post screenshots of.

### Sound design (tasteful, optional)

A *single* configurable on/off in View → Sound. When enabled:

- A muted "click" on toggle buttons (sample-perfect, mixed below
  -30 dBFS so it never competes with the user's audio).
- A short tape-rewind whoosh on Stop. Lasts maybe 200 ms.
- A vinyl-needle-drop "tick" when starting playback from frame 0.
- A glass-wind-chime tone on milestone events (first save, 100th
  take, first export). Suggested but skippable.

Pure cosmetic. **Always default OFF**, surfaced in View as
"Studio sounds" so curious users find it.

### Hidden moments

- **First save**: a tiny flourish — the title bar gets a Roman
  numeral I beside it for one second, then fades.
- **100th take in the recordings filespace**: a one-line haiku
  appears in the status bar. Different one each time, picked
  from a small built-in collection. Disappears on next status
  message.
- **First Suno import**: the import-result modal's verdict line
  comes in with a slow type-on animation, like a 1980s movie
  terminal.

These are not jokes. They're **rituals** — the things that turn a
tool into a place.

### What we're emphatically not

- Skeuomorphic to the point of frustration. (No real-physics
  knobs that take three full mouse-rotations to traverse.)
- Maximalist. Every animation is sub-second; every flourish is
  optional or invisible-by-default.
- Default-on sounds. The audio belongs to the user's mix, full
  stop.

---

## II. Visualization explorations

The current state of TinyBooth's visualization is honest and
useful — peak meters, FFT spectrum on Record, waveform lanes on
Mix, LUFS readout on the master. **All necessary. None mesmerizing.**

This section is the dump. Ideas range from "could ship in a
weekend" to "would need a PhD". Numbered for cross-reference; not
ordered by priority.

### Realtime mix-bus visualizations

These run while the user listens to a project play. They go on
or near the master strip / transport bar.

1. **Mood ring.** A halo around the master strip whose hue tracks
   the current spectral centroid: warm red-orange for bass-heavy
   mixes, cool cyan for bright ones. Saturation tracks integrated
   LUFS (louder = more saturated). Watch your mix glow toward its
   mood; check that you're not sitting in one place too long.
   *Effort: small. Insight: real (most amateurs over-EQ in one
   direction without noticing).*

2. **Mandala mix field.** Each stem is a petal in a circular
   mandala. Petal length = current loudness, petal hue = role
   color (vocals warm, bass deep blue, drums white-hot, etc.).
   The mandala blooms with the song and folds down at quiet
   passages. One glance tells you which stem is dominating.
   *Effort: medium. Insight: stems-fighting-for-space at a glance.*

3. **Constellation of stems.** Each stem is a star plotted in a
   2D space — X = spectral centroid, Y = current loudness.
   Stars drift as the music plays; the *shape* of the
   constellation tells you the mix's character. A coherent mix
   has stars distributed across the field; a muddy mix has them
   bunched at low-Y, low-X.
   *Effort: medium. Insight: spatial intuition for spectral
   balance.*

4. **Lissajous goniometer with persistence.** Stereo signals as
   X/Y plot (L on X, R on Y). Add phosphor-style persistence
   trails (samples decay over ~2 seconds) so the figure draws
   a slow, organic shape. Phase issues become visually obvious
   — anti-phase content draws a flat line at 45°.
   *Effort: small. Insight: high — phase relationships are
   currently invisible without a paid plugin.*

5. **DNA helix for stereo.** L and R channels rendered as twin
   strands; phase difference modulates the twist rate. Mostly
   ornamental but gorgeous; serves as a richer goniometer.
   *Effort: medium-high. Insight: same as #4 with extra style.*

6. **Spectral waterfall with LUFS contour.** Standard
   spectrogram (time on Y, frequency on X, magnitude as color)
   plus an overlaid LUFS contour line. Shows you where in the
   song you got loud, and *where* in the spectrum the loudness
   came from.
   *Effort: medium. Insight: high — finds the moment in the song
   that pushed integrated LUFS over target.*

7. **Spectrogram river.** Like #6 but presented as a top-down
   view of a river: bass = deep eddies near the bottom, treble
   = ripples on the surface. Kind of a metaphor; works
   surprisingly well for "where does this song *flow*."

### Per-stem visualizations

These live on or beside the channel strips on the Mix tab.

8. **Resonance trees.** Each stem is a tree drawn beside its
   strip. Trunk grows with sustained energy; branches bloom on
   transients; leaves fall during silences. By end of song each
   stem has its own tree-shape — a unique fingerprint of how
   that stem behaved in the mix. Save the tree thumbnails as
   the Project tab's track row icons.
   *Effort: large. Insight: medium. Pure delight: max.*

9. **Particle-field amplitude meter.** Replace the boring peak
   meter with a particle source: amplitude controls emission
   rate, frequency content controls particle hue, polarity-flip
   reverses gravity. Watch your strips erupt and settle.
   *Effort: medium. Insight: low (peak meter does the same job
   functionally). Vibes: through the roof.*

10. **Per-stem mood orb.** Each strip's gain area gets a small
    orb whose color and texture reflect the stem's spectral
    character (warm bass = ember, bright synth = electric blue,
    vocals = soft pearl). Orb pulsates with current loudness.
    Cheap, beautiful, immediately readable.
    *Effort: small. Insight: medium.*

### Edit-time visualizations

These live in the Correction window or Profile editor — they
make filter / dynamics adjustments visceral instead of numerical.

11. **EQ as a force field.** The 4-band parametric EQ becomes a
    spectrum-space rubber sheet. Each enabled band is a magnet:
    peaks pull the sheet up, cuts pull it down, Q controls
    falloff width. The frequency response draws live; the
    user's input audio shows up as a shimmer crossing the
    sheet.
    *Effort: medium-large. Insight: huge. This is the
    industry-standard "EQ-with-spectrum" workflow upgrade
    rendered with TinyBooth's character.*

12. **Compressor as a hydraulic press.** A small animated
    diagram beside the compressor's controls: a piston whose
    position is the gain reduction, attack/release control
    piston speed, ratio sets piston angle. When the
    compressor squashes a transient, the piston *drops* —
    you literally see it work. Releases on a spring-back
    animation.
    *Effort: medium. Insight: high (compressor behaviour is
    notoriously opaque to amateurs).*

13. **Gate as a guillotine.** Threshold sets a horizontal blade
    height across a tiny waveform thumbnail. Audio dipping
    below the blade is sliced cleanly out. Visceral.
    *Effort: small. Insight: medium.*

14. **De-esser as a guard cat.** Frequency cursor looks like a
    cat watching the spectrum. When sibilance spikes hit the
    cursor's band, the cat pounces — visible reduction
    feedback, slightly comedic, immediately legible.
    *Effort: medium. Insight: medium. Charm: 10/10.*

15. **Polarity flip as a coin toss.** Pressing Ø animates a
    tiny coin flipping in the strip; lands heads or tails. The
    button's Ø glyph rotates 180° with the flip. Cosmetic but
    sells the operation viscerally — the user *feels* that the
    waveform just inverted.

### Memory / archive visualizations

The whole-song / cross-session views.

16. **Memory ribbon.** A tall vertical scroll on the right edge
    of the Mix tab — every minute of every playback session
    contributes a thin sliver to it (compressed spectrogram +
    peak envelope). Over weeks, the ribbon becomes a tapestry
    of how much you worked on this project, when, and what
    sections you dwelled on. Click any sliver to seek there.
    *Effort: large. Insight: surprisingly real (you discover
    sections you've over-listened to).*

17. **Tarot card sessions.** Saving a project produces a
    procedurally-generated tarot-style card image based on its
    contents — waveform shape becomes the central figure,
    stems become the suit, mixdown LUFS becomes the
    illumination level, project name becomes the title.
    Stored as `<project>.tarot.png` next to the manifest.
    Browse your projects via card spread. **Open Recent**
    becomes "Draw a card."
    *Effort: large. Insight: zero. Identity / mystique: enormous.*

18. **Project waveform ID.** Every project gets a unique
    one-line stylized waveform — its mixdown decimated and
    rendered as a strict horizontal stroke with a procedural
    color gradient based on the project name's hash. Use it
    everywhere: title bar, Project tab header, Recent menu.
    A signature your projects accumulate.
    *Effort: small. Insight: zero. Identity: enormous.*

### Going-nuts territory

Ideas that probably won't ship but would be glorious if they did.

19. **Audio-reactive fluid simulation.** Spectrum bins seed a
    Navier-Stokes solver running on the GPU. Frequencies push
    fluid up; bass creates whirlpools; transients splash. The
    Mix tab's lane background becomes a slow-moving liquid
    that breathes with the music. This is "screensaver energy"
    but if done with restraint it could be jaw-dropping.
    *Effort: huge. Insight: actually some — fluid behavior
    reflects spectral energy distribution surprisingly well.*

20. **Mandelbrot of the song.** Render a fractal seeded by the
    song's spectral fingerprint. Each project gets its own
    fractal portrait. Dive-in animation when you open the
    project. Pure ritual; pure visual fanservice.

21. **Aurora-borealis EQ response.** The frequency-response
    curve of the master correction chain drawn as living
    aurora — colors pull from the actual audio's spectrum,
    flowing slowly. Extends across the top of the Mix tab.

22. **Tempo-locked clock face.** When playback is on a
    tempo-locked Suno track, the transport bar becomes an
    analog clock face, beats sweep around the dial like a
    second hand, stems orbit at different distances by
    tempo subdivision. Read tempo at a glance, see syncopation
    as orbital wobble.

---

## III. Tactile controls — mini-game feel

Every control should be **slightly more pleasurable** to operate
than its plain-egui equivalent. The goal isn't realism — it's
*delight*.

23. **Knob momentum.** Click-drag a knob and release: the knob
    coasts to a stop with simulated rotational inertia (a few
    hundred ms of decay). Lets the user "spin" knobs for
    expressive sweeps and "tap" them for fine adjustments.
    Subtle but transformative.

24. **Magnetic snap zones.** Faders / knobs at unity (0 dB),
    centre pan, default Q, etc. have a faint magnetic pull.
    Override-able by holding shift; resists casual nudging
    away from sane defaults. The "feel" is the gravity.

25. **Detent clicks.** Every 3 dB on a fader: a tiny audible
    *and* visible click. Tactile feedback for fine adjustments.
    Optional via the Studio sounds toggle.

26. **Springy fader release.** When a fader is released after
    a fast drag, the handle "bobs" once — under-damped spring
    — before settling. 80 ms of life.

27. **Vinyl-scrub seek bar.** The transport's playback position
    bar becomes a stylized vinyl record. Drag the needle to
    seek; while dragging, audio scrubs in real time
    (granular synthesis at the seek position). The most
    emotionally satisfying way to find a moment in a song.
    *Effort: medium-large. Insight: actually high (real-time
    audio scrub is a power move). Vibes: maximal.*

28. **Pinball settings menu.** The Admin / View menus open as
    a small pinball-table layout — flippers route a ball into
    pockets, each pocket is a setting. Sounds gimmicky but
    works wonderfully for "I never remember where the dark
    mode toggle is" — kinaesthetic memory takes over from
    spatial memory.

29. **Loadout cards for presets.** Recording-tone profiles
    surface as a deck of trading cards: each card has the
    profile's chain visualized as small shape (the EQ curve,
    a compressor symbol, etc.) plus its name and description.
    Flip the deck to switch presets.
    *Effort: medium. Insight: real (profiles are otherwise
    invisible — most users can't articulate what each one
    does without reading the description).*

30. **Crystal ball A/B.** The Mix tab's A/B-bypass toggle
    rendered as a small crystal ball, with the ball's
    interior subtly showing the current spectrum. Click to
    toggle bypass; the ball flickers as the chains drop in
    or out.

31. **Constellation button grid.** Frequently-used actions
    (record, play, save, export) form a small constellation
    of buttons in the corner. Click a button and a faint
    line draws to the previous one — over time you trace
    your most-used path through the app.

---

## IV. Little delights

Bits that don't fit elsewhere but earn their pixels.

- **Undo with confetti** — undoing the deletion of a track
  briefly lights up the restored row. Small feedback, big
  emotional comfort.
- **Save with a satisfying thunk.** No animation, just one
  short low-frequency thump in the Studio-sounds bank. Like
  closing a heavy oak desk drawer.
- **First-launch tour as a séance.** Instead of dialog
  pop-ups, a candle gradually lights a series of UI areas
  in sequence. "Here lies the recording booth." Skippable;
  also rerunable from Help → Light the candle.
- **Idle splash** — if nothing has happened in 20 minutes,
  the screen subtly dims and a single slowly-animated dust
  mote drifts across. Movement at all wakes everything back
  up. Implies the studio is asleep.
- **Pin-the-cat.** The studio cat (see §I) can be moved
  by dragging it. Wherever you put it, it stays. Tiny
  per-user persistence, signals "this is your space."
- **Occluding panels recoil.** When a modal opens, the panels
  it covers don't just disappear — they pull back fractionally,
  like making room for a guest.

---

## V. What we'd actually ship for v0.5.0

The above is wish-casting. Here's the honest "what one or two
of these is actually attainable as a release":

**Tier 1 — would ship as v0.5.0 polish, alongside the take browser
and reference A/B**:

- **#1 Mood ring** on the master strip. ~120 LOC. Real
  insight, low risk, sets the visual tone for everything else.
- **#11 EQ as a force field** in the Correction editor. The
  industry-standard EQ-with-spectrum workflow, rendered with
  our character. Biggest user value of any item on the list.
  ~300 LOC + design pass.
- **#23–26 fader / knob micro-physics**. Implemented as a
  tiny per-control state crate; affects every slider. ~100
  LOC, immediately felt by every user.

**Tier 2 — could ship as v0.6.0 if v0.5.0 lands well**:

- **#2 Mandala mix field** as an alternative master visual.
- **#27 Vinyl-scrub seek bar**. Real-time scrub is a power
  move and we already have the buffer.
- **#18 Project waveform ID** — small, identity-defining.

**Tier 3 — defer, but capture as RFCs so they stay alive**:

- #16 Memory ribbon
- #17 Tarot card sessions
- #19 Audio-reactive fluid sim
- #29 Loadout cards
- The studio cat

**Tier ∞ — never, probably**:

- #20 Mandelbrot portrait. Beautiful in the abstract;
  impossible to keep simple.
- #28 Pinball menu. The kind of thing a 17-person team can
  ship; a one-person team should not try.

---

## Closing — the bedroom-studio mystic

TinyBooth's user is alone in their room, late at night, trying
to make something good out of stems an AI gave them. They are
not a professional. They have no acoustic treatment. Their
monitors are headphones from 2019.

What they need from us is **not more tools**. They have plenty
of tools already. They need a *place* — a small, warm, slightly
strange place that takes their work seriously and gives them
back a little of the magic they're looking for.

The features in this document aren't bullet-point upgrades.
They're load-bearing for *atmosphere*. Atmosphere is what makes
someone come back to a tool tomorrow night. Atmosphere is what
turns a free Rust app into a thing they tell friends about.

Ship a few of these well. Don't try to ship all of them. The
bedroom-studio mystic doesn't appreciate clutter.

---

*Questions, dissents, additions: open a PR against this file
or kick off a feature-request RFC under
[`docs/feature-requests/`](feature-requests/). Wild ideas
welcome — that's the whole point.*
