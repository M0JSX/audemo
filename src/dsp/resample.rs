//! Windowed-sinc resampler for arbitrary ratios.

use std::f64::consts::PI;

/// Resample `x` by `ratio` = output_rate / input_rate.
/// Sinc kernel half-width in input samples (Preferences > Data > Sample
/// Rate Conversion quality): 8 low, 16 medium, 32 high.
static HALF_WIDTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(32);

pub fn set_quality(half_width: u32) {
    HALF_WIDTH.store(half_width.clamp(4, 64), std::sync::atomic::Ordering::Relaxed);
}

pub fn resample(x: &[f32], ratio: f64) -> Vec<f32> {
    if x.is_empty() {
        return Vec::new();
    }
    if (ratio - 1.0).abs() < 1e-9 {
        return x.to_vec();
    }
    let out_len = ((x.len() as f64) * ratio).round().max(1.0) as usize;
    let step = 1.0 / ratio;
    // Anti-alias cutoff relative to the input Nyquist.
    let cutoff = ratio.min(1.0) * 0.94;
    let half = (HALF_WIDTH.load(std::sync::atomic::Ordering::Relaxed) as f64 / cutoff).ceil() as i64;
    let n = x.len() as i64;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let t = i as f64 * step;
        let centre = t.floor() as i64;
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        for j in (centre - half + 1)..=(centre + half) {
            let d = t - j as f64;
            let arg = d * cutoff;
            let sinc = if arg.abs() < 1e-9 { 1.0 } else { (PI * arg).sin() / (PI * arg) };
            // Blackman window over the kernel span.
            let wx = (d / half as f64).clamp(-1.0, 1.0);
            let win = 0.42 + 0.5 * (PI * wx).cos() + 0.08 * (2.0 * PI * wx).cos();
            let k = sinc * win;
            wsum += k;
            if j >= 0 && j < n {
                acc += x[j as usize] as f64 * k;
            }
        }
        out.push(if wsum.abs() > 1e-12 { (acc / wsum) as f32 } else { 0.0 });
    }
    out
}

pub fn resample_channels(chs: &[Vec<f32>], ratio: f64) -> Vec<Vec<f32>> {
    chs.iter().map(|c| resample(c, ratio)).collect()
}

/// Change channel count: mono↔stereo and arbitrary down/up-mixing.
pub fn remap_channels(chs: &[Vec<f32>], target: usize) -> Vec<Vec<f32>> {
    let target = target.max(1);
    let have = chs.len();
    if have == target || have == 0 {
        return chs.to_vec();
    }
    if target == 1 {
        let len = chs[0].len();
        let k = 1.0 / have as f32;
        let mut m = vec![0.0f32; len];
        for c in chs {
            for (o, s) in m.iter_mut().zip(c) {
                *o += s * k;
            }
        }
        return vec![m];
    }
    (0..target).map(|i| chs[i.min(have - 1)].clone()).collect()
}
