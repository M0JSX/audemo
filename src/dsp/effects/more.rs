//! Phase 2 effects: the rest of Audition's processing set.

use super::amplitude::{compress, limit};
use super::modulation::{chorus, flanger};
use super::prelude::*;
use super::reverb::studio_reverb;
use super::special::shape;
use super::Category::*;
use super::super::loudness::integrated_lufs;

const GEQ20: [f32; 20] = [
    31.5, 44.0, 63.0, 88.0, 125.0, 177.0, 250.0, 355.0, 500.0, 710.0, 1000.0, 1400.0, 2000.0, 2800.0, 4000.0, 5600.0, 8000.0,
    11200.0, 16000.0, 20000.0,
];
const GEQ30: [f32; 30] = [
    25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0,
    1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10000.0, 12500.0, 16000.0, 20000.0,
];

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn band_label(f: f32) -> &'static str {
    leak(if f >= 1000.0 { format!("{} kHz", f / 1000.0) } else { format!("{f} Hz") })
}

fn geq_params(bands: &[f32]) -> Vec<ParamDef> {
    let mut v: Vec<ParamDef> = bands.iter().enumerate().map(|(i, f)| float(leak(format!("b{i}")), band_label(*f), -20.0, 20.0, 0.0, "dB")).collect();
    v.push(float("master", "Master gain", -20.0, 20.0, 0.0, "dB"));
    v
}

fn geq_filters(bands: &[f32], q: f32, p: &Params, sr: f32) -> Vec<Biquad> {
    bands
        .iter()
        .enumerate()
        .filter(|(_, f)| **f < sr * 0.45)
        .map(|(i, f)| Biquad::new(FilterType::Peak, sr, *f, q, p.f(&format!("b{i}"))))
        .collect()
}

fn filter_all(i: &[Vec<f32>], filters: &[Biquad], gain_db: f32) -> Vec<Vec<f32>> {
    let g = db_to_lin(gain_db);
    i.iter()
        .map(|ch| {
            let mut c = ch.clone();
            let mut f = filters.to_vec();
            run_cascade(&mut c, &mut f);
            c.iter_mut().for_each(|s| *s *= g);
            c
        })
        .collect()
}

fn notch_filters(p: &Params, sr: f32) -> Vec<Biquad> {
    let q = [40.0, 12.0, 4.0][p.c("width").min(2)];
    (1..=6)
        .filter(|n| p.b(&format!("n{n}_on")))
        .map(|n| Biquad::new(FilterType::Peak, sr, p.f(&format!("n{n}_f")), q, -p.f(&format!("n{n}_d"))))
        .collect()
}

