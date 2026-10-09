use super::prelude::*;
use super::Category::Filter;

pub const GEQ_BANDS: [(&str, &str, f32); 10] = [
    ("g31", "31.5 Hz", 31.5),
    ("g63", "63 Hz", 63.0),
    ("g125", "125 Hz", 125.0),
    ("g250", "250 Hz", 250.0),
    ("g500", "500 Hz", 500.0),
    ("g1k", "1 kHz", 1000.0),
    ("g2k", "2 kHz", 2000.0),
    ("g4k", "4 kHz", 4000.0),
    ("g8k", "8 kHz", 8000.0),
    ("g16k", "16 kHz", 16000.0),
];

pub fn defs() -> Vec<EffectDef> {
    let mut geq_params = vec![];
    for (k, label, _) in GEQ_BANDS {
        geq_params.push(float(k, label, -20.0, 20.0, 0.0, "dB"));
    }
    geq_params.push(float("master", "Master gain", -20.0, 20.0, 0.0, "dB"));

    vec![
        EffectDef::new(
            "parametric_eq",
            "Parametric Equalizer",
            Filter,
            "Five-band EQ with shelves and optional low/high-cut. Drag the points on the curve.",
            vec![
                float("ls_f", "Frequency", 20.0, 1000.0, 100.0, "Hz").log().decimals(0).group("Low shelf"),
                float("ls_g", "Gain", -24.0, 24.0, 0.0, "dB").group("Low shelf"),
                float("p1_f", "Frequency", 20.0, 20000.0, 300.0, "Hz").log().decimals(0).group("Band 1"),
                float("p1_g", "Gain", -24.0, 24.0, 0.0, "dB").group("Band 1"),
                float("p1_q", "Q", 0.1, 20.0, 1.0, "").log().group("Band 1"),
                float("p2_f", "Frequency", 20.0, 20000.0, 1500.0, "Hz").log().decimals(0).group("Band 2"),
                float("p2_g", "Gain", -24.0, 24.0, 0.0, "dB").group("Band 2"),
                float("p2_q", "Q", 0.1, 20.0, 1.0, "").log().group("Band 2"),
                float("p3_f", "Frequency", 20.0, 20000.0, 5000.0, "Hz").log().decimals(0).group("Band 3"),
                float("p3_g", "Gain", -24.0, 24.0, 0.0, "dB").group("Band 3"),
                float("p3_q", "Q", 0.1, 20.0, 1.0, "").log().group("Band 3"),
                float("hs_f", "Frequency", 1000.0, 20000.0, 10000.0, "Hz").log().decimals(0).group("High shelf"),
                float("hs_g", "Gain", -24.0, 24.0, 0.0, "dB").group("High shelf"),
                toggle("hp_on", "Low cut (high-pass)", false).group("Cut filters"),
                float("hp_f", "Low cut freq", 10.0, 2000.0, 80.0, "Hz").log().decimals(0).group("Cut filters"),
                toggle("lp_on", "High cut (low-pass)", false).group("Cut filters"),
                float("lp_f", "High cut freq", 1000.0, 22000.0, 16000.0, "Hz").log().decimals(0).group("Cut filters"),
                float("master", "Master gain", -24.0, 24.0, 0.0, "dB").group("Output"),
            ],
            parametric_eq,
        )
        .response(|p, sr, f| cascade_response_db(&peq_filters(p, sr), f, sr) + p.f("master"))
        .handles(vec![("ls_f", "ls_g"), ("p1_f", "p1_g"), ("p2_f", "p2_g"), ("p3_f", "p3_g"), ("hs_f", "hs_g")])
        .presets(vec![
            ("Vocal Presence", vec![("hp_on", Value::B(true)), ("hp_f", Value::F(90.0)), ("p1_f", Value::F(250.0)), ("p1_g", Value::F(-3.0)), ("p2_f", Value::F(3500.0)), ("p2_g", Value::F(3.5)), ("hs_g", Value::F(2.0))]),
            ("Telephone", vec![("hp_on", Value::B(true)), ("hp_f", Value::F(400.0)), ("lp_on", Value::B(true)), ("lp_f", Value::F(3200.0)), ("p2_f", Value::F(1500.0)), ("p2_g", Value::F(6.0))]),
            ("Remove Rumble", vec![("hp_on", Value::B(true)), ("hp_f", Value::F(60.0))]),
            ("Warmth", vec![("ls_f", Value::F(150.0)), ("ls_g", Value::F(3.0)), ("hs_g", Value::F(-2.0))]),
            ("SSB Voice (2.4 kHz)", vec![("hp_on", Value::B(true)), ("hp_f", Value::F(300.0)), ("lp_on", Value::B(true)), ("lp_f", Value::F(2700.0)), ("p2_f", Value::F(1800.0)), ("p2_g", Value::F(4.0))]),
        ]),
        EffectDef::new(
            "graphic_eq",
            "Graphic Equalizer (10 Bands)",
            Filter,
            "Octave-spaced band gains.",
            geq_params,
            graphic_eq,
        )
        .response(|p, sr, f| cascade_response_db(&geq_filters(p, sr), f, sr) + p.f("master"))
        .presets(vec![
            ("Smiley Face", vec![("g31", Value::F(5.0)), ("g63", Value::F(4.0)), ("g125", Value::F(2.0)), ("g500", Value::F(-2.0)), ("g1k", Value::F(-2.0)), ("g4k", Value::F(2.0)), ("g8k", Value::F(4.0)), ("g16k", Value::F(5.0))]),
            ("Bass Boost", vec![("g31", Value::F(6.0)), ("g63", Value::F(5.0)), ("g125", Value::F(3.0))]),
            ("Treble Boost", vec![("g4k", Value::F(2.0)), ("g8k", Value::F(4.0)), ("g16k", Value::F(6.0))]),
        ]),
        EffectDef::new(
            "scientific_filter",
            "Scientific Filter",
            Filter,
            "Steep Butterworth low-pass, high-pass, band-pass or band-stop.",
            vec![
                choice("kind", "Type", &["Low pass", "High pass", "Band pass", "Band stop"], 0),
                float("freq", "Cutoff / centre", 10.0, 22000.0, 1000.0, "Hz").log().decimals(0),
                float("q", "Q (band types)", 0.1, 30.0, 2.0, "").log(),
                choice("order", "Order", &["2-pole", "4-pole", "6-pole", "8-pole"], 1),
                float("gain", "Output gain", -24.0, 24.0, 0.0, "dB"),
            ],
            scientific,
        )
        .response(|p, sr, f| cascade_response_db(&sci_filters(p, sr), f, sr) + p.f("gain")),
    ]
}

