//! Built-in visualizer modules. Each implements
//! [`crate::VizModule`] and is registered in
//! `super::default_modules()`.

pub mod chladni;
pub mod chroma;
pub mod health;
pub mod hyperbolic;
pub mod lissajous;
pub mod lorenz;
pub mod mandala;
pub mod onion;
pub mod optical_flow;
pub mod particles;
pub mod phase_portrait;
pub mod reassigned;
pub mod recurrence;
pub mod saliency;
pub mod similarity;
pub mod som;
pub mod spectrogram;
pub mod spectrum_bars;
pub mod tda;
pub mod vectorscope;

pub use chladni::Chladni;
pub use chroma::Chroma;
pub use health::Health;
pub use hyperbolic::Hyperbolic;
pub use lissajous::Lissajous;
pub use lorenz::Lorenz;
pub use mandala::Mandala;
pub use onion::OnionSkin;
pub use optical_flow::OpticalFlow;
pub use particles::Particles;
pub use phase_portrait::PhasePortrait;
pub use reassigned::Reassigned;
pub use recurrence::Recurrence;
pub use saliency::Saliency;
pub use similarity::Similarity;
pub use som::Som;
pub use spectrogram::Spectrogram;
pub use spectrum_bars::SpectrumBars;
pub use tda::Tda;
pub use vectorscope::Vectorscope;
