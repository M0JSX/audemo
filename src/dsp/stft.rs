//! Short-time Fourier transform framework (Hann, overlap-add).

use super::fft::{Complex, Fft};
use super::util::hann;

/// Analyse `x`, let `f` modify each frame's full spectrum in place, then
/// resynthesise. `f` gets (spectrum, frame index). Output length == input.
pub fn stft_process(
    x: &[f32],
    n: usize,
    hop: usize,
    mut f: impl FnMut(&mut [Complex], usize),
) -> Vec<f32> {
    stft_process_multi(&[x], n, hop, |bufs, frame| f(&mut bufs[0], frame))
        .pop()
        .unwrap_or_default()
}

/// Like [`stft_process`] but frames several equal-length signals together so
/// the closure can look across channels (e.g. stereo centre extraction).
pub fn stft_process_multi(
    xs: &[&[f32]],
    n: usize,
    hop: usize,
    mut f: impl FnMut(&mut [Vec<Complex>], usize),
) -> Vec<Vec<f32>> {
    let len = xs.iter().map(|x| x.len()).min().unwrap_or(0);
    if len == 0 {
        return xs.iter().map(|_| Vec::new()).collect();
    }
    let w = hann(n);
    let fft = Fft::new(n);
    let pad = n;
    let total = len + 2 * pad + n;
    let mut outs = vec![vec![0.0f32; total]; xs.len()];
    let mut wsum = vec![0.0f32; total];
    let mut bufs = vec![vec![Complex::ZERO; n]; xs.len()];
    let mut pos = 0usize;
    let mut frame = 0usize;
    while pos < len + pad {
        for (x, buf) in xs.iter().zip(bufs.iter_mut()) {
            for i in 0..n {
                let idx = pos + i;
                let s = if idx >= pad && idx - pad < len { x[idx - pad] } else { 0.0 };
                buf[i] = Complex::new(s * w[i], 0.0);
            }
            fft.forward(buf);
        }
        f(&mut bufs, frame);
        for (buf, out) in bufs.iter_mut().zip(outs.iter_mut()) {
            fft.inverse(buf);
            for i in 0..n {
                out[pos + i] += buf[i].re * w[i];
            }
        }
        for i in 0..n {
            wsum[pos + i] += w[i] * w[i];
        }
        pos += hop;
        frame += 1;
    }
    outs.into_iter()
        .map(|out| {
            (0..len)
                .map(|i| {
                    let ws = wsum[i + pad];
                    if ws > 1e-4 {
                        out[i + pad] / ws
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect()
}

/// Mean magnitude per bin (0..=n/2) of a signal, used for noise prints.
pub fn mean_magnitude(x: &[f32], n: usize) -> Vec<f32> {
    let bins = n / 2 + 1;
    let mut acc = vec![0.0f64; bins];
    let w = hann(n);
    let fft = Fft::new(n);
    let mut buf = vec![Complex::ZERO; n];
    let hop = n / 4;
    let mut frames = 0usize;
    let mut pos = 0;
    while pos + n <= x.len() {
        for i in 0..n {
            buf[i] = Complex::new(x[pos + i] * w[i], 0.0);
        }
        fft.forward(&mut buf);
        for k in 0..bins {
            acc[k] += buf[k].norm() as f64;
        }
        frames += 1;
        pos += hop;
    }
    let d = frames.max(1) as f64;
    acc.into_iter().map(|v| (v / d) as f32).collect()
}

/// Multiply bins 0..=n/2 by `gains` keeping conjugate symmetry.
pub fn apply_gains(buf: &mut [Complex], gains: &[f32]) {
    let n = buf.len();
    for k in 0..=n / 2 {
        let g = gains[k];
        buf[k] = buf[k].scale(g);
        if k != 0 && k != n / 2 {
            buf[n - k] = buf[n - k].scale(g);
        }
    }
}
