use super::prelude::*;
use super::Category::{Amplitude, Basic};

pub fn basic() -> Vec<EffectDef> {
    vec![
        EffectDef::new("invert", "Invert", Basic, "Flip the polarity of every sample.", vec![], |i, _, _| {
            Ok(i.iter().map(|c| c.iter().map(|s| -s).collect()).collect())
        }),
        EffectDef::new("reverse", "Reverse", Basic, "Play the selection backwards.", vec![], |i, _, _| {
            Ok(i.iter().map(|c| c.iter().rev().copied().collect()).collect())
        }),
        EffectDef::new("silence", "Silence", Basic, "Replace the selection with digital silence.", vec![], |i, _, _| {
            Ok(i.iter().map(|c| vec![0.0; c.len()]).collect())
        }),
    ]
}

pub fn defs() -> Vec<EffectDef> {
    const SHAPES: &[&str] = &["Linear", "Logarithmic", "Cosine (S-curve)", "Exponential"];
    vec![
        EffectDef::new(
            "amplify",
            "Amplify",
            Amplitude,
            "Boost or cut level per channel.",
            vec![
                float("left", "Left / mono gain", -40.0, 40.0, 0.0, "dB"),
                float("right", "Right gain", -40.0, 40.0, 0.0, "dB"),
                toggle("link", "Link sliders", true),
            ],
            amplify,
        )
        .presets(vec![
            ("+3 dB Boost", vec![("left", Value::F(3.0))]),
            ("+6 dB Boost", vec![("left", Value::F(6.0))]),
            ("-3 dB Cut", vec![("left", Value::F(-3.0))]),
            ("-6 dB Cut", vec![("left", Value::F(-6.0))]),
        ]),
        EffectDef::new(
            "normalize",
            "Normalize",
            Amplitude,
            "Scale audio so its loudest point reaches a target level.",
            vec![
                choice("mode", "Normalize by", &["Peak level", "RMS level"], 0),
                float("target", "Target", -40.0, 0.0, -0.1, "dBFS"),
                toggle("together", "Normalize all channels equally", true),
                toggle("dc", "DC bias adjust", false),
            ],
            normalize,
        )
        .presets(vec![
            ("Normalize to -0.1 dB", vec![]),
            ("Normalize to -1 dB", vec![("target", Value::F(-1.0))]),
            ("Normalize to -3 dB", vec![("target", Value::F(-3.0))]),
            ("RMS to -20 dB", vec![("mode", Value::C(1)), ("target", Value::F(-20.0))]),
        ]),
        EffectDef::new(
            "fade_in",
            "Fade In",
            Amplitude,
            "Ramp the selection up from silence.",
            vec![choice("shape", "Curve", SHAPES, 2)],
            |i, _, p| Ok(fade(i, true, p.c("shape"))),
        ),
        EffectDef::new(
            "fade_out",
            "Fade Out",
            Amplitude,
            "Ramp the selection down to silence.",
            vec![choice("shape", "Curve", SHAPES, 2)],
            |i, _, p| Ok(fade(i, false, p.c("shape"))),
        ),
        EffectDef::new(
            "dynamics",
            "Dynamics Processing",
            Amplitude,
            "Compressor with soft knee, attack/release and make-up gain.",
            vec![
                float("threshold", "Threshold", -60.0, 0.0, -20.0, "dB"),
                float("ratio", "Ratio", 1.0, 30.0, 4.0, ":1").log(),
                float("knee", "Knee", 0.0, 24.0, 6.0, "dB"),
                float("attack", "Attack", 0.1, 200.0, 10.0, "ms").log(),
                float("release", "Release", 5.0, 3000.0, 150.0, "ms").log(),
                float("makeup", "Make-up gain", -12.0, 36.0, 0.0, "dB"),
                toggle("link", "Link channels", true),
            ],
            |i, c, p| {
                Ok(compress(
                    i,
                    c.sample_rate,
                    p.f("threshold"),
                    p.f("ratio"),
                    p.f("knee"),
                    p.f("attack"),
                    p.f("release"),
                    p.f("makeup"),
                    p.b("link"),
                ))
            },
        )
        .presets(vec![
            ("Gentle Vocal", vec![("threshold", Value::F(-18.0)), ("ratio", Value::F(2.5)), ("makeup", Value::F(3.0))]),
            ("Broadcast", vec![("threshold", Value::F(-24.0)), ("ratio", Value::F(6.0)), ("attack", Value::F(3.0)), ("makeup", Value::F(8.0))]),
            ("Drum Punch", vec![("threshold", Value::F(-15.0)), ("ratio", Value::F(4.0)), ("attack", Value::F(30.0)), ("release", Value::F(80.0)), ("knee", Value::F(2.0))]),
            ("Heavy Squash", vec![("threshold", Value::F(-35.0)), ("ratio", Value::F(20.0)), ("attack", Value::F(1.0)), ("makeup", Value::F(18.0))]),
        ]),
        EffectDef::new(
            "hard_limiter",
            "Hard Limiter",
            Amplitude,
            "Brick-wall look-ahead limiter. Nothing passes above the ceiling.",
            vec![
                float("ceiling", "Maximum amplitude", -24.0, 0.0, -0.1, "dB"),
                float("boost", "Input boost", 0.0, 24.0, 0.0, "dB"),
                float("lookahead", "Look-ahead time", 0.5, 20.0, 5.0, "ms"),
                float("release", "Release time", 10.0, 1000.0, 80.0, "ms").log(),
            ],
            hard_limiter,
        )
        .presets(vec![
            ("Limit to -0.1 dB", vec![]),
            ("Limit to -1 dB", vec![("ceiling", Value::F(-1.0))]),
            ("+6 dB Loudness", vec![("boost", Value::F(6.0)), ("ceiling", Value::F(-0.3))]),
        ]),
        EffectDef::new(
            "noise_gate",
            "Noise Gate",
            Amplitude,
            "Mute audio that falls below the threshold.",
            vec![
                float("threshold", "Threshold", -90.0, 0.0, -45.0, "dB"),
                float("range", "Attenuation", 0.0, 90.0, 40.0, "dB"),
                float("attack", "Attack", 0.1, 100.0, 2.0, "ms").log(),
                float("hold", "Hold", 0.0, 1000.0, 50.0, "ms"),
                float("release", "Release", 5.0, 2000.0, 150.0, "ms").log(),
            ],
            noise_gate,
        ),
        EffectDef::new(
            "leveler",
            "Speech Volume Leveler",
            Amplitude,
            "Automatic gain riding that evens out quiet and loud speech.",
            vec![
                float("target", "Target volume", -40.0, -6.0, -20.0, "dB"),
                float("max_gain", "Maximum gain", 0.0, 30.0, 12.0, "dB"),
                float("speed", "Leveling speed", 50.0, 5000.0, 800.0, "ms").log(),
                float("noise", "Noise floor", -90.0, -20.0, -50.0, "dB"),
            ],
            leveler,
        ),
    ]
}

