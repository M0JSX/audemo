use super::biquad::*;
use super::effects::*;
use super::fft::*;
use super::params::*;
use super::peaks::PeakCache;
use super::resample::*;
use super::util::*;

const SR: u32 = 44100;

fn sine(f: f32, secs: f32, amp: f32) -> Vec<f32> {
    let n = (secs * SR as f32) as usize;
    (0..n).map(|i| amp * (std::f32::consts::TAU * f * i as f32 / SR as f32).sin()).collect()
}

fn ctx(ch: usize) -> Ctx<'static> {
    Ctx { sample_rate: SR, channels: ch, noise_print: None }
}

fn rms_between(x: &[f32], a: usize, b: usize) -> f32 {
    rms_ch(&x[a..b])
}

/// Energy of `x` at frequency `f` (single-bin DFT).
fn tone_level(x: &[f32], f: f32) -> f32 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &s) in x.iter().enumerate() {
        let a = std::f64::consts::TAU * f as f64 * i as f64 / SR as f64;
        re += s as f64 * a.cos();
        im += s as f64 * a.sin();
    }
    (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
}

fn find(reg: &[EffectDef], id: &str) -> usize {
    reg.iter().position(|e| e.id == id).unwrap_or_else(|| panic!("no effect {id}"))
}

#[test]
fn fft_round_trip() {
    let fft = Fft::new(256);
    let orig: Vec<Complex> = (0..256).map(|i| Complex::new((i as f32 * 0.37).sin(), 0.0)).collect();
    let mut b = orig.clone();
    fft.forward(&mut b);
    fft.inverse(&mut b);
    for (a, b) in orig.iter().zip(&b) {
        assert!((a.re - b.re).abs() < 1e-4 && b.im.abs() < 1e-4);
    }
}

#[test]
fn fft_finds_tone() {
    let n = 1024;
    let fft = Fft::new(n);
    let mut b: Vec<Complex> = (0..n)
        .map(|i| Complex::new((std::f32::consts::TAU * 64.0 * i as f32 / n as f32).cos(), 0.0))
        .collect();
    fft.forward(&mut b);
    let peak = (0..n / 2).max_by(|&a, &c| b[a].norm().partial_cmp(&b[c].norm()).unwrap()).unwrap();
    assert_eq!(peak, 64);
}

#[test]
fn convolution_matches_direct() {
    let x: Vec<f32> = (0..3000).map(|i| ((i * 7919) % 101) as f32 / 50.0 - 1.0).collect();
    let h: Vec<f32> = (0..700).map(|i| (-(i as f32) / 100.0).exp()).collect();
    let y = fft_convolve(&x, &h);
    assert_eq!(y.len(), x.len() + h.len() - 1);
    for &n in &[0usize, 10, 699, 1500, 3500] {
        let mut d = 0.0f64;
        for k in 0..h.len() {
            if n >= k && n - k < x.len() {
                d += x[n - k] as f64 * h[k] as f64;
            }
        }
        assert!((d as f32 - y[n]).abs() < 1e-2, "n={n} {d} vs {}", y[n]);
    }
}

#[test]
fn biquad_lowpass_attenuates() {
    let mut lp = butterworth(FilterType::LowPass, SR as f32, 1000.0, 4);
    let mut hi = sine(8000.0, 0.5, 0.5);
    let mut lo = sine(200.0, 0.5, 0.5);
    run_cascade(&mut hi, &mut lp);
    run_cascade(&mut lo, &mut lp);
    let r_hi = rms_between(&hi, 5000, hi.len());
    let r_lo = rms_between(&lo, 5000, lo.len());
    assert!(lin_to_db(r_hi / r_lo) < -60.0, "{}", lin_to_db(r_hi / r_lo));
    let f = Biquad::new(FilterType::Peak, SR as f32, 1000.0, 1.0, 6.0);
    assert!((f.response_db(1000.0, SR as f32) - 6.0).abs() < 0.05);
}

