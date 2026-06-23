//! Built-in visualizer modules. Each implements
//! [`crate::ui::visualizer::VizModule`] and is registered in
//! `super::default_modules()`.

pub mod chladni;
pub mod chroma;
pub mod health;
pub mod lissajous;
pub mod lorenz;
pub mod mandala;
pub mod onion;
pub mod particles;
pub mod phase_portrait;
pub mod reassigned;
pub mod recurrence;
pub mod similarity;
pub mod som;
pub mod spectrogram;
pub mod spectrum_bars;
pub mod vectorscope;

pub use chladni::Chladni;
pub use chroma::Chroma;
pub use health::Health;
pub use lissajous::Lissajous;
pub use lorenz::Lorenz;
pub use mandala::Mandala;
pub use onion::OnionSkin;
pub use particles::Particles;
pub use phase_portrait::PhasePortrait;
pub use reassigned::Reassigned;
pub use recurrence::Recurrence;
pub use similarity::Similarity;
pub use som::Som;
pub use spectrogram::Spectrogram;
pub use spectrum_bars::SpectrumBars;
pub use vectorscope::Vectorscope;
