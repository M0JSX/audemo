//! Spectral selections (time × frequency) and the operations on them:
//! applying any processed result only inside the selection, and healing
//! (rebuilding the selected area from the audio around it).

use super::fft::Complex;
use super::stft::{istft, stft_frames};

/// Analysis size for spectral edits: about 21 ms windows (fine enough in
/// time for clicks, fine enough in frequency for tones), 8× overlap.
pub fn spec_size(sr: u32) -> (usize, usize) {
    let n = if sr <= 24000 {
        512
    } else if sr <= 50000 {
        1024
    } else {
        2048
    };
    (n, n / 8)
}

/// A region of the spectral display. Times are in samples, frequencies in Hz.
#[derive(Clone, Debug, PartialEq)]
pub enum SpecShape {
    Rect { a: f64, b: f64, f0: f32, f1: f32 },
    /// Closed freehand outline.
    Lasso(Vec<(f64, f32)>),
    /// Painted strokes: centres plus the brush radius in samples and Hz.
    Brush { pts: Vec<(f64, f32)>, rt: f64, rf: f32 },
}

impl SpecShape {
    /// Sample range the shape covers.
    pub fn bounds(&self) -> (f64, f64) {
        match self {
            SpecShape::Rect { a, b, .. } => (a.min(*b), a.max(*b)),
            SpecShape::Lasso(p) => p.iter().fold((f64::MAX, f64::MIN), |m, q| (m.0.min(q.0), m.1.max(q.0))),
            SpecShape::Brush { pts, rt, .. } => pts.iter().fold((f64::MAX, f64::MIN), |m, q| (m.0.min(q.0 - rt), m.1.max(q.0 + rt))),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            SpecShape::Rect { a, b, f0, f1 } => (a - b).abs() < 1.0 || (f0 - f1).abs() < 1.0,
            SpecShape::Lasso(p) => p.len() < 3,
            SpecShape::Brush { pts, .. } => pts.is_empty(),
        }
    }

    pub fn contains(&self, t: f64, f: f32) -> bool {
        match self {
            SpecShape::Rect { a, b, f0, f1 } => t >= a.min(*b) && t <= a.max(*b) && f >= f0.min(*f1) && f <= f0.max(*f1),
            SpecShape::Lasso(p) => {
                // Even-odd rule.
                let mut inside = false;
                let n = p.len();
                for i in 0..n {
                    let (xi, yi) = p[i];
                    let (xj, yj) = p[(i + n - 1) % n];
                    if (yi > f) != (yj > f) {
                        let x = xi + (f - yi) as f64 / (yj - yi) as f64 * (xj - xi);
                        if t < x {
                            inside = !inside;
                        }
                    }
                }
                inside
            }
            SpecShape::Brush { pts, rt, rf } => pts.iter().any(|(pt, pf)| {
                let dt = (t - pt) / rt.max(1e-9);
                let df = (f - pf) / rf.max(1e-9);
                dt * dt + (df * df) as f64 <= 1.0
            }),
        }
    }

    /// The shape moved by `dt` samples (to make it relative to a slice).
    pub fn shifted(&self, dt: f64) -> SpecShape {
        match self {
            SpecShape::Rect { a, b, f0, f1 } => SpecShape::Rect { a: a + dt, b: b + dt, f0: *f0, f1: *f1 },
            SpecShape::Lasso(p) => SpecShape::Lasso(p.iter().map(|(t, f)| (t + dt, *f)).collect()),
            SpecShape::Brush { pts, rt, rf } => SpecShape::Brush { pts: pts.iter().map(|(t, f)| (t + dt, *f)).collect(), rt: *rt, rf: *rf },
        }
    }
}

