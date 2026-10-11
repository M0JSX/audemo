//! Remix: change a piece of music's length by cutting or repeating whole
//! beats where the music sounds alike, then trimming the last fraction of a
//! beat with a small time stretch, so the result has the length asked for.

use super::fft::Complex;
use super::stft::stft_frames;

const HOP: usize = 512;
const N_FFT: usize = 2048;
const BANDS: usize = 32;

/// Beat grid of a piece: beat start positions in samples and the beat length.
#[derive(Clone, Debug)]
pub struct Beats {
    pub starts: Vec<usize>,
    pub period: f64,
    /// Per-beat spectral features (unit length).
    feats: Vec<[f32; BANDS]>,
}

impl Beats {
    pub fn bpm(&self, sr: u32) -> f64 {
        60.0 * sr as f64 / self.period
    }
}

fn band_edges(n: usize, sr: u32) -> Vec<usize> {
    // Log-spaced bands from 40 Hz to 16 kHz.
    let bins = n / 2;
    (0..=BANDS)
        .map(|b| {
            let f = 40.0 * (16000.0f64 / 40.0).powf(b as f64 / BANDS as f64);
            ((f * n as f64 / sr as f64) as usize).clamp(1, bins)
        })
        .collect()
}

/// Find the beat grid (None for audio too short or without a pulse).
pub fn analyse(mono: &[f32], sr: u32) -> Option<Beats> {
    let frames = stft_frames(mono, N_FFT, HOP);
    if frames.len() < 64 {
        return None;
    }
    let edges = band_edges(N_FFT, sr);
    let feat: Vec<[f32; BANDS]> = frames
        .iter()
        .map(|fr: &Vec<Complex>| {
            let mut f = [0.0f32; BANDS];
            for b in 0..BANDS {
                let (lo, hi) = (edges[b], edges[b + 1].max(edges[b] + 1));
                let e: f32 = fr[lo..hi].iter().map(|c| c.norm_sqr()).sum::<f32>() / (hi - lo) as f32;
                f[b] = (1e-9 + e).ln();
            }
            f
        })
        .collect();
    // Onset strength: summed positive change across bands.
    let onset: Vec<f32> = (0..feat.len()).map(|t| if t == 0 { 0.0 } else { (0..BANDS).map(|b| (feat[t][b] - feat[t - 1][b]).max(0.0)).sum() }).collect();
    let mean = onset.iter().sum::<f32>() / onset.len() as f32;
    let o: Vec<f32> = onset.iter().map(|v| (v - mean).max(0.0)).collect();
    // Tempo from the onset autocorrelation, 60–180 BPM, leaning to ~120.
    let fps = sr as f64 / HOP as f64;
    let (lo, hi) = ((fps * 60.0 / 180.0) as usize, (fps * 60.0 / 60.0) as usize);
    let ac = |lag: usize| -> f64 { (0..o.len().saturating_sub(lag)).map(|t| (o[t] * o[t + lag]) as f64).sum() };
    let mut best = (0usize, f64::MIN);
    let mut acs = vec![0.0f64; hi + 2];
    for lag in lo.max(2)..=hi.min(o.len() / 4) {
        acs[lag] = ac(lag);
        let bpm = 60.0 * fps / lag as f64;
        let weight = (-((bpm / 120.0).log2()).powi(2) / (2.0 * 0.9f64.powi(2))).exp();
        let score = acs[lag] * weight;
        if score > best.1 {
            best = (lag, score);
        }
    }
    if best.0 == 0 || best.1 <= 0.0 {
        return None;
    }
    let lag = best.0;
    let (a, b, c) = (acs[lag - 1], acs[lag], acs[(lag + 1).min(hi + 1)]);
    let den = a - 2.0 * b + c;
    let period_frames = lag as f64 + if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
    // Phase: the offset whose grid lands on the most onsets.
    let mut phase = (0.0f64, f64::MIN);
    let steps = (period_frames.ceil() as usize).max(1);
    for p in 0..steps {
        let mut s = 0.0f64;
        let mut k = 0usize;
        loop {
            let t = (p as f64 + k as f64 * period_frames).round() as usize;
            if t >= o.len() {
                break;
            }
            s += o[t] as f64;
            k += 1;
        }
        if s > phase.1 {
            phase = (p as f64, s);
        }
    }
    let mut starts = Vec::new();
    let mut feats = Vec::new();
    let mut k = 0usize;
    loop {
        let t0 = phase.0 + k as f64 * period_frames;
        let t1 = t0 + period_frames;
        if t1 as usize >= feat.len() {
            break;
        }
        starts.push((t0 * HOP as f64) as usize);
        let mut f = [0.0f32; BANDS];
        let (a, b) = (t0 as usize, (t1 as usize).max(t0 as usize + 1));
        for t in a..b {
            for (x, y) in f.iter_mut().zip(feat[t].iter()) {
                *x += *y;
            }
        }
        feats.push(f);
        k += 1;
    }
    if starts.len() < 16 {
        return None;
    }
    // Normalise each band across the piece, then each beat to unit length.
    for bnd in 0..BANDS {
        let m = feats.iter().map(|f| f[bnd]).sum::<f32>() / feats.len() as f32;
        let sd = (feats.iter().map(|f| (f[bnd] - m).powi(2)).sum::<f32>() / feats.len() as f32).sqrt().max(1e-6);
        for f in feats.iter_mut() {
            f[bnd] = (f[bnd] - m) / sd;
        }
    }
    for f in feats.iter_mut() {
        let n = f.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        f.iter_mut().for_each(|x| *x /= n);
    }
    Some(Beats { starts, period: period_frames * HOP as f64, feats })
}

