//! Streaming versions of effects for multitrack track, bus and master racks.
//!
//! The offline effects process a whole selection at once. These process
//! audio block by block as it plays, keeping their state (filter memories,
//! envelopes, delay lines, reverb tails) between blocks, and they accept new
//! parameters while running without resetting that state. Each one matches
//! its offline counterpart sample for sample (the limiter adds its look-ahead
//! as latency), which the tests check.

use super::modulation::DelayLine;
use super::prelude::*;
use super::{filter, more, reverb, special};
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

pub trait RtEffect: Send {
    fn set_params(&mut self, p: &Params);
    /// Clear all state (start of playback).
    fn reset(&mut self);
    /// Process one block of stereo audio in place.
    fn process(&mut self, l: &mut [f32], r: &mut [f32]);
}

/// Stands in for an effect that can't run (a plug-in that isn't available).
pub struct Passthrough;

impl RtEffect for Passthrough {
    fn set_params(&mut self, _: &Params) {}
    fn reset(&mut self) {}
    fn process(&mut self, _: &mut [f32], _: &mut [f32]) {}
}

/// Effects that can run in a track rack, by effect id.
pub const RT_EFFECTS: &[&str] = &[
    "amplify",
    "parametric_eq",
    "graphic_eq",
    "graphic_eq20",
    "graphic_eq30",
    "notch_filter",
    "scientific_filter",
    "dc_offset",
    "dynamics",
    "single_band_compressor",
    "tube_compressor",
    "hard_limiter",
    "noise_gate",
    "deesser",
    "delay",
    "echo",
    "chorus",
    "flanger",
    "chorus_flanger",
    "studio_reverb",
    "distortion",
    "stereo_expander",
    "channel_mixer",
    "pan",
];

pub fn supports(id: &str) -> bool {
    RT_EFFECTS.contains(&id) || (is_plugin_id(id) && PLUGIN_MAKER.get().is_some())
}

fn is_plugin_id(id: &str) -> bool {
    id.starts_with("vst3:") || id.starts_with("au:")
}

/// Creates streaming instances of third-party plug-ins (ids `vst3:…`, `au:…`);
/// installed by the host so this module stays dependency-free.
pub type PluginMaker = fn(&str, &Params, u32) -> Option<Box<dyn RtEffect>>;
static PLUGIN_MAKER: std::sync::OnceLock<PluginMaker> = std::sync::OnceLock::new();

pub fn set_plugin_maker(f: PluginMaker) {
    let _ = PLUGIN_MAKER.set(f);
}

/// A streaming instance of effect `id` with parameters `p`.
pub fn make(id: &str, p: &Params, sr: u32) -> Option<Box<dyn RtEffect>> {
    let srf = sr as f32;
    let mut e: Box<dyn RtEffect> = match id {
        "amplify" => Box::new(Gain::default()),
        "parametric_eq" => Box::new(Cascade::new(srf, |p, sr| (filter::peq_filters(p, sr), p.f("master")))),
        "graphic_eq" => Box::new(Cascade::new(srf, |p, sr| (filter::geq_filters(p, sr), p.f("master")))),
        "graphic_eq20" => Box::new(Cascade::new(srf, |p, sr| (more::geq_filters(&more::GEQ20, 2.87, p, sr), p.f("master")))),
        "graphic_eq30" => Box::new(Cascade::new(srf, |p, sr| (more::geq_filters(&more::GEQ30, 4.32, p, sr), p.f("master")))),
        "notch_filter" => Box::new(Cascade::new(srf, |p, sr| (more::notch_filters(p, sr), p.f("output")))),
        "scientific_filter" => Box::new(Cascade::new(srf, |p, sr| (filter::sci_filters(p, sr), p.f("gain")))),
        "dc_offset" => Box::new(Cascade::new(srf, |_, sr| (butterworth(FilterType::HighPass, sr, 10.0, 2), 0.0))),
        "dynamics" | "single_band_compressor" | "tube_compressor" => Box::new(Compressor::new(id, sr)),
        "hard_limiter" => Box::new(Limiter::new(sr)),
        "noise_gate" => Box::new(Gate::new(sr)),
        "deesser" => Box::new(DeEsser::new(sr)),
        "delay" => Box::new(Delay::new(sr)),
        "echo" => Box::new(Echo::new(sr)),
        "chorus" | "flanger" | "chorus_flanger" => Box::new(ModDelay::new(id, sr)),
        "studio_reverb" => Box::new(Reverb::new(sr)),
        "distortion" => Box::new(Distortion::new(sr)),
        "stereo_expander" => Box::new(Expander::new(sr)),
        "channel_mixer" => Box::new(Matrix::default()),
        "pan" => Box::new(Pan::default()),
        _ if is_plugin_id(id) => return PLUGIN_MAKER.get().and_then(|f| f(id, p, sr)),
        _ => return None,
    };
    e.set_params(p);
    Some(e)
}