/// Soft mask over STFT frames × bins (0..=n/2) for `shape` (already relative
/// to the signal start): 1 inside, 0 outside, feathered by one cell.
pub fn mask(shape: &SpecShape, frames: usize, sr: u32, n: usize, hop: usize) -> Vec<Vec<f32>> {
    let bins = n / 2 + 1;
    let (lo, hi) = shape.bounds();
    let raw: Vec<Vec<f32>> = (0..frames)
        .map(|f| {
            let t = (f * hop) as f64;
            if t < lo - hop as f64 || t > hi + hop as f64 {
                return vec![0.0; bins];
            }
            (0..bins).map(|k| shape.contains(t, k as f32 * sr as f32 / n as f32) as u8 as f32).collect()
        })
        .collect();
    // 3×3 feather so edges blend rather than step.
    (0..frames)
        .map(|f| {
            (0..bins)
                .map(|k| {
                    if raw[f][k] >= 1.0 {
                        return 1.0;
                    }
                    let mut s = 0.0;
                    let mut c = 0.0;
                    for df in -1i64..=1 {
                        for dk in -1i64..=1 {
                            let (ff, kk) = (f as i64 + df, k as i64 + dk);
                            if ff >= 0 && kk >= 0 && (ff as usize) < frames && (kk as usize) < bins {
                                s += raw[ff as usize][kk as usize];
                                c += 1.0;
                            }
                        }
                    }
                    0.5 * s / c
                })
                .collect()
        })
        .collect()
}

fn set_bin(buf: &mut [Complex], k: usize, v: Complex) {
    let n = buf.len();
    buf[k] = v;
    if k != 0 && k != n / 2 {
        buf[n - k] = v.conj();
    }
}

/// Keep `orig` outside the selection and `processed` inside it.
/// `shape` is relative to the start of the slices.
pub fn blend(orig: &[Vec<f32>], processed: &[Vec<f32>], shape: &SpecShape, sr: u32) -> Vec<Vec<f32>> {
    let (n, hop) = spec_size(sr);
    orig.iter()
        .zip(processed.iter())
        .map(|(o, p)| {
            let len = o.len();
            let mut fo = stft_frames(o, n, hop);
            let fp = stft_frames(&p[..len.min(p.len())], n, hop);
            let m = mask(shape, fo.len(), sr, n, hop);
            for (f, frame) in fo.iter_mut().enumerate() {
                for k in 0..=n / 2 {
                    let w = m[f][k];
                    if w > 0.0 {
                        let v = frame[k].scale(1.0 - w) + fp.get(f).map(|x| x[k]).unwrap_or(Complex::ZERO).scale(w);
                        set_bin(frame, k, v);
                    }
                }
            }
            istft(&mut fo, n, hop, len)
        })
        .collect()
}

