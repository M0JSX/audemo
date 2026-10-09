use super::prelude::*;
use super::Category::Restoration;

const FFT_SIZES: &[&str] = &["1024", "2048", "4096", "8192"];

pub fn capture_noise_print(sel: &[Vec<f32>], sr: u32) -> Result<NoiseProfile, String> {
    let max = sr as usize * 30;
    let len = len_of(sel);
    if len < 4096 {
        return Err("Select at least 0.1 s of noise-only audio to capture a noise print.".into());
    }
    Ok(NoiseProfile {
        samples: sel.iter().map(|c| c[..len.min(max)].to_vec()).collect(),
        sample_rate: sr,
    })
}

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "noise_reduction",
            "Noise Reduction (process)",
            Restoration,
            "Spectral noise removal using the captured noise print (Shift+P on a noise-only selection first).",
            vec![
                float("amount", "Noise reduction", 0.0, 100.0, 80.0, "%"),
                float("reduce", "Reduce by", 0.0, 60.0, 20.0, "dB"),
                float("sensitivity", "Sensitivity", 1.0, 10.0, 6.0, ""),
                float("decay", "Spectral decay rate", 0.0, 100.0, 65.0, "%"),
                float("smoothing", "Frequency smoothing", 0.0, 10.0, 2.0, "bins").decimals(0),
                choice("fft", "FFT size", FFT_SIZES, 2),
                toggle("noise_only", "Output noise only", false),
            ],
            noise_reduction,
        )
        .presets(vec![
            ("Light Reduction", vec![("amount", Value::F(60.0)), ("reduce", Value::F(10.0))]),
            ("Heavy Reduction", vec![("amount", Value::F(95.0)), ("reduce", Value::F(35.0)), ("sensitivity", Value::F(8.0))]),
            ("HF Radio Static", vec![("amount", Value::F(85.0)), ("reduce", Value::F(18.0)), ("sensitivity", Value::F(7.0)), ("decay", Value::F(80.0)), ("fft", Value::C(1))]),
        ]),
        EffectDef::new(
            "adaptive_nr",
            "Adaptive Noise Reduction",
            Restoration,
            "Learns the noise floor as it goes. No noise print needed; good for hiss and steady background noise.",
            vec![
                float("reduce", "Reduce noise by", 0.0, 40.0, 12.0, "dB"),
                float("noisiness", "Noisiness", 1.0, 100.0, 40.0, "%"),
                float("track", "Noise floor tracking", 0.5, 20.0, 3.0, "dB/s"),
                float("decay", "Signal smoothing", 0.0, 95.0, 60.0, "%"),
                choice("fft", "FFT size", FFT_SIZES, 1),
            ],
            adaptive_nr,
        ),
        EffectDef::new(
            "dehummer",
            "DeHummer",
            Restoration,
            "Notch out mains hum and its harmonics.",
            vec![
                choice("freq", "Fundamental", &["50 Hz (UK / EU)", "60 Hz (US)"], 0),
                float("harmonics", "Number of harmonics", 1.0, 12.0, 6.0, "").decimals(0),
                float("q", "Notch Q", 5.0, 120.0, 30.0, "").log(),
                float("fine", "Frequency trim", -3.0, 3.0, 0.0, "Hz"),
                toggle("hum_only", "Output hum only", false),
            ],
            dehummer,
        )
        .presets(vec![
            ("50 Hz, 4 harmonics", vec![("harmonics", Value::F(4.0))]),
            ("60 Hz, 6 harmonics", vec![("freq", Value::C(1))]),
        ]),
        EffectDef::new(
            "declicker",
            "Click/Pop Eliminator",
            Restoration,
            "Find short transients that don't belong and interpolate over them.",
            vec![
                float("sensitivity", "Sensitivity", 1.0, 100.0, 35.0, ""),
                float("max_ms", "Maximum click length", 0.05, 5.0, 1.0, "ms"),
            ],
            declicker,
        ),
        EffectDef::new(
            "declipper",
            "DeClipper",
            Restoration,
            "Rebuild flattened peaks with cubic interpolation, then trim the level so they fit.",
            vec![
                float("threshold", "Clip threshold", 50.0, 100.0, 97.0, "% of peak"),
                float("gain", "Output gain", -12.0, 0.0, -3.0, "dB"),
                float("max_ms", "Maximum clip length", 0.1, 20.0, 5.0, "ms"),
            ],
            declipper,
        ),
    ]
}

