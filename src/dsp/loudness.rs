//! ITU-R BS.1770-4 / EBU R128 loudness: K-weighting, integrated loudness
//! (LUFS), loudness range (LU), momentary/short-term, and true peak.

use super::biquad::Biquad;

/// K-weighting filter pair (pre-filter shelf + RLB high-pass) for `sr`,
/// using the analytic coefficients from libebur128.
pub fn k_weighting(sr: f64) -> [Biquad; 2] {
    let f0 = 1681.974450955533;
    let g = 3.999843853973347;
    let q = 0.7071752369554196;
    let k = (std::f64::consts::PI * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::from_coeffs(
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    let f0 = 38.13547087602444;
    let q = 0.5003270373238773;
    let k = (std::f64::consts::PI * f0 / sr).tan();
    let d = 1.0 + k / q + k * k;
    let hp = Biquad::from_coeffs(1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / d, (1.0 - k / q + k * k) / d);
    [shelf, hp]
}

/// Mean-square energy of K-weighted audio in consecutive 100 ms steps,
/// summed over channels (BS.1770 channel weights are 1.0 for L/R/mono).
fn step_energies(chs: &[Vec<f32>], sr: u32) -> (Vec<f64>, usize) {
    let step = (sr as usize / 10).max(1);
    let len = chs.first().map(|c| c.len()).unwrap_or(0);
    let n_steps = len / step;
    let mut acc = vec![0.0f64; n_steps];
    for c in chs {
        let [mut f1, mut f2] = k_weighting(sr as f64);
        for (i, &s) in c.iter().take(n_steps * step).enumerate() {
            let y = f2.process(f1.process(s)) as f64;
            acc[i / step] += y * y;
        }
    }
    for e in acc.iter_mut() {
        *e /= step as f64;
    }
    (acc, step)
}

fn lufs(energy: f64) -> f64 {
    if energy <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * energy.log10()
    }
}

/// Block energies for windows of `win_steps` × 100 ms, hopping 100 ms.
fn windows(steps: &[f64], win_steps: usize) -> Vec<f64> {
    if steps.len() < win_steps || win_steps == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(steps.len() - win_steps + 1);
    let mut sum: f64 = steps[..win_steps].iter().sum();
    out.push(sum / win_steps as f64);
    for i in win_steps..steps.len() {
        sum += steps[i] - steps[i - win_steps];
        out.push(sum.max(0.0) / win_steps as f64);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Integrated loudness in LUFS (None if too short or silent).
    pub integrated: Option<f64>,
    /// Loudness range in LU.
    pub range: Option<f64>,
    /// Highest momentary (400 ms) loudness.
    pub max_momentary: Option<f64>,
    /// Highest short-term (3 s) loudness.
    pub max_short_term: Option<f64>,
    /// True peak in dBTP.
    pub true_peak_db: f64,
}

pub fn measure(chs: &[Vec<f32>], sr: u32) -> Loudness {
    let (steps, _) = step_energies(chs, sr);
    let blocks = windows(&steps, 4); // 400 ms, 75% overlap
    let integrated = gated_integrated(&blocks);
    let st = windows(&steps, 30); // 3 s
    let range = loudness_range(&st);
    let max_momentary = blocks.iter().copied().map(lufs).filter(|v| v.is_finite()).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    let max_short_term = st.iter().copied().map(lufs).filter(|v| v.is_finite()).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    Loudness { integrated, range, max_momentary, max_short_term, true_peak_db: true_peak_db(chs) }
}

pub fn integrated_lufs(chs: &[Vec<f32>], sr: u32) -> Option<f64> {
    let (steps, _) = step_energies(chs, sr);
    gated_integrated(&windows(&steps, 4))
}

fn gated_integrated(blocks: &[f64]) -> Option<f64> {
    let above_abs: Vec<f64> = blocks.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
    if above_abs.is_empty() {
        return None;
    }
    let ungated = above_abs.iter().sum::<f64>() / above_abs.len() as f64;
    let rel = lufs(ungated) - 10.0;
    let gated: Vec<f64> = above_abs.into_iter().filter(|&e| lufs(e) > rel).collect();
    if gated.is_empty() {
        return None;
    }
    Some(lufs(gated.iter().sum::<f64>() / gated.len() as f64))
}

/// EBU Tech 3342 loudness range from short-term block energies.
fn loudness_range(st: &[f64]) -> Option<f64> {
    let above_abs: Vec<f64> = st.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
    if above_abs.len() < 2 {
        return None;
    }
    let ungated = above_abs.iter().sum::<f64>() / above_abs.len() as f64;
    let rel = lufs(ungated) - 20.0;
    let mut vals: Vec<f64> = above_abs.into_iter().map(lufs).filter(|&l| l > rel).collect();
    if vals.len() < 2 {
        return None;
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| vals[((vals.len() - 1) as f64 * p).round() as usize];
    Some(pct(0.95) - pct(0.10))
}

/// 4× oversampled peak (8-tap windowed-sinc interpolation), in dBTP.
pub fn true_peak_db(chs: &[Vec<f32>]) -> f64 {
    const TAPS: i64 = 8;
    // Precompute kernels for the three in-between phases.
    let kernels: Vec<Vec<f64>> = (1..4)
        .map(|ph| {
            let frac = ph as f64 / 4.0;
            (-TAPS + 1..=TAPS)
                .map(|j| {
                    let d = frac - j as f64;
                    let sinc = if d.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d) };
                    let w = 0.5 + 0.5 * (std::f64::consts::PI * d / (TAPS as f64 + 0.5)).cos();
                    sinc * w
                })
                .collect::<Vec<f64>>()
        })
        .map(|k: Vec<f64>| {
            let sum: f64 = k.iter().sum();
            k.into_iter().map(|v| v / sum).collect()
        })
        .collect();
    let mut peak = 0.0f64;
    for c in chs {
        let n = c.len() as i64;
        for i in 0..n {
            peak = peak.max(c[i as usize].abs() as f64);
            if i + TAPS >= n || i - TAPS + 1 < 0 {
                continue;
            }
            for k in &kernels {
                let mut acc = 0.0;
                for (t, j) in (-TAPS + 1..=TAPS).enumerate() {
                    acc += c[(i + j) as usize] as f64 * k[t];
                }
                peak = peak.max(acc.abs());
            }
        }
    }
    if peak <= 1e-12 {
        -200.0
    } else {
        20.0 * peak.log10()
    }
}