// ------------------------------------------------------------------ gain

#[derive(Default)]
struct Gain {
    gl: f32,
    gr: f32,
}

impl RtEffect for Gain {
    fn set_params(&mut self, p: &Params) {
        self.gl = db_to_lin(p.f("left"));
        self.gr = if p.b("link") { self.gl } else { db_to_lin(p.f("right")) };
    }
    fn reset(&mut self) {}
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        l.iter_mut().for_each(|s| *s *= self.gl);
        r.iter_mut().for_each(|s| *s *= self.gr);
    }
}

// ------------------------------------------------------------------ filters

type Builder = fn(&Params, f32) -> (Vec<Biquad>, f32);

/// A biquad cascade per channel plus output gain (EQs and filters).
struct Cascade {
    sr: f32,
    build: Builder,
    l: Vec<Biquad>,
    r: Vec<Biquad>,
    gain: f32,
}

impl Cascade {
    fn new(sr: f32, build: Builder) -> Self {
        Cascade { sr, build, l: Vec::new(), r: Vec::new(), gain: 1.0 }
    }
}

impl RtEffect for Cascade {
    fn set_params(&mut self, p: &Params) {
        let (f, g) = (self.build)(p, self.sr);
        if f.len() == self.l.len() {
            // Same shape: new coefficients, keep the filter memories.
            for ((a, b), n) in self.l.iter_mut().zip(self.r.iter_mut()).zip(&f) {
                a.copy_coeffs(n);
                b.copy_coeffs(n);
            }
        } else {
            self.l = f.clone();
            self.r = f;
        }
        self.gain = db_to_lin(g);
    }
    fn reset(&mut self) {
        self.l.iter_mut().chain(self.r.iter_mut()).for_each(|b| b.reset());
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (buf, fs) in [(l, &mut self.l), (r, &mut self.r)] {
            for s in buf.iter_mut() {
                let mut v = *s;
                for f in fs.iter_mut() {
                    v = f.process(v);
                }
                *s = v * self.gain;
            }
        }
    }
}

// ------------------------------------------------------------------ dynamics