pub fn defs() -> Vec<EffectDef> {
    let mut v = vec![
        // ------------------------------------------------ Amplitude and Compression
        EffectDef::new(
            "single_band_compressor",
            "Single-band Compressor",
            Amplitude,
            "Classic hard-knee compressor.",
            vec![
                float("threshold", "Threshold", -60.0, 0.0, -20.0, "dB"),
                float("ratio", "Ratio", 1.0, 30.0, 4.0, ":1").log(),
                float("attack", "Attack", 0.1, 500.0, 10.0, "ms").log(),
                float("release", "Release", 5.0, 5000.0, 100.0, "ms").log(),
                float("output", "Output gain", -30.0, 30.0, 0.0, "dB"),
            ],
            |i, c, p| Ok(compress(i, c.sample_rate, p.f("threshold"), p.f("ratio"), 0.0, p.f("attack"), p.f("release"), p.f("output"), true)),
        )
        .presets(vec![
            ("Voice Leveller", vec![("threshold", Value::F(-24.0)), ("ratio", Value::F(3.0)), ("output", Value::F(6.0))]),
            ("Limit Peaks", vec![("threshold", Value::F(-6.0)), ("ratio", Value::F(20.0)), ("attack", Value::F(0.5))]),
        ]),
        EffectDef::new(
            "tube_compressor",
            "Tube-modeled Compressor",
            Amplitude,
            "Compressor with warm, asymmetric valve-style saturation.",
            vec![
                float("threshold", "Threshold", -60.0, 0.0, -20.0, "dB"),
                float("ratio", "Ratio", 1.0, 30.0, 3.0, ":1").log(),
                float("attack", "Attack", 0.1, 500.0, 15.0, "ms").log(),
                float("release", "Release", 5.0, 5000.0, 200.0, "ms").log(),
                float("drive", "Tube warmth", 0.0, 100.0, 30.0, "%"),
                float("output", "Output gain", -30.0, 30.0, 0.0, "dB"),
            ],
            |i, c, p| {
                let mut out = compress(i, c.sample_rate, p.f("threshold"), p.f("ratio"), 6.0, p.f("attack"), p.f("release"), p.f("output"), true);
                let k = 1.0 + p.f("drive") / 100.0 * 3.0;
                if k > 1.001 {
                    let (np, nn) = (k.tanh(), (0.8 * k).tanh());
                    for ch in out.iter_mut() {
                        for s in ch.iter_mut() {
                            *s = if *s >= 0.0 { (k * *s).tanh() / np } else { (0.8 * k * *s).tanh() / nn };
                        }
                    }
                }
                Ok(out)
            },
        ),
        EffectDef::new(
            "multiband_compressor",
            "Multiband Compressor",
            Amplitude,
            "Four independent compressors split at three crossover frequencies, with an output limiter.",
            {
                let mut p = vec![
                    float("x1", "Low / low-mid", 40.0, 1000.0, 120.0, "Hz").log().decimals(0).group("Crossovers"),
                    float("x2", "Low-mid / high-mid", 300.0, 8000.0, 2000.0, "Hz").log().decimals(0).group("Crossovers"),
                    float("x3", "High-mid / high", 2000.0, 18000.0, 10000.0, "Hz").log().decimals(0).group("Crossovers"),
                ];
                for (b, name) in [(1, "Band 1 (low)"), (2, "Band 2"), (3, "Band 3"), (4, "Band 4 (high)")] {
                    p.push(float(leak(format!("t{b}")), "Threshold", -60.0, 0.0, -18.0, "dB").group(name));
                    p.push(float(leak(format!("r{b}")), "Ratio", 1.0, 20.0, 3.0, ":1").log().group(name));
                    p.push(float(leak(format!("g{b}")), "Gain", -18.0, 18.0, 0.0, "dB").group(name));
                }
                p.push(float("attack", "Attack", 0.1, 200.0, 10.0, "ms").log().group("All bands"));
                p.push(float("release", "Release", 10.0, 2000.0, 150.0, "ms").log().group("All bands"));
                p.push(toggle("limit", "Output limiter", true).group("Output"));
                p.push(float("ceiling", "Limiter ceiling", -12.0, 0.0, -0.1, "dB").group("Output"));
                p.push(float("output", "Output gain", -18.0, 18.0, 0.0, "dB").group("Output"));
                p
            },
            multiband,
        )
        .presets(vec![
            ("Broadcast", vec![("t1", Value::F(-24.0)), ("t2", Value::F(-22.0)), ("t3", Value::F(-22.0)), ("t4", Value::F(-24.0)), ("r1", Value::F(4.0)), ("r2", Value::F(4.0)), ("r3", Value::F(4.0)), ("r4", Value::F(4.0)), ("output", Value::F(6.0))]),
            ("Gentle Mastering", vec![("r1", Value::F(1.8)), ("r2", Value::F(1.5)), ("r3", Value::F(1.5)), ("r4", Value::F(1.8)), ("output", Value::F(2.0))]),
        ]),
        EffectDef::new(
            "deesser",
            "DeEsser",
            Amplitude,
            "Tames harsh \"s\" and \"sh\" sounds by compressing the sibilant band.",
            vec![
                choice("mode", "Mode", &["Multiband (sibilant band only)", "Broadband (whole signal)"], 0),
                float("threshold", "Threshold", -60.0, 0.0, -30.0, "dB"),
                float("freq", "Centre frequency", 2000.0, 12000.0, 6500.0, "Hz").log().decimals(0),
                float("bandwidth", "Bandwidth", 500.0, 8000.0, 3500.0, "Hz").log().decimals(0),
                toggle("listen", "Output sibilance only", false),
            ],
            deesser,
        ),
        EffectDef::new(
            "dc_offset",
            "DC Offset Correction",
            Amplitude,
            "Remove a constant bias so the waveform centres on zero.",
            vec![choice("method", "Method", &["Remove average offset", "High-pass below 10 Hz"], 0)],
            |i, c, p| {
                if p.c("method") == 1 {
                    let f = butterworth(FilterType::HighPass, c.sample_rate as f32, 10.0, 2);
                    return Ok(filter_all(i, &f, 0.0));
                }
                Ok(i.iter()
                    .map(|ch| {
                        let mean = ch.iter().map(|&s| s as f64).sum::<f64>() / ch.len().max(1) as f64;
                        ch.iter().map(|s| s - mean as f32).collect()
                    })
                    .collect())
            },
        ),
        EffectDef::new(
            "match_loudness",
            "Match Loudness",
            Amplitude,
            "Set integrated loudness (ITU-R BS.1770 / EBU R128) to a target, limiting peaks if needed.",
            vec![
                float("target", "Target loudness", -40.0, -5.0, -16.0, "LUFS"),
                toggle("limit", "Use limiting", true),
                float("ceiling", "True-peak limit", -9.0, 0.0, -1.0, "dBTP"),
            ],
            match_loudness,
        )
        .presets(vec![
            ("EBU R128 (-23 LUFS)", vec![("target", Value::F(-23.0)), ("ceiling", Value::F(-1.0))]),
            ("ATSC A/85 (-24 LUFS)", vec![("target", Value::F(-24.0)), ("ceiling", Value::F(-2.0))]),
            ("Podcast (-16 LUFS)", vec![("target", Value::F(-16.0)), ("ceiling", Value::F(-1.0))]),
            ("Streaming (-14 LUFS)", vec![("target", Value::F(-14.0)), ("ceiling", Value::F(-1.0))]),
        ]),
        // ------------------------------------------------ Delay and Echo
        EffectDef::new(
            "multitap_delay",
            "Multitap Delay",
            Delay,
            "Up to four independent delay taps, each with its own feedback, level and pan.",
            {
                let mut p = vec![float("dry", "Dry level", -60.0, 6.0, 0.0, "dB")];
                for (t, d, pan) in [(1, 150.0, -60.0), (2, 300.0, 60.0), (3, 450.0, -30.0), (4, 600.0, 30.0)] {
                    let g = leak(format!("Tap {t}"));
                    p.push(toggle(leak(format!("t{t}_on")), "Enabled", t <= 2).group(g));
                    p.push(float(leak(format!("t{t}_ms")), "Delay", 1.0, 3000.0, d, "ms").group(g));
                    p.push(float(leak(format!("t{t}_lv")), "Level", -40.0, 6.0, -6.0, "dB").group(g));
                    p.push(float(leak(format!("t{t}_fb")), "Feedback", 0.0, 90.0, 20.0, "%").group(g));
                    p.push(float(leak(format!("t{t}_pan")), "Pan", -100.0, 100.0, pan, "L/R").group(g));
                }
                p
            },
            multitap,
        ),
        // ------------------------------------------------ Filter and EQ
        EffectDef::new(
            "notch_filter",
            "Notch Filter",
            Filter,
            "Up to six narrow cuts for whistles, carriers and tones.",
            {
                let mut p = vec![choice("width", "Notch width", &["Narrow", "Medium", "Wide"], 0)];
                for (n, f) in [(1, 1000.0), (2, 2000.0), (3, 3000.0), (4, 4000.0), (5, 5000.0), (6, 6000.0)] {
                    let g = leak(format!("Notch {n}"));
                    p.push(toggle(leak(format!("n{n}_on")), "Enabled", n == 1).group(g));
                    p.push(float(leak(format!("n{n}_f")), "Frequency", 20.0, 20000.0, f, "Hz").log().decimals(0).group(g));
                    p.push(float(leak(format!("n{n}_d")), "Depth", 0.0, 60.0, 40.0, "dB").group(g));
                }
                p.push(float("output", "Output gain", -20.0, 20.0, 0.0, "dB"));
                p
            },
            |i, c, p| Ok(filter_all(i, &notch_filters(p, c.sample_rate as f32), p.f("output"))),
        )
        .response(|p, sr, f| cascade_response_db(&notch_filters(p, sr), f, sr) + p.f("output"))
        .presets(vec![
            ("1 kHz Test Tone", vec![]),
            ("Sweepable Whistle (2.6 kHz)", vec![("n1_f", Value::F(2600.0))]),
        ]),
        EffectDef::new("graphic_eq20", "Graphic Equalizer (20 Bands)", Filter, "Half-octave band gains.", geq_params(&GEQ20), |i, c, p| {
            Ok(filter_all(i, &geq_filters(&GEQ20, 2.87, p, c.sample_rate as f32), p.f("master")))
        })
        .response(|p, sr, f| cascade_response_db(&geq_filters(&GEQ20, 2.87, p, sr), f, sr) + p.f("master")),
        EffectDef::new("graphic_eq30", "Graphic Equalizer (30 Bands)", Filter, "Third-octave band gains.", geq_params(&GEQ30), |i, c, p| {
            Ok(filter_all(i, &geq_filters(&GEQ30, 4.32, p, c.sample_rate as f32), p.f("master")))
        })
        .response(|p, sr, f| cascade_response_db(&geq_filters(&GEQ30, 4.32, p, sr), f, sr) + p.f("master")),
        // ------------------------------------------------ Modulation
        EffectDef::new(
            "chorus_flanger",
            "Chorus/Flanger",
            Modulation,
            "Quick chorus or flange with just four controls.",
            vec![
                choice("mode", "Mode", &["Chorus", "Flanger"], 0),
                float("speed", "Speed", 0.05, 8.0, 0.8, "Hz").log(),
                float("width", "Width", 0.0, 100.0, 50.0, "%"),
                float("intensity", "Intensity", 0.0, 100.0, 20.0, "%"),
                float("mix", "Mix", 0.0, 100.0, 50.0, "%"),
            ],
            |i, c, p| {
                let mut q = Params::default();
                q.set("rate", Value::F(p.f("speed")));
                q.set("mix", Value::F(p.f("mix")));
                if p.c("mode") == 0 {
                    q.set("voices", Value::C(1));
                    q.set("delay", Value::F(15.0));
                    q.set("depth", Value::F(0.5 + p.f("width") / 100.0 * 8.0));
                    q.set("feedback", Value::F(p.f("intensity") * 0.6));
                    q.set("width", Value::F(70.0));
                    chorus(i, c, &q)
                } else {
                    q.set("delay", Value::F(0.5));
                    q.set("depth", Value::F(0.5 + p.f("width") / 100.0 * 6.0));
                    q.set("feedback", Value::F(p.f("intensity") * 0.9));
                    q.set("phase", Value::F(90.0));
                    flanger(i, c, &q)
                }
            },
        ),
        // ------------------------------------------------ Noise Reduction / Restoration
        EffectDef::new(
            "hiss_reduction",
            "Hiss Reduction",
            Restoration,
            "Reduce tape and preamp hiss above a cutoff, learning the noise floor as it goes.",
            vec![
                float("reduce", "Reduce by", 0.0, 40.0, 15.0, "dB"),
                float("cutoff", "Cutoff frequency", 500.0, 12000.0, 3000.0, "Hz").log().decimals(0),
                float("floor", "Noise floor offset", -10.0, 20.0, 6.0, "dB"),
                choice("fft", "FFT size", &["1024", "2048", "4096"], 1),
            ],
            hiss_reduction,
        ),
        EffectDef::new(
            "delete_silence",
            "Delete Silence",
            Restoration,
            "Shorten pauses: any stretch quieter than the threshold for longer than the minimum is cut down.",
            vec![
                float("threshold", "Silence threshold", -90.0, -20.0, -50.0, "dB"),
                float("min_ms", "Minimum silence", 100.0, 5000.0, 800.0, "ms").log(),
                float("keep_ms", "Shorten silence to", 0.0, 2000.0, 250.0, "ms"),
            ],
            delete_silence,
        )
        .changes_length(),
        // ------------------------------------------------ Special
        EffectDef::new(
            "doppler",
            "Doppler Shifter",
            Special,
            "Simulate a sound passing by: pitch, level and pan change as it approaches and recedes.",
            vec![
                float("speed", "Speed", 1.0, 120.0, 25.0, "m/s"),
                float("distance", "Closest distance", 1.0, 200.0, 10.0, "m"),
                toggle("volume", "Adjust volume by distance", true),
                toggle("pan", "Adjust panning by direction", true),
            ],
            doppler,
        ),
        EffectDef::new(
            "guitar_suite",
            "Guitar Suite",
            Special,
            "Compressor, distortion, tone and speaker-cabinet modelling in one.",
            vec![
                float("comp", "Compressor", 0.0, 100.0, 40.0, "%"),
                choice("dist", "Distortion", &["Soft clip", "Hard clip", "Tube", "Fuzz (foldback)"], 2),
                float("drive", "Drive", 0.0, 48.0, 18.0, "dB"),
                float("tone", "Tone", 500.0, 10000.0, 3500.0, "Hz").log().decimals(0),
                choice("cab", "Cabinet", &["1x12 combo", "4x12 stack", "2x15 bass", "None"], 0),
                float("mix", "Mix", 0.0, 100.0, 100.0, "%"),
                float("output", "Output", -36.0, 12.0, -8.0, "dB"),
            ],
            guitar_suite,
        ),
        EffectDef::new(
            "mastering",
            "Mastering",
            Special,
            "One-stop finishing: tone shaping, ambience, exciter, width and a loudness maximizer.",
            vec![
                float("low", "Low shelf (100 Hz)", -12.0, 12.0, 0.0, "dB").group("Equalizer"),
                float("high", "High shelf (10 kHz)", -12.0, 12.0, 0.0, "dB").group("Equalizer"),
                float("reverb", "Reverb", 0.0, 100.0, 0.0, "%").group("Character"),
                float("exciter", "Exciter", 0.0, 100.0, 20.0, "%").group("Character"),
                float("widen", "Widener", 0.0, 100.0, 0.0, "%").group("Character"),
                float("max", "Loudness maximizer", 0.0, 100.0, 30.0, "%").group("Output"),
                float("output", "Output gain", -12.0, 6.0, 0.0, "dB").group("Output"),
            ],
            mastering,
        )
        .presets(vec![
            ("Subtle Polish", vec![("high", Value::F(1.5)), ("exciter", Value::F(15.0)), ("max", Value::F(20.0))]),
            ("Loud and Bright", vec![("low", Value::F(2.0)), ("high", Value::F(3.0)), ("exciter", Value::F(40.0)), ("widen", Value::F(30.0)), ("max", Value::F(70.0))]),
        ]),
        // ------------------------------------------------ Generate
        EffectDef::new(
            "gen_dtmf",
            "DTMF Tones",
            Generate,
            "Dual-tone multi-frequency signalling (0-9, *, #, A-D); other characters insert a pause.",
            vec![
                text("digits", "Digits", "1234"),
                float("tone_ms", "Tone length", 20.0, 1000.0, 100.0, "ms"),
                float("gap_ms", "Gap", 0.0, 1000.0, 100.0, "ms"),
                float("amp", "Amplitude", -40.0, 0.0, -6.0, "dBFS"),
            ],
            dtmf,
        )
        .generator(),
    ];
    // Automatic Phase Correction lives with the stereo tools.
    v.push(
        EffectDef::new(
            "auto_phase",
            "Automatic Phase Correction",
            Stereo,
            "Line up left and right when one channel arrives late (e.g. two mics), and fix reversed polarity.",
            vec![float("max_ms", "Maximum shift", 0.5, 20.0, 5.0, "ms"), toggle("polarity", "Correct polarity", true)],
            auto_phase,
        )
        .stereo_only(),
    );
    v
}