pub fn peq_filters(p: &Params, sr: f32) -> Vec<Biquad> {
    let mut v = vec![
        Biquad::new(FilterType::LowShelf, sr, p.f("ls_f"), 0.707, p.f("ls_g")),
        Biquad::new(FilterType::Peak, sr, p.f("p1_f"), p.f("p1_q"), p.f("p1_g")),
        Biquad::new(FilterType::Peak, sr, p.f("p2_f"), p.f("p2_q"), p.f("p2_g")),
        Biquad::new(FilterType::Peak, sr, p.f("p3_f"), p.f("p3_q"), p.f("p3_g")),
        Biquad::new(FilterType::HighShelf, sr, p.f("hs_f"), 0.707, p.f("hs_g")),
    ];
    if p.b("hp_on") {
        v.extend(butterworth(FilterType::HighPass, sr, p.f("hp_f"), 4));
    }
    if p.b("lp_on") {
        v.extend(butterworth(FilterType::LowPass, sr, p.f("lp_f"), 4));
    }
    v
}

pub(super) fn geq_filters(p: &Params, sr: f32) -> Vec<Biquad> {
    GEQ_BANDS
        .iter()
        .filter(|(_, _, f)| *f < sr * 0.45)
        .map(|(k, _, f)| Biquad::new(FilterType::Peak, sr, *f, 1.41, p.f(k)))
        .collect()
}

pub(super) fn sci_filters(p: &Params, sr: f32) -> Vec<Biquad> {
    let order = (p.c("order") + 1) * 2;
    let f = p.f("freq");
    match p.c("kind") {
        0 => butterworth(FilterType::LowPass, sr, f, order),
        1 => butterworth(FilterType::HighPass, sr, f, order),
        k => {
            let t = if k == 2 { FilterType::BandPass } else { FilterType::Notch };
            (0..order / 2).map(|_| Biquad::new(t, sr, f, p.f("q"), 0.0)).collect()
        }
    }
}

fn apply_filters(i: &[Vec<f32>], filters: &[Biquad], gain_db: f32) -> Vec<Vec<f32>> {
    let g = db_to_lin(gain_db);
    i.iter()
        .map(|ch| {
            let mut c = ch.clone();
            let mut f = filters.to_vec();
            run_cascade(&mut c, &mut f);
            if g != 1.0 {
                c.iter_mut().for_each(|s| *s *= g);
            }
            c
        })
        .collect()
}

fn parametric_eq(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    Ok(apply_filters(i, &peq_filters(p, c.sample_rate as f32), p.f("master")))
}

fn graphic_eq(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    Ok(apply_filters(i, &geq_filters(p, c.sample_rate as f32), p.f("master")))
}

fn scientific(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    Ok(apply_filters(i, &sci_filters(p, c.sample_rate as f32), p.f("gain")))
}
