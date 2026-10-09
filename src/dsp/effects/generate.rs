use super::prelude::*;
use super::Category::Generate;
use std::f64::consts::TAU;

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "gen_tone",
            "Tones",
            Generate,
            "Sine, square, triangle or sawtooth test tones, fixed or swept.",
            vec![
                choice("wave", "Waveform", &["Sine", "Square", "Triangle", "Sawtooth"], 0),
                float("freq", "Frequency", 1.0, 22000.0, 1000.0, "Hz").log().decimals(1),
                toggle("sweep", "Sweep frequency", false),
                float("end_freq", "End frequency", 1.0, 22000.0, 8000.0, "Hz").log().decimals(1),
                float("amp", "Amplitude", -96.0, 0.0, -6.0, "dBFS"),
                float("duration", "Duration", 0.01, 600.0, 5.0, "s").log().decimals(2),
            ],
            tone,
        )
        .generator()
        .presets(vec![
            ("1 kHz Reference (-18 dBFS)", vec![("amp", Value::F(-18.0))]),
            ("440 Hz A", vec![("freq", Value::F(440.0))]),
            ("1750 Hz Toneburst", vec![("freq", Value::F(1750.0)), ("duration", Value::F(0.5))]),
            ("20 Hz – 20 kHz Sweep", vec![("freq", Value::F(20.0)), ("sweep", Value::B(true)), ("end_freq", Value::F(20000.0)), ("duration", Value::F(10.0))]),
        ]),
        EffectDef::new(
            "gen_noise",
            "Noise",
            Generate,
            "White, pink or brown noise.",
            vec![
                choice("colour", "Colour", &["White", "Pink", "Brown"], 1),
                float("amp", "Amplitude", -96.0, 0.0, -12.0, "dBFS"),
                float("duration", "Duration", 0.01, 600.0, 5.0, "s").log().decimals(2),
                toggle("uncorrelated", "Independent noise per channel", true),
            ],
            noise,
        )
        .generator(),
        EffectDef::new(
            "gen_silence",
            "Silence",
            Generate,
            "Insert a stretch of silence.",
            vec![float("duration", "Duration", 0.01, 600.0, 1.0, "s").log().decimals(2)],
            |_, c, p| {
                let n = secs_to_samples(p.f("duration"), c.sample_rate);
                Ok(vec![vec![0.0; n]; c.channels.max(1)])
            },
        )
        .generator(),
    ]
}

#[inline]
fn poly_blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

fn tone(_: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f64;
    let n = secs_to_samples(p.f("duration"), c.sample_rate);
    let amp = db_to_lin(p.f("amp")) as f64;
    let f0 = p.f("freq") as f64;
    let f1 = if p.b("sweep") { p.f("end_freq") as f64 } else { f0 };
    let wave = p.c("wave");
    let edge = ((0.005 * sr) as usize).min(n / 2).max(1);
    let mut phase = 0.0f64; // 0..1
    let mut tri = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let t = k as f64 / n.max(1) as f64;
        // Exponential sweep sounds even across octaves.
        let f = if f1 != f0 { f0 * (f1 / f0).powf(t) } else { f0 };
        let dt = (f / sr).min(0.5);
        let v = match wave {
            1 => {
                let mut s = if phase < 0.5 { 1.0 } else { -1.0 };
                s += poly_blep(phase, dt);
                s -= poly_blep((phase + 0.5) % 1.0, dt);
                s
            }
            2 => {
                let mut sq = if phase < 0.5 { 1.0 } else { -1.0 };
                sq += poly_blep(phase, dt);
                sq -= poly_blep((phase + 0.5) % 1.0, dt);
                tri = dt * sq * 4.0 + (1.0 - dt * 0.05) * tri;
                tri
            }
            3 => 2.0 * phase - 1.0 - poly_blep(phase, dt),
            _ => (TAU * phase).sin(),
        };
        let fade = if k < edge {
            k as f64 / edge as f64
        } else if k + edge > n {
            (n - k) as f64 / edge as f64
        } else {
            1.0
        };
        out.push((v * amp * fade) as f32);
        phase += dt;
        if phase >= 1.0 {
            phase -= 1.0;
        }
    }
    Ok(vec![out; c.channels.max(1)])
}

fn noise(_: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let n = secs_to_samples(p.f("duration"), c.sample_rate);
    let amp = db_to_lin(p.f("amp"));
    let colour = p.c("colour");
    let chans = c.channels.max(1);
    let gen = |seed: u64| {
        let mut rng = Rng::new(seed);
        let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
        let mut brown = 0.0f32;
        let mut v: Vec<f32> = (0..n)
            .map(|_| {
                let w = rng.bipolar();
                match colour {
                    1 => {
                        // Paul Kellet's refined pink filter.
                        b0 = 0.99886 * b0 + w * 0.0555179;
                        b1 = 0.99332 * b1 + w * 0.0750759;
                        b2 = 0.96900 * b2 + w * 0.1538520;
                        b3 = 0.86650 * b3 + w * 0.3104856;
                        b4 = 0.55000 * b4 + w * 0.5329522;
                        b5 = -0.7616 * b5 - w * 0.0168980;
                        let pk = b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362;
                        b6 = w * 0.115926;
                        pk
                    }
                    2 => {
                        brown = (brown + w * 0.02) * 0.998;
                        brown
                    }
                    _ => w,
                }
            })
            .collect();
        let pk = peak_ch(&v).max(1e-9);
        v.iter_mut().for_each(|s| *s *= amp / pk);
        v
    };
    if p.b("uncorrelated") {
        Ok((0..chans).map(|ci| gen(1234 + ci as u64 * 7919)).collect())
    } else {
        Ok(vec![gen(1234); chans])
    }
}