fn gain_computer(level_db: f32, thr: f32, ratio: f32, knee: f32) -> f32 {
    let over = level_db - thr;
    let slope = 1.0 / ratio.max(1.0) - 1.0;
    if knee > 0.0 && 2.0 * over.abs() <= knee {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// Dynamics Processing, Single-band and Tube-modeled Compressor.
struct Compressor {
    kind: &'static str,
    sr: u32,
    thr: f32,
    ratio: f32,
    knee: f32,
    att: f32,
    rel: f32,
    makeup: f32,
    link: bool,
    /// Tube warmth: (k, positive norm, negative norm) when active.
    tube: Option<(f32, f32, f32)>,
    env: [f32; 2],
}

impl Compressor {
    fn new(id: &str, sr: u32) -> Self {
        let kind = match id {
            "dynamics" => "dynamics",
            "tube_compressor" => "tube_compressor",
            _ => "single_band_compressor",
        };
        Compressor { kind, sr, thr: -20.0, ratio: 4.0, knee: 0.0, att: 0.0, rel: 0.0, makeup: 1.0, link: true, tube: None, env: [0.0; 2] }
    }
}

impl RtEffect for Compressor {
    fn set_params(&mut self, p: &Params) {
        self.thr = p.f("threshold");
        self.ratio = p.f("ratio");
        self.att = smooth_coeff(p.f("attack"), self.sr);
        self.rel = smooth_coeff(p.f("release"), self.sr);
        match self.kind {
            "dynamics" => {
                self.knee = p.f("knee");
                self.makeup = db_to_lin(p.f("makeup"));
                self.link = p.b("link");
                self.tube = None;
            }
            "tube_compressor" => {
                self.knee = 6.0;
                self.makeup = db_to_lin(p.f("output"));
                self.link = true;
                let k = 1.0 + p.f("drive") / 100.0 * 3.0;
                self.tube = if k > 1.001 { Some((k, k.tanh(), (0.8 * k).tanh())) } else { None };
            }
            _ => {
                self.knee = 0.0;
                self.makeup = db_to_lin(p.f("output"));
                self.link = true;
                self.tube = None;
            }
        }
    }
    fn reset(&mut self) {
        self.env = [0.0; 2];
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let (att, rel) = (self.att, self.rel);
        let step = |env: &mut f32, level: f32, thr, ratio, knee| {
            let gr = gain_computer(lin_to_db(level), thr, ratio, knee);
            *env = if gr < *env { att * *env + (1.0 - att) * gr } else { rel * *env + (1.0 - rel) * gr };
        };
        for n in 0..l.len() {
            if self.link {
                step(&mut self.env[0], l[n].abs().max(r[n].abs()), self.thr, self.ratio, self.knee);
                let g = db_to_lin(self.env[0]) * self.makeup;
                l[n] *= g;
                r[n] *= g;
            } else {
                step(&mut self.env[0], l[n].abs(), self.thr, self.ratio, self.knee);
                step(&mut self.env[1], r[n].abs(), self.thr, self.ratio, self.knee);
                l[n] *= db_to_lin(self.env[0]) * self.makeup;
                r[n] *= db_to_lin(self.env[1]) * self.makeup;
            }
            if let Some((k, np, nn)) = self.tube {
                for s in [&mut l[n], &mut r[n]] {
                    *s = if *s >= 0.0 { (k * *s).tanh() / np } else { (0.8 * k * *s).tanh() / nn };
                }
            }
        }
    }
}

/// Look-ahead brick-wall limiter; output is delayed by the look-ahead time.
struct Limiter {
    sr: u32,
    ceiling: f32,
    boost: f32,
    la: usize,
    rel: f32,
    /// Delayed input, la frames.
    dl: Vec<f32>,
    dr: Vec<f32>,
    w: usize,
    /// Monotonic deque of (frame, required gain) for the running minimum.
    dq: VecDeque<(u64, f32)>,
    /// Last la + 1 forward minima, for the box smoothing.
    avg: Vec<f32>,
    avg_i: usize,
    sum: f64,
    g: f32,
    n: u64,
}

impl Limiter {
    fn new(sr: u32) -> Self {
        let max = ms_to_samples(20.0, sr) + 2;
        Limiter {
            sr,
            ceiling: 1.0,
            boost: 1.0,
            la: 0,
            rel: 0.0,
            dl: Vec::with_capacity(max),
            dr: Vec::with_capacity(max),
            w: 0,
            dq: VecDeque::with_capacity(max + 2),
            avg: Vec::with_capacity(max),
            avg_i: 0,
            sum: 0.0,
            g: 1.0,
            n: 0,
        }
    }
}

impl RtEffect for Limiter {
    fn set_params(&mut self, p: &Params) {
        self.ceiling = db_to_lin(p.f("ceiling"));
        self.boost = db_to_lin(p.f("boost"));
        self.rel = smooth_coeff(p.f("release"), self.sr);
        let la = ms_to_samples(p.f("lookahead"), self.sr).max(1);
        if la != self.la {
            self.la = la;
            self.reset();
        }
    }
    fn reset(&mut self) {
        let n = self.la + 1;
        // Read-before-write on `la` frames gives exactly `la` frames of delay.
        self.dl.clear();
        self.dl.resize(self.la, 0.0);
        self.dr.clear();
        self.dr.resize(self.la, 0.0);
        self.avg.clear();
        self.avg.resize(n, 0.0);
        self.dq.clear();
        self.w = 0;
        self.avg_i = 0;
        self.sum = 0.0;
        self.g = 1.0;
        self.n = 0;
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let la = self.la as u64;
        let win = self.la + 1;
        for k in 0..l.len() {
            let pk = l[k].abs().max(r[k].abs()) * self.boost;
            let req = if pk > self.ceiling { self.ceiling / pk } else { 1.0 };
            while self.dq.back().map(|b| b.1 >= req).unwrap_or(false) {
                self.dq.pop_back();
            }
            self.dq.push_back((self.n, req));
            while self.dq.front().map(|f| f.0 + la < self.n).unwrap_or(false) {
                self.dq.pop_front();
            }
            // Forward minimum for the frame now leaving the delay line.
            let fm = if self.n >= la { self.dq.front().map(|f| f.1).unwrap_or(1.0) } else { 1.0 };
            if self.n >= la {
                self.sum += fm as f64 - self.avg[self.avg_i] as f64;
                self.avg[self.avg_i] = fm;
                self.avg_i = (self.avg_i + 1) % win;
            }
            let count = ((self.n.saturating_sub(la)) + 1).min(win as u64) as f64;
            let s = if self.n >= la { (self.sum / count) as f32 } else { 1.0 };
            let (xl, xr) = (self.dl[self.w], self.dr[self.w]);
            self.dl[self.w] = l[k];
            self.dr[self.w] = r[k];
            self.w = (self.w + 1) % self.la;
            if self.n >= la {
                self.g = if s < self.g { s } else { self.rel * self.g + (1.0 - self.rel) * s };
            }
            l[k] = (xl * self.boost * self.g).clamp(-self.ceiling, self.ceiling);
            r[k] = (xr * self.boost * self.g).clamp(-self.ceiling, self.ceiling);
            self.n += 1;
        }
    }
}

struct Gate {
    sr: u32,
    thr: f32,
    floor: f32,
    att: f32,
    rel: f32,
    hold: usize,
    det_decay: f32,
    env: f32,
    g: f32,
    hold_left: usize,
}

impl Gate {
    fn new(sr: u32) -> Self {
        Gate { sr, thr: 0.0, floor: 0.0, att: 0.0, rel: 0.0, hold: 0, det_decay: smooth_coeff(10.0, sr), env: 0.0, g: 0.0, hold_left: 0 }
    }
}

impl RtEffect for Gate {
    fn set_params(&mut self, p: &Params) {
        let first = self.thr == 0.0 && self.floor == 0.0;
        self.thr = db_to_lin(p.f("threshold"));
        self.floor = db_to_lin(-p.f("range"));
        self.att = smooth_coeff(p.f("attack"), self.sr);
        self.rel = smooth_coeff(p.f("release"), self.sr);
        self.hold = ms_to_samples(p.f("hold"), self.sr);
        if first {
            self.g = self.floor;
        }
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.g = self.floor;
        self.hold_left = 0;
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for n in 0..l.len() {
            let level = l[n].abs().max(r[n].abs());
            self.env = level.max(self.env * self.det_decay);
            let open = if self.env >= self.thr {
                self.hold_left = self.hold;
                true
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
                true
            } else {
                false
            };
            let target = if open { 1.0 } else { self.floor };
            self.g = if target > self.g { self.att * self.g + (1.0 - self.att) * target } else { self.rel * self.g + (1.0 - self.rel) * target };
            l[n] *= self.g;
            r[n] *= self.g;
        }
    }
}

struct DeEsser {
    sr: u32,
    bands: [[Biquad; 2]; 2],
    thr: f32,
    broadband: bool,
    listen: bool,
    att: f32,
    rel: f32,
    env: f32,
    gr: f32,
}

impl DeEsser {
    fn new(sr: u32) -> Self {
        DeEsser {
            sr,
            bands: [[Biquad::identity(); 2]; 2],
            thr: -30.0,
            broadband: false,
            listen: false,
            att: smooth_coeff(1.0, sr),
            rel: smooth_coeff(60.0, sr),
            env: 0.0,
            gr: 0.0,
        }
    }
}

impl RtEffect for DeEsser {
    fn set_params(&mut self, p: &Params) {
        let sr = self.sr as f32;
        let f = p.f("freq");
        let q = (f / p.f("bandwidth").max(10.0)).clamp(0.3, 10.0);
        let b = Biquad::new(FilterType::BandPass, sr, f, q, 0.0);
        for ch in self.bands.iter_mut() {
            for s in ch.iter_mut() {
                s.copy_coeffs(&b);
            }
        }
        self.thr = p.f("threshold");
        self.broadband = p.c("mode") == 1;
        self.listen = p.b("listen");
    }
    fn reset(&mut self) {
        self.bands.iter_mut().flatten().for_each(|b| b.reset());
        self.env = 0.0;
        self.gr = 0.0;
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for n in 0..l.len() {
            let x = [l[n], r[n]];
            let mut band = [0.0f32; 2];
            for c in 0..2 {
                let first = self.bands[c][0].process(x[c]);
                band[c] = self.bands[c][1].process(first);
            }
            let lvl = band[0].abs().max(band[1].abs());
            self.env = if lvl > self.env { self.att * self.env + (1.0 - self.att) * lvl } else { self.rel * self.env + (1.0 - self.rel) * lvl };
            let over = lin_to_db(self.env) - self.thr;
            let target = if over > 0.0 { -over * 0.75 } else { 0.0 };
            self.gr = if target < self.gr { self.att * self.gr + (1.0 - self.att) * target } else { self.rel * self.gr + (1.0 - self.rel) * target };
            let g = db_to_lin(self.gr);
            let out = |c: usize| if self.listen { band[c] } else if self.broadband { x[c] * g } else { x[c] - band[c] + band[c] * g };
            l[n] = out(0);
            r[n] = out(1);
        }
    }
}

// ------------------------------------------------------------------ delays

/// A fixed-size ring buffer read `d` frames behind the write position.
struct Ring {
    buf: Vec<f32>,
    w: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Ring { buf: vec![0.0; len.max(2)], w: 0 }
    }
    #[inline]
    fn read(&self, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.w + n - d.min(n - 1)) % n]
    }
    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.w] = x;
        self.w = (self.w + 1) % self.buf.len();
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
    }
}