fn multiband(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let xs = [p.f("x1"), p.f("x2").max(p.f("x1") * 1.2), p.f("x3").max(p.f("x2") * 1.2)];
    // Subtractive split: low = LR4 low-pass of x, then peel bands off the remainder.
    let n_ch = i.len();
    let mut bands: Vec<Vec<Vec<f32>>> = vec![Vec::with_capacity(n_ch); 4];
    for ch in i {
        let mut rest = ch.clone();
        for (b, &f) in xs.iter().enumerate() {
            let mut low = rest.clone();
            let mut lp = butterworth(FilterType::LowPass, sr, f, 2);
            lp.extend(butterworth(FilterType::LowPass, sr, f, 2));
            run_cascade(&mut low, &mut lp);
            for (r, l) in rest.iter_mut().zip(&low) {
                *r -= l;
            }
            bands[b].push(low);
        }
        bands[3].push(rest);
    }
    let mut out = vec![vec![0.0f32; i[0].len()]; n_ch];
    for (b, band) in bands.into_iter().enumerate() {
        let k = b + 1;
        let comp = compress(&band, c.sample_rate, p.f(&format!("t{k}")), p.f(&format!("r{k}")), 3.0, p.f("attack"), p.f("release"), p.f(&format!("g{k}")), true);
        for (o, ch) in out.iter_mut().zip(comp) {
            for (a, b) in o.iter_mut().zip(ch) {
                *a += b;
            }
        }
    }
    let g = db_to_lin(p.f("output"));
    out.iter_mut().for_each(|ch| ch.iter_mut().for_each(|s| *s *= g));
    if p.b("limit") {
        out = limit(&out, c.sample_rate, p.f("ceiling"), 0.0, 3.0, 60.0);
    }
    Ok(out)
}