fn amplify(i: &[Vec<f32>], _: &Ctx, p: &Params) -> Res {
    let gl = db_to_lin(p.f("left"));
    let gr = if p.b("link") { gl } else { db_to_lin(p.f("right")) };
    Ok(i.iter()
        .enumerate()
        .map(|(n, c)| {
            let g = if n == 1 { gr } else { gl };
            c.iter().map(|s| s * g).collect()
        })
        .collect())
}

fn normalize(i: &[Vec<f32>], _: &Ctx, p: &Params) -> Res {
    let mut out = i.to_vec();
    if p.b("dc") {
        for c in out.iter_mut() {
            let mean = c.iter().map(|&s| s as f64).sum::<f64>() / c.len().max(1) as f64;
            for s in c.iter_mut() {
                *s -= mean as f32;
            }
        }
    }
    let target = db_to_lin(p.f("target"));
    let rms = p.c("mode") == 1;
    let measure = |c: &[f32]| if rms { rms_ch(c) } else { peak_ch(c) };
    if p.b("together") {
        let m = out.iter().map(|c| measure(c)).fold(0.0, f32::max);
        if m > 1e-9 {
            let g = target / m;
            for c in out.iter_mut() {
                c.iter_mut().for_each(|s| *s *= g);
            }
        }
    } else {
        for c in out.iter_mut() {
            let m = measure(c);
            if m > 1e-9 {
                let g = target / m;
                c.iter_mut().for_each(|s| *s *= g);
            }
        }
    }
    Ok(out)
}

fn fade_curve(t: f32, shape: usize) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match shape {
        1 => 1.0 - (1.0 - t) * (1.0 - t),
        2 => 0.5 - 0.5 * (std::f32::consts::PI * t).cos(),
        3 => t * t,
        _ => t,
    }
}

pub fn fade(i: &[Vec<f32>], fade_in: bool, shape: usize) -> Vec<Vec<f32>> {
    i.iter()
        .map(|c| {
            let n = c.len().max(2) as f32 - 1.0;
            c.iter()
                .enumerate()
                .map(|(k, s)| {
                    let t = k as f32 / n;
                    let g = if fade_in { fade_curve(t, shape) } else { fade_curve(1.0 - t, shape) };
                    s * g
                })
                .collect()
        })
        .collect()
}