/// Rebuild the selected area from its surroundings: for each frequency bin,
/// the magnitude across each selected stretch is interpolated (in dB)
/// between the frames just before and just after it; phase is kept.
pub fn heal(chs: &[Vec<f32>], shape: &SpecShape, sr: u32) -> Vec<Vec<f32>> {
    let (n, hop) = spec_size(sr);
    chs.iter()
        .map(|x| {
            let len = x.len();
            let mut frames = stft_frames(x, n, hop);
            let nf = frames.len();
            let raw = mask(shape, nf, sr, n, hop);
            // Widen in time by half a window so the frames used as references
            // don't themselves contain any of what is being removed.
            let reach = (n / 2).div_ceil(hop);
            let m: Vec<Vec<f32>> = (0..nf)
                .map(|f| {
                    (0..=n / 2)
                        .map(|k| {
                            let lo = f.saturating_sub(reach);
                            let hi = (f + reach).min(nf - 1);
                            (lo..=hi).map(|g| raw[g][k]).fold(0.0, f32::max)
                        })
                        .collect()
                })
                .collect();
            for k in 0..=n / 2 {
                let mut f = 0;
                while f < nf {
                    if m[f][k] <= 0.0 {
                        f += 1;
                        continue;
                    }
                    let start = f;
                    while f < nf && m[f][k] > 0.0 {
                        f += 1;
                    }
                    let end = f; // exclusive
                    let mag = |i: usize| frames[i][k].norm().max(1e-9).ln();
                    let left = if start > 0 { Some(mag(start - 1)) } else { None };
                    let right = if end < nf { Some(mag(end)) } else { None };
                    let (l, r) = match (left, right) {
                        (Some(l), Some(r)) => (l, r),
                        (Some(l), None) => (l, l),
                        (None, Some(r)) => (r, r),
                        (None, None) => continue,
                    };
                    let span = (end - start + 1) as f32;
                    for (j, i) in (start..end).enumerate() {
                        let t = (j + 1) as f32 / span;
                        let target = (l + (r - l) * t).exp();
                        let cur = frames[i][k];
                        let norm = cur.norm();
                        let healed = if norm > 1e-12 { cur.scale(target / norm) } else { Complex::new(target, 0.0) };
                        let w = m[i][k];
                        set_bin(&mut frames[i], k, cur.scale(1.0 - w) + healed.scale(w));
                    }
                }
            }
            istft(&mut frames, n, hop, len)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn tone(f: f32, n: usize, sr: u32) -> Vec<f32> {
        (0..n).map(|i| 0.5 * (TAU * f * i as f32 / sr as f32).sin()).collect()
    }

    fn level_at(x: &[f32], f: f32, sr: u32, a: usize, b: usize) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &s) in x[a..b].iter().enumerate() {
            let ph = TAU as f64 * f as f64 * (a + i) as f64 / sr as f64;
            re += s as f64 * ph.cos();
            im += s as f64 * ph.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / (b - a) as f64) as f32
    }

    #[test]
    fn shapes_contain() {
        let r = SpecShape::Rect { a: 100.0, b: 0.0, f0: 1000.0, f1: 2000.0 };
        assert!(r.contains(50.0, 1500.0) && !r.contains(150.0, 1500.0) && !r.contains(50.0, 500.0));
        let l = SpecShape::Lasso(vec![(0.0, 0.0), (100.0, 0.0), (100.0, 1000.0), (0.0, 1000.0)]);
        assert!(l.contains(50.0, 500.0) && !l.contains(150.0, 500.0));
        let b = SpecShape::Brush { pts: vec![(50.0, 500.0)], rt: 10.0, rf: 100.0 };
        assert!(b.contains(55.0, 550.0) && !b.contains(70.0, 500.0));
        assert_eq!(b.bounds(), (40.0, 60.0));
    }

    #[test]
    fn identity_blend_reconstructs() {
        let sr = 16000;
        let x = tone(440.0, 12000, sr);
        let y = blend(&[x.clone()], &[x.clone()], &SpecShape::Rect { a: 2000.0, b: 6000.0, f0: 0.0, f1: 8000.0 }, sr);
        let worst = x.iter().zip(&y[0]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn spectral_delete_removes_only_the_selected_band() {
        let sr = 16000;
        let n = 16000;
        let x: Vec<f32> = tone(500.0, n, sr).iter().zip(tone(3000.0, n, sr)).map(|(a, b)| a + b).collect();
        // "Delete" 2.5–3.5 kHz between 0.25 s and 0.75 s.
        let shape = SpecShape::Rect { a: 4000.0, b: 12000.0, f0: 2500.0, f1: 3500.0 };
        let y = blend(&[x.clone()], &[vec![0.0; n]], &shape, sr);
        let mid = (6000, 10000);
        assert!(level_at(&y[0], 3000.0, sr, mid.0, mid.1) < 0.02, "3 kHz removed");
        assert!((level_at(&y[0], 500.0, sr, mid.0, mid.1) - 0.5).abs() < 0.02, "500 Hz kept");
        assert!((level_at(&y[0], 3000.0, sr, 0, 3000) - 0.5).abs() < 0.02, "outside the time range kept");
    }

    #[test]
    fn heal_removes_a_burst() {
        let sr = 16000;
        let n = 16000;
        let mut x = tone(800.0, n, sr);
        let clean = x.clone();
        // A loud 2 kHz burst in the middle.
        for i in 7900..8100 {
            x[i] += 0.8 * (TAU * 2000.0 * i as f32 / sr as f32).sin();
        }
        let shape = SpecShape::Rect { a: 7700.0, b: 8300.0, f0: 1500.0, f1: 2600.0 };
        let y = heal(&[x.clone()], &shape, sr);
        let before = level_at(&x, 2000.0, sr, 7900, 8100);
        let after = level_at(&y[0], 2000.0, sr, 7900, 8100);
        assert!(after < before * 0.25, "burst reduced: {before} -> {after}");
        assert!((level_at(&y[0], 800.0, sr, 7900, 8100) - level_at(&clean, 800.0, sr, 7900, 8100)).abs() < 0.05, "tone kept");
    }
}
