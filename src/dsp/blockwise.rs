//! Effects that change each sample on its own (Invert, Silence, Amplify,
//! Normalize, Fade In/Out) run straight from a document's chunked audio a
//! block at a time, so even hours of selection never need to be loaded at
//! once. The results match the whole-selection versions in
//! `effects::amplitude` sample for sample.

use super::params::Params;
use super::util::db_to_lin;
use crate::audio::{Audio, Builder};

const BLOCK: usize = 1 << 16;

/// Effects that can run block by block.
pub fn supports(id: &str) -> bool {
    matches!(id, "invert" | "silence" | "amplify" | "normalize" | "fade_in" | "fade_out")
}

/// Per-sample operation: (channel, frame offset in the selection, sample) → sample.
enum Op {
    Gain(Vec<f32>),
    Offset { mean: Vec<f32>, gain: Vec<f32> },
    Fade { fade_in: bool, shape: usize, n: f32 },
    Zero,
}

/// Process frames [a, b) of `src` with effect `id`. Channels that aren't
/// active pass through untouched. Returns the replacement audio for [a, b).
pub fn run(id: &str, p: &Params, src: &Audio, a: usize, b: usize, active: &[bool]) -> Result<Audio, String> {
    let n_ch = src.n_ch();
    let len = b.saturating_sub(a);
    if len == 0 || n_ch == 0 {
        return Err("Nothing selected to process.".into());
    }
    let on = |c: usize| active.get(c).copied().unwrap_or(true);
    let op = match id {
        "invert" => Op::Gain(vec![-1.0; n_ch]),
        "silence" => Op::Zero,
        "amplify" => {
            let gl = db_to_lin(p.f("left"));
            let gr = if p.b("link") { gl } else { db_to_lin(p.f("right")) };
            Op::Gain((0..n_ch).map(|c| if c == 1 { gr } else { gl }).collect())
        }
        "fade_in" | "fade_out" => Op::Fade { fade_in: id == "fade_in", shape: p.c("shape"), n: len.max(2) as f32 - 1.0 },
        "normalize" => normalize_op(p, src, a, b, &on),
        _ => return Err(format!("{id} can't be processed block by block.")),
    };
    let mut out = Builder::new(n_ch);
    let mut buf = vec![vec![0.0f32; BLOCK]; n_ch];
    let mut pos = 0;
    while pos < len {
        let n = BLOCK.min(len - pos);
        for (c, ch) in buf.iter_mut().enumerate() {
            ch.resize(n, 0.0);
            src.read(c, a + pos, ch);
            if !on(c) {
                continue;
            }
            match &op {
                Op::Zero => ch.iter_mut().for_each(|s| *s = 0.0),
                Op::Gain(g) => ch.iter_mut().for_each(|s| *s *= g[c]),
                Op::Offset { mean, gain } => ch.iter_mut().for_each(|s| *s = (*s - mean[c]) * gain[c]),
                Op::Fade { fade_in, shape, n } => {
                    for (k, s) in ch.iter_mut().enumerate() {
                        let t = (pos + k) as f32 / n;
                        let g = if *fade_in { super::effects::fade_curve(t, *shape) } else { super::effects::fade_curve(1.0 - t, *shape) };
                        *s *= g;
                    }
                }
            }
            ch.iter_mut().for_each(|s| {
                if !s.is_finite() {
                    *s = 0.0
                }
            });
        }
        out.push(&buf);
        pos += n;
    }
    Ok(out.finish())
}