fn deesser(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let f = p.f("freq");
    let q = (f / p.f("bandwidth").max(10.0)).clamp(0.3, 10.0);
    let thr = p.f("threshold");
    let broadband = p.c("mode") == 1;
    let listen = p.b("listen");
    let len = len_of(i);
    let bands: Vec<Vec<f32>> = i
        .iter()
        .map(|ch| {
            let mut b = ch.clone();
            let mut f1 = [Biquad::new(FilterType::BandPass, sr, f, q, 0.0), Biquad::new(FilterType::BandPass, sr, f, q, 0.0)];
            run_cascade(&mut b, &mut f1);
            b
        })
        .collect();
    let att = smooth_coeff(1.0, c.sample_rate);
    let rel = smooth_coeff(60.0, c.sample_rate);
    let mut env = 0.0f32;
    let mut gr = 0.0f32;
    let mut out = i.to_vec();
    for n in 0..len {
        let lvl = bands.iter().map(|b| b[n].abs()).fold(0.0, f32::max);
        env = if lvl > env { att * env + (1.0 - att) * lvl } else { rel * env + (1.0 - rel) * lvl };
        let over = lin_to_db(env) - thr;
        let target = if over > 0.0 { -over * 0.75 } else { 0.0 };
        gr = if target < gr { att * gr + (1.0 - att) * target } else { rel * gr + (1.0 - rel) * target };
        let g = db_to_lin(gr);
        for (ci, o) in out.iter_mut().enumerate() {
            let x = i[ci][n];
            let b = bands[ci][n];
            o[n] = if listen {
                b
            } else if broadband {
                x * g
            } else {
                x - b + b * g
            };
        }
    }
    Ok(out)
}

