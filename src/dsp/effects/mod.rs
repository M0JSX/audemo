//! Effect registry. Every effect is a pure function from input channels to
//! output channels, described by a list of parameters the UI renders.

mod amplitude;
mod delay;
mod filter;
mod generate;
mod modulation;
mod more;
mod restoration;
mod reverb;
pub mod rt;
mod special;
mod stereo;
mod timepitch;

pub(crate) mod prelude {
    pub use super::super::biquad::*;
    pub use super::super::fft::*;
    pub use super::super::params::*;
    pub use super::super::resample::*;
    pub use super::super::stft::*;
    pub use super::super::util::*;
    pub use super::{Ctx, EffectDef, NoiseProfile, Res};
}

use super::params::{ParamDef, Params, Value};

pub type Res = Result<Vec<Vec<f32>>, String>;
pub type ProcessFn = fn(&[Vec<f32>], &Ctx, &Params) -> Res;
pub type ResponseFn = fn(&Params, f32, f32) -> f32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Basic,
    Amplitude,
    Delay,
    Filter,
    Modulation,
    Restoration,
    Reverb,
    Special,
    Stereo,
    TimePitch,
    Generate,
}

impl Category {
    pub const MENU_ORDER: [Category; 10] = [
        Category::Amplitude,
        Category::Delay,
        Category::Filter,
        Category::Modulation,
        Category::Restoration,
        Category::Reverb,
        Category::Special,
        Category::Stereo,
        Category::TimePitch,
        Category::Generate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Basic => "Basic",
            Category::Amplitude => "Amplitude and Compression",
            Category::Delay => "Delay and Echo",
            Category::Filter => "Filter and EQ",
            Category::Modulation => "Modulation",
            Category::Restoration => "Noise Reduction / Restoration",
            Category::Reverb => "Reverb",
            Category::Special => "Special",
            Category::Stereo => "Stereo Imagery",
            Category::TimePitch => "Time and Pitch",
            Category::Generate => "Generate",
        }
    }
}

/// Samples captured with "Capture Noise Print".
#[derive(Clone, Debug)]
pub struct NoiseProfile {
    pub samples: Vec<Vec<f32>>,
    pub sample_rate: u32,
}

pub struct Ctx<'a> {
    pub sample_rate: u32,
    pub channels: usize,
    pub noise_print: Option<&'a NoiseProfile>,
}

pub struct EffectDef {
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
    pub params: Vec<ParamDef>,
    pub presets: Vec<(&'static str, Vec<(&'static str, Value)>)>,
    pub process: ProcessFn,
    /// Frequency response (dB) for EQ-style effects: (params, sr, freq).
    pub response: Option<ResponseFn>,
    /// (frequency key, gain key) pairs that can be dragged on the curve.
    pub handles: Vec<(&'static str, &'static str)>,
    /// Generators ignore their input and insert at the cursor.
    pub generator: bool,
    /// The output may differ in length from the input.
    pub changes_length: bool,
    /// Needs at least two channels.
    pub stereo_only: bool,
}

impl EffectDef {
    pub fn new(
        id: &'static str,
        name: &'static str,
        category: Category,
        description: &'static str,
        params: Vec<ParamDef>,
        process: ProcessFn,
    ) -> Self {
        EffectDef {
            id,
            name,
            category,
            description,
            params,
            presets: Vec::new(),
            process,
            response: None,
            handles: Vec::new(),
            generator: false,
            changes_length: false,
            stereo_only: false,
        }
    }
    pub fn presets(mut self, p: Vec<(&'static str, Vec<(&'static str, Value)>)>) -> Self {
        self.presets = p;
        self
    }
    pub fn response(mut self, f: ResponseFn) -> Self {
        self.response = Some(f);
        self
    }
    pub fn handles(mut self, h: Vec<(&'static str, &'static str)>) -> Self {
        self.handles = h;
        self
    }
    pub fn generator(mut self) -> Self {
        self.generator = true;
        self.changes_length = true;
        self
    }
    pub fn changes_length(mut self) -> Self {
        self.changes_length = true;
        self
    }
    pub fn stereo_only(mut self) -> Self {
        self.stereo_only = true;
        self
    }
    pub fn default_params(&self) -> Params {
        Params::defaults(&self.params)
    }
    pub fn preset_params(&self, name: &str) -> Option<Params> {
        self.presets
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| Params::with_preset(&self.params, v))
    }
    pub fn run(&self, input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
        if self.stereo_only && input.len() < 2 && !self.generator {
            return Err(format!("{} needs stereo audio.", self.name));
        }
        if !self.generator && input.first().map(|c| c.is_empty()).unwrap_or(true) {
            return Err("Nothing selected to process.".into());
        }
        let out = (self.process)(input, ctx, p)?;
        Ok(out
            .into_iter()
            .map(|c| c.into_iter().map(|s| if s.is_finite() { s } else { 0.0 }).collect())
            .collect())
    }
}

/// Every effect, in menu order.
pub fn registry() -> Vec<EffectDef> {
    let mut v = Vec::new();
    v.extend(amplitude::basic());
    v.extend(amplitude::defs());
    v.extend(delay::defs());
    v.extend(filter::defs());
    v.extend(modulation::defs());
    v.extend(restoration::defs());
    v.extend(reverb::defs());
    v.extend(special::defs());
    v.extend(stereo::defs());
    v.extend(timepitch::defs());
    v.extend(generate::defs());
    v.extend(more::defs());
    v
}

pub use amplitude::{fade_shape, fade_shaped};
pub use restoration::capture_noise_print;