struct Delay {
    sr: u32,
    rings: [Ring; 2],
    d: [usize; 2],
    mix: [f32; 2],
    sign: f32,
}

impl Delay {
    fn new(sr: u32) -> Self {
        let max = ms_to_samples(2000.0, sr) + 2;
        Delay { sr, rings: [Ring::new(max), Ring::new(max)], d: [0; 2], mix: [0.0; 2], sign: 1.0 }
    }
}

impl RtEffect for Delay {
    fn set_params(&mut self, p: &Params) {
        self.d = [ms_to_samples(p.f("left_ms"), self.sr), ms_to_samples(p.f("right_ms"), self.sr)];
        self.mix = [p.f("left_mix") / 100.0, p.f("right_mix") / 100.0];
        self.sign = if p.b("invert") { -1.0 } else { 1.0 };
    }
    fn reset(&mut self) {
        self.rings.iter_mut().for_each(|r| r.clear());
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (c, buf) in [l, r].into_iter().enumerate() {
            let ring = &mut self.rings[c];
            for s in buf.iter_mut() {
                let x = *s;
                ring.write(x);
                let wet = if self.d[c] == 0 { x } else { ring.read(self.d[c] + 1) };
                *s = x * (1.0 - self.mix[c]) + wet * self.mix[c] * self.sign;
            }
        }
    }
}

