use super::prelude::*;
use super::Category::Modulation;
use std::f32::consts::{PI, TAU};

struct DelayLine {
    buf: Vec<f32>,
    w: usize,
}

impl DelayLine {
    fn new(max: usize) -> Self {
        DelayLine { buf: vec![0.0; max.max(4) + 4], w: 0 }
    }
    fn push(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) % self.buf.len();
    }
    /// Read `d` samples behind the most recent write (fractional).
    fn read(&self, d: f32) -> f32 {
        let n = self.buf.len();
        let d = d.clamp(1.0, (n - 3) as f32);
        let pos = self.w as f32 - d;
        let pos = if pos < 0.0 { pos + n as f32 } else { pos };
        let i0 = pos.floor() as usize % n;
        let i1 = (i0 + 1) % n;
        let fr = pos - pos.floor();
        self.buf[i0] * (1.0 - fr) + self.buf[i1] * fr
    }
}

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "chorus",
            "Chorus",
            Modulation,
            "Several slightly detuned, delayed voices for a thicker sound.",
            vec![
                choice("voices", "Voices", &["2", "3", "4", "5", "6", "8"], 2),
                float("delay", "Delay time", 5.0, 50.0, 18.0, "ms"),
                float("depth", "Modulation depth", 0.1, 20.0, 4.0, "ms"),
                float("rate", "Modulation rate", 0.05, 8.0, 0.8, "Hz").log(),
                float("feedback", "Feedback", 0.0, 80.0, 0.0, "%"),
                float("width", "Stereo width", 0.0, 100.0, 70.0, "%"),
                float("mix", "Mix", 0.0, 100.0, 50.0, "%"),
            ],
            chorus,
        )
        .presets(vec![
            ("Subtle Doubling", vec![("voices", Value::C(0)), ("depth", Value::F(2.0)), ("mix", Value::F(35.0))]),
            ("Thick Ensemble", vec![("voices", Value::C(5)), ("depth", Value::F(7.0)), ("rate", Value::F(0.5)), ("mix", Value::F(60.0))]),
        ]),
        EffectDef::new(
            "flanger",
            "Flanger",
            Modulation,
            "Short swept delay with feedback, the classic jet sweep.",
            vec![
                float("delay", "Initial delay", 0.1, 20.0, 1.0, "ms"),
                float("depth", "Sweep depth", 0.1, 20.0, 4.0, "ms"),
                float("rate", "Rate", 0.02, 10.0, 0.3, "Hz").log(),
                float("feedback", "Feedback", -95.0, 95.0, 60.0, "%"),
                float("phase", "Stereo phase", 0.0, 180.0, 90.0, "°"),
                float("mix", "Mix", 0.0, 100.0, 50.0, "%"),
            ],
            flanger,
        )
        .presets(vec![
            ("Jet", vec![("feedback", Value::F(85.0)), ("rate", Value::F(0.12)), ("depth", Value::F(6.0))]),
            ("Through-Zero Light", vec![("delay", Value::F(0.2)), ("feedback", Value::F(20.0)), ("mix", Value::F(50.0))]),
        ]),
        EffectDef::new(
            "phaser",
            "Phaser",
            Modulation,
            "Swept all-pass stages create moving notches.",
            vec![
                choice("stages", "Stages", &["2", "4", "6", "8", "10", "12"], 2),
                float("centre", "Centre frequency", 100.0, 5000.0, 800.0, "Hz").log().decimals(0),
                float("depth", "Depth", 0.0, 4.0, 2.0, "oct"),
                float("rate", "Rate", 0.02, 10.0, 0.4, "Hz").log(),
                float("feedback", "Feedback", -90.0, 90.0, 40.0, "%"),
                float("phase", "Stereo phase", 0.0, 180.0, 90.0, "°"),
                float("mix", "Mix", 0.0, 100.0, 50.0, "%"),
            ],
            phaser,
        ),
    ]
}

fn chorus(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let voices = [2usize, 3, 4, 5, 6, 8][p.c("voices").min(5)];
    let base = p.f("delay") * 0.001 * sr;
    let depth = p.f("depth") * 0.001 * sr;
    let rate = p.f("rate");
    let fb = p.f("feedback") / 100.0;
    let width = p.f("width") / 100.0;
    let mix = p.f("mix") / 100.0;
    let max = (base + depth + 8.0) as usize;
    Ok(i.iter()
        .enumerate()
        .map(|(ci, ch)| {
            let mut line = DelayLine::new(max);
            let mut last = 0.0f32;
            let ch_phase = ci as f32 * PI * 0.5 * width;
            ch.iter()
                .enumerate()
                .map(|(n, &x)| {
                    line.push(x + last * fb);
                    let t = n as f32 / sr;
                    let mut wet = 0.0;
                    for v in 0..voices {
                        let ph = TAU * rate * t + TAU * v as f32 / voices as f32 + ch_phase;
                        let rate_skew = 1.0 + 0.07 * v as f32;
                        let lfo = 0.5 + 0.5 * (ph * rate_skew).sin();
                        wet += line.read(base + depth * lfo);
                    }
                    wet /= voices as f32;
                    last = wet;
                    x * (1.0 - mix) + wet * mix
                })
                .collect()
        })
        .collect())
}

fn flanger(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let base = p.f("delay") * 0.001 * sr;
    let depth = p.f("depth") * 0.001 * sr;
    let rate = p.f("rate");
    let fb = p.f("feedback") / 100.0;
    let phase = p.f("phase").to_radians();
    let mix = p.f("mix") / 100.0;
    let max = (base + depth + 8.0) as usize;
    Ok(i.iter()
        .enumerate()
        .map(|(ci, ch)| {
            let mut line = DelayLine::new(max);
            let mut last = 0.0f32;
            ch.iter()
                .enumerate()
                .map(|(n, &x)| {
                    line.push(x + last * fb);
                    let lfo = 0.5 + 0.5 * (TAU * rate * n as f32 / sr + ci as f32 * phase).sin();
                    let wet = line.read(base + depth * lfo);
                    last = wet;
                    x * (1.0 - mix) + wet * mix
                })
                .collect()
        })
        .collect())
}

fn phaser(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let stages = [2usize, 4, 6, 8, 10, 12][p.c("stages").min(5)];
    let centre = p.f("centre");
    let depth = p.f("depth");
    let rate = p.f("rate");
    let fb = p.f("feedback") / 100.0;
    let phase = p.f("phase").to_radians();
    let mix = p.f("mix") / 100.0;
    Ok(i.iter()
        .enumerate()
        .map(|(ci, ch)| {
            let mut z = vec![0.0f32; stages];
            let mut last = 0.0f32;
            let mut a = 0.0f32;
            ch.iter()
                .enumerate()
                .map(|(n, &x)| {
                    if n % 16 == 0 {
                        let lfo = (TAU * rate * n as f32 / sr + ci as f32 * phase).sin();
                        let fc = (centre * 2f32.powf(depth * lfo)).clamp(20.0, sr * 0.45);
                        let t = (PI * fc / sr).tan();
                        a = (t - 1.0) / (t + 1.0);
                    }
                    let mut v = x + last * fb;
                    for s in z.iter_mut() {
                        let y = a * v + *s;
                        *s = v - a * y;
                        v = y;
                    }
                    last = v;
                    x * (1.0 - mix) + v * mix
                })
                .collect()
        })
        .collect())
}
