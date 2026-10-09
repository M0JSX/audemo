use super::prelude::*;
use super::Category::Reverb;

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "studio_reverb",
            "Studio Reverb",
            Reverb,
            "Fast algorithmic room reverb.",
            vec![
                float("room", "Room size", 0.0, 100.0, 60.0, "%"),
                float("damping", "Damping", 0.0, 100.0, 45.0, "%"),
                float("width", "Width", 0.0, 100.0, 100.0, "%"),
                float("predelay", "Pre-delay", 0.0, 200.0, 12.0, "ms"),
                float("lowcut", "Low cut", 20.0, 1000.0, 80.0, "Hz").log().decimals(0),
                float("dry", "Dry", -60.0, 6.0, 0.0, "dB"),
                float("wet", "Wet", -60.0, 6.0, -12.0, "dB"),
            ],
            studio_reverb,
        )
        .presets(vec![
            ("Small Room", vec![("room", Value::F(35.0)), ("damping", Value::F(60.0)), ("wet", Value::F(-14.0))]),
            ("Vocal Plate", vec![("room", Value::F(70.0)), ("damping", Value::F(30.0)), ("predelay", Value::F(25.0)), ("wet", Value::F(-10.0))]),
            ("Great Hall", vec![("room", Value::F(92.0)), ("damping", Value::F(25.0)), ("predelay", Value::F(40.0)), ("wet", Value::F(-8.0))]),
        ]),
        EffectDef::new(
            "full_reverb",
            "Full Reverb",
            Reverb,
            "Convolution reverb with a synthesised impulse response; smooth, dense tails.",
            vec![
                float("decay", "Decay time (RT60)", 0.1, 10.0, 1.8, "s").log(),
                float("predelay", "Pre-delay", 0.0, 250.0, 20.0, "ms"),
                float("damping", "High-frequency damping", 0.0, 100.0, 50.0, "%"),
                float("early", "Early reflections", 0.0, 100.0, 40.0, "%"),
                float("width", "Stereo width", 0.0, 100.0, 80.0, "%"),
                float("dry", "Dry", -60.0, 6.0, 0.0, "dB"),
                float("wet", "Wet", -60.0, 6.0, -10.0, "dB"),
            ],
            full_reverb,
        )
        .presets(vec![
            ("Drum Room", vec![("decay", Value::F(0.6)), ("damping", Value::F(60.0)), ("early", Value::F(70.0))]),
            ("Concert Hall", vec![("decay", Value::F(2.8)), ("predelay", Value::F(35.0)), ("damping", Value::F(40.0))]),
            ("Cathedral", vec![("decay", Value::F(6.5)), ("predelay", Value::F(60.0)), ("damping", Value::F(30.0)), ("wet", Value::F(-6.0))]),
        ]),
    ]
}

pub(super) struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    pub(super) fn new(len: usize) -> Self {
        Comb { buf: vec![0.0; len.max(1)], idx: 0, store: 0.0 }
    }
    #[inline]
    pub(super) fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.idx] = x + self.store * feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

pub(super) struct AllPass {
    buf: Vec<f32>,
    idx: usize,
}

