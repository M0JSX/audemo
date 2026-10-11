//! Effects driven by a drawn curve (Fade Envelope, Gain Envelope, Pitch
//! Bender, FFT Filter, Graphic Phase Shifter), the Frequency Band Splitter
//! and the Sound Remover.

use super::prelude::*;
use super::Category::{Amplitude, Filter, Restoration, Special, Stereo, TimePitch};

const FFT_SIZES: &[&str] = &["512", "1024", "2048", "4096", "8192", "16384", "32768"];

fn fft_size(p: &Params, key: &str) -> usize {
    512 << p.c(key).min(6)
}

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "fade_envelope",
            "Fade Envelope",
            Amplitude,
            "Draw how the volume rises and falls across the selection (100 % = unchanged).",
            vec![time_curve("env", "Envelope", (0.0, 100.0, "%"), "0:100,100:100"), toggle("spline", "Spline curves", false)],
            |input, _, p| {
                let pts = p.curve("env");
                let spline = p.b("spline");
                Ok(envelope(input, |u| eval_curve(&pts, u * 100.0, spline, false).clamp(0.0, 100.0) / 100.0))
            },
        )
        .presets(vec![
            ("Smooth Fade In", vec![("env", Value::S("0:0,100:100".into())), ("spline", Value::B(true))]),
            ("Smooth Fade Out", vec![("env", Value::S("0:100,100:0".into())), ("spline", Value::B(true))]),
            ("Dip in the Middle", vec![("env", Value::S("0:100,35:100,50:30,65:100,100:100".into()))]),
            ("Swell", vec![("env", Value::S("0:20,60:100,100:60".into())), ("spline", Value::B(true))]),
            ("Tremolo-like Pulses", vec![("env", Value::S("0:100,12.5:30,25:100,37.5:30,50:100,62.5:30,75:100,87.5:30,100:100".into()))]),
        ]),
        EffectDef::new(
            "gain_envelope",
            "Gain Envelope",
            Amplitude,
            "Draw a gain change across the selection, in decibels (the bottom of the graph is silence).",
            vec![time_curve("env", "Gain", (-40.0, 20.0, "dB"), "0:0,100:0"), toggle("spline", "Spline curves", false)],
            |input, _, p| {
                let pts = p.curve("env");
                let spline = p.b("spline");
                Ok(envelope(input, |u| {
                    let db = eval_curve(&pts, u * 100.0, spline, false).clamp(-40.0, 20.0);
                    if db <= -39.9 {
                        0.0
                    } else {
                        db_to_lin(db)
                    }
                }))
            },
        )
        .presets(vec![
            ("Gentle Boost", vec![("env", Value::S("0:0,20:3,80:3,100:0".into())), ("spline", Value::B(true))]),
            ("Duck the Middle", vec![("env", Value::S("0:0,30:0,35:-12,65:-12,70:0,100:0".into()))]),
            ("Ramp Up 6 dB", vec![("env", Value::S("0:0,100:6".into()))]),
        ]),
        EffectDef::new(
            "pitch_bender",
            "Pitch Bender",
            TimePitch,
            "Bend pitch over time by changing speed, like a tape or record slowing and speeding up (length changes with it).",
            vec![time_curve("bend", "Pitch", (-24.0, 24.0, "semitones"), "0:0,100:0"), toggle("spline", "Spline curves", true)],
            pitch_bender,
        )
        .changes_length()
        .presets(vec![
            ("Turntable Stop", vec![("bend", Value::S("0:0,60:-2,100:-24".into()))]),
            ("Tape Start", vec![("bend", Value::S("0:-24,40:-2,100:0".into()))]),
            ("Warble", vec![("bend", Value::S("0:0,25:0.5,50:-0.5,75:0.5,100:0".into()))]),
            ("Rise One Octave", vec![("bend", Value::S("0:0,100:12".into()))]),
        ]),
        EffectDef::new(
            "auto_pitch",
            "Automatic Pitch Correction",
            TimePitch,
            "Pull a voice or solo instrument to the nearest note of a scale. Faster attack gives the hard, robotic effect; slower sounds natural.",
            vec![
                choice("scale", "Scale", &["Major", "Minor", "Chromatic"], 2),
                choice("key", "Key", NOTE_NAMES, 0),
                float("attack", "Attack", 1.0, 10.0, 5.0, "").decimals(1),
                float("sensitivity", "Sensitivity", 1.0, 100.0, 50.0, "").decimals(0),
                float("amount", "Correction", 0.0, 100.0, 100.0, "%"),
                float("a4", "Calibration (A4)", 410.0, 470.0, 440.0, "Hz").decimals(1),
                float("fmin", "Lowest pitch", 50.0, 500.0, 70.0, "Hz").log(),
                float("fmax", "Highest pitch", 200.0, 2000.0, 1000.0, "Hz").log(),
            ],
            auto_pitch,
        )
        .presets(vec![
            ("Natural Vocal", vec![("attack", Value::F(3.0))]),
            ("Hard Tune", vec![("attack", Value::F(10.0))]),
            ("C Major", vec![("scale", Value::C(0)), ("key", Value::C(0))]),
            ("A Minor", vec![("scale", Value::C(1)), ("key", Value::C(9))]),
        ]),
        EffectDef::new(
            "manual_pitch",
            "Manual Pitch Correction",
            TimePitch,
            "Draw pitch changes over time without changing the length (voices and solo instruments). View > Show Pitch Display shows the pitch to correct.",
            vec![
                time_curve("bend", "Pitch change", (-12.0, 12.0, "semitones"), "0:0,100:0"),
                toggle("spline", "Spline curves", true),
                float("fmin", "Lowest pitch", 50.0, 500.0, 70.0, "Hz").log(),
                float("fmax", "Highest pitch", 200.0, 2000.0, 1000.0, "Hz").log(),
            ],
            manual_pitch,
        )
        .presets(vec![
            ("Up a Semitone", vec![("bend", Value::S("0:1,100:1".into()))]),
            ("Down a Semitone", vec![("bend", Value::S("0:-1,100:-1".into()))]),
            ("Lift the Ending", vec![("bend", Value::S("0:0,70:0,100:2".into()))]),
        ]),
        EffectDef::new(
            "fft_filter",
            "FFT Filter",
            Filter,
            "Draw any frequency response: high- and low-pass, notches, band boosts. Larger FFT sizes give sharper filters with more time smearing.",
            vec![freq_curve("gain", "Response", (-60.0, 20.0, "dB"), "20:0,20000:0"), choice("fft", "FFT size", FFT_SIZES, 3), toggle("spline", "Spline curves", false)],
            fft_filter,
        )
        .presets(vec![
            ("Low Pass 1 kHz", vec![("gain", Value::S("20:0,800:0,1200:-60,20000:-60".into()))]),
            ("High Pass 100 Hz", vec![("gain", Value::S("20:-60,70:-60,120:0,20000:0".into()))]),
            ("Telephone", vec![("gain", Value::S("20:-60,250:-60,350:0,3000:0,3800:-60,20000:-60".into()))]),
            ("Remove Rumble", vec![("gain", Value::S("20:-60,40:-60,80:0,20000:0".into()))]),
            ("Notch 1 kHz", vec![("gain", Value::S("20:0,950:0,1000:-60,1050:0,20000:0".into())), ("fft", Value::C(5))]),
            ("Air Boost", vec![("gain", Value::S("20:0,6000:0,12000:6,20000:6".into())), ("spline", Value::B(true))]),
        ]),
        EffectDef::new(
            "graphic_phase_shifter",
            "Graphic Phase Shifter",
            Stereo,
            "Draw a phase shift against frequency, for one or both channels (widening, or matching two microphones).",
            vec![
                freq_curve("phase", "Phase shift", (-180.0, 180.0, "°"), "20:0,20000:0"),
                choice("channel", "Channel", &["Left", "Right", "Both"], 0),
                choice("fft", "FFT size", FFT_SIZES, 2),
                toggle("spline", "Spline curves", false),
            ],
            phase_shifter,
        )
        .presets(vec![
            ("Widen: Shift Highs 90°", vec![("phase", Value::S("20:0,300:0,2000:90,20000:90".into())), ("spline", Value::B(true))]),
            ("Invert Lows (Left)", vec![("phase", Value::S("20:180,200:180,400:0,20000:0".into()))]),
        ]),
        EffectDef::new(
            "band_splitter",
            "Frequency Band Splitter",
            Special,
            "Split the selection into 2 to 8 frequency bands, each opened as a new file. Mixed back together the bands add up to the original exactly.",
            vec![
                choice("bands", "Bands", &["2", "3", "4", "5", "6", "7", "8"], 1),
                float("c1", "Crossover 1", 20.0, 20000.0, 800.0, "Hz").log(),
                float("c2", "Crossover 2", 20.0, 20000.0, 4000.0, "Hz").log(),
                float("c3", "Crossover 3", 20.0, 20000.0, 8000.0, "Hz").log(),
                float("c4", "Crossover 4", 20.0, 20000.0, 11000.0, "Hz").log(),
                float("c5", "Crossover 5", 20.0, 20000.0, 14000.0, "Hz").log(),
                float("c6", "Crossover 6", 20.0, 20000.0, 17000.0, "Hz").log(),
                float("c7", "Crossover 7", 20.0, 20000.0, 19000.0, "Hz").log(),
                float("slope", "Crossover width", 0.05, 2.0, 0.33, "octaves").decimals(2),
            ],
            band_splitter,
        )
        .presets(vec![
            ("Low / High at 1 kHz", vec![("bands", Value::C(0)), ("c1", Value::F(1000.0))]),
            ("Bass / Mid / Treble", vec![("bands", Value::C(1)), ("c1", Value::F(250.0)), ("c2", Value::F(4000.0))]),
            ("Four Bands", vec![("bands", Value::C(2)), ("c1", Value::F(150.0)), ("c2", Value::F(800.0)), ("c3", Value::F(5000.0))]),
        ]),
        EffectDef::new(
            "sound_remover",
            "Sound Remover",
            Restoration,
            "Remove a sound you have taught it (sirens, phone rings, a recurring tone). First select an example of the sound alone and choose Effects > Noise Reduction / Restoration > Learn Sound Model.",
            vec![
                float("amount", "Reduce by", 0.0, 100.0, 100.0, "%"),
                float("model", "Sound model complexity", 1.0, 30.0, 8.0, "").decimals(0),
                float("content", "Content complexity", 1.0, 60.0, 16.0, "").decimals(0),
                float("passes", "Refinement passes", 5.0, 100.0, 30.0, "").decimals(0),
                choice("fft", "FFT size", &["1024", "2048", "4096"], 1),
            ],
            sound_remover,
        )
        .presets(vec![("Gentle", vec![("amount", Value::F(70.0))]), ("Thorough", vec![("passes", Value::F(60.0)), ("content", Value::F(24.0))])]),
    ]
}