struct Echo {
    sr: u32,
    rings: [Ring; 2],
    d: usize,
    fb: f32,
    level: f32,
    pingpong: bool,
}

impl Echo {
    fn new(sr: u32) -> Self {
        let max = ms_to_samples(3000.0, sr) + 2;
        Echo { sr, rings: [Ring::new(max), Ring::new(max)], d: 1, fb: 0.0, level: 0.0, pingpong: false }
    }
}

impl RtEffect for Echo {
    fn set_params(&mut self, p: &Params) {
        self.d = ms_to_samples(p.f("delay_ms"), self.sr).max(1);
        self.fb = p.f("feedback") / 100.0;
        self.level = p.f("level") / 100.0;
        self.pingpong = p.b("pingpong");
    }
    fn reset(&mut self) {
        self.rings.iter_mut().for_each(|r| r.clear());
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let d = self.d;
        for n in 0..l.len() {
            if self.pingpong {
                let input = 0.5 * (l[n] + r[n]);
                let ol = self.rings[0].read(d);
                let or = self.rings[1].read(d);
                self.rings[0].write(input + or * self.fb);
                self.rings[1].write(ol * self.fb);
                l[n] += ol * self.level;
                r[n] += or * self.level;
            } else {
                for (c, s) in [&mut l[n], &mut r[n]].into_iter().enumerate() {
                    let o = self.rings[c].read(d);
                    self.rings[c].write(*s + o * self.fb);
                    *s += o * self.level;
                }
            }
        }
    }
}

/// Chorus, Flanger and the quick Chorus/Flanger.
struct ModDelay {
    id: &'static str,
    sr: u32,
    lines: [DelayLine; 2],
    last: [f32; 2],
    voices: usize,
    base: f32,
    depth: f32,
    rate: f32,
    fb: f32,
    mix: f32,
    /// Phase offset of the right channel (radians).
    ch_phase: f32,
    chorus: bool,
    n: u64,
}

impl ModDelay {
    fn new(id: &str, sr: u32) -> Self {
        let id = match id {
            "chorus" => "chorus",
            "flanger" => "flanger",
            _ => "chorus_flanger",
        };
        let max = ms_to_samples(80.0, sr);
        ModDelay {
            id,
            sr,
            lines: [DelayLine::new(max), DelayLine::new(max)],
            last: [0.0; 2],
            voices: 1,
            base: 1.0,
            depth: 1.0,
            rate: 1.0,
            fb: 0.0,
            mix: 0.5,
            ch_phase: 0.0,
            chorus: true,
            n: 0,
        }
    }
}