impl AllPass {
    pub(super) fn new(len: usize) -> Self {
        AllPass { buf: vec![0.0; len.max(1)], idx: 0 }
    }
    #[inline]
    pub(super) fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let out = -x + b;
        self.buf[self.idx] = x + b * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

pub(super) const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
pub(super) const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
pub(super) const SPREAD: usize = 23;

pub(super) fn studio_reverb(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let scale = sr / 44100.0;
    let feedback = p.f("room") / 100.0 * 0.28 + 0.7;
    let damp = p.f("damping") / 100.0 * 0.4;
    let width = p.f("width") / 100.0;
    let wet = db_to_lin(p.f("wet")) * 3.0;
    let dry = db_to_lin(p.f("dry"));
    let wet1 = wet * (width / 2.0 + 0.5);
    let wet2 = wet * ((1.0 - width) / 2.0);
    let pre = ms_to_samples(p.f("predelay"), c.sample_rate);
    let len = len_of(i);
    let stereo = i.len() >= 2;

    let mk = |spread: usize| {
        (
            COMBS.iter().map(|&l| Comb::new(((l + spread) as f32 * scale) as usize)).collect::<Vec<_>>(),
            ALLPASSES.iter().map(|&l| AllPass::new(((l + spread) as f32 * scale) as usize)).collect::<Vec<_>>(),
        )
    };
    let (mut cl, mut al) = mk(0);
    let (mut cr, mut ar) = mk(SPREAD);
    let mut hp_l = Biquad::new(FilterType::HighPass, sr, p.f("lowcut"), 0.707, 0.0);
    let mut hp_r = hp_l;
    let mut out = i.to_vec();
    for n in 0..len {
        let src = if n >= pre { n - pre } else { usize::MAX };
        let input = if src == usize::MAX {
            0.0
        } else if stereo {
            (i[0][src] + i[1][src]) * 0.5
        } else {
            i[0][src]
        } * 0.015;
        let mut ol = 0.0;
        let mut or = 0.0;
        for cmb in cl.iter_mut() {
            ol += cmb.process(input, feedback, damp);
        }
        for cmb in cr.iter_mut() {
            or += cmb.process(input, feedback, damp);
        }
        for a in al.iter_mut() {
            ol = a.process(ol);
        }
        for a in ar.iter_mut() {
            or = a.process(or);
        }
        let ol = hp_l.process(ol);
        let or = hp_r.process(or);
        if stereo {
            out[0][n] = i[0][n] * dry + ol * wet1 + or * wet2;
            out[1][n] = i[1][n] * dry + or * wet1 + ol * wet2;
        } else {
            out[0][n] = i[0][n] * dry + (ol + or) * 0.5 * wet;
        }
    }
    Ok(out)
}

/// Exponentially decaying noise with progressive high-frequency loss.
fn synth_ir(sr: u32, decay: f32, damping: f32, early: f32, seed: u64) -> Vec<f32> {
    let len = ((decay * 1.2).min(12.0) * sr as f32) as usize + 1;
    let mut rng = Rng::new(seed);
    let k = 6.9078 / (decay * sr as f32); // -60 dB at RT60
    let mut ir = Vec::with_capacity(len);
    let mut lp = 0.0f32;
    for n in 0..len {
        let t = n as f32 / len as f32;
        // Lowpass coefficient rises over time = more HF loss later.
        let a = (damping * (0.15 + 0.8 * t)).min(0.97);
        lp = lp * a + rng.bipolar() * (1.0 - a);
        let env = (-k * n as f32).exp();
        ir.push(lp * env);
    }
    // A handful of discrete early reflections in the first 80 ms.
    let er_span = (0.08 * sr as f32) as usize;
    for j in 0..12 {
        let pos = (rng.next_u64() as usize % er_span.max(1)).min(len - 1);
        let amp = early * (1.0 - j as f32 / 12.0) * if rng.bipolar() > 0.0 { 1.0 } else { -1.0 };
        ir[pos] += amp;
    }
    let energy: f32 = ir.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
    ir.iter_mut().for_each(|v| *v /= energy);
    ir
}

fn full_reverb(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate;
    let decay = p.f("decay");
    let damping = p.f("damping") / 100.0;
    let early = p.f("early") / 100.0 * 0.6;
    let width = p.f("width") / 100.0;
    let pre = ms_to_samples(p.f("predelay"), sr);
    let wet = db_to_lin(p.f("wet"));
    let dry = db_to_lin(p.f("dry"));
    let a = synth_ir(sr, decay, damping, early, 11);
    let b = synth_ir(sr, decay, damping, early, 29);
    let irs: Vec<Vec<f32>> = (0..i.len())
        .map(|ci| {
            let mut ir = vec![0.0f32; pre];
            if ci % 2 == 0 {
                ir.extend_from_slice(&a);
            } else {
                ir.extend(a.iter().zip(b.iter()).map(|(x, y)| x * (1.0 - width) + y * width));
            }
            ir
        })
        .collect();
    Ok(i.iter()
        .zip(irs.iter())
        .map(|(ch, ir)| {
            let conv = fft_convolve(ch, ir);
            ch.iter().zip(conv.iter()).map(|(x, w)| x * dry + w * wet).collect()
        })
        .collect())
}