fn fft_size(p: &Params) -> usize {
    1024 << p.c("fft").min(3)
}

fn noise_reduction(i: &[Vec<f32>], _c: &Ctx, p: &Params) -> Res {
    let profile = _c.noise_print.ok_or(
        "No noise print yet. Select a noise-only region and choose Capture Noise Print (Shift+P).",
    )?;
    let n = fft_size(p);
    let hop = n / 4;
    let bins = n / 2 + 1;
    let amount = p.f("amount") / 100.0;
    let floor = db_to_lin(-p.f("reduce"));
    let alpha = 1.0 + p.f("sensitivity") / 3.0;
    let decay = p.f("decay") / 100.0 * 0.95;
    let smooth = p.f("smoothing").round() as usize;
    let noise_only = p.b("noise_only");
    let mut out = Vec::with_capacity(i.len());
    for (ci, ch) in i.iter().enumerate() {
        let ns = &profile.samples[ci.min(profile.samples.len() - 1)];
        if ns.len() < n {
            return Err(format!(
                "Noise print is shorter than the FFT size ({n} samples). Capture a longer noise print or pick a smaller FFT size."
            ));
        }
        let nm: Vec<f32> = mean_magnitude(ns, n).into_iter().map(|m| m * alpha).collect();
        let mut prev = vec![1.0f32; bins];
        let mut gains = vec![1.0f32; bins];
        let processed = stft_process(ch, n, hop, |buf, _| {
            for k in 0..bins {
                let mag = buf[k].norm();
                let g = if mag > 1e-12 {
                    let r = (nm[k] / mag).min(1.0);
                    (1.0 - r * r).max(0.0).sqrt()
                } else {
                    0.0
                };
                let g = g.max(floor);
                // Fast attack, slow decay across frames.
                let g = if g > prev[k] { g } else { decay * prev[k] + (1.0 - decay) * g };
                prev[k] = g;
                gains[k] = g;
            }
            if smooth > 0 {
                let src = gains.clone();
                for k in 0..bins {
                    let a = k.saturating_sub(smooth);
                    let b = (k + smooth).min(bins - 1);
                    let s: f32 = src[a..=b].iter().sum();
                    gains[k] = s / (b - a + 1) as f32;
                }
            }
            for g in gains.iter_mut() {
                let mixed = 1.0 - amount * (1.0 - *g);
                *g = if noise_only { 1.0 - mixed } else { mixed };
            }
            apply_gains(buf, &gains);
        });
        out.push(processed);
    }
    Ok(out)
}

fn adaptive_nr(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let n = fft_size(p);
    let hop = n / 4;
    let bins = n / 2 + 1;
    let floor = db_to_lin(-p.f("reduce"));
    let alpha = 1.0 + p.f("noisiness") / 100.0 * 3.0;
    let rise = db_to_lin(p.f("track") * hop as f32 / c.sample_rate as f32);
    let decay = p.f("decay") / 100.0;
    Ok(i.iter()
        .map(|ch| {
            let mut est: Vec<f32> = Vec::new();
            let mut prev = vec![1.0f32; bins];
            let mut gains = vec![1.0f32; bins];
            stft_process(ch, n, hop, |buf, frame| {
                if est.is_empty() {
                    est = (0..bins).map(|k| buf[k].norm()).collect();
                }
                for k in 0..bins {
                    let mag = buf[k].norm();
                    // Minimum-statistics style tracker: falls fast, rises slowly.
                    est[k] = if mag < est[k] { 0.7 * est[k] + 0.3 * mag } else { est[k] * rise };
                    if frame < 4 {
                        est[k] = est[k].min(mag.max(1e-9));
                    }
                    let nm = est[k] * alpha;
                    let g = if mag > 1e-12 {
                        let r = (nm / mag).min(1.0);
                        (1.0 - r * r).max(0.0).sqrt()
                    } else {
                        0.0
                    };
                    let g = g.max(floor);
                    let g = if g > prev[k] { g } else { decay * prev[k] + (1.0 - decay) * g };
                    prev[k] = g;
                    gains[k] = g;
                }
                apply_gains(buf, &gains);
            })
        })
        .collect())
}

