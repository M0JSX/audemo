//! Amplitude statistics, spectrum and stereo correlation for the analysis
//! panels.

use super::fft::{Complex, Fft};
use super::loudness;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelStats {
    pub peak_db: f64,
    pub true_peak_db: f64,
    pub max_sample: f32,
    pub min_sample: f32,
    pub clipped: usize,
    pub total_rms_db: f64,
    pub max_rms_db: f64,
    pub min_rms_db: f64,
    pub avg_rms_db: f64,
    pub dc_offset_pct: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmplitudeStats {
    pub channels: Vec<ChannelStats>,
    pub integrated_lufs: Option<f64>,
    pub loudness_range: Option<f64>,
    pub true_peak_db: f64,
    pub duration_s: f64,
}

fn db(x: f64) -> f64 {
    if x <= 1e-12 {
        -200.0
    } else {
        20.0 * x.log10()
    }
}

pub fn amplitude_stats(chs: &[Vec<f32>], sr: u32) -> AmplitudeStats {
    let win = (sr as usize / 20).max(1); // 50 ms RMS windows
    let channels = chs
        .iter()
        .map(|c| {
            let mut st = ChannelStats { min_sample: 0.0, max_sample: 0.0, ..Default::default() };
            let mut sum = 0.0f64;
            let mut sq = 0.0f64;
            let mut run = 0usize;
            for &s in c {
                st.max_sample = st.max_sample.max(s);
                st.min_sample = st.min_sample.min(s);
                sum += s as f64;
                sq += (s as f64) * (s as f64);
                if s.abs() >= 0.999 {
                    run += 1;
                    if run == 3 {
                        st.clipped += 3;
                    } else if run > 3 {
                        st.clipped += 1;
                    }
                } else {
                    run = 0;
                }
            }
            let n = c.len().max(1) as f64;
            st.peak_db = db(st.max_sample.abs().max(st.min_sample.abs()) as f64);
            st.true_peak_db = loudness::true_peak_db(std::slice::from_ref(c));
            st.total_rms_db = db((sq / n).sqrt());
            st.dc_offset_pct = sum / n * 100.0;
            let mut wins: Vec<f64> = c
                .chunks(win)
                .filter(|w| w.len() == win)
                .map(|w| (w.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / win as f64).sqrt())
                .collect();
            if wins.is_empty() {
                wins.push((sq / n).sqrt());
            }
            let max = wins.iter().cloned().fold(0.0, f64::max);
            // Ignore digital silence for the minimum, as Audition does.
            let min = wins.iter().cloned().filter(|&v| v > 1e-7).fold(f64::MAX, f64::min);
            let avg = wins.iter().sum::<f64>() / wins.len() as f64;
            st.max_rms_db = db(max);
            st.min_rms_db = if min == f64::MAX { -200.0 } else { db(min) };
            st.avg_rms_db = db(avg);
            st
        })
        .collect();
    let l = loudness::measure(chs, sr);
    AmplitudeStats {
        channels,
        integrated_lufs: l.integrated,
        loudness_range: l.range,
        true_peak_db: l.true_peak_db,
        duration_s: chs.first().map(|c| c.len()).unwrap_or(0) as f64 / sr as f64,
    }
}

/// Magnitude spectrum (dBFS per bin, 0..=n/2) of `n` samples centred on
/// `centre`, Blackman-Harris windowed and scaled so a full-scale sine reads 0 dB.
pub fn spectrum_at(x: &[f32], centre: usize, n: usize) -> Vec<f32> {
    let w: Vec<f32> = (0..n)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / n as f64;
            (0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()) as f32
        })
        .collect();
    let norm = 2.0 / w.iter().sum::<f32>();
    let fft = Fft::new(n);
    let start = centre as i64 - n as i64 / 2;
    let mut buf: Vec<Complex> = (0..n)
        .map(|i| {
            let idx = start + i as i64;
            let s = if idx >= 0 && (idx as usize) < x.len() { x[idx as usize] } else { 0.0 };
            Complex::new(s * w[i], 0.0)
        })
        .collect();
    fft.forward(&mut buf);
    (0..=n / 2)
        .map(|k| {
            let m = buf[k].norm() * norm;
            if m > 1e-9 { 20.0 * m.log10() } else { -180.0 }
        })
        .collect()
}

/// Average power spectrum over [a, b) using hops of n/2.
pub fn spectrum_average(x: &[f32], a: usize, b: usize, n: usize) -> Vec<f32> {
    let bins = n / 2 + 1;
    let mut acc = vec![0.0f64; bins];
    let mut count = 0usize;
    let mut c = a + n / 2;
    let max_frames = 400;
    let hop = ((b.saturating_sub(a)) / max_frames).max(n / 2);
    while c + n / 2 <= b.max(a + n) && count < max_frames {
        for (k, v) in spectrum_at(x, c, n).into_iter().enumerate() {
            acc[k] += 10f64.powf(v as f64 / 10.0);
        }
        count += 1;
        c += hop;
    }
    acc.into_iter()
        .map(|p| {
            let p = p / count.max(1) as f64;
            if p > 1e-18 { (10.0 * p.log10()) as f32 } else { -180.0 }
        })
        .collect()
}

/// Pearson correlation of L and R over a window: +1 mono, 0 wide, -1 out of phase.
pub fn correlation(l: &[f32], r: &[f32]) -> f32 {
    let (mut lr, mut ll, mut rr) = (0.0f64, 0.0f64, 0.0f64);
    for (a, b) in l.iter().zip(r) {
        lr += (*a as f64) * (*b as f64);
        ll += (*a as f64) * (*a as f64);
        rr += (*b as f64) * (*b as f64);
    }
    let d = (ll * rr).sqrt();
    if d < 1e-12 {
        0.0
    } else {
        (lr / d) as f32
    }
}
