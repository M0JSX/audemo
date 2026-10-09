//! Real-time Effects Rack: a background thread renders the active file
//! through the rack into a [`StreamBuf`] just ahead of the playhead, so
//! playback, seeking and looping work as normal with the effects audible.
//!
//! The offline effects process overlapping chunks: each chunk is rendered
//! with a pre-roll (so filters, compressors and reverbs have settled) and a
//! short overlap that is cross-faded into the next chunk.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::app::run_rack;
use crate::dsp::effects::{EffectDef, NoiseProfile};
use crate::dsp::params::Params;
use crate::engine::{Buffer, StreamBuf, STREAM_BLOCK};

#[derive(Clone, Debug, PartialEq)]
pub struct RackSettings {
    pub slots: Vec<(usize, Params)>,
    pub mix: f32,
    pub in_db: f32,
    pub out_db: f32,
}

pub struct LiveRack {
    pub doc_id: u64,
    pub audio: Buffer,
    pub sb: Arc<StreamBuf>,
    pub settings: RackSettings,
    shared: Arc<Mutex<(u64, RackSettings)>>,
    cancel: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

impl Drop for LiveRack {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn align_down(x: usize) -> usize {
    x / STREAM_BLOCK * STREAM_BLOCK
}

fn align_up(x: usize) -> usize {
    x.div_ceil(STREAM_BLOCK) * STREAM_BLOCK
}

impl LiveRack {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        effects: Arc<Vec<EffectDef>>,
        audio: Buffer,
        sample_rate: u32,
        noise_print: Option<NoiseProfile>,
        settings: RackSettings,
        doc_id: u64,
        cursor: usize,
    ) -> LiveRack {
        let len = audio.first().map(|c| c.len()).unwrap_or(0);
        let sb = StreamBuf::new(len, audio.len(), cursor.min(len));
        let shared = Arc::new(Mutex::new((1u64, settings.clone())));
        let cancel = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let src = audio.clone();
        let n_ch = audio.len();
        let job = StreamRenderer {
            sb: sb.clone(),
            shared: shared.clone(),
            cancel: cancel.clone(),
            error: error.clone(),
            sr: sample_rate,
            preroll: sample_rate as usize * 3 / 2,
            xfade: (sample_rate as usize / 200).max(16), // 5 ms
            render: Box::new(move |set: &RackSettings, a, b| {
                let input: Vec<Vec<f32>> = src.iter().map(|c| c[a..b].to_vec()).collect();
                run_rack(&effects, &set.slots, input, sample_rate, n_ch, noise_print.as_ref(), set.mix, set.in_db, set.out_db)
            }),
        };
        std::thread::Builder::new()
            .name("audemo-live-rack".into())
            .spawn(move || job.run())
            .ok();
        LiveRack { doc_id, audio, sb, settings, shared, cancel, error }
    }