pub const NOTE_NAMES: &[&str] = &["C", "C♯/D♭", "D", "D♯/E♭", "E", "F", "F♯/G♭", "G", "G♯/A♭", "A", "A♯/B♭", "B"];

/// The reference for pitch tracking: the sum of the channels.
fn mono_of(input: &[Vec<f32>]) -> Vec<f32> {
    let len = len_of(input);
    let k = 1.0 / input.len().max(1) as f32;
    (0..len).map(|i| input.iter().map(|c| c[i]).sum::<f32>() * k).collect()
}

fn auto_pitch(input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
    use super::super::pitch::{midi_of, psola, track};
    let sr = ctx.sample_rate;
    // Sensitivity 1–100 → YIN threshold 0.05 (strict) … 0.45 (lenient).
    let threshold = 0.05 + 0.4 * (p.f("sensitivity").clamp(1.0, 100.0) - 1.0) / 99.0;
    let tr = track(&mono_of(input), sr, p.f("fmin"), p.f("fmax").max(p.f("fmin") + 50.0), threshold);
    let a4 = p.f("a4").clamp(400.0, 480.0);
    let key = p.c("key").min(11) as i32;
    let steps: &[i32] = match p.c("scale") {
        0 => &[0, 2, 4, 5, 7, 9, 11],
        1 => &[0, 2, 3, 5, 7, 8, 10],
        _ => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    };
    let amount = (p.f("amount") / 100.0).clamp(0.0, 1.0);
    // Attack 1 (slow, ~300 ms) … 10 (instant).
    let attack = p.f("attack").clamp(1.0, 10.0);
    let tau_s = if attack >= 9.95 { 0.0 } else { 0.3 * (0.005f32 / 0.3).powf((attack - 1.0) / 9.0) };
    let frame_s = tr.hop as f32 / sr as f32;
    let alpha = if tau_s <= 0.0 { 1.0 } else { 1.0 - (-frame_s / tau_s).exp() };
    // Correction (semitones) per frame, smoothed.
    let mut corr = vec![0.0f32; tr.f0.len()];
    let mut cur = 0.0f32;
    for (i, &f) in tr.f0.iter().enumerate() {
        let target = if f > 0.0 {
            let m = midi_of(f, a4);
            let base = m.round() as i32;
            // The nearest note of the scale.
            let best = (base - 2..=base + 2).filter(|n| steps.contains(&(((n - key) % 12 + 12) % 12))).min_by(|a, b| (*a as f32 - m).abs().partial_cmp(&(*b as f32 - m).abs()).unwrap()).unwrap_or(base);
            (best as f32 - m) * amount
        } else {
            0.0
        };
        cur += (target - cur) * alpha;
        corr[i] = cur;
    }
    let hop = tr.hop;
    Ok(psola(input, &tr, |t| {
        let i = (t / hop).min(corr.len().saturating_sub(1));
        2f32.powf(corr[i] / 12.0)
    }))
}

