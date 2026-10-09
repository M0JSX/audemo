use super::prelude::*;
use super::Category::TimePitch;
use std::f64::consts::TAU;

const FFT_SIZES: &[&str] = &["1024 (percussive)", "2048 (balanced)", "4096 (tonal)"];

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "stretch_pitch",
            "Stretch and Pitch",
            TimePitch,
            "Change duration and pitch independently (phase vocoder with peak phase locking).",
            vec![
                float("stretch", "Stretch (new length)", 25.0, 400.0, 100.0, "%"),
                float("semitones", "Pitch shift", -24.0, 24.0, 0.0, "semitones"),
                float("cents", "Fine tune", -100.0, 100.0, 0.0, "cents").decimals(0),
                choice("fft", "Precision", FFT_SIZES, 1),
            ],
            stretch_pitch,
        )
        .changes_length()
        .presets(vec![
            ("Slow Down 20%", vec![("stretch", Value::F(125.0))]),
            ("Speed Up 20%", vec![("stretch", Value::F(80.0))]),
            ("Octave Up", vec![("semitones", Value::F(12.0))]),
            ("Octave Down", vec![("semitones", Value::F(-12.0))]),
        ]),
        EffectDef::new(
            "pitch_shifter",
            "Pitch Shifter",
            TimePitch,
            "Transpose without changing length.",
            vec![
                float("semitones", "Semitones", -24.0, 24.0, 0.0, "st").decimals(0),
                float("cents", "Cents", -100.0, 100.0, 0.0, "cents").decimals(0),
                choice("fft", "Precision", FFT_SIZES, 1),
            ],
            |i, c, p| {
                let mut q = p.clone();
                q.set("stretch", Value::F(100.0));
                stretch_pitch(i, c, &q)
            },
        )
        .presets(vec![
            ("Chipmunk", vec![("semitones", Value::F(7.0))]),
            ("Deep Voice", vec![("semitones", Value::F(-5.0))]),
        ]),
        EffectDef::new(
            "varispeed",
            "Varispeed",
            TimePitch,
            "Tape-style speed change: pitch and length change together.",
            vec![float("speed", "Speed", 25.0, 400.0, 100.0, "%")],
            |i, _, p| {
                let ratio = 100.0 / p.f("speed").max(1.0) as f64;
                Ok(resample_channels(i, ratio))
            },
        )
        .changes_length()
        .presets(vec![
            ("33 to 45 RPM", vec![("speed", Value::F(135.0))]),
            ("45 to 33 RPM", vec![("speed", Value::F(74.07))]),
        ]),
    ]
}

fn stretch_pitch(i: &[Vec<f32>], _: &Ctx, p: &Params) -> Res {
    let stretch = (p.f("stretch") / 100.0).clamp(0.1, 10.0) as f64;
    let pitch = 2f64.powf((p.f("semitones") as f64 + p.f("cents") as f64 / 100.0) / 12.0);
    let n = 1024 << p.c("fft").min(2);
    if (stretch - 1.0).abs() < 1e-6 && (pitch - 1.0).abs() < 1e-6 {
        return Ok(i.to_vec());
    }
    let target_len = (len_of(i) as f64 * stretch).round() as usize;
    Ok(i.iter()
        .map(|ch| {
            let stretched = pv_stretch(ch, stretch * pitch, n);
            let mut y = resample(&stretched, 1.0 / pitch);
            y.resize(target_len, 0.0);
            y
        })
        .collect())
}

/// Phase-vocoder time stretch with identity phase locking (Laroche & Dolson).
/// `factor` = output length / input length.
pub fn pv_stretch(x: &[f32], factor: f64, n: usize) -> Vec<f32> {
    if x.is_empty() || (factor - 1.0).abs() < 1e-9 {
        return x.to_vec();
    }
    let hs = n / 4;
    let ha = hs as f64 / factor;
    let bins = n / 2 + 1;
    let w = hann(n);
    let fft = Fft::new(n);
    let pad = n;
    let padded: Vec<f32> = std::iter::repeat(0.0)
        .take(pad)
        .chain(x.iter().copied())
        .chain(std::iter::repeat(0.0).take(pad + n))
        .collect();
    let out_len = (x.len() as f64 * factor).round() as usize;
    let frames = ((padded.len() - n) as f64 / ha).floor() as usize + 1;
    let total_out = frames * hs + n;
    let mut out = vec![0.0f32; total_out];
    let mut wsum = vec![0.0f32; total_out];
    let mut prev_phase = vec![0.0f64; bins];
    let mut sum_phase = vec![0.0f64; bins];
    let mut buf = vec![Complex::ZERO; n];
    let mut mag = vec![0.0f32; bins];
    let mut phase = vec![0.0f64; bins];
    let mut last_pos: Option<usize> = None;
    for j in 0..frames {
        let pos = (j as f64 * ha).round() as usize;
        if pos + n > padded.len() {
            break;
        }
        for i in 0..n {
            buf[i] = Complex::new(padded[pos + i] * w[i], 0.0);
        }
        fft.forward(&mut buf);
        for k in 0..bins {
            mag[k] = buf[k].norm();
            phase[k] = buf[k].arg() as f64;
        }
        match last_pos {
            None => sum_phase.copy_from_slice(&phase),
            Some(lp) => {
                let hop_a = (pos - lp).max(1) as f64;
                // Advance every bin, then lock non-peak bins to their peak.
                for k in 0..bins {
                    let omega = TAU * k as f64 / n as f64;
                    let dphi = wrap_phase(phase[k] - prev_phase[k] - omega * hop_a);
                    let freq = omega + dphi / hop_a;
                    sum_phase[k] += freq * hs as f64;
                }
                let mut peaks = Vec::new();
                for k in 2..bins.saturating_sub(2) {
                    let m = mag[k];
                    if m > mag[k - 1] && m >= mag[k + 1] && m > mag[k - 2] && m >= mag[k + 2] {
                        peaks.push(k);
                    }
                }
                if !peaks.is_empty() {
                    let mut pi = 0;
                    let locked = sum_phase.clone();
                    for k in 0..bins {
                        while pi + 1 < peaks.len()
                            && (peaks[pi + 1] as i64 - k as i64).abs() < (peaks[pi] as i64 - k as i64).abs()
                        {
                            pi += 1;
                        }
                        let pk = peaks[pi];
                        if k != pk {
                            sum_phase[k] = locked[pk] + (phase[k] - phase[pk]);
                        }
                    }
                }
            }
        }
        prev_phase.copy_from_slice(&phase);
        last_pos = Some(pos);
        for k in 0..bins {
            let c = Complex::from_polar(mag[k], sum_phase[k] as f32);
            buf[k] = c;
            if k != 0 && k != n / 2 {
                buf[n - k] = c.conj();
            }
        }
        buf[0].im = 0.0;
        buf[n / 2].im = 0.0;
        fft.inverse(&mut buf);
        let o = j * hs;
        for i in 0..n {
            out[o + i] += buf[i].re * w[i];
            wsum[o + i] += w[i] * w[i];
        }
        for s in sum_phase.iter_mut() {
            *s = wrap_phase(*s);
        }
    }
    let start = (pad as f64 * factor).round() as usize;
    (0..out_len)
        .map(|i| {
            let idx = start + i;
            if idx < out.len() && wsum[idx] > 1e-3 {
                out[idx] / wsum[idx]
            } else {
                0.0
            }
        })
        .collect()
}