#[test]
fn every_effect_runs_with_defaults_and_presets() {
    let reg = registry();
    assert!(reg.len() >= 35, "only {} effects", reg.len());
    let l = sine(440.0, 1.0, 0.5);
    let r: Vec<f32> = sine(660.0, 1.0, 0.4);
    let input = vec![l, r];
    let profile = capture_noise_print(&[sine(3000.0, 0.5, 0.01), sine(3000.0, 0.5, 0.01)], SR).unwrap();
    for e in &reg {
        let mut all = vec![("(default)", e.default_params())];
        for (name, _) in &e.presets {
            all.push((name, e.preset_params(name).unwrap()));
        }
        for (name, p) in all {
            let c = Ctx { sample_rate: SR, channels: 2, noise_print: Some(&profile) };
            let out = e.run(&input, &c, &p).unwrap_or_else(|err| panic!("{} {name}: {err}", e.id));
            assert_eq!(out.len(), 2, "{} channel count", e.id);
            if !e.changes_length {
                assert_eq!(out[0].len(), input[0].len(), "{} {name} length", e.id);
            }
            assert!(out[0].len() == out[1].len(), "{} ragged", e.id);
            let pk = peak(&out);
            assert!(pk.is_finite() && pk < 50.0, "{} {name} peak {pk}", e.id);
        }
    }
}

#[test]
fn mono_input_rejected_by_stereo_effects_only() {
    let reg = registry();
    let mono = vec![sine(440.0, 0.2, 0.5)];
    for e in &reg {
        let res = e.run(&mono, &ctx(1), &e.default_params());
        if e.stereo_only {
            assert!(res.is_err(), "{}", e.id);
        } else if e.id != "noise_reduction" {
            assert!(res.is_ok(), "{}: {:?}", e.id, res.err());
        }
    }
}

#[test]
fn normalize_hits_target() {
    let reg = registry();
    let e = &reg[find(&reg, "normalize")];
    let p = e.preset_params("Normalize to -3 dB").unwrap();
    let out = e.run(&[sine(100.0, 0.5, 0.1)], &ctx(1), &p).unwrap();
    assert!((lin_to_db(peak(&out)) + 3.0).abs() < 0.01);
}

#[test]
fn limiter_respects_ceiling() {
    let reg = registry();
    let e = &reg[find(&reg, "hard_limiter")];
    let mut p = e.default_params();
    p.set("ceiling", Value::F(-6.0));
    p.set("boost", Value::F(12.0));
    let x = sine(220.0, 1.0, 0.9);
    let out = e.run(&[x], &ctx(1), &p).unwrap();
    assert!(peak(&out) <= db_to_lin(-6.0) + 1e-5, "{}", lin_to_db(peak(&out)));
    // It should still be loud, not just silenced.
    assert!(rms_ch(&out[0]) > 0.2);
}

#[test]
fn compressor_reduces_level_above_threshold() {
    let reg = registry();
    let e = &reg[find(&reg, "dynamics")];
    let out = e.run(&[sine(220.0, 1.0, 0.9)], &ctx(1), &e.default_params()).unwrap();
    assert!(rms_between(&out[0], 22050, 44100) < 0.9 * 0.707 * 0.5);
}

#[test]
fn dehummer_removes_50hz() {
    let reg = registry();
    let e = &reg[find(&reg, "dehummer")];
    let speech = sine(700.0, 2.0, 0.3);
    let hum = sine(50.0, 2.0, 0.3);
    let x: Vec<f32> = speech.iter().zip(&hum).map(|(a, b)| a + b).collect();
    let out = e.run(&[x], &ctx(1), &e.default_params()).unwrap();
    let tail = &out[0][44100..];
    assert!(tone_level(tail, 50.0) < 0.01, "hum {}", tone_level(tail, 50.0));
    assert!(tone_level(tail, 700.0) > 0.27);
}