fn match_loudness(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let measured = integrated_lufs(i, c.sample_rate)
        .ok_or("The audio is too short or too quiet to measure (needs at least 0.4 s above -70 LUFS).")?;
    let gain = db_to_lin((p.f("target") as f64 - measured) as f32);
    let mut out: Vec<Vec<f32>> = i.iter().map(|ch| ch.iter().map(|s| s * gain).collect()).collect();
    if p.b("limit") {
        // A little headroom under the true-peak ceiling for inter-sample overs.
        let ceiling = p.f("ceiling") - 0.5;
        if peak(&out) > db_to_lin(ceiling) {
            out = limit(&out, c.sample_rate, ceiling, 0.0, 5.0, 80.0);
        }
    }
    Ok(out)
}

fn multitap(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let len = len_of(i);
    let n_ch = i.len();
    let dry = db_to_lin(p.f("dry"));
    let mono: Vec<f32> = (0..len).map(|n| i.iter().map(|ch| ch[n]).sum::<f32>() / n_ch as f32).collect();
    let mut out: Vec<Vec<f32>> = i.iter().map(|ch| ch.iter().map(|s| s * dry).collect()).collect();
    for t in 1..=4 {
        if !p.b(&format!("t{t}_on")) {
            continue;
        }
        let d = ms_to_samples(p.f(&format!("t{t}_ms")), c.sample_rate).max(1);
        let lv = db_to_lin(p.f(&format!("t{t}_lv")));
        let fb = p.f(&format!("t{t}_fb")) / 100.0;
        let pan = (p.f(&format!("t{t}_pan")) / 100.0).clamp(-1.0, 1.0);
        let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (gl, gr) = (a.cos() * std::f32::consts::SQRT_2, a.sin() * std::f32::consts::SQRT_2);
        let mut buf = vec![0.0f32; d];
        let mut idx = 0;
        for n in 0..len {
            let o = buf[idx];
            buf[idx] = mono[n] + o * fb;
            idx = (idx + 1) % d;
            if n_ch >= 2 {
                out[0][n] += o * lv * gl.min(1.0);
                out[1][n] += o * lv * gr.min(1.0);
            } else {
                out[0][n] += o * lv;
            }
        }
    }
    Ok(out)
}

