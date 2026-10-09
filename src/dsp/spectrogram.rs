//! Spectrogram for the spectral frequency display.

use super::fft::{Complex, Fft};
use super::util::hann;

/// Compute `cols` spectra covering samples [start, end) of `x`.
/// Returns magnitudes in dBFS, column-major: `out[col * bins + bin]`,
/// with `bins = fft_size / 2`.
pub fn spectrogram_range(
    x: &[f32],
    start: f64,
    end: f64,
    cols: usize,
    fft_size: usize,
) -> Vec<f32> {
    let bins = fft_size / 2;
    let mut out = vec![-160.0f32; cols * bins];
    if x.is_empty() || cols == 0 || end <= start {
        return out;
    }
    let w = hann(fft_size);
    let wsum: f32 = w.iter().sum();
    let norm = 2.0 / wsum;
    let fft = Fft::new(fft_size);
    let mut buf = vec![Complex::ZERO; fft_size];
    let step = (end - start) / cols as f64;
    let half = (fft_size / 2) as i64;
    for col in 0..cols {
        let centre = (start + (col as f64 + 0.5) * step) as i64;
        let s0 = centre - half;
        let mut energy = 0.0f32;
        for i in 0..fft_size {
            let idx = s0 + i as i64;
            let v = if idx >= 0 && (idx as usize) < x.len() { x[idx as usize] } else { 0.0 };
            energy += v.abs();
            buf[i] = Complex::new(v * w[i], 0.0);
        }
        if energy == 0.0 {
            continue;
        }
        fft.forward(&mut buf);
        let row = &mut out[col * bins..(col + 1) * bins];
        for k in 0..bins {
            let m = buf[k].norm() * norm;
            row[k] = if m > 1e-8 { 20.0 * m.log10() } else { -160.0 };
        }
    }
    out
}

/// Heat-map colour for a level in dB (floor..0).
pub fn heat_colour(db: f32, floor: f32) -> [u8; 3] {
    const STOPS: [(f32, [f32; 3]); 6] = [
        (0.00, [4.0, 4.0, 12.0]),
        (0.22, [28.0, 14.0, 96.0]),
        (0.45, [128.0, 22.0, 140.0]),
        (0.65, [222.0, 50.0, 60.0]),
        (0.83, [250.0, 150.0, 30.0]),
        (1.00, [255.0, 248.0, 205.0]),
    ];
    let t = ((db - floor) / -floor).clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let u = (t - t0) / (t1 - t0);
            return [
                (c0[0] + (c1[0] - c0[0]) * u) as u8,
                (c0[1] + (c1[1] - c0[1]) * u) as u8,
                (c0[2] + (c1[2] - c0[2]) * u) as u8,
            ];
        }
    }
    [255, 248, 205]
}