/// Measure the selection (peaks come straight from the chunk summaries
/// unless DC is being removed) and work out each channel's gain.
fn normalize_op(p: &Params, src: &Audio, a: usize, b: usize, on: &dyn Fn(usize) -> bool) -> Op {
    let n_ch = src.n_ch();
    let len = b - a;
    let dc = p.b("dc");
    let rms = p.c("mode") == 1;
    let mut buf = vec![0.0f32; BLOCK];
    let scan = |c: usize, buf: &mut Vec<f32>, f: &mut dyn FnMut(&[f32])| {
        let mut pos = 0;
        while pos < len {
            let n = BLOCK.min(len - pos);
            buf.resize(n, 0.0);
            src.read(c, a + pos, buf);
            f(buf);
            pos += n;
        }
    };
    let mut mean = vec![0.0f32; n_ch];
    if dc {
        for (c, m) in mean.iter_mut().enumerate() {
            let mut sum = 0.0f64;
            scan(c, &mut buf, &mut |x| sum += x.iter().map(|&s| s as f64).sum::<f64>());
            *m = (sum / len.max(1) as f64) as f32;
        }
    }
    // Inactive channels are measured as they would be by the whole-selection
    // effect (which only ever sees the active channels).
    let mut level = vec![0.0f32; n_ch];
    for c in (0..n_ch).filter(|&c| on(c)) {
        let m = mean[c];
        level[c] = if rms {
            let mut sum = 0.0f64;
            scan(c, &mut buf, &mut |x| sum += x.iter().map(|&s| ((s - m) as f64) * ((s - m) as f64)).sum::<f64>());
            (sum / len as f64).sqrt() as f32
        } else if dc {
            let mut pk = 0.0f32;
            scan(c, &mut buf, &mut |x| pk = x.iter().fold(pk, |p, &s| p.max((s - m).abs())));
            pk
        } else {
            let (lo, hi) = src.min_max(c, a, b);
            lo.abs().max(hi.abs())
        };
    }
    let target = db_to_lin(p.f("target"));
    let gain_for = |m: f32| if m > 1e-9 { target / m } else { 1.0 };
    let gain = if p.b("together") {
        let g = gain_for(level.iter().copied().fold(0.0, f32::max));
        vec![g; n_ch]
    } else {
        level.iter().map(|&m| gain_for(m)).collect()
    };
    Op::Offset { mean, gain }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::effects::{registry, Ctx};
    use crate::dsp::params::Value;

    #[test]
    fn matches_whole_selection_effects() {
        let len = 200_000;
        let mk = |ph: f32, dc: f32| (0..len).map(|i| (i as f32 * 0.0013 + ph).sin() * 0.4 + dc).collect::<Vec<f32>>();
        let chans = vec![mk(0.0, 0.05), mk(1.0, -0.1)];
        let src = Audio::from(chans.clone());
        let (a, b) = (12_345, 170_001);
        let input: Vec<Vec<f32>> = chans.iter().map(|c| c[a..b].to_vec()).collect();
        let ctx = Ctx { sample_rate: 48000, channels: 2, noise_print: None, sound_model: None };
        let defs = registry();
        let mut cases: Vec<(&str, Params)> = vec![("invert", Params::default()), ("silence", Params::default())];
        let mut amp = Params::default();
        amp.set("left", Value::F(-4.5));
        amp.set("right", Value::F(3.0));
        cases.push(("amplify", amp));
        for shape in 0..4 {
            let mut f = Params::default();
            f.set("shape", Value::C(shape));
            cases.push(("fade_in", f.clone()));
            cases.push(("fade_out", f));
        }
        for (mode, dc, together) in [(0, false, true), (0, true, false), (1, false, false), (1, true, true)] {
            let mut n = Params::default();
            n.set("mode", Value::C(mode));
            n.set("target", Value::F(-2.0));
            n.set("dc", Value::B(dc));
            n.set("together", Value::B(together));
            cases.push(("normalize", n));
        }
        for (id, p) in cases {
            let def = defs.iter().find(|d| d.id == id).unwrap();
            let want = def.run(&input, &ctx, &p).unwrap();
            let got = run(id, &p, &src, a, b, &[true, true]).unwrap();
            assert_eq!(got.len(), b - a);
            for c in 0..2 {
                for i in (0..b - a).step_by(97) {
                    let (w, g) = (want[c][i], got.sample(c, i));
                    assert!((w - g).abs() <= 1e-5 * w.abs().max(1.0), "{id} {p:?} ch{c} @{i}: {w} vs {g}");
                }
            }
        }
        // Inactive channels pass through.
        let got = run("silence", &Params::default(), &src, a, b, &[false, true]).unwrap();
        assert_eq!(got.sample(0, 10), chans[0][a + 10]);
        assert_eq!(got.sample(1, 10), 0.0);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::dsp::params::Value;

    /// 40 minutes of stereo at 48 kHz with a small memory budget: build,
    /// amplify block by block, save as WAV. `cargo test -- --ignored bench`
    #[test]
    #[ignore]
    fn bench_long_file() {
        let t = std::time::Instant::now();
        crate::audio::set_budget(256 << 20);
        let sr = 48000usize;
        let frames = sr * 60 * 40;
        let mut b = Builder::new(2);
        let blk = 48000;
        let mut l = vec![0.0f32; blk];
        let mut pos = 0;
        while pos < frames {
            for (k, s) in l.iter_mut().enumerate() {
                *s = ((pos + k) as f32 * 0.01).sin() * 0.3;
            }
            b.push(&[l.clone(), l.clone()]);
            pos += blk;
        }
        let au = b.finish();
        eprintln!("built {} frames in {:.1} s; usage {:?}", au.len(), t.elapsed().as_secs_f32(), crate::audio::usage());
        let t = std::time::Instant::now();
        let mut p = Params::default();
        p.set("left", Value::F(-3.0));
        p.set("link", Value::B(true));
        let out = run("amplify", &p, &au, 0, au.len(), &[true, true]).unwrap();
        let joined = au.replace_with(0, au.len(), &out);
        eprintln!("amplified in {:.1} s; usage {:?}", t.elapsed().as_secs_f32(), crate::audio::usage());
        let t = std::time::Instant::now();
        let dir = std::env::temp_dir().join("audemo-bench.wav");
        let s = crate::export::ExportSettings::default();
        crate::export::save_audio(&dir, &joined, sr as u32, &s, &Default::default(), &[], &Default::default()).unwrap();
        eprintln!("saved {} MB in {:.1} s", std::fs::metadata(&dir).unwrap().len() >> 20, t.elapsed().as_secs_f32());
        let _ = std::fs::remove_file(&dir);
        crate::audio::set_budget(0);
    }
}