impl RtEffect for ModDelay {
    fn set_params(&mut self, p: &Params) {
        let sr = self.sr as f32;
        // (chorus?, voices, delay ms, depth ms, rate, feedback 0..1, width/phase, mix)
        let (chorus, voices, delay, depth, rate, fb, spread, mix) = match self.id {
            "chorus" => (true, [2usize, 3, 4, 5, 6, 8][p.c("voices").min(5)], p.f("delay"), p.f("depth"), p.f("rate"), p.f("feedback") / 100.0, p.f("width") / 100.0, p.f("mix") / 100.0),
            "flanger" => (false, 1, p.f("delay"), p.f("depth"), p.f("rate"), p.f("feedback") / 100.0, p.f("phase").to_radians(), p.f("mix") / 100.0),
            _ if p.c("mode") == 0 => (true, 3, 15.0, 0.5 + p.f("width") / 100.0 * 8.0, p.f("speed"), p.f("intensity") * 0.6 / 100.0, 0.7, p.f("mix") / 100.0),
            _ => (false, 1, 0.5, 0.5 + p.f("width") / 100.0 * 6.0, p.f("speed"), p.f("intensity") * 0.9 / 100.0, 90f32.to_radians(), p.f("mix") / 100.0),
        };
        self.chorus = chorus;
        self.voices = voices;
        self.base = delay * 0.001 * sr;
        self.depth = depth * 0.001 * sr;
        self.rate = rate;
        self.fb = fb;
        self.mix = mix;
        self.ch_phase = if chorus { PI * 0.5 * spread } else { spread };
    }
    fn reset(&mut self) {
        let max = ms_to_samples(80.0, self.sr);
        self.lines = [DelayLine::new(max), DelayLine::new(max)];
        self.last = [0.0; 2];
        self.n = 0;
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let sr = self.sr as f32;
        for k in 0..l.len() {
            let t = self.n as f32 / sr;
            for (c, s) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                let x = *s;
                let line = &mut self.lines[c];
                line.push(x + self.last[c] * self.fb);
                let cp = c as f32 * self.ch_phase;
                let wet = if self.chorus {
                    let mut w = 0.0;
                    for v in 0..self.voices {
                        let ph = TAU * self.rate * t + TAU * v as f32 / self.voices as f32 + cp;
                        let lfo = 0.5 + 0.5 * (ph * (1.0 + 0.07 * v as f32)).sin();
                        w += line.read(self.base + self.depth * lfo);
                    }
                    w / self.voices as f32
                } else {
                    let lfo = 0.5 + 0.5 * (TAU * self.rate * t + cp).sin();
                    line.read(self.base + self.depth * lfo)
                };
                self.last[c] = wet;
                *s = x * (1.0 - self.mix) + wet * self.mix;
            }
            self.n += 1;
        }
    }
}

// ------------------------------------------------------------------ reverb

struct Reverb {
    sr: u32,
    cl: Vec<reverb::Comb>,
    cr: Vec<reverb::Comb>,
    al: Vec<reverb::AllPass>,
    ar: Vec<reverb::AllPass>,
    hp: [Biquad; 2],
    pre: Ring,
    pre_d: usize,
    feedback: f32,
    damp: f32,
    wet1: f32,
    wet2: f32,
    dry: f32,
}

impl Reverb {
    fn new(sr: u32) -> Self {
        let scale = sr as f32 / 44100.0;
        let mk = |spread: usize| {
            (
                reverb::COMBS.iter().map(|&l| reverb::Comb::new(((l + spread) as f32 * scale) as usize)).collect::<Vec<_>>(),
                reverb::ALLPASSES.iter().map(|&l| reverb::AllPass::new(((l + spread) as f32 * scale) as usize)).collect::<Vec<_>>(),
            )
        };
        let (cl, al) = mk(0);
        let (cr, ar) = mk(reverb::SPREAD);
        Reverb {
            sr,
            cl,
            cr,
            al,
            ar,
            hp: [Biquad::identity(); 2],
            pre: Ring::new(ms_to_samples(200.0, sr) + 2),
            pre_d: 0,
            feedback: 0.84,
            damp: 0.2,
            wet1: 0.0,
            wet2: 0.0,
            dry: 1.0,
        }
    }
}

impl RtEffect for Reverb {
    fn set_params(&mut self, p: &Params) {
        self.feedback = p.f("room") / 100.0 * 0.28 + 0.7;
        self.damp = p.f("damping") / 100.0 * 0.4;
        let width = p.f("width") / 100.0;
        let wet = db_to_lin(p.f("wet")) * 3.0;
        self.dry = db_to_lin(p.f("dry"));
        self.wet1 = wet * (width / 2.0 + 0.5);
        self.wet2 = wet * ((1.0 - width) / 2.0);
        self.pre_d = ms_to_samples(p.f("predelay"), self.sr);
        let hp = Biquad::new(FilterType::HighPass, self.sr as f32, p.f("lowcut"), 0.707, 0.0);
        self.hp.iter_mut().for_each(|b| b.copy_coeffs(&hp));
    }
    fn reset(&mut self) {
        let fresh = Reverb::new(self.sr);
        self.cl = fresh.cl;
        self.cr = fresh.cr;
        self.al = fresh.al;
        self.ar = fresh.ar;
        self.hp.iter_mut().for_each(|b| b.reset());
        self.pre.clear();
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for n in 0..l.len() {
            self.pre.write((l[n] + r[n]) * 0.5);
            let input = if self.pre_d == 0 { (l[n] + r[n]) * 0.5 } else { self.pre.read(self.pre_d + 1) } * 0.015;
            let (mut ol, mut or) = (0.0, 0.0);
            for c in self.cl.iter_mut() {
                ol += c.process(input, self.feedback, self.damp);
            }
            for c in self.cr.iter_mut() {
                or += c.process(input, self.feedback, self.damp);
            }
            for a in self.al.iter_mut() {
                ol = a.process(ol);
            }
            for a in self.ar.iter_mut() {
                or = a.process(or);
            }
            let ol = self.hp[0].process(ol);
            let or = self.hp[1].process(or);
            let (xl, xr) = (l[n], r[n]);
            l[n] = xl * self.dry + ol * self.wet1 + or * self.wet2;
            r[n] = xr * self.dry + or * self.wet1 + ol * self.wet2;
        }
    }
}