#[test]
fn noise_reduction_lowers_noise_and_keeps_tone() {
    let reg = registry();
    let e = &reg[find(&reg, "noise_reduction")];
    let mut rng = Rng::new(5);
    let noise: Vec<f32> = (0..SR as usize * 2).map(|_| rng.bipolar() * 0.05).collect();
    let profile = capture_noise_print(&[noise[..SR as usize].to_vec()], SR).unwrap();
    let tone = sine(1000.0, 1.0, 0.3);
    let mixed: Vec<f32> = tone.iter().zip(&noise[SR as usize..]).map(|(a, b)| a + b).collect();
    let c = Ctx { sample_rate: SR, channels: 1, noise_print: Some(&profile) };
    let out = e.run(&[mixed.clone()], &c, &e.default_params()).unwrap();
    assert!(tone_level(&out[0][4096..40000], 1000.0) > 0.27, "tone {}", tone_level(&out[0][4096..40000], 1000.0));
    // Noise on its own drops by roughly the configured amount (80% ~ -11 dB).
    let quiet = e.run(&[noise[SR as usize..].to_vec()], &c, &e.default_params()).unwrap();
    let before = rms_between(&noise[SR as usize..], 4096, 40000);
    let after = rms_between(&quiet[0], 4096, 40000);
    assert!(lin_to_db(after / before) < -9.0, "only {} dB", lin_to_db(after / before));
    // And it refuses politely without a print.
    assert!(e.run(&[mixed], &ctx(1), &e.default_params()).is_err());
}

#[test]
fn stretch_changes_length_not_pitch() {
    let x = sine(440.0, 1.0, 0.5);
    let reg = registry();
    let e = &reg[find(&reg, "stretch_pitch")];
    let p = e.preset_params("Slow Down 20%").unwrap();
    let out = e.run(&[x.clone()], &ctx(1), &p).unwrap();
    assert_eq!(out[0].len(), (x.len() as f32 * 1.25).round() as usize);
    let mid = &out[0][8000..out[0].len() - 8000];
    assert!(tone_level(mid, 440.0) > 0.3, "{}", tone_level(mid, 440.0));
}

#[test]
fn pitch_shift_keeps_length_moves_pitch() {
    let x = sine(440.0, 1.0, 0.5);
    let reg = registry();
    let e = &reg[find(&reg, "pitch_shifter")];
    let mut p = e.default_params();
    p.set("semitones", Value::F(12.0));
    let out = e.run(&[x.clone()], &ctx(1), &p).unwrap();
    assert_eq!(out[0].len(), x.len());
    let mid = &out[0][8000..36000];
    assert!(tone_level(mid, 880.0) > 0.25, "880: {}", tone_level(mid, 880.0));
    assert!(tone_level(mid, 440.0) < 0.05, "440: {}", tone_level(mid, 440.0));
}

#[test]
fn resample_preserves_frequency() {
    let x = sine(1000.0, 0.5, 0.5);
    let y = resample(&x, 48000.0 / 44100.0);
    assert_eq!(y.len(), (x.len() as f64 * 48000.0 / 44100.0).round() as usize);
    // Measured at the new rate a 1 kHz tone must still be 1 kHz.
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, &s) in y[2000..20000].iter().enumerate() {
        let a = std::f64::consts::TAU * 1000.0 * i as f64 / 48000.0;
        re += s as f64 * a.cos();
        im += s as f64 * a.sin();
    }
    let lvl = 2.0 * (re * re + im * im).sqrt() / 18000.0;
    assert!((lvl - 0.5).abs() < 0.02, "{lvl}");
}

#[test]
fn declipper_restores_peaks() {
    let reg = registry();
    let e = &reg[find(&reg, "declipper")];
    let clean = sine(200.0, 0.2, 1.0);
    let clipped: Vec<f32> = clean.iter().map(|s| s.clamp(-0.7, 0.7)).collect();
    let mut p = e.default_params();
    p.set("gain", Value::F(0.0));
    let out = e.run(&[clipped.clone()], &ctx(1), &p).unwrap();
    let err_before: f32 = clean.iter().zip(&clipped).map(|(a, b)| (a - b).abs()).sum();
    let err_after: f32 = clean.iter().zip(&out[0]).map(|(a, b)| (a - b).abs()).sum();
    assert!(err_after < err_before * 0.5, "{err_after} vs {err_before}");
}

