//! RBJ "Audio EQ Cookbook" biquads, transposed direct form II, f64 state.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peak,
    LowShelf,
    HighShelf,
    AllPass,
}

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Build from normalised coefficients (a0 = 1).
    pub fn from_coeffs(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Biquad { b0, b1, b2, a1, a2, z1: 0.0, z2: 0.0 }
    }

    /// Take `o`'s coefficients but keep this filter's state (for smooth
    /// parameter changes while audio runs).
    pub fn copy_coeffs(&mut self, o: &Biquad) {
        self.b0 = o.b0;
        self.b1 = o.b1;
        self.b2 = o.b2;
        self.a1 = o.a1;
        self.a2 = o.a2;
    }

    pub fn identity() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 }
    }

    pub fn new(kind: FilterType, sr: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let sr = sr as f64;
        let f = (freq as f64).clamp(1.0, sr * 0.499);
        let q = (q as f64).max(0.01);
        let w0 = 2.0 * PI * f / sr;
        let (sw, cw) = w0.sin_cos();
        let alpha = sw / (2.0 * q);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterType::LowPass => {
                let b1 = 1.0 - cw;
                (b1 / 2.0, b1, b1 / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
            FilterType::HighPass => {
                let b1 = -(1.0 + cw);
                ((1.0 + cw) / 2.0, b1, (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
            FilterType::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::Notch => (1.0, -2.0 * cw, 1.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::AllPass => {
                (1.0 - alpha, -2.0 * cw, 1.0 + alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
            FilterType::Peak => (
                1.0 + alpha * a,
                -2.0 * cw,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cw,
                1.0 - alpha / a,
            ),
            FilterType::LowShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + sa),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - sa),
                    (a + 1.0) + (a - 1.0) * cw + sa,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - sa,
                )
            }
            FilterType::HighShelf => {
                let sa = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + sa),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - sa),
                    (a + 1.0) - (a - 1.0) * cw + sa,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - sa,
                )
            }
        };
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y as f32
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Magnitude response in dB at `freq`.
    pub fn response_db(&self, freq: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * freq as f64 / sr as f64;
        let (s1, c1) = (-w).sin_cos();
        let (s2, c2) = (-2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = self.b1 * s1 + self.b2 * s2;
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = self.a1 * s1 + self.a2 * s2;
        let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30);
        (10.0 * mag2.max(1e-30).log10()) as f32
    }
}

/// Q values for a Butterworth cascade of the given (even) order.
pub fn butterworth_qs(order: usize) -> Vec<f32> {
    let order = order.max(2) & !1;
    (0..order / 2)
        .map(|k| {
            let theta = PI * (2 * k + 1) as f64 / (2 * order) as f64;
            (1.0 / (2.0 * theta.cos())) as f32
        })
        .collect()
}

pub fn butterworth(kind: FilterType, sr: f32, freq: f32, order: usize) -> Vec<Biquad> {
    butterworth_qs(order)
        .into_iter()
        .map(|q| Biquad::new(kind, sr, freq, q, 0.0))
        .collect()
}

/// Run a cascade over one channel in place.
pub fn run_cascade(ch: &mut [f32], filters: &mut [Biquad]) {
    for f in filters.iter_mut() {
        f.reset();
    }
    for s in ch.iter_mut() {
        let mut v = *s;
        for f in filters.iter_mut() {
            v = f.process(v);
        }
        *s = v;
    }
}

pub fn cascade_response_db(filters: &[Biquad], freq: f32, sr: f32) -> f32 {
    filters.iter().map(|f| f.response_db(freq, sr)).sum()
}