    /// New rack settings: re-render from just ahead of the playhead.
    pub fn update(&mut self, settings: RackSettings) {
        if settings == self.settings {
            return;
        }
        self.settings = settings.clone();
        if let Ok(mut s) = self.shared.lock() {
            s.0 += 1;
            s.1 = settings;
        }
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }
}

/// Renders `[start, end)` of some audio for settings `S`.
pub type RenderFn<S> = Box<dyn Fn(&S, usize, usize) -> Result<Vec<Vec<f32>>, String> + Send>;

/// Keeps a [`StreamBuf`] filled just ahead of its playhead, re-rendering
/// whenever the settings version changes. `preroll` frames before each chunk
/// are rendered and discarded (for stateful effects); `xfade` frames after it
/// are cross-faded into the next chunk.
pub struct StreamRenderer<S> {
    pub sb: Arc<StreamBuf>,
    pub shared: Arc<Mutex<(u64, S)>>,
    pub cancel: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub sr: u32,
    pub preroll: usize,
    pub xfade: usize,
    pub render: RenderFn<S>,
}

impl<S: Clone> StreamRenderer<S> {
    pub fn run(self) {
        let len = self.sb.len;
        if len == 0 {
            return;
        }
        let sr = self.sr as usize;
        let preroll = self.preroll;
        let xfade = self.xfade;
        let first_chunk = align_up(sr / 5);
        let max_chunk = align_up(sr * 2);
        let lead = align_up(sr / 10);
        let max_ahead = sr * 30;

        let mut version = 0u64;
        let mut settings: Option<S> = None;
        let mut next = 0usize; // next frame to render (block aligned)
        let mut gen_start = 0usize; // where this pass started
        let mut done = 0usize; // frames rendered this pass
        let mut chunk = first_chunk;
        let mut tail: Option<(usize, Vec<Vec<f32>>)> = None;
        let mut failed = false;

        while !self.cancel.load(Ordering::Relaxed) {
            let (v, s) = match self.shared.lock() {
                Ok(g) => (g.0, g.1.clone()),
                Err(_) => return,
            };
            let pos = (self.sb.pos().max(0.0) as usize).min(len - 1);
            let (rs, re, looping) = self.sb.region();
            let (lo, hi) = if looping && re > rs { (align_down(rs), align_up(re).min(len)) } else { (0, len) };
            let cycle = hi - lo;
            let offset = |x: usize| -> usize {
                // Distance from the start of this pass, wrapping round the cycle.
                if x >= gen_start { x - gen_start } else { x + cycle - gen_start }
            };
            let restart = |at: usize| -> usize { align_down(at.clamp(lo, hi.saturating_sub(1))) };

            if v != version {
                version = v;
                settings = Some(s);
                failed = false;
                next = if self.sb.is_ready(pos) { restart(pos + lead) } else { restart(pos) };
                if next >= hi {
                    next = lo;
                }
                gen_start = next;
                done = 0;
                chunk = first_chunk;
                tail = None;
            } else if failed {
                std::thread::sleep(Duration::from_millis(30));
                continue;
            } else if pos >= lo && pos < hi {
                // Playhead waiting on audio we have not rendered, or it has
                // overtaken the renderer: start again from the playhead.
                let waiting = !self.sb.is_ready(pos) && align_down(pos) != next;
                let overtaken = done < cycle && offset(pos) > offset(next) && offset(pos) < cycle;
                if waiting || overtaken {
                    next = restart(if waiting { pos } else { pos + lead });
                    gen_start = next;
                    done = 0;
                    chunk = first_chunk;
                    tail = None;
                }
            }

            let ahead = if next >= pos { next - pos } else { next + cycle - pos };
            if done >= cycle || (ahead > max_ahead && self.sb.is_ready(pos)) {
                std::thread::sleep(Duration::from_millis(15));
                continue;
            }

            let a = next;
            let b = (a + chunk).min(hi);
            let pre_start = a.saturating_sub(preroll);
            let x_end = (b + xfade).min(len);
            let set = settings.as_ref().unwrap();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.render)(set, pre_start, x_end)))
            .unwrap_or_else(|_| Err("an effect failed unexpectedly".into()));
            let mut out = match r {
                Ok(o) => o,
                Err(e) => {
                    if let Ok(mut er) = self.error.lock() {
                        *er = Some(e);
                    }
                    failed = true;
                    continue;
                }
            };
            let want = x_end - pre_start;
            for c in out.iter_mut() {
                c.resize(want, 0.0);
            }
            let (s0, s1) = (a - pre_start, b - pre_start);
            let mut seg: Vec<Vec<f32>> = out.iter().map(|c| c[s0..s1].to_vec()).collect();
            if let Some((ts, t)) = tail.as_ref().filter(|(_, t)| !t.is_empty() && !t[0].is_empty()) {
                if *ts == a {
                    for (c, ch) in seg.iter_mut().enumerate() {
                        let tc = &t[c.min(t.len() - 1)];
                        let n = tc.len().min(ch.len());
                        for i in 0..n {
                            let w = (i + 1) as f32 / (n + 1) as f32;
                            ch[i] = tc[i] * (1.0 - w) + ch[i] * w;
                        }
                    }
                }
            }
            // Settings may have changed while rendering: drop stale audio.
            if self.shared.lock().map(|g| g.0 != version).unwrap_or(true) {
                continue;
            }
            self.sb.write(a, &seg);
            tail = Some((b, out.iter().map(|c| c[s1..].to_vec()).collect()));
            done += b - a;
            next = b;
            if next >= hi {
                next = lo;
                tail = None;
            }
            chunk = (chunk * 2).min(max_chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::effects;

    #[test]
    fn live_render_matches_offline_for_static_gain() {
        let effects = Arc::new(effects::registry());
        let amp = effects.iter().position(|e| e.id == "amplify").unwrap();
        let mut p = effects[amp].default_params();
        p.set("left", crate::dsp::params::Value::F(-6.0206));
        let sr = 8000u32;
        let n = 3 * sr as usize + 123;
        let x: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        let audio: Buffer = Arc::new(vec![x.clone()]);
        let set = RackSettings { slots: vec![(amp, p)], mix: 100.0, in_db: 0.0, out_db: 0.0 };
        let lr = LiveRack::start(effects, audio, sr, None, set, 1, 0);
        let t0 = std::time::Instant::now();
        while (0..n).any(|i| !lr.sb.is_ready(i)) {
            assert!(t0.elapsed() < Duration::from_secs(10), "render timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        for i in (0..n).step_by(97) {
            let v = lr.sb.sample_for_test(0, i);
            assert!((v - x[i] * 0.5).abs() < 1e-4, "frame {i}: {v} vs {}", x[i] * 0.5);
        }
    }
}