// ------------------------------------------------------------------ special / stereo

struct Distortion {
    sr: u32,
    kind: usize,
    drive: f32,
    mix: f32,
    out_g: f32,
    levels: f32,
    ds: usize,
    tone: [Biquad; 2],
    held: [f32; 2],
    n: u64,
}

impl Distortion {
    fn new(sr: u32) -> Self {
        Distortion { sr, kind: 0, drive: 1.0, mix: 1.0, out_g: 1.0, levels: 128.0, ds: 1, tone: [Biquad::identity(); 2], held: [0.0; 2], n: 0 }
    }
}

impl RtEffect for Distortion {
    fn set_params(&mut self, p: &Params) {
        self.kind = p.c("kind");
        self.drive = db_to_lin(p.f("drive"));
        self.mix = p.f("mix") / 100.0;
        self.out_g = db_to_lin(p.f("output"));
        self.levels = 2f32.powf(p.f("bits").round().max(1.0) - 1.0);
        self.ds = p.f("downsample").round().max(1.0) as usize;
        let t = Biquad::new(FilterType::LowPass, self.sr as f32, p.f("tone"), 0.707, 0.0);
        self.tone.iter_mut().for_each(|b| b.copy_coeffs(&t));
    }
    fn reset(&mut self) {
        self.tone.iter_mut().for_each(|b| b.reset());
        self.held = [0.0; 2];
        self.n = 0;
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for k in 0..l.len() {
            let crush_now = self.n % self.ds as u64 == 0;
            for (c, s) in [&mut l[k], &mut r[k]].into_iter().enumerate() {
                let x = *s;
                let wet = if self.kind == 4 {
                    if crush_now {
                        self.held[c] = ((x * self.drive).clamp(-1.0, 1.0) * self.levels).round() / self.levels;
                    }
                    self.held[c]
                } else {
                    special::shape(self.kind, x * self.drive)
                };
                let wet = self.tone[c].process(wet);
                *s = (x * (1.0 - self.mix) + wet * self.mix) * self.out_g;
            }
            self.n += 1;
        }
    }
}

struct Expander {
    sr: u32,
    w: f32,
    centre: f32,
    bass_mono: bool,
    lp: Vec<Biquad>,
}

impl Expander {
    fn new(sr: u32) -> Self {
        Expander { sr, w: 1.0, centre: 0.0, bass_mono: false, lp: butterworth(FilterType::LowPass, sr as f32, 120.0, 2) }
    }
}

impl RtEffect for Expander {
    fn set_params(&mut self, p: &Params) {
        self.w = p.f("width") / 100.0;
        self.centre = p.f("centre") / 100.0;
        self.bass_mono = p.b("bass_mono");
    }
    fn reset(&mut self) {
        self.lp = butterworth(FilterType::LowPass, self.sr as f32, 120.0, 2);
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for n in 0..l.len() {
            let m = 0.5 * (l[n] + r[n]);
            let mut s = 0.5 * (l[n] - r[n]);
            if self.bass_mono {
                let mut low = s;
                for f in self.lp.iter_mut() {
                    low = f.process(low);
                }
                s = low + (s - low) * self.w;
            } else {
                s *= self.w;
            }
            let ml = m * (1.0 - self.centre).min(1.0);
            let mr = m * (1.0 + self.centre).min(1.0);
            l[n] = ml + s;
            r[n] = mr - s;
        }
    }
}

#[derive(Default)]
struct Matrix {
    m: [f32; 4],
    sl: f32,
    sr: f32,
}

impl RtEffect for Matrix {
    fn set_params(&mut self, p: &Params) {
        self.m = [p.f("ll") / 100.0, p.f("lr") / 100.0, p.f("rl") / 100.0, p.f("rr") / 100.0];
        self.sl = if p.b("inv_l") { -1.0 } else { 1.0 };
        self.sr = if p.b("inv_r") { -1.0 } else { 1.0 };
    }
    fn reset(&mut self) {}
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for n in 0..l.len() {
            let (a, b) = (l[n], r[n]);
            l[n] = (a * self.m[0] + b * self.m[1]) * self.sl;
            r[n] = (a * self.m[2] + b * self.m[3]) * self.sr;
        }
    }
}