fn manual_pitch(input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
    use super::super::pitch::{psola, track};
    let tr = track(&mono_of(input), ctx.sample_rate, p.f("fmin"), p.f("fmax").max(p.f("fmin") + 50.0), 0.2);
    let pts = p.curve("bend");
    let spline = p.b("spline");
    let len = len_of(input).max(2) as f32;
    Ok(psola(input, &tr, |t| 2f32.powf(eval_curve(&pts, t as f32 / (len - 1.0) * 100.0, spline, false).clamp(-12.0, 12.0) / 12.0)))
}

/// Multiply each channel by `gain(u)`, u = 0..1 across the selection.
fn envelope(input: &[Vec<f32>], gain: impl Fn(f32) -> f32) -> Vec<Vec<f32>> {
    let len = len_of(input);
    let d = (len.max(2) - 1) as f32;
    let g: Vec<f32> = (0..len).map(|i| gain(i as f32 / d)).collect();
    input.iter().map(|c| c.iter().zip(&g).map(|(s, g)| s * g).collect()).collect()
}

/// Read each channel at a varying speed (2^(semitones/12)), the curve
/// following the input's time.
fn pitch_bender(input: &[Vec<f32>], _: &Ctx, p: &Params) -> Res {
    let pts = p.curve("bend");
    let spline = p.b("spline");
    let len = len_of(input);
    if len < 4 {
        return Ok(input.to_vec());
    }
    // Read positions, shared by every channel.
    let mut pos = Vec::with_capacity(len);
    let mut t = 0.0f64;
    let last = (len - 1) as f64;
    while t < last {
        pos.push(t);
        let semis = eval_curve(&pts, (t / last * 100.0) as f32, spline, false).clamp(-24.0, 24.0);
        t += 2f64.powf(semis as f64 / 12.0);
        if pos.len() > len * 20 {
            break;
        }
    }
    Ok(input
        .iter()
        .map(|c| {
            pos.iter()
                .map(|&t| {
                    let i = t.floor() as isize;
                    let f = (t - i as f64) as f32;
                    let at = |k: isize| c[k.clamp(0, len as isize - 1) as usize];
                    hermite(at(i - 1), at(i), at(i + 1), at(i + 2), f)
                })
                .collect()
        })
        .collect())
}

fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// Per-bin values of a frequency curve, bins 0..=n/2.
fn bin_curve(pts: &[(f32, f32)], n: usize, sr: u32, spline: bool) -> Vec<f32> {
    (0..=n / 2).map(|k| eval_curve(pts, (k as f32 * sr as f32 / n as f32).max(1.0), spline, true)).collect()
}

fn fft_filter(input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
    let n = fft_size(p, "fft");
    let gains: Vec<f32> = bin_curve(&p.curve("gain"), n, ctx.sample_rate, p.b("spline")).into_iter().map(|db| if db <= -59.9 { 0.0 } else { db_to_lin(db.min(20.0)) }).collect();
    Ok(input.iter().map(|c| stft_process(c, n, n / 4, |buf, _| apply_gains(buf, &gains))).collect())
}

fn phase_shifter(input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
    let n = fft_size(p, "fft");
    let phase: Vec<f32> = bin_curve(&p.curve("phase"), n, ctx.sample_rate, p.b("spline")).into_iter().map(|d| d.clamp(-180.0, 180.0).to_radians()).collect();
    let which = p.c("channel");
    Ok(input
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            let shift = match which {
                0 => ci == 0,
                1 => ci == 1 || input.len() == 1,
                _ => true,
            };
            if !shift {
                return c.clone();
            }
            stft_process(c, n, n / 4, |buf, _| {
                for k in 1..n / 2 {
                    let (s, co) = phase[k].sin_cos();
                    let rot = |z: Complex, s: f32| Complex::new(z.re * co - z.im * s, z.re * s + z.im * co);
                    buf[k] = rot(buf[k], s);
                    buf[n - k] = rot(buf[n - k], -s);
                }
            })
        })
        .collect())
}

