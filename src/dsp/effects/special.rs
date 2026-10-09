use super::amplitude::compress;
use super::prelude::*;
use super::Category::Special;

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "distortion",
            "Distortion",
            Special,
            "Waveshaping saturation, clipping and bit-crushing.",
            vec![
                choice("kind", "Type", &["Soft clip", "Hard clip", "Tube (asymmetric)", "Foldback", "Bit crusher"], 0),
                float("drive", "Drive", 0.0, 48.0, 12.0, "dB"),
                float("bits", "Bit depth (crusher)", 1.0, 16.0, 8.0, "bits").decimals(0),
                float("downsample", "Downsample (crusher)", 1.0, 32.0, 4.0, "x").decimals(0),
                float("tone", "Tone (low-pass)", 500.0, 20000.0, 12000.0, "Hz").log().decimals(0),
                float("mix", "Mix", 0.0, 100.0, 100.0, "%"),
                float("output", "Output", -36.0, 6.0, -6.0, "dB"),
            ],
            distortion,
        )
        .presets(vec![
            ("Warm Saturation", vec![("drive", Value::F(6.0)), ("kind", Value::C(2)), ("output", Value::F(-3.0))]),
            ("Fuzz", vec![("drive", Value::F(36.0)), ("kind", Value::C(1)), ("tone", Value::F(4500.0)), ("output", Value::F(-12.0))]),
            ("8-bit Console", vec![("kind", Value::C(4)), ("bits", Value::F(8.0)), ("downsample", Value::F(6.0)), ("drive", Value::F(0.0)), ("output", Value::F(0.0))]),
            ("Radio Overdrive", vec![("kind", Value::C(0)), ("drive", Value::F(18.0)), ("tone", Value::F(3000.0)), ("output", Value::F(-8.0))]),
        ]),
        EffectDef::new(
            "vocal_enhancer",
            "Vocal Enhancer",
            Special,
            "One-step clarity for voice or music: tuned EQ plus gentle compression.",
            vec![
                choice("mode", "Mode", &["Male voice", "Female voice", "Music"], 0),
                float("amount", "Amount", 0.0, 100.0, 70.0, "%"),
            ],
            vocal_enhancer,
        ),
    ]
}

fn shape(kind: usize, x: f32) -> f32 {
    match kind {
        1 => x.clamp(-1.0, 1.0),
        2 => {
            if x >= 0.0 {
                x.tanh()
            } else {
                (x * 0.6).tanh() / 0.6f32.tanh().max(1e-6) * 0.8
            }
        }
        3 => {
            // Foldback: reflect anything past ±1.
            let mut v = x;
            for _ in 0..8 {
                if v > 1.0 {
                    v = 2.0 - v;
                } else if v < -1.0 {
                    v = -2.0 - v;
                } else {
                    break;
                }
            }
            v.clamp(-1.0, 1.0)
        }
        _ => x.tanh(),
    }
}

fn distortion(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let kind = p.c("kind");
    let drive = db_to_lin(p.f("drive"));
    let mix = p.f("mix") / 100.0;
    let out_g = db_to_lin(p.f("output"));
    let levels = 2f32.powf(p.f("bits").round().max(1.0) - 1.0);
    let ds = p.f("downsample").round().max(1.0) as usize;
    Ok(i.iter()
        .map(|ch| {
            let mut tone = Biquad::new(FilterType::LowPass, sr, p.f("tone"), 0.707, 0.0);
            let mut held = 0.0f32;
            ch.iter()
                .enumerate()
                .map(|(n, &x)| {
                    let wet = if kind == 4 {
                        if n % ds == 0 {
                            held = ((x * drive).clamp(-1.0, 1.0) * levels).round() / levels;
                        }
                        held
                    } else {
                        shape(kind, x * drive)
                    };
                    let wet = tone.process(wet);
                    (x * (1.0 - mix) + wet * mix) * out_g
                })
                .collect()
        })
        .collect())
}

fn vocal_enhancer(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let a = p.f("amount") / 100.0;
    let mut filters = match p.c("mode") {
        0 => vec![
            Biquad::new(FilterType::HighPass, sr, 75.0, 0.707, 0.0),
            Biquad::new(FilterType::Peak, sr, 300.0, 1.2, -3.0 * a),
            Biquad::new(FilterType::Peak, sr, 3000.0, 0.9, 3.5 * a),
            Biquad::new(FilterType::HighShelf, sr, 9000.0, 0.707, 1.5 * a),
        ],
        1 => vec![
            Biquad::new(FilterType::HighPass, sr, 100.0, 0.707, 0.0),
            Biquad::new(FilterType::Peak, sr, 400.0, 1.2, -2.5 * a),
            Biquad::new(FilterType::Peak, sr, 5000.0, 0.9, 3.0 * a),
            Biquad::new(FilterType::HighShelf, sr, 11000.0, 0.707, 2.5 * a),
        ],
        _ => vec![
            Biquad::new(FilterType::LowShelf, sr, 90.0, 0.707, 2.5 * a),
            Biquad::new(FilterType::Peak, sr, 450.0, 0.8, -1.5 * a),
            Biquad::new(FilterType::HighShelf, sr, 8000.0, 0.707, 2.5 * a),
        ],
    };
    let eq: Vec<Vec<f32>> = i
        .iter()
        .map(|ch| {
            let mut x = ch.clone();
            run_cascade(&mut x, &mut filters);
            x
        })
        .collect();
    Ok(compress(&eq, c.sample_rate, -22.0, 1.0 + 2.0 * a, 8.0, 8.0, 120.0, 2.5 * a, true))
}
