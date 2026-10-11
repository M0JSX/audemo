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
    let len = chs[0].len();
    if target == 1 && have <= 2 {
        let k = 1.0 / have as f32;
        let mut m = vec![0.0f32; len];
        for c in chs {
            for (o, s) in m.iter_mut().zip(c) {
                *o += s * k;
            }
        }
        return vec![m];
    }
    if have > 2 && target <= 2 {
        // Surround to stereo (ITU-style: centre and surrounds at -3 dB, LFE dropped).
        let mut l = vec![0.0f32; len];
        let mut r = vec![0.0f32; len];
        let mut v = vec![0.0f32; have];
        for i in 0..len {
            for (c, x) in v.iter_mut().enumerate() {
                *x = chs[c][i];
            }
            let [a, b] = surround_downmix(&v);
            l[i] = a;
            r[i] = b;
        }
        if target == 1 {
            return vec![l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect()];
        }
        return vec![l, r];
    }
    if have <= 2 && target > 2 {
        // Mono to the centre; stereo to the front left and right.
        let mut out = vec![vec![0.0f32; len]; target];
        if have == 1 {
            out[2.min(target - 1)] = chs[0].clone();
        } else {
            out[0] = chs[0].clone();
            out[1] = chs[1].clone();
        }
        return out;
    }
    if have == 8 && target == 6 {
        // 7.1 to 5.1: the side channels fold into the rears.
        let k = std::f32::consts::FRAC_1_SQRT_2;
        let mut out: Vec<Vec<f32>> = chs[..6].to_vec();
        for i in 0..len {
            out[4][i] += k * chs[6][i];
            out[5][i] += k * chs[7][i];
        }
        return out;
    }
    if have <= 2 {
        // Mono to stereo: both sides.
        return (0..target).map(|i| chs[i.min(have - 1)].clone()).collect();
    }
    (0..target).map(|i| chs.get(i).cloned().unwrap_or_else(|| vec![0.0; len])).collect()
}

/// Stereo from one frame of surround in WAV channel order (FL FR FC LFE
/// BL BR SL SR …): centre and surrounds at -3 dB, LFE left out.
pub fn surround_downmix(v: &[f32]) -> [f32; 2] {
    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match v.len() {
        0 => [0.0, 0.0],
        1 => [v[0], v[0]],
        2 => [v[0], v[1]],
        3 => [v[0] + K * v[2], v[1] + K * v[2]],
        4 => [v[0] + K * v[2], v[1] + K * v[3]],
        5 => [v[0] + K * v[2] + K * v[3], v[1] + K * v[2] + K * v[4]],
        6 => [v[0] + K * v[2] + K * v[4], v[1] + K * v[2] + K * v[5]],
        7 => [v[0] + K * v[2] + 0.5 * v[4] + K * v[5], v[1] + K * v[2] + 0.5 * v[4] + K * v[6]],
        _ => [v[0] + K * v[2] + K * v[4] + K * v[6], v[1] + K * v[2] + K * v[5] + K * v[7]],
    }
}

/// Channel names in WAV order for a channel count.
pub fn channel_names(n: usize) -> &'static [&'static str] {
    match n {
        1 => &["M"],
        2 => &["L", "R"],
        3 => &["L", "R", "C"],
        4 => &["L", "R", "Ls", "Rs"],
        5 => &["L", "R", "C", "Ls", "Rs"],
        6 => &["L", "R", "C", "LFE", "Ls", "Rs"],
        7 => &["L", "R", "C", "LFE", "Cs", "Ls", "Rs"],
        _ => &["L", "R", "C", "LFE", "Lb", "Rb", "Ls", "Rs"],
    }
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
    fn surround_remaps() {
        let six: Vec<Vec<f32>> = (0..6).map(|c| vec![c as f32 + 1.0; 4]).collect();
        let st = remap_channels(&six, 2);
        let k = std::f32::consts::FRAC_1_SQRT_2;
        assert!((st[0][0] - (1.0 + 3.0 * k + 5.0 * k)).abs() < 1e-5);
        assert!((st[1][0] - (2.0 + 3.0 * k + 6.0 * k)).abs() < 1e-5);
        let up = remap_channels(&[vec![0.5; 3], vec![0.25; 3]], 6);
        assert_eq!((up.len(), up[0][0], up[1][0], up[2][0], up[5][0]), (6, 0.5, 0.25, 0.0, 0.0));
        assert_eq!(remap_channels(&[vec![0.5; 3]], 6)[2][0], 0.5, "mono goes to the centre");
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