/// Crossover frequencies in use, ascending.
pub fn band_crossovers(p: &Params, sr: u32) -> Vec<f32> {
    let bands = p.c("bands").min(6) + 2;
    let nyq = sr as f32 / 2.0;
    let mut c: Vec<f32> = (1..bands).map(|i| p.f(["c1", "c2", "c3", "c4", "c5", "c6", "c7"][i - 1]).clamp(20.0, nyq * 0.95)).collect();
    c.sort_by(|a, b| a.partial_cmp(b).unwrap());
    c.dedup_by(|a, b| (*a / *b) < 1.01);
    c
}

/// Output: the bands one after another, each with the input's channels.
fn band_splitter(input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
    let n = 4096;
    let cross = band_crossovers(p, ctx.sample_rate);
    let width = p.f("slope").clamp(0.05, 2.0);
    let bands = cross.len() + 1;
    // Share of each bin above a crossover: 0 below, 1 above, a raised
    // cosine over `width` octaves around it. Band masks are differences of
    // these, so they always add up to 1.
    let above = |f: f32, c: f32| {
        let o = (f.max(1.0) / c).log2() / width + 0.5;
        if o <= 0.0 {
            0.0
        } else if o >= 1.0 {
            1.0
        } else {
            0.5 - 0.5 * (o * std::f32::consts::PI).cos()
        }
    };
    let freqs: Vec<f32> = (0..=n / 2).map(|k| k as f32 * ctx.sample_rate as f32 / n as f32).collect();
    let masks: Vec<Vec<f32>> = (0..bands)
        .map(|b| {
            freqs
                .iter()
                .map(|&f| {
                    let lo = if b == 0 { 1.0 } else { above(f, cross[b - 1]) };
                    let hi = if b + 1 == bands { 0.0 } else { above(f, cross[b]) };
                    (lo - hi).max(0.0)
                })
                .collect()
        })
        .collect();
    let mut out = Vec::with_capacity(bands * input.len());
    for m in &masks {
        for c in input {
            out.push(stft_process(c, n, n / 4, |buf, _| apply_gains(buf, m)));
        }
    }
    Ok(out)
}