#[test]
fn declicker_removes_spike() {
    let reg = registry();
    let e = &reg[find(&reg, "declicker")];
    let mut x = sine(300.0, 0.3, 0.3);
    x[5000] = 0.95;
    x[5001] = -0.8;
    let out = e.run(&[x], &ctx(1), &e.default_params()).unwrap();
    assert!(out[0][5000].abs() < 0.4 && out[0][5001].abs() < 0.4, "{} {}", out[0][5000], out[0][5001]);
}

#[test]
fn generators_produce_requested_duration() {
    let reg = registry();
    for id in ["gen_tone", "gen_noise", "gen_silence"] {
        let e = &reg[find(&reg, id)];
        let mut p = e.default_params();
        p.set("duration", Value::F(0.5));
        let out = e.run(&[vec![], vec![]], &ctx(2), &p).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), SR as usize / 2, "{id}");
    }
    let e = &reg[find(&reg, "gen_tone")];
    let mut p = e.default_params();
    p.set("amp", Value::F(-6.0));
    let out = e.run(&[vec![]], &ctx(1), &p).unwrap();
    assert!((lin_to_db(peak(&out)) + 6.0).abs() < 0.1);
}

#[test]
fn eq_response_matches_processing() {
    let reg = registry();
    let e = &reg[find(&reg, "parametric_eq")];
    let mut p = e.default_params();
    p.set("p2_f", Value::F(1000.0));
    p.set("p2_g", Value::F(9.0));
    let resp = (e.response.unwrap())(&p, SR as f32, 1000.0);
    assert!((resp - 9.0).abs() < 0.2, "{resp}");
    let out = e.run(&[sine(1000.0, 1.0, 0.1)], &ctx(1), &p).unwrap();
    let gain = lin_to_db(rms_between(&out[0], 10000, 44100) / (0.1 / 2f32.sqrt()));
    assert!((gain - 9.0).abs() < 0.3, "{gain}");
}

#[test]
fn centre_extractor_removes_centred_tone() {
    let reg = registry();
    let e = &reg[find(&reg, "centre_extract")];
    let vocal = sine(1000.0, 1.0, 0.3);
    let guitar = sine(330.0, 1.0, 0.3);
    let l: Vec<f32> = vocal.iter().zip(&guitar).map(|(v, g)| v + g).collect();
    let r = vocal.clone();
    let out = e.run(&[l, r], &ctx(2), &e.default_params()).unwrap();
    let seg = &out[0][8000..36000];
    assert!(tone_level(seg, 1000.0) < 0.1, "vocal {}", tone_level(seg, 1000.0));
    assert!(tone_level(seg, 330.0) > 0.15, "guitar {}", tone_level(seg, 330.0));
}

#[test]
fn peak_cache_matches_raw() {
    let x = sine(50.0, 2.0, 0.8);
    let chs = vec![x.clone()];
    let pc = PeakCache::build(&chs);
    let (lo, hi) = pc.min_max(&chs, 0, 0, x.len());
    assert!((hi - 0.8).abs() < 0.01 && (lo + 0.8).abs() < 0.01);
    let (lo2, hi2) = pc.min_max(&chs, 0, 100, 110);
    let raw = &x[100..110];
    assert_eq!(hi2, raw.iter().cloned().fold(f32::MIN, f32::max));
    assert_eq!(lo2, raw.iter().cloned().fold(f32::MAX, f32::min));
}

#[test]
fn time_format_round_trip() {
    assert_eq!(format_time(44100.0 * 75.25, SR), "1:15.250");
    assert_eq!(parse_time("1:15.25", SR), Some(44100.0 * 75.25));
    assert_eq!(parse_time("2.5", SR), Some(44100.0 * 2.5));
    assert_eq!(parse_time("x", SR), None);
}

#[test]
fn remap_mono_stereo() {
    let m = vec![vec![1.0, 0.5]];
    let s = remap_channels(&m, 2);
    assert_eq!(s.len(), 2);
    let back = remap_channels(&s, 1);
    assert_eq!(back[0], vec![1.0, 0.5]);
}