#[inline]
fn gain_computer(level_db: f32, thr: f32, ratio: f32, knee: f32) -> f32 {
    let over = level_db - thr;
    let slope = 1.0 / ratio.max(1.0) - 1.0;
    if knee > 0.0 && 2.0 * over.abs() <= knee {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

#[allow(clippy::too_many_arguments)]
pub fn compress(
    i: &[Vec<f32>],
    sr: u32,
    thr: f32,
    ratio: f32,
    knee: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
    link: bool,
) -> Vec<Vec<f32>> {
    let att = smooth_coeff(attack_ms, sr);
    let rel = smooth_coeff(release_ms, sr);
    let makeup = db_to_lin(makeup_db);
    let len = len_of(i);
    let nch = i.len();
    let mut out = i.to_vec();
    let groups: Vec<Vec<usize>> = if link { vec![(0..nch).collect()] } else { (0..nch).map(|c| vec![c]).collect() };
    for g in groups {
        let mut env = 0.0f32; // gain reduction in dB (<= 0)
        for n in 0..len {
            let level = g.iter().map(|&c| i[c][n].abs()).fold(0.0, f32::max);
            let gr = gain_computer(lin_to_db(level), thr, ratio, knee);
            env = if gr < env { att * env + (1.0 - att) * gr } else { rel * env + (1.0 - rel) * gr };
            let gain = db_to_lin(env) * makeup;
            for &c in &g {
                out[c][n] = i[c][n] * gain;
            }
        }
    }
    out
}

fn hard_limiter(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let ceiling = db_to_lin(p.f("ceiling"));
    let boost = db_to_lin(p.f("boost"));
    let la = ms_to_samples(p.f("lookahead"), c.sample_rate).max(1);
    let rel = smooth_coeff(p.f("release"), c.sample_rate);
    let len = len_of(i);
    // Required gain per sample.
    let req: Vec<f32> = (0..len)
        .map(|n| {
            let pk = i.iter().map(|ch| ch[n].abs()).fold(0.0, f32::max) * boost;
            if pk > ceiling { ceiling / pk } else { 1.0 }
        })
        .collect();
    // Forward-looking minimum over [n, n + la].
    let mut fmin = vec![1.0f32; len];
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for n in (0..len).rev() {
        while let Some(&b) = dq.back() {
            if req[b] >= req[n] {
                dq.pop_back();
            } else {
                break;
            }
        }
        dq.push_back(n);
        while let Some(&f) = dq.front() {
            if f > n + la {
                dq.pop_front();
            } else {
                break;
            }
        }
        fmin[n] = req[*dq.front().unwrap()];
    }
    // Box-smooth over the look-ahead window (backwards) so gain ramps in.
    let mut prefix = vec![0.0f64; len + 1];
    for n in 0..len {
        prefix[n + 1] = prefix[n] + fmin[n] as f64;
    }
    let mut out = i.to_vec();
    let mut g = 1.0f32;
    for n in 0..len {
        let a = n.saturating_sub(la);
        // Mean of the forward minimum over [n - la, n]; every term is <= req[n]
        // when a peak lies at n, so the ramp never overshoots.
        let s = ((prefix[n + 1] - prefix[a]) / (n + 1 - a) as f64) as f32;
        g = if s < g { s } else { rel * g + (1.0 - rel) * s };
        for (o, ch) in out.iter_mut().zip(i.iter()) {
            let v = ch[n] * boost * g;
            o[n] = v.clamp(-ceiling, ceiling);
        }
    }
    Ok(out)
}

fn noise_gate(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate;
    let thr = db_to_lin(p.f("threshold"));
    let floor = db_to_lin(-p.f("range"));
    let att = smooth_coeff(p.f("attack"), sr);
    let rel = smooth_coeff(p.f("release"), sr);
    let hold = ms_to_samples(p.f("hold"), sr);
    let det_decay = smooth_coeff(10.0, sr);
    let len = len_of(i);
    let mut out = i.to_vec();
    let mut env = 0.0f32;
    let mut g = floor;
    let mut hold_left = 0usize;
    for n in 0..len {
        let level = i.iter().map(|ch| ch[n].abs()).fold(0.0, f32::max);
        env = level.max(env * det_decay);
        let open = if env >= thr {
            hold_left = hold;
            true
        } else if hold_left > 0 {
            hold_left -= 1;
            true
        } else {
            false
        };
        let target = if open { 1.0 } else { floor };
        g = if target > g { att * g + (1.0 - att) * target } else { rel * g + (1.0 - rel) * target };
        for (o, ch) in out.iter_mut().zip(i.iter()) {
            o[n] = ch[n] * g;
        }
    }
    Ok(out)
}

fn leveler(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate;
    let target = p.f("target");
    let max_gain = p.f("max_gain");
    let noise = p.f("noise");
    let win = smooth_coeff(50.0, sr);
    let speed = smooth_coeff(p.f("speed"), sr);
    let len = len_of(i);
    let mut out = i.to_vec();
    let mut ms = 0.0f32;
    let mut gain_db = 0.0f32;
    for n in 0..len {
        let sq = i.iter().map(|ch| ch[n] * ch[n]).fold(0.0, f32::max);
        ms = win * ms + (1.0 - win) * sq;
        let level = lin_to_db(ms.sqrt());
        if level > noise {
            let want = (target - level).clamp(-max_gain, max_gain);
            gain_db = speed * gain_db + (1.0 - speed) * want;
        }
        let g = db_to_lin(gain_db);
        for (o, ch) in out.iter_mut().zip(i.iter()) {
            o[n] = (ch[n] * g).clamp(-0.999, 0.999);
        }
    }
    Ok(out)
}