/// A tiny deterministic generator for NMF starting values.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 0.9 + 0.1
    }
}

/// KL-divergence NMF: V (f × t) ≈ W (f × k) H (k × t). Only the columns of
/// W from `fixed` on are updated. Matrices are row-major Vec<f32>.
fn nmf(v: &[f32], f: usize, t: usize, w: &mut [f32], k: usize, h: &mut [f32], fixed: usize, iters: usize) {
    let eps = 1e-9f32;
    let mut wh = vec![0.0f32; f * t];
    let mut ratio = vec![0.0f32; f * t];
    let product = |w: &[f32], h: &[f32], wh: &mut [f32]| {
        for i in 0..f {
            let row = &mut wh[i * t..(i + 1) * t];
            row.iter_mut().for_each(|x| *x = 0.0);
            for j in 0..k {
                let a = w[i * k + j];
                if a == 0.0 {
                    continue;
                }
                let hr = &h[j * t..(j + 1) * t];
                for (x, &b) in row.iter_mut().zip(hr) {
                    *x += a * b;
                }
            }
        }
    };
    for _ in 0..iters {
        product(w, h, &mut wh);
        for ((r, &vv), &x) in ratio.iter_mut().zip(v).zip(&wh) {
            *r = vv / (x + eps);
        }
        // H ← H ⊙ (Wᵀ R) / (Wᵀ 1)
        for j in 0..k {
            let wsum: f32 = (0..f).map(|i| w[i * k + j]).sum::<f32>() + eps;
            let mut num = vec![0.0f32; t];
            for i in 0..f {
                let a = w[i * k + j];
                if a == 0.0 {
                    continue;
                }
                for (nn, &r) in num.iter_mut().zip(&ratio[i * t..(i + 1) * t]) {
                    *nn += a * r;
                }
            }
            for (hh, nn) in h[j * t..(j + 1) * t].iter_mut().zip(num) {
                *hh *= nn / wsum;
            }
        }
        if fixed >= k {
            continue;
        }
        product(w, h, &mut wh);
        for ((r, &vv), &x) in ratio.iter_mut().zip(v).zip(&wh) {
            *r = vv / (x + eps);
        }
        // W ← W ⊙ (R Hᵀ) / (1 Hᵀ), free columns only.
        for j in fixed..k {
            let hr = &h[j * t..(j + 1) * t];
            let hsum: f32 = hr.iter().sum::<f32>() + eps;
            for i in 0..f {
                let num: f32 = ratio[i * t..(i + 1) * t].iter().zip(hr).map(|(r, b)| r * b).sum();
                w[i * k + j] *= num / hsum;
            }
        }
    }
}

/// Magnitudes (f × t, row-major) of STFT frames, bins 0..=n/2.
fn magnitudes(frames: &[Vec<Complex>], n: usize) -> (Vec<f32>, usize, usize) {
    let (f, t) = (n / 2 + 1, frames.len());
    let mut v = vec![0.0f32; f * t];
    for (j, fr) in frames.iter().enumerate() {
        for i in 0..f {
            v[i * t + j] = fr[i].norm();
        }
    }
    (v, f, t)
}

/// Templates of the learned sound: NMF on the model's magnitudes.
fn learn_templates(model: &[f32], n: usize, k: usize, iters: usize) -> Vec<f32> {
    let frames = stft_frames(model, n, n / 4);
    let (v, f, t) = magnitudes(&frames, n);
    let mut rng = Lcg(0x5eed);
    let mut w: Vec<f32> = (0..f * k).map(|_| rng.next()).collect();
    let mut h: Vec<f32> = (0..k * t).map(|_| rng.next()).collect();
    nmf(&v, f, t, &mut w, k, &mut h, 0, iters);
    // Unit-sum columns, so activations carry the level.
    for j in 0..k {
        let s: f32 = (0..f).map(|i| w[i * k + j]).sum::<f32>().max(1e-9);
        for i in 0..f {
            w[i * k + j] /= s;
        }
    }
    w
}