#[derive(Default)]
struct Pan {
    gl: f32,
    gr: f32,
}

impl RtEffect for Pan {
    fn set_params(&mut self, p: &Params) {
        let x = (p.f("pan") / 100.0).clamp(-1.0, 1.0);
        let (gl, gr) = if p.c("law") == 0 {
            let a = (x + 1.0) * std::f32::consts::FRAC_PI_4;
            (a.cos() * std::f32::consts::SQRT_2, a.sin() * std::f32::consts::SQRT_2)
        } else {
            ((1.0 - x).min(1.0), (1.0 + x).min(1.0))
        };
        self.gl = gl.min(1.0);
        self.gr = gr.min(1.0);
    }
    fn reset(&mut self) {}
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        l.iter_mut().for_each(|s| *s *= self.gl);
        r.iter_mut().for_each(|s| *s *= self.gr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::registry;
    use super::*;

    fn signal(n: usize, sr: u32) -> (Vec<f32>, Vec<f32>) {
        let mut rng = Rng::new(7);
        let l: Vec<f32> = (0..n).map(|i| 0.6 * (TAU * 220.0 * i as f32 / sr as f32).sin() * (1.0 + (i as f32 * 0.0007).sin()) * 0.5 + 0.1 * rng.bipolar()).collect();
        let r: Vec<f32> = (0..n).map(|i| 0.5 * (TAU * 330.0 * i as f32 / sr as f32).sin() + 0.1 * rng.bipolar()).collect();
        (l, r)
    }

    /// Every streaming effect matches its offline version when fed in
    /// uneven blocks, for the default settings and every preset.
    #[test]
    fn streaming_matches_offline() {
        let sr = 44100u32;
        let reg = registry();
        let (l, r) = signal(30_000, sr);
        for id in RT_EFFECTS {
            let def = reg.iter().find(|e| e.id == *id).unwrap_or_else(|| panic!("no effect {id}"));
            let mut sets = vec![("(default)".to_string(), def.default_params())];
            for (name, _) in &def.presets {
                sets.push((name.to_string(), def.preset_params(name).unwrap()));
            }
            for (name, mut p) in sets {
                if *id == "dc_offset" {
                    // Removing the average needs the whole file; live uses the high-pass method.
                    p.set("method", Value::C(1));
                }
                let ctx = Ctx { sample_rate: sr, channels: 2, noise_print: None };
                let want = def.run(&[l.clone(), r.clone()], &ctx, &p).unwrap();
                let mut e = make(id, &p, sr).unwrap();
                let (mut ol, mut or) = (l.clone(), r.clone());
                let mut a = 0;
                let mut k = 0;
                while a < ol.len() {
                    let b = (a + [1, 63, 512, 1000, 4096][k % 5]).min(ol.len());
                    e.process(&mut ol[a..b], &mut or[a..b]);
                    a = b;
                    k += 1;
                }
                // The limiter delays its output by the look-ahead.
                let lag = if *id == "hard_limiter" { ms_to_samples(p.f("lookahead"), sr).max(1) } else { 0 };
                let mut worst = 0.0f32;
                for n in lag..ol.len() {
                    worst = worst.max((ol[n] - want[0][n - lag]).abs()).max((or[n] - want[1][n - lag]).abs());
                }
                assert!(worst < 2e-4, "{id} / {name}: max difference {worst}");
            }
        }
    }

    #[test]
    fn parameter_changes_keep_state() {
        let sr = 48000;
        let reg = registry();
        let def = reg.iter().find(|e| e.id == "echo").unwrap();
        let mut p = def.default_params();
        let mut e = make("echo", &p, sr).unwrap();
        let mut l = vec![0.0f32; 4800];
        let mut r = vec![0.0f32; 4800];
        l[0] = 1.0;
        e.process(&mut l, &mut r);
        // Changing the level mid-tail must not clear the delay line.
        p.set("level", Value::F(100.0));
        e.set_params(&p);
        let mut l2 = vec![0.0f32; 48000];
        let mut r2 = vec![0.0f32; 48000];
        e.process(&mut l2, &mut r2);
        assert!(l2.iter().any(|s| s.abs() > 0.1), "echo tail survived the parameter change");
        e.reset();
        let mut l3 = vec![0.0f32; 48000];
        let mut r3 = vec![0.0f32; 48000];
        e.process(&mut l3, &mut r3);
        assert!(l3.iter().all(|s| *s == 0.0));
    }
}
