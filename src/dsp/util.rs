//! Small numeric helpers shared by every effect.

pub const MIN_DB: f32 = -200.0;

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-10 {
        MIN_DB
    } else {
        20.0 * x.log10()
    }
}

/// Peak absolute sample value across all channels.
pub fn peak(chs: &[Vec<f32>]) -> f32 {
    chs.iter().map(|c| peak_ch(c)).fold(0.0, f32::max)
}

pub fn peak_ch(c: &[f32]) -> f32 {
    c.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

pub fn rms_ch(c: &[f32]) -> f32 {
    if c.is_empty() {
        return 0.0;
    }
    let sum: f64 = c.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / c.len() as f64).sqrt() as f32
}

pub fn ms_to_samples(ms: f32, sr: u32) -> usize {
    ((ms.max(0.0) as f64) * 0.001 * sr as f64).round() as usize
}

pub fn secs_to_samples(s: f32, sr: u32) -> usize {
    ((s.max(0.0) as f64) * sr as f64).round() as usize
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
/// `y = c * y + (1 - c) * x`. Zero ms gives an instant response.
pub fn smooth_coeff(ms: f32, sr: u32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sr as f32)).exp()
    }
}

/// Periodic Hann window (sums to a constant under 50% / 75% overlap).
pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = std::f64::consts::PI * 2.0 * i as f64 / n as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect()
}

pub fn wrap_phase(p: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    p - tau * ((p + std::f64::consts::PI) / tau).floor()
}

pub fn len_of(chs: &[Vec<f32>]) -> usize {
    chs.first().map(|c| c.len()).unwrap_or(0)
}

/// Deterministic xorshift PRNG; good enough for noise and dither.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in [-1, 1).
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Copy a sub-range out of every channel.
pub fn slice_range(chs: &[Vec<f32>], a: usize, b: usize) -> Vec<Vec<f32>> {
    chs.iter()
        .map(|c| {
            let a = a.min(c.len());
            let b = b.min(c.len()).max(a);
            c[a..b].to_vec()
        })
        .collect()
}

pub fn format_time(samples: f64, sr: u32) -> String {
    let secs = (samples / sr.max(1) as f64).max(0.0);
    let total_ms = (secs * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let s = (total_ms / 1000) % 60;
    let m = (total_ms / 60_000) % 60;
    let h = total_ms / 3_600_000;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    }
}

/// Parse "m:ss.mmm", "h:mm:ss.mmm" or plain seconds into samples.
pub fn parse_time(text: &str, sr: u32) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let mut secs = 0.0f64;
    for part in t.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        secs = secs * 60.0 + v;
    }
    if secs < 0.0 {
        return None;
    }
    Some(secs * sr as f64)
}