/// How alike the music is starting at beats `i` and `j` (four beats compared).
fn similarity(b: &Beats, i: usize, j: usize) -> f32 {
    let n = b.feats.len();
    let mut s = 0.0f32;
    let mut k = 0;
    for m in 0..4 {
        if i + m >= n || j + m >= n {
            break;
        }
        s += b.feats[i + m].iter().zip(b.feats[j + m].iter()).map(|(x, y)| x * y).sum::<f32>();
        k += 1;
    }
    if k == 0 {
        -1.0
    } else {
        s / k as f32
    }
}

/// The best jump of `span` beats: (i, j) with j = i + span, both away from
/// the first and last bars, where beat i and beat j sound most alike.
fn best_jump(b: &Beats, span: usize) -> Option<(usize, usize, f32)> {
    let n = b.starts.len();
    let mut best: Option<(usize, usize, f32)> = None;
    for i in 4..n.saturating_sub(span + 4) {
        let j = i + span;
        // Prefer bar-aligned cuts (multiples of four beats from the start).
        let s = similarity(b, i, j) + if i % 4 == 0 { 0.05 } else { 0.0 };
        if best.map(|x| s > x.2).unwrap_or(true) {
            best = Some((i, j, s));
        }
    }
    best
}

/// Join `parts` (sample ranges of `chans`) with short equal-power crossfades.
fn assemble(chans: &[Vec<f32>], parts: &[(usize, usize)], xfade: usize) -> Vec<Vec<f32>> {
    chans
        .iter()
        .map(|c| {
            let mut out: Vec<f32> = Vec::new();
            for (k, &(a, b)) in parts.iter().enumerate() {
                let seg = &c[a.min(c.len())..b.min(c.len())];
                if k == 0 || out.is_empty() {
                    out.extend_from_slice(seg);
                    continue;
                }
                let x = xfade.min(out.len()).min(seg.len());
                let n0 = out.len() - x;
                for i in 0..x {
                    let t = (i as f32 + 0.5) / x as f32;
                    let (g_out, g_in) = ((t * std::f32::consts::FRAC_PI_2).cos(), (t * std::f32::consts::FRAC_PI_2).sin());
                    out[n0 + i] = out[n0 + i] * g_out + seg[i] * g_in;
                }
                out.extend_from_slice(&seg[x..]);
            }
            out
        })
        .collect()
}

/// The piece cut or extended to about `target` samples on beat
/// boundaries (within a beat), or None if no beat grid was found.
pub fn remix_beats(chans: &[Vec<f32>], sr: u32, target: usize) -> Option<Vec<Vec<f32>>> {
    let len = chans.first()?.len();
    let k = 1.0 / chans.len().max(1) as f32;
    let mono: Vec<f32> = (0..len).map(|i| chans.iter().map(|c| c[i]).sum::<f32>() * k).collect();
    let b = analyse(&mono, sr)?;
    let n = b.starts.len();
    let beat = b.period;
    let delta_beats = (target as f64 - len as f64) / beat;
    let xfade = (sr as usize * 30 / 1000).max(16);
    let at = |k: usize| if k >= n { len } else { b.starts[k] };
    if delta_beats.abs() < 1.0 {
        return Some(chans.to_vec());
    }
    let round4 = |x: f64| if x.abs() >= 8.0 { ((x / 4.0).round() * 4.0) as usize } else { x.round() as usize };
    if delta_beats < 0.0 {
        // Shorten: play up to beat i, then carry on from beat j.
        let span = round4(-delta_beats).max(1);
        if span + 8 >= n {
            return None;
        }
        let (i, j, _) = best_jump(&b, span)?;
        Some(assemble(chans, &[(0, at(i)), (at(j), len)], xfade))
    } else {
        // Lengthen: at beat j jump back to beat i (and again if needed).
        let want = round4(delta_beats).max(1);
        let max_span = n.saturating_sub(8).max(4);
        let span = want.min(max_span).max(4.min(max_span));
        let (i, j, _) = best_jump(&b, span)?;
        let repeats = (want as f64 / span as f64).round().max(1.0) as usize;
        let mut parts = vec![(0, at(j))];
        for _ in 1..repeats {
            parts.push((at(i), at(j)));
        }
        parts.push((at(i), len));
        Some(assemble(chans, &parts, xfade))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM: a kick-like click every beat, chords changing every bar.
    fn music(secs: f32, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f32) as usize;
        let beat = sr as usize / 2;
        (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let in_beat = i % beat;
                let kick = if in_beat < 2000 { (1.0 - in_beat as f32 / 2000.0) * (t * 60.0 * std::f32::consts::TAU).sin() } else { 0.0 };
                let bar = (i / (beat * 4)) % 4;
                let f = [220.0, 261.6, 196.0, 174.6][bar];
                kick * 0.6 + (t * f * std::f32::consts::TAU).sin() * 0.2
            })
            .collect()
    }

    #[test]
    fn finds_the_tempo() {
        let sr = 22050;
        let b = analyse(&music(20.0, sr), sr).expect("beats");
        assert!((b.bpm(sr) - 120.0).abs() < 2.0, "{} BPM", b.bpm(sr));
    }

    #[test]
    fn shortens_and_lengthens_by_whole_beats() {
        let sr = 22050;
        let x = vec![music(30.0, sr)];
        let beat = sr as f64 / 2.0;
        let short = remix_beats(&x, sr, (20.0 * sr as f32) as usize).unwrap();
        assert!(((short[0].len() as f64) - 20.0 * sr as f64).abs() <= beat + 1.0, "{}", short[0].len() as f64 / sr as f64);
        let long = remix_beats(&x, sr, (45.0 * sr as f32) as usize).unwrap();
        assert!(((long[0].len() as f64) - 45.0 * sr as f64).abs() <= beat * 2.0 + 1.0, "{}", long[0].len() as f64 / sr as f64);
    }
}