fn dehummer(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let f0 = if p.c("freq") == 1 { 60.0 } else { 50.0 } + p.f("fine");
    let harmonics = p.f("harmonics").round().max(1.0) as usize;
    let q = p.f("q");
    let hum_only = p.b("hum_only");
    let filters: Vec<Biquad> = (1..=harmonics)
        .map(|h| f0 * h as f32)
        .filter(|f| *f < sr * 0.45)
        .map(|f| Biquad::new(FilterType::Notch, sr, f, q, 0.0))
        .collect();
    Ok(i.iter()
        .map(|ch| {
            let mut y = ch.clone();
            let mut f = filters.clone();
            run_cascade(&mut y, &mut f);
            if hum_only {
                ch.iter().zip(y.iter()).map(|(x, y)| x - y).collect()
            } else {
                y
            }
        })
        .collect())
}

/// Cubic Hermite fill of x[i0+1 .. i1) from the endpoints and their slopes.
fn hermite_fill(x: &mut [f32], i0: usize, i1: usize) {
    if i0 == 0 || i1 + 1 >= x.len() || i1 <= i0 + 1 {
        return;
    }
    let l = (i1 - i0) as f32;
    let p0 = x[i0];
    let p1 = x[i1];
    let m0 = (x[i0] - x[i0 - 1]) * l;
    let m1 = (x[i1 + 1] - x[i1]) * l;
    for k in (i0 + 1)..i1 {
        let t = (k - i0) as f32 / l;
        let t2 = t * t;
        let t3 = t2 * t;
        x[k] = (2.0 * t3 - 3.0 * t2 + 1.0) * p0
            + (t3 - 2.0 * t2 + t) * m0
            + (-2.0 * t3 + 3.0 * t2) * p1
            + (t3 - t2) * m1;
    }
}

fn declicker(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let k = 2.0 + (100.0 - p.f("sensitivity")) / 8.0;
    let max_len = ms_to_samples(p.f("max_ms"), c.sample_rate).max(1);
    Ok(i.iter()
        .map(|ch| {
            let len = ch.len();
            let mut x = ch.clone();
            if len < 16 {
                return x;
            }
            // Second-order prediction residual.
            let mut e = vec![0.0f32; len];
            for n in 2..len {
                e[n] = (ch[n] - (2.0 * ch[n - 1] - ch[n - 2])).abs();
            }
            let win = 512usize.min(len);
            let mut prefix = vec![0.0f64; len + 1];
            for n in 0..len {
                prefix[n + 1] = prefix[n] + e[n] as f64;
            }
            let local = |n: usize| {
                let a = n.saturating_sub(win / 2);
                let b = (n + win / 2).min(len);
                ((prefix[b] - prefix[a]) / (b - a).max(1) as f64) as f32
            };
            let mut n = 3;
            while n + 3 < len {
                if e[n] > k * local(n) + 1e-4 {
                    let start = n;
                    let mut end = n;
                    let mut m = n + 1;
                    while m < len && m - start <= max_len + 2 {
                        if e[m] > k * local(m) + 1e-4 {
                            end = m;
                        }
                        m += 1;
                    }
                    if end - start <= max_len {
                        let i0 = start.saturating_sub(2).max(1);
                        let i1 = (end + 2).min(len - 2);
                        hermite_fill(&mut x, i0, i1);
                    }
                    n = end + 3;
                } else {
                    n += 1;
                }
            }
            x
        })
        .collect())
}

fn declipper(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let gain = db_to_lin(p.f("gain"));
    let max_len = ms_to_samples(p.f("max_ms"), c.sample_rate).max(2);
    Ok(i.iter()
        .map(|ch| {
            let pk = peak_ch(ch);
            let thr = pk * p.f("threshold") / 100.0;
            let mut x = ch.clone();
            let len = x.len();
            let mut n = 2;
            while n + 2 < len {
                if ch[n].abs() >= thr && pk > 0.0 {
                    let sign = ch[n].signum();
                    let start = n;
                    let mut end = n;
                    while end + 1 < len && ch[end + 1].abs() >= thr && ch[end + 1].signum() == sign {
                        end += 1;
                    }
                    if end > start && end - start <= max_len && start >= 2 && end + 2 < len {
                        hermite_fill(&mut x, start - 1, end + 1);
                    }
                    n = end + 1;
                } else {
                    n += 1;
                }
            }
            x.iter_mut().for_each(|s| *s *= gain);
            x
        })
        .collect())
}