fn hiss_reduction(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let n = 1024 << p.c("fft").min(2);
    let hop = n / 4;
    let bins = n / 2 + 1;
    let floor = db_to_lin(-p.f("reduce"));
    let offset = db_to_lin(p.f("floor"));
    let cut_bin = p.f("cutoff") / c.sample_rate as f32 * n as f32;
    // Smooth 1/3-octave ramp in around the cutoff.
    let weight: Vec<f32> = (0..bins)
        .map(|k| {
            let r = (k as f32 / cut_bin.max(1.0)).log2() * 3.0 + 0.5;
            r.clamp(0.0, 1.0)
        })
        .collect();
    let rise = db_to_lin(3.0 * hop as f32 / c.sample_rate as f32);
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
                    est[k] = if mag < est[k] { 0.7 * est[k] + 0.3 * mag } else { est[k] * rise };
                    if frame < 4 {
                        est[k] = est[k].min(mag.max(1e-9));
                    }
                    let nm = est[k] * offset;
                    let g = if mag > 1e-12 { (1.0 - (nm / mag).min(1.0).powi(2)).max(0.0).sqrt() } else { 0.0 };
                    let g = g.max(floor);
                    let g = if g > prev[k] { g } else { 0.6 * prev[k] + 0.4 * g };
                    prev[k] = g;
                    gains[k] = 1.0 - weight[k] * (1.0 - g);
                }
                apply_gains(buf, &gains);
            })
        })
        .collect())
}

fn delete_silence(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate;
    let len = len_of(i);
    let frame = ms_to_samples(10.0, sr).max(1);
    let thr = db_to_lin(p.f("threshold"));
    let min_frames = (p.f("min_ms") / 10.0).ceil() as usize;
    let keep = ms_to_samples(p.f("keep_ms"), sr);
    let n_frames = len / frame;
    let silent: Vec<bool> = (0..n_frames)
        .map(|f| {
            let a = f * frame;
            i.iter().map(|ch| rms_ch(&ch[a..a + frame])).fold(0.0, f32::max) < thr
        })
        .collect();
    // Collect regions to remove.
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let mut f = 0;
    while f < n_frames {
        if silent[f] {
            let s = f;
            while f < n_frames && silent[f] {
                f += 1;
            }
            if f - s >= min_frames {
                let (a, b) = (s * frame, f * frame);
                let half = keep / 2;
                if b - a > keep {
                    cuts.push((a + half, b - (keep - half)));
                }
            }
        } else {
            f += 1;
        }
    }
    if cuts.is_empty() {
        return Ok(i.to_vec());
    }
    let xf = ms_to_samples(2.0, sr).max(1);
    Ok(i.iter()
        .map(|ch| {
            let mut out = Vec::with_capacity(len);
            let mut pos = 0;
            for &(a, b) in &cuts {
                out.extend_from_slice(&ch[pos..a]);
                // Short crossfade over the join.
                let n = xf.min(out.len()).min(len - b);
                let base = out.len() - n;
                for k in 0..n {
                    let t = (k + 1) as f32 / (n + 1) as f32;
                    out[base + k] = out[base + k] * (1.0 - t) + ch[b + k] * t;
                }
                pos = b + n;
            }
            out.extend_from_slice(&ch[pos..]);
            out
        })
        .collect())
}

