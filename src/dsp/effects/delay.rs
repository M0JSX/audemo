use super::prelude::*;
use super::Category::Delay;

pub fn defs() -> Vec<EffectDef> {
    vec![
        EffectDef::new(
            "delay",
            "Delay",
            Delay,
            "Single delayed copy per channel, for slap-back and widening.",
            vec![
                float("left_ms", "Left delay", 0.0, 2000.0, 120.0, "ms").group("Left"),
                float("left_mix", "Left mix", 0.0, 100.0, 40.0, "%").group("Left"),
                float("right_ms", "Right delay", 0.0, 2000.0, 180.0, "ms").group("Right"),
                float("right_mix", "Right mix", 0.0, 100.0, 40.0, "%").group("Right"),
                toggle("invert", "Invert delayed signal", false),
            ],
            delay,
        )
        .presets(vec![
            ("Slap Back", vec![("left_ms", Value::F(90.0)), ("right_ms", Value::F(90.0)), ("left_mix", Value::F(35.0)), ("right_mix", Value::F(35.0))]),
            ("Stereo Widener", vec![("left_ms", Value::F(0.0)), ("right_ms", Value::F(18.0)), ("left_mix", Value::F(0.0)), ("right_mix", Value::F(50.0))]),
        ]),
        EffectDef::new(
            "echo",
            "Echo",
            Delay,
            "Repeating echoes with feedback and optional ping-pong.",
            vec![
                float("delay_ms", "Delay time", 10.0, 3000.0, 350.0, "ms"),
                float("feedback", "Feedback", 0.0, 95.0, 45.0, "%"),
                float("level", "Echo level", 0.0, 100.0, 50.0, "%"),
                toggle("pingpong", "Ping-pong (stereo)", false),
            ],
            echo,
        )
        .presets(vec![
            ("Canyon", vec![("delay_ms", Value::F(700.0)), ("feedback", Value::F(55.0)), ("level", Value::F(45.0))]),
            ("Ping Pong", vec![("delay_ms", Value::F(280.0)), ("feedback", Value::F(50.0)), ("pingpong", Value::B(true))]),
            ("Short Doubling", vec![("delay_ms", Value::F(40.0)), ("feedback", Value::F(10.0)), ("level", Value::F(40.0))]),
        ]),
        EffectDef::new(
            "analog_delay",
            "Analog Delay",
            Delay,
            "Echo with a darkening, saturating feedback loop like tape or bucket-brigade units.",
            vec![
                choice("mode", "Mode", &["Tape", "Tube", "Analog (BBD)"], 0),
                float("delay_ms", "Delay", 20.0, 3000.0, 400.0, "ms"),
                float("feedback", "Feedback", 0.0, 110.0, 55.0, "%"),
                float("trash", "Trash (drive)", 0.0, 100.0, 30.0, "%"),
                float("dry", "Dry output", 0.0, 100.0, 80.0, "%"),
                float("wet", "Wet output", 0.0, 100.0, 50.0, "%"),
            ],
            analog_delay,
        ),
    ]
}

fn delay(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sign = if p.b("invert") { -1.0 } else { 1.0 };
    Ok(i.iter()
        .enumerate()
        .map(|(n, ch)| {
            let (ms, mix) = if n == 1 {
                (p.f("right_ms"), p.f("right_mix"))
            } else {
                (p.f("left_ms"), p.f("left_mix"))
            };
            let d = ms_to_samples(ms, c.sample_rate);
            let mix = mix / 100.0;
            (0..ch.len())
                .map(|k| {
                    let wet = if k >= d { ch[k - d] } else { 0.0 };
                    ch[k] * (1.0 - mix) + wet * mix * sign
                })
                .collect()
        })
        .collect())
}

fn echo(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let d = ms_to_samples(p.f("delay_ms"), c.sample_rate).max(1);
    let fb = p.f("feedback") / 100.0;
    let level = p.f("level") / 100.0;
    let len = len_of(i);
    if p.b("pingpong") && i.len() >= 2 {
        let mut bl = vec![0.0f32; d];
        let mut br = vec![0.0f32; d];
        let mut out = i.to_vec();
        let mut idx = 0;
        for n in 0..len {
            let input = 0.5 * (i[0][n] + i[1][n]);
            let ol = bl[idx];
            let or = br[idx];
            bl[idx] = input + or * fb;
            br[idx] = ol * fb;
            out[0][n] = i[0][n] + ol * level;
            out[1][n] = i[1][n] + or * level;
            idx = (idx + 1) % d;
        }
        return Ok(out);
    }
    Ok(i.iter()
        .map(|ch| {
            let mut buf = vec![0.0f32; d];
            let mut idx = 0;
            ch.iter()
                .map(|&x| {
                    let o = buf[idx];
                    buf[idx] = x + o * fb;
                    idx = (idx + 1) % d;
                    x + o * level
                })
                .collect()
        })
        .collect())
}

fn analog_delay(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let d = ms_to_samples(p.f("delay_ms"), c.sample_rate).max(1);
    let fb = p.f("feedback") / 100.0;
    let drive = 1.0 + p.f("trash") / 100.0 * 6.0;
    let dry = p.f("dry") / 100.0;
    let wet = p.f("wet") / 100.0;
    let (cut, hp) = match p.c("mode") {
        0 => (4500.0, 60.0),
        1 => (7000.0, 120.0),
        _ => (2800.0, 200.0),
    };
    Ok(i.iter()
        .map(|ch| {
            let mut buf = vec![0.0f32; d];
            let mut idx = 0;
            let mut lp = Biquad::new(FilterType::LowPass, sr, cut, 0.707, 0.0);
            let mut hpf = Biquad::new(FilterType::HighPass, sr, hp, 0.707, 0.0);
            ch.iter()
                .map(|&x| {
                    let o = buf[idx];
                    let looped = hpf.process(lp.process(o * fb));
                    let sat = (looped * drive).tanh() / drive.tanh().max(1e-6) * 0.95;
                    buf[idx] = x + sat;
                    idx = (idx + 1) % d;
                    x * dry + o * wet
                })
                .collect()
        })
        .collect())
}
