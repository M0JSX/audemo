//! Windowed-sinc resampler for arbitrary ratios.

use std::f64::consts::PI;

/// Sinc kernel half-width in input samples (Preferences > Data > Sample
/// Rate Conversion quality): 8 low, 16 medium, 32 high.
static HALF_WIDTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(16);

pub fn set_quality(half_width: u32) {
    HALF_WIDTH.store(half_width.clamp(4, 64), std::sync::atomic::Ordering::Relaxed);
}

/// Kernel table steps per input sample. Linear interpolation between steps
/// keeps the kernel within ~1e-7 of the exact windowed sinc.
const TABLE_RES: usize = 1024;

/// Resample `x` by `ratio` = output_rate / input_rate.
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
    // The kernel depends only on the distance d from the output position,
    // so it is tabulated once for d in [0, half] (it is symmetric).
    let table: Vec<f64> = (0..=half as usize * TABLE_RES + 1)
        .map(|i| {
            let d = i as f64 / TABLE_RES as f64;
            let arg = d * cutoff;
            let sinc = if arg.abs() < 1e-9 { 1.0 } else { (PI * arg).sin() / (PI * arg) };
            // Blackman window over the kernel span.
            let wx = (d / half as f64).min(1.0);
            let win = 0.42 + 0.5 * (PI * wx).cos() + 0.08 * (2.0 * PI * wx).cos();
            if wx >= 1.0 { 0.0 } else { sinc * win }
        })
        .collect();
    let kernel = |d: f64| -> f64 {
        let p = d.abs() * TABLE_RES as f64;
        let i = p as usize;
        if i + 1 >= table.len() {
            return 0.0;
        }
        let f = p - i as f64;
        table[i] + (table[i + 1] - table[i]) * f
    };
    let n = x.len() as i64;
    let taps = 2 * half as usize;
    // For a rational ratio (44.1k↔48k is 160/147) output positions cycle
    // through `den` fractional phases, so whole kernels are precomputed.
    let phases: Option<(usize, usize, Vec<f64>)> = rational(ratio).map(|(num, den)| {
        // Output i sits at input position i·den/num; phase = (i·den) mod num.
        let mut w = Vec::with_capacity(num * taps);
        for ph in 0..num {
            let frac = ph as f64 / num as f64;
            let ks: Vec<f64> = (0..taps).map(|k| kernel(frac + (half - 1) as f64 - k as f64)).collect();
            let sum: f64 = ks.iter().sum();
            w.extend(ks.iter().map(|k| if sum.abs() > 1e-12 { k / sum } else { 0.0 }));
        }
        (num, den, w)
    });
    let render = |range: std::ops::Range<usize>, out: &mut [f32]| {
        for (o, i) in out.iter_mut().zip(range) {
            if let Some((num, den, w)) = &phases {
                let pos = i as u128 * *den as u128;
                let centre = (pos / *num as u128) as i64;
                let first = centre - half + 1;
                if first >= 0 && centre + half < n {
                    let ph = (pos % *num as u128) as usize;
                    let ws = &w[ph * taps..(ph + 1) * taps];
                    let xs = &x[first as usize..first as usize + taps];
                    let acc: f64 = xs.iter().zip(ws).map(|(&s, &k)| s as f64 * k).sum();
                    *o = acc as f32;
                    continue;
                }
            }
            let t = i as f64 * step;
            let centre = t.floor() as i64;
            let mut acc = 0.0f64;
            let mut wsum = 0.0f64;
            for j in (centre - half + 1)..=(centre + half) {
                let k = kernel(t - j as f64);
                wsum += k;
                if j >= 0 && j < n {
                    acc += x[j as usize] as f64 * k;
                }
            }
            *o = if wsum.abs() > 1e-12 { (acc / wsum) as f32 } else { 0.0 };
        }
    };
    let mut out = vec![0.0f32; out_len];
    // Long signals are split across cores; each output sample is independent.
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
    if threads <= 1 || out_len < 1 << 16 {
        render(0..out_len, &mut out);
    } else {
        let per = out_len.div_ceil(threads);
        std::thread::scope(|sc| {
            for (k, part) in out.chunks_mut(per).enumerate() {
                let render = &render;
                sc.spawn(move || {
                    let a = k * per;
                    render(a..a + part.len(), part);
                });
            }
        });
    }
    out
}

/// `ratio` as num/den in lowest terms, when den is small enough for a
/// table of per-phase kernels.
fn rational(ratio: f64) -> Option<(usize, usize)> {
    (1..=1000usize).find_map(|den| {
        let num = ratio * den as f64;
        let r = num.round();
        ((num - r).abs() < 1e-9 * num.max(1.0) && r >= 1.0 && r <= 4000.0).then(|| (r as usize, den))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The direct (untabulated) windowed sinc the table replaces.
    pub(super) fn exact(x: &[f32], ratio: f64) -> Vec<f32> {
        let out_len = ((x.len() as f64) * ratio).round().max(1.0) as usize;
        let cutoff = ratio.min(1.0) * 0.94;
        let half = (HALF_WIDTH.load(std::sync::atomic::Ordering::Relaxed) as f64 / cutoff).ceil() as i64;
        (0..out_len)
            .map(|i| {
                let t = i as f64 / ratio;
                let c = t.floor() as i64;
                let (mut acc, mut ws) = (0.0, 0.0);
                for j in (c - half + 1)..=(c + half) {
                    let d = t - j as f64;
                    let arg = d * cutoff;
                    let sinc = if arg.abs() < 1e-9 { 1.0 } else { (PI * arg).sin() / (PI * arg) };
                    let wx = (d / half as f64).clamp(-1.0, 1.0);
                    let k = sinc * (0.42 + 0.5 * (PI * wx).cos() + 0.08 * (2.0 * PI * wx).cos());
                    ws += k;
                    if j >= 0 && (j as usize) < x.len() {
                        acc += x[j as usize] as f64 * k;
                    }
                }
                (acc / ws) as f32
            })
            .collect()
    }

    #[test]
    fn table_matches_exact_kernel() {
        // Long enough to be split across threads.
        let x: Vec<f32> = (0..100_000).map(|i| ((i as f32 * 0.037).sin() * 0.7 + (i as f32 * 0.61).sin() * 0.2)).collect();
        for ratio in [48000.0 / 44100.0, 44100.0 / 48000.0, 16000.0 / 48000.0] {
            let (a, b) = (resample(&x, ratio), exact(&x, ratio));
            assert_eq!(a.len(), b.len());
            let err = a.iter().zip(&b).map(|(p, q)| (p - q).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-5, "ratio {ratio}: max error {err}");
        }
    }
}

#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn speed() {
        let x: Vec<f32> = (0..2_000_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let t = std::time::Instant::now();
        let _ = super::resample(&x, 48000.0 / 44100.0);
        let new = t.elapsed();
        let t = std::time::Instant::now();
        let _ = super::tests::exact(&x, 48000.0 / 44100.0);
        eprintln!("new {:?}  old {:?}", new, t.elapsed());
    }
}
