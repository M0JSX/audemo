use super::prelude::*;
use super::Category::Stereo;

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "channel_mixer",
            "Channel Mixer",
            Stereo,
            "Rebuild each output channel from a mix of the inputs.",
            vec![
                float("ll", "Left from left", -200.0, 200.0, 100.0, "%").group("New left"),
                float("lr", "Left from right", -200.0, 200.0, 0.0, "%").group("New left"),
                toggle("inv_l", "Invert left", false).group("New left"),
                float("rl", "Right from left", -200.0, 200.0, 0.0, "%").group("New right"),
                float("rr", "Right from right", -200.0, 200.0, 100.0, "%").group("New right"),
                toggle("inv_r", "Invert right", false).group("New right"),
            ],
            channel_mixer,
        )
        .stereo_only()
        .presets(vec![
            ("Swap Channels", vec![("ll", Value::F(0.0)), ("lr", Value::F(100.0)), ("rl", Value::F(100.0)), ("rr", Value::F(0.0))]),
            ("Mono Mix", vec![("ll", Value::F(50.0)), ("lr", Value::F(50.0)), ("rl", Value::F(50.0)), ("rr", Value::F(50.0))]),
            ("Left to Both", vec![("lr", Value::F(0.0)), ("rl", Value::F(100.0)), ("rr", Value::F(0.0))]),
            ("Right to Both", vec![("ll", Value::F(0.0)), ("lr", Value::F(100.0)), ("rl", Value::F(0.0))]),
            ("Mid / Side Encode", vec![("ll", Value::F(50.0)), ("lr", Value::F(50.0)), ("rl", Value::F(50.0)), ("rr", Value::F(-50.0))]),
        ]),
        EffectDef::new(
            "stereo_expander",
            "Stereo Expander",
            Stereo,
            "Narrow or widen the stereo image with mid/side scaling.",
            vec![
                float("width", "Stereo width", 0.0, 300.0, 150.0, "%"),
                float("centre", "Centre position", -100.0, 100.0, 0.0, "%"),
                toggle("bass_mono", "Keep bass in mono (below 120 Hz)", true),
            ],
            stereo_expander,
        )
        .stereo_only(),
        EffectDef::new(
            "centre_extract",
            "Centre Channel Extractor",
            Stereo,
            "Isolate or remove whatever sits in the centre (often the lead vocal).",
            vec![
                choice("mode", "Mode", &["Remove centre", "Extract centre"], 0),
                float("amount", "Amount", 0.0, 100.0, 100.0, "%"),
                float("low", "Low frequency", 20.0, 2000.0, 150.0, "Hz").log().decimals(0),
                float("high", "High frequency", 1000.0, 20000.0, 9000.0, "Hz").log().decimals(0),
            ],
            centre_extract,
        )
        .stereo_only(),
        EffectDef::new(
            "pan",
            "Pan",
            Stereo,
            "Shift the stereo balance left or right.",
            vec![
                float("pan", "Pan", -100.0, 100.0, 0.0, "L / R"),
                choice("law", "Pan law", &["Constant power (-3 dB)", "Linear (-6 dB)"], 0),
            ],
            pan,
        )
        .stereo_only(),
    ]
}

fn channel_mixer(i: &[Vec<f32>], _: &Ctx, p: &Params) -> Res {
    let (l, r) = (&i[0], &i[1]);
    let sl = if p.b("inv_l") { -1.0 } else { 1.0 };
    let sr = if p.b("inv_r") { -1.0 } else { 1.0 };
    let (ll, lr, rl, rr) = (p.f("ll") / 100.0, p.f("lr") / 100.0, p.f("rl") / 100.0, p.f("rr") / 100.0);
    let mut out = i.to_vec();
    for n in 0..l.len() {
        out[0][n] = (l[n] * ll + r[n] * lr) * sl;
        out[1][n] = (l[n] * rl + r[n] * rr) * sr;
    }
    Ok(out)
}

fn stereo_expander(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let w = p.f("width") / 100.0;
    let centre = p.f("centre") / 100.0;
    let bass_mono = p.b("bass_mono");
    let mut lp = butterworth(FilterType::LowPass, sr, 120.0, 2);
    let mut out = i.to_vec();
    for n in 0..i[0].len() {
        let (l, r) = (i[0][n], i[1][n]);
        let m = 0.5 * (l + r);
        let mut s = 0.5 * (l - r);
        if bass_mono {
            let mut low = s;
            for f in lp.iter_mut() {
                low = f.process(low);
            }
            s = low + (s - low) * w;
        } else {
            s *= w;
        }
        let ml = m * (1.0 - centre).min(1.0);
        let mr = m * (1.0 + centre).min(1.0);
        out[0][n] = ml + s;
        out[1][n] = mr - s;
    }
    Ok(out)
}

fn centre_extract(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let amount = p.f("amount") / 100.0;
    let extract = p.c("mode") == 1;
    let n = 2048;
    let bins = n / 2;
    let lo = (p.f("low") / sr * n as f32) as usize;
    let hi = ((p.f("high") / sr * n as f32) as usize).min(bins);
    let mut out = stft_process_multi(&[&i[0], &i[1]], n, n / 4, |b, _| {
        for k in 0..=bins {
            let (l, r) = (b[0][k], b[1][k]);
            let c = (l + r).scale(0.5);
            // 1.0 when L and R agree in this bin (centre-panned), 0.0 when opposed.
            let sim = 1.0 - (l - r).norm() / (l.norm() + r.norm() + 1e-9);
            let in_band = k >= lo && k <= hi;
            let m = if in_band { sim.clamp(0.0, 1.0).powi(4) * amount } else { 0.0 };
            let (nl, nr) = if extract {
                let keep = 1.0 - amount;
                (l.scale(keep) + c.scale(m), r.scale(keep) + c.scale(m))
            } else {
                (l - c.scale(m), r - c.scale(m))
            };
            b[0][k] = nl;
            b[1][k] = nr;
            if k != 0 && k != bins {
                b[0][n - k] = nl.conj();
                b[1][n - k] = nr.conj();
            }
        }
    });
    let mut res = i.to_vec();
    res[1] = out.pop().unwrap();
    res[0] = out.pop().unwrap();
    Ok(res)
}

fn pan(i: &[Vec<f32>], _: &Ctx, p: &Params) -> Res {
    let x = (p.f("pan") / 100.0).clamp(-1.0, 1.0);
    let (gl, gr) = if p.c("law") == 0 {
        let a = (x + 1.0) * std::f32::consts::FRAC_PI_4;
        (a.cos() * std::f32::consts::SQRT_2, a.sin() * std::f32::consts::SQRT_2)
    } else {
        ((1.0 - x).min(1.0), (1.0 + x).min(1.0))
    };
    let mut out = i.to_vec();
    out[0].iter_mut().for_each(|s| *s *= gl.min(1.0));
    out[1].iter_mut().for_each(|s| *s *= gr.min(1.0));
    Ok(out)
}
