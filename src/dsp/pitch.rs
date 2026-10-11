//! Pitch tracking (YIN) and pitch-synchronous overlap-add (PSOLA) pitch
//! shifting that keeps the length, for Pitch Correction and the pitch display.

use super::resample::resample;

/// Fundamental frequency over time: `f0[i]` (Hz, 0 = unvoiced) is at
/// sample `i * hop` of the analysed signal's rate.
#[derive(Clone, Debug, Default)]
pub struct PitchTrack {
    pub hop: usize,
    pub sample_rate: u32,
    pub f0: Vec<f32>,
}

impl PitchTrack {
    /// f0 at sample `t` (nearest frame).
    pub fn at(&self, t: f64) -> f32 {
        if self.f0.is_empty() || self.hop == 0 {
            return 0.0;
        }
        let i = (t / self.hop as f64).round().max(0.0) as usize;
        self.f0[i.min(self.f0.len() - 1)]
    }
}

/// The analysis rate: plenty for voices and instruments up to ~1 kHz.
const DET_RATE: u32 = 16000;

/// Track the pitch of `x` (one channel) every 10 ms with YIN. `threshold`
/// is YIN's aperiodicity limit (0.05 strict … 0.5 lenient); quieter than
/// -50 dBFS counts as unvoiced.
pub fn track(x: &[f32], sr: u32, f_min: f32, f_max: f32, threshold: f32) -> PitchTrack {
    let hop = (sr as usize / 100).max(1);
    let n_frames = x.len() / hop + 1;
    let y = if sr != DET_RATE { resample(x, DET_RATE as f64 / sr as f64) } else { x.to_vec() };
    let w = 400; // 25 ms integration window
    let max_lag = ((DET_RATE as f32 / f_min.max(40.0)) as usize).min(800);
    let min_lag = ((DET_RATE as f32 / f_max.min(2000.0)) as usize).max(2);
    let mut f0 = vec![0.0f32; n_frames];
    let mut d = vec![0.0f32; max_lag + 2];
    for (fi, out) in f0.iter_mut().enumerate() {
        // Frame centred on the hop position.
        let centre = (fi as f64 * hop as f64 * DET_RATE as f64 / sr as f64) as isize;
        let start = centre - (w as isize + max_lag as isize) / 2;
        if start < 0 || start as usize + w + max_lag + 1 >= y.len() {
            continue;
        }
        let fr = &y[start as usize..start as usize + w + max_lag + 1];
        let energy: f32 = fr[..w].iter().map(|s| s * s).sum::<f32>() / w as f32;
        if energy < 1e-5 {
            continue; // below -50 dBFS
        }
        // Difference function and its cumulative-mean normalisation.
        for (tau, dt) in d.iter_mut().enumerate().take(max_lag + 1).skip(1) {
            let mut acc = 0.0f32;
            for j in 0..w {
                let e = fr[j] - fr[j + tau];
                acc += e * e;
            }
            *dt = acc;
        }
        let mut running = 0.0f32;
        let mut best: Option<usize> = None;
        let mut cm = vec![1.0f32; max_lag + 1];
        for tau in 1..=max_lag {
            running += d[tau];
            cm[tau] = if running > 0.0 { d[tau] * tau as f32 / running } else { 1.0 };
        }
        let mut tau = min_lag;
        while tau < max_lag {
            if cm[tau] < threshold {
                while tau + 1 < max_lag && cm[tau + 1] < cm[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let Some(t) = best else { continue };
        // Parabolic interpolation around the minimum.
        let (a, b, c) = (cm[t - 1], cm[t], cm[(t + 1).min(max_lag)]);
        let den = a - 2.0 * b + c;
        let shift = if den.abs() > 1e-9 { 0.5 * (a - c) / den } else { 0.0 };
        let period = t as f32 + shift.clamp(-1.0, 1.0);
        let f = DET_RATE as f32 / period;
        if f >= f_min && f <= f_max {
            *out = f;
        }
    }
    // Drop single-frame octave blips and isolated voiced frames.
    let raw = f0.clone();
    for i in 1..f0.len().saturating_sub(1) {
        let (p, c, n) = (raw[i - 1], raw[i], raw[i + 1]);
        if c > 0.0 && p == 0.0 && n == 0.0 {
            f0[i] = 0.0;
        } else if c > 0.0 && p > 0.0 && n > 0.0 {
            let ratio = c / (0.5 * (p + n));
            if !(0.7..=1.4).contains(&ratio) {
                f0[i] = 0.5 * (p + n);
            }
        }
    }
    PitchTrack { hop, sample_rate: sr, f0 }
}

/// Shift pitch by `ratio(t)` (2.0 = an octave up) at each sample `t`,
/// keeping the length: voiced stretches are cut into pitch periods (from
/// `pitch`) and laid down closer together or further apart; unvoiced
/// stretches are passed through.
pub fn psola(input: &[Vec<f32>], pitch: &PitchTrack, ratio: impl Fn(usize) -> f32) -> Vec<Vec<f32>> {
    let len = input.first().map(|c| c.len()).unwrap_or(0);
    if len == 0 {
        return input.to_vec();
    }
    let sr = pitch.sample_rate.max(1) as f64;
    let step_unvoiced = (sr * 0.005).max(8.0);
    let mono: Vec<f32> = (0..len).map(|i| input.iter().map(|c| c[i]).sum::<f32>()).collect();
    // Analysis marks: one per pitch period, on the local peak.
    let mut marks: Vec<(usize, f64, bool)> = Vec::new(); // (position, period, voiced)
    let mut t = 0.0f64;
    while (t as usize) < len {
        let f = pitch.at(t);
        if f > 0.0 {
            let period = sr / f as f64;
            let guess = t as usize;
            let r = (period * 0.25) as usize;
            let lo = guess.saturating_sub(r);
            let hi = (guess + r).min(len - 1);
            let peak = (lo..=hi).max_by(|&a, &b| mono[a].partial_cmp(&mono[b]).unwrap()).unwrap_or(guess);
            let pos = if marks.last().map(|m| peak > m.0).unwrap_or(true) { peak } else { guess };
            marks.push((pos, period, true));
            t = pos as f64 + period;
        } else {
            marks.push((t as usize, step_unvoiced, false));
            t += step_unvoiced;
        }
    }
    let mut out = vec![vec![0.0f32; len]; input.len()];
    let mut wsum = vec![0.0f32; len];
    let mut s = 0.0f64; // synthesis position
    let mut k = 0usize;
    while (s as usize) < len {
        // The analysis mark nearest the synthesis position.
        while k + 1 < marks.len() && (marks[k + 1].0 as f64) <= s {
            k += 1;
        }
        let pick = if k + 1 < marks.len() && (marks[k + 1].0 as f64 - s) < (s - marks[k].0 as f64) { k + 1 } else { k };
        let (centre, period, voiced) = marks[pick];
        let r = if voiced { (ratio(s as usize) as f64).clamp(0.25, 4.0) } else { 1.0 };
        let half = period.round().max(2.0) as isize;
        let dst = s.round() as isize;
        // A Hann window two periods long, centred on the mark.
        for i in -half..half {
            let src = centre as isize + i;
            let d = dst + i;
            if src < 0 || src >= len as isize || d < 0 || d >= len as isize {
                continue;
            }
            let w = 0.5 + 0.5 * (std::f64::consts::PI * i as f64 / half as f64).cos();
            let w = w as f32;
            for (o, c) in out.iter_mut().zip(input) {
                o[d as usize] += c[src as usize] * w;
            }
            wsum[d as usize] += w;
        }
        s += if voiced { period / r } else { period };
    }
    for (i, w) in wsum.iter().enumerate() {
        if *w > 1e-3 {
            for o in out.iter_mut() {
                o[i] /= w;
            }
        }
    }
    out
}

/// MIDI note number (69 = A4) of a frequency.
pub fn midi_of(f: f32, a4: f32) -> f32 {
    69.0 + 12.0 * (f / a4).log2()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f32, secs: f32, sr: u32) -> Vec<f32> {
        // A voice-like tone: fundamental plus harmonics.
        (0..(secs * sr as f32) as usize)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (1..=5).map(|h| (std::f32::consts::TAU * f * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.3
            })
            .collect()
    }

    #[test]
    fn tracks_a_steady_pitch() {
        let x = sine(220.0, 1.0, 44100);
        let tr = track(&x, 44100, 60.0, 1000.0, 0.15);
        let mid: Vec<f32> = tr.f0[10..90].to_vec();
        assert!(mid.iter().all(|f| (f - 220.0).abs() < 2.0), "{:?}", &mid[..5]);
    }

    #[test]
    fn psola_shifts_pitch_and_keeps_length() {
        let sr = 44100;
        let x = sine(200.0, 1.0, sr);
        let tr = track(&x, sr, 60.0, 1000.0, 0.15);
        let ratio = 2f32.powf(2.0 / 12.0); // up a whole tone
        let y = psola(&[x.clone()], &tr, |_| ratio);
        assert_eq!(y[0].len(), x.len());
        let t2 = track(&y[0], sr, 60.0, 1000.0, 0.15);
        let f: Vec<f32> = t2.f0[20..80].iter().copied().filter(|f| *f > 0.0).collect();
        let mean = f.iter().sum::<f32>() / f.len() as f32;
        assert!((mean - 200.0 * ratio).abs() < 4.0, "shifted to {mean}");
    }
}