fn doppler(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f64;
    let len = len_of(i);
    let n_ch = i.len();
    let speed = p.f("speed") as f64;
    let dist = p.f("distance") as f64;
    let total = len as f64 / sr;
    const C: f64 = 343.0;
    let mono: Vec<f32> = (0..len).map(|n| i.iter().map(|ch| ch[n]).sum::<f32>() / n_ch as f32).collect();
    let read = |t: f64| -> f32 {
        let pos = t * sr;
        if pos < 0.0 {
            return 0.0;
        }
        let i0 = pos.floor() as usize;
        let fr = (pos - i0 as f64) as f32;
        let a = mono.get(i0).copied().unwrap_or(0.0);
        let b = mono.get(i0 + 1).copied().unwrap_or(0.0);
        a + (b - a) * fr
    };
    let mut out = vec![vec![0.0f32; len]; n_ch];
    for n in 0..len {
        let t = n as f64 / sr;
        let x = speed * (t - total / 2.0);
        let d = (x * x + dist * dist).sqrt();
        let v = read(t - (d - dist) / C);
        let g = if p.b("volume") { (dist / d) as f32 } else { 1.0 };
        if n_ch >= 2 && p.b("pan") {
            let pan = (x / d) as f32;
            let a = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            out[0][n] = v * g * a.cos() * std::f32::consts::SQRT_2.min(1.0);
            out[1][n] = v * g * a.sin() * std::f32::consts::SQRT_2.min(1.0);
        } else {
            for o in out.iter_mut() {
                o[n] = v * g;
            }
        }
    }
    Ok(out)
}

fn guitar_suite(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let comp = p.f("comp") / 100.0;
    let x = if comp > 0.0 { compress(i, c.sample_rate, -10.0 - comp * 30.0, 1.0 + comp * 7.0, 6.0, 5.0, 120.0, comp * 10.0, true) } else { i.to_vec() };
    let kind = [0usize, 1, 2, 3][p.c("dist").min(3)];
    let drive = db_to_lin(p.f("drive"));
    let mix = p.f("mix") / 100.0;
    let out_g = db_to_lin(p.f("output"));
    let mut cab: Vec<Biquad> = vec![Biquad::new(FilterType::LowPass, sr, p.f("tone"), 0.707, 0.0)];
    match p.c("cab") {
        0 => cab.extend([
            Biquad::new(FilterType::HighPass, sr, 90.0, 0.707, 0.0),
            Biquad::new(FilterType::Peak, sr, 2000.0, 1.0, 4.0),
            Biquad::new(FilterType::LowPass, sr, 4500.0, 0.9, 0.0),
        ]),
        1 => cab.extend([
            Biquad::new(FilterType::HighPass, sr, 70.0, 0.707, 0.0),
            Biquad::new(FilterType::Peak, sr, 1200.0, 0.9, 3.0),
            Biquad::new(FilterType::LowPass, sr, 3500.0, 0.9, 0.0),
        ]),
        2 => cab.extend([
            Biquad::new(FilterType::HighPass, sr, 40.0, 0.707, 0.0),
            Biquad::new(FilterType::Peak, sr, 700.0, 0.8, 3.0),
            Biquad::new(FilterType::LowPass, sr, 2500.0, 0.8, 0.0),
        ]),
        _ => {}
    }
    Ok(x.iter()
        .zip(i)
        .map(|(ch, dry)| {
            let mut wet: Vec<f32> = ch.iter().map(|s| shape(kind, s * drive)).collect();
            let mut f = cab.clone();
            run_cascade(&mut wet, &mut f);
            wet.iter().zip(dry).map(|(w, d)| (d * (1.0 - mix) + w * mix) * out_g).collect()
        })
        .collect())
}

fn mastering(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f32;
    let shelves = [
        Biquad::new(FilterType::LowShelf, sr, 100.0, 0.707, p.f("low")),
        Biquad::new(FilterType::HighShelf, sr, 10000.0, 0.707, p.f("high")),
    ];
    let mut x = filter_all(i, &shelves, 0.0);
    let rv = p.f("reverb") / 100.0;
    if rv > 0.0 {
        let mut q = Params::default();
        for (k, v) in [("room", 40.0), ("damping", 55.0), ("width", 100.0), ("predelay", 10.0), ("lowcut", 150.0), ("dry", 0.0)] {
            q.set(k, Value::F(v));
        }
        q.set("wet", Value::F(-36.0 + rv * 24.0));
        x = studio_reverb(&x, c, &q)?;
    }
    let ex = p.f("exciter") / 100.0;
    if ex > 0.0 {
        let hp = butterworth(FilterType::HighPass, sr, 3000.0, 2);
        let band = filter_all(&x, &hp, 0.0);
        for (ch, b) in x.iter_mut().zip(band) {
            for (s, h) in ch.iter_mut().zip(b) {
                *s += (h * 4.0).tanh() * 0.25 * ex;
            }
        }
    }
    let w = 1.0 + p.f("widen") / 100.0;
    if x.len() >= 2 && w > 1.0 {
        for n in 0..x[0].len() {
            let (l, r) = (x[0][n], x[1][n]);
            let m = 0.5 * (l + r);
            let s = 0.5 * (l - r) * w;
            x[0][n] = m + s;
            x[1][n] = m - s;
        }
    }
    let boost = p.f("max") / 100.0 * 12.0;
    let g = db_to_lin(p.f("output"));
    x = limit(&x, c.sample_rate, -0.3, boost, 5.0, 100.0);
    x.iter_mut().for_each(|ch| ch.iter_mut().for_each(|s| *s *= g));
    Ok(x)
}