fn sound_remover(input: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Res {
    let Some(model) = ctx.sound_model else {
        return Err("Teach the Sound Remover the sound first: select a passage containing only that sound, then choose Effects > Noise Reduction / Restoration > Learn Sound Model.".into());
    };
    let n = 1024 << p.c("fft").min(2);
    let ks = p.f("model").round().clamp(1.0, 30.0) as usize;
    let kb = p.f("content").round().clamp(1.0, 60.0) as usize;
    let iters = p.f("passes").round().clamp(5.0, 100.0) as usize;
    let amount = (p.f("amount") / 100.0).clamp(0.0, 1.0);
    // The model as one mono signal at the file's rate.
    let mono: Vec<f32> = {
        let len = len_of(&model.samples);
        let k = 1.0 / model.samples.len().max(1) as f32;
        let m: Vec<f32> = (0..len).map(|i| model.samples.iter().map(|c| c[i]).sum::<f32>() * k).collect();
        if model.sample_rate != ctx.sample_rate {
            resample(&m, ctx.sample_rate as f64 / model.sample_rate as f64)
        } else {
            m
        }
    };
    if mono.len() < n * 2 {
        return Err("The learned sound model is too short; learn it from a longer selection.".into());
    }
    let ws = learn_templates(&mono, n, ks, iters);
    let f = n / 2 + 1;
    let k = ks + kb;
    let hop = n / 4;
    let mut out = Vec::with_capacity(input.len());
    for c in input {
        let mut frames = stft_frames(c, n, hop);
        // Work in blocks of frames to bound memory and time.
        let block = 400;
        let mut start = 0;
        let mut rng = Lcg(0xb0b);
        while start < frames.len() {
            let end = (start + block).min(frames.len());
            let (v, _, t) = magnitudes(&frames[start..end], n);
            let mut w = vec![0.0f32; f * k];
            for i in 0..f {
                for j in 0..ks {
                    w[i * k + j] = ws[i * ks + j];
                }
                for j in ks..k {
                    w[i * k + j] = rng.next() / f as f32;
                }
            }
            let mut h: Vec<f32> = (0..k * t).map(|_| rng.next()).collect();
            nmf(&v, f, t, &mut w, k, &mut h, ks, iters);
            // Soft mask of the sound's share in each bin.
            for (jj, fr) in frames[start..end].iter_mut().enumerate() {
                let mut gains = vec![1.0f32; f];
                for (i, g) in gains.iter_mut().enumerate() {
                    let mut s = 0.0f32;
                    let mut all = 1e-9f32;
                    for j in 0..k {
                        let x = w[i * k + j] * h[j * t + jj];
                        all += x;
                        if j < ks {
                            s += x;
                        }
                    }
                    let share = (s / all).clamp(0.0, 1.0);
                    *g = 1.0 - amount * share;
                }
                apply_gains(fr, &gains);
            }
            start = end;
        }
        out.push(istft(&mut frames, n, hop, c.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(sr: u32) -> Ctx<'static> {
        Ctx { sample_rate: sr, channels: 1, noise_print: None, sound_model: None }
    }

    #[test]
    fn curves_interpolate() {
        let pts = parse_curve("0:0,50:100,100:0");
        assert_eq!(eval_curve(&pts, 25.0, false, false), 50.0);
        assert_eq!(eval_curve(&pts, 50.0, true, false), 100.0);
        assert_eq!(eval_curve(&pts, -5.0, false, false), 0.0);
        let f = parse_curve("100:0,1000:-20");
        assert!((eval_curve(&f, 316.2278, false, true) + 10.0).abs() < 0.01, "halfway in log frequency");
    }

    #[test]
    fn band_splitter_sums_to_input() {
        let sr = 48000;
        let x: Vec<f32> = (0..24000).map(|i| ((i as f32 * 0.05).sin() + (i as f32 * 0.9).sin() * 0.3) * 0.5).collect();
        let mut p = Params::defaults(&defs().iter().find(|d| d.id == "band_splitter").unwrap().params);
        p.set("bands", Value::C(2));
        let out = band_splitter(&[x.clone()], &ctx(sr), &p).unwrap();
        assert_eq!(out.len(), 4);
        let err = (0..x.len()).map(|i| (out.iter().map(|b| b[i]).sum::<f32>() - x[i]).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-3, "bands should add up to the input, max error {err}");
    }

    #[test]
    fn gain_envelope_scales() {
        let p = Params::with_preset(&defs().iter().find(|d| d.id == "gain_envelope").unwrap().params, &[("env", Value::S("0:-6.0206,100:-6.0206".into()))]);
        let out = (defs().iter().find(|d| d.id == "gain_envelope").unwrap().process)(&[vec![1.0; 100]], &ctx(48000), &p).unwrap();
        assert!((out[0][50] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn pitch_bender_octave_up_halves_length() {
        let p = Params::with_preset(&defs().iter().find(|d| d.id == "pitch_bender").unwrap().params, &[("bend", Value::S("0:12,100:12".into()))]);
        let out = pitch_bender(&[vec![0.0; 10000]], &ctx(48000), &p).unwrap();
        assert!((out[0].len() as i64 - 5000).abs() <= 2, "{}", out[0].len());
    }

    #[test]
    fn auto_pitch_pulls_to_the_note() {
        use super::super::super::pitch::track;
        let sr = 44100;
        // 25 cents flat of A3 (220 Hz), with harmonics like a voice.
        let f = 220.0 * 2f32.powf(-0.25 / 12.0);
        let x: Vec<f32> = (0..sr as usize).map(|i| (1..=4).map(|h| (std::f32::consts::TAU * f * h as f32 * i as f32 / sr as f32).sin() / h as f32).sum::<f32>() * 0.3).collect();
        let def = defs().into_iter().find(|d| d.id == "auto_pitch").unwrap();
        let p = Params::with_preset(&def.params, &[("attack", Value::F(10.0))]);
        let y = (def.process)(&[x], &ctx(sr), &p).unwrap();
        let t = track(&y[0], sr, 60.0, 1000.0, 0.15);
        let v: Vec<f32> = t.f0[30..80].iter().copied().filter(|f| *f > 0.0).collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean - 220.0).abs() < 1.5, "corrected to {mean}");
    }

    #[test]
    fn sound_remover_removes_a_learned_tone() {
        let sr = 16000;
        let tone = |i: usize| (i as f32 * std::f32::consts::TAU * 1000.0 / sr as f32).sin() * 0.5;
        let model = NoiseProfile { samples: vec![(0..16000).map(tone).collect()], sample_rate: sr };
        // Content: a 300 Hz tone plus the 1 kHz "siren".
        let x: Vec<f32> = (0..32000).map(|i| (i as f32 * std::f32::consts::TAU * 300.0 / sr as f32).sin() * 0.5 + tone(i)).collect();
        let c = Ctx { sample_rate: sr, channels: 1, noise_print: None, sound_model: Some(&model) };
        let p = Params::defaults(&defs().iter().find(|d| d.id == "sound_remover").unwrap().params);
        let y = sound_remover(&[x], &c, &p).unwrap();
        let level = |f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, s) in y[0][4000..28000].iter().enumerate() {
                let a = std::f64::consts::TAU * f as f64 * i as f64 / sr as f64;
                re += *s as f64 * a.cos();
                im += *s as f64 * a.sin();
            }
            2.0 * (re * re + im * im).sqrt() / 24000.0
        };
        assert!(level(1000.0) < 0.1, "learned tone left at {}", level(1000.0));
        assert!(level(300.0) > 0.35, "other content kept at {}", level(300.0));
    }
}
