//! Audemo DSP engine.
//!
//! Everything in this module is plain `std` Rust with no external crates, so
//! it can be unit-tested on its own:
//!
//! ```text
//! rustc --edition 2021 --test -O src/dsp/mod.rs -o dsp_tests && ./dsp_tests
//! ```
//!
//! Only `super::` paths are used inside this tree so it compiles both as a
//! module of the app and as a standalone test crate.

#![allow(dead_code)]

pub mod analysis;
pub mod biquad;
pub mod effects;
pub mod fft;
pub mod loudness;
pub mod params;
pub mod peaks;
pub mod resample;
pub mod spectral;
pub mod spectrogram;
pub mod stft;
pub mod util;

#[cfg(test)]
mod tests;