fn dtmf_pair(ch: char) -> Option<(f64, f64)> {
    let (row, col) = match ch.to_ascii_uppercase() {
        '1' => (0, 0),
        '2' => (0, 1),
        '3' => (0, 2),
        'A' => (0, 3),
        '4' => (1, 0),
        '5' => (1, 1),
        '6' => (1, 2),
        'B' => (1, 3),
        '7' => (2, 0),
        '8' => (2, 1),
        '9' => (2, 2),
        'C' => (2, 3),
        '*' => (3, 0),
        '0' => (3, 1),
        '#' => (3, 2),
        'D' => (3, 3),
        _ => return None,
    };
    Some(([697.0, 770.0, 852.0, 941.0][row], [1209.0, 1336.0, 1477.0, 1633.0][col]))
}

fn dtmf(_: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let sr = c.sample_rate as f64;
    let tone = ms_to_samples(p.f("tone_ms"), c.sample_rate);
    let gap = ms_to_samples(p.f("gap_ms"), c.sample_rate);
    let amp = db_to_lin(p.f("amp")) as f64 * 0.5;
    let edge = ms_to_samples(2.0, c.sample_rate).min(tone / 2).max(1);
    let mut out: Vec<f32> = Vec::new();
    for ch in p.s("digits").chars().filter(|c| !c.is_whitespace()) {
        match dtmf_pair(ch) {
            Some((lo, hi)) => {
                for k in 0..tone {
                    let t = k as f64 / sr;
                    let fade = (k.min(tone - 1 - k) as f64 / edge as f64).min(1.0);
                    let v = amp * ((std::f64::consts::TAU * lo * t).sin() + (std::f64::consts::TAU * hi * t).sin()) * fade;
                    out.push(v as f32);
                }
            }
            None => out.extend(std::iter::repeat(0.0).take(tone)),
        }
        out.extend(std::iter::repeat(0.0).take(gap));
    }
    if out.is_empty() {
        return Err("Enter at least one digit (0-9, *, #, A-D).".into());
    }
    Ok(vec![out; c.channels.max(1)])
}

fn auto_phase(i: &[Vec<f32>], c: &Ctx, p: &Params) -> Res {
    let len = len_of(i);
    let max_lag = ms_to_samples(p.f("max_ms"), c.sample_rate).max(1) as i64;
    // Analyse up to ~6 s from the middle of the selection.
    let n_an = len.min(1 << 18);
    let start = (len - n_an) / 2;
    let n = (2 * n_an).next_power_of_two();
    let fft = Fft::new(n);
    let mut a = vec![Complex::ZERO; n];
    let mut b = vec![Complex::ZERO; n];
    for k in 0..n_an {
        a[k].re = i[0][start + k];
        b[k].re = i[1][start + k];
    }
    fft.forward(&mut a);
    fft.forward(&mut b);
    for k in 0..n {
        a[k] = a[k] * b[k].conj();
    }
    fft.inverse(&mut a);
    // a[k] = sum L[m + k] R[m]  (k > 0: left lags right).
    let mut best = (0i64, 0.0f32);
    for lag in -max_lag..=max_lag {
        let idx = if lag >= 0 { lag as usize } else { (n as i64 + lag) as usize };
        let v = a[idx].re;
        if v.abs() > best.1.abs() {
            best = (lag, v);
        }
    }
    let (lag, val) = best;
    let mut out = i.to_vec();
    let shift = |ch: &[f32], d: usize| -> Vec<f32> {
        let mut v = vec![0.0f32; d.min(ch.len())];
        v.extend_from_slice(&ch[..ch.len() - d.min(ch.len())]);
        v
    };
    if lag > 0 {
        out[1] = shift(&i[1], lag as usize);
    } else if lag < 0 {
        out[0] = shift(&i[0], (-lag) as usize);
    }
    if p.b("polarity") && val < 0.0 {
        out[1].iter_mut().for_each(|s| *s = -*s);
    }
    Ok(out)
}
