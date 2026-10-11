//! The Essential Sound panel: mark multitrack clips as Dialogue, Music,
//! SFX or Ambience and fix them with a few plain controls. The controls
//! become effects at the end of the clip's own effects rack, so the result
//! can be inspected and tweaked there; loudness matching sets the clip
//! gain; ducking writes volume automation; Remix changes a music clip's
//! length on beat boundaries.

use eframe::egui::{self, RichText, Ui};

use crate::app::{Action, App, JobKind};
use crate::dsp::params::Value;
use crate::session::{FxSlot, Session};
use crate::theme::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundType {
    Dialogue,
    Music,
    Sfx,
    Ambience,
}

impl SoundType {
    pub const ALL: [SoundType; 4] = [SoundType::Dialogue, SoundType::Music, SoundType::Sfx, SoundType::Ambience];

    pub fn label(self) -> &'static str {
        match self {
            SoundType::Dialogue => "Dialogue",
            SoundType::Music => "Music",
            SoundType::Sfx => "SFX",
            SoundType::Ambience => "Ambience",
        }
    }

    fn code(self) -> &'static str {
        match self {
            SoundType::Dialogue => "dialogue",
            SoundType::Music => "music",
            SoundType::Sfx => "sfx",
            SoundType::Ambience => "ambience",
        }
    }

    /// Loudness target for Auto-Match (LUFS).
    pub fn default_target(self) -> f32 {
        match self {
            SoundType::Dialogue => -23.0,
            SoundType::Music => -25.0,
            SoundType::Sfx => -23.0,
            SoundType::Ambience => -30.0,
        }
    }
}

pub const EQ_PRESETS: &[&str] = &["Vocal Presence", "Warmth", "Telephone", "SSB Voice (2.4 kHz)"];
pub const REVERB_PRESETS: &[&str] = &["Small Room", "Vocal Plate", "Great Hall"];

/// A clip's Essential Sound settings. `None` controls are off.
#[derive(Clone, Debug, PartialEq)]
pub struct Essential {
    pub kind: SoundType,
    pub target: f32,
    /// Repair (Dialogue), amounts 0–10.
    pub noise: Option<f32>,
    pub rumble: Option<f32>,
    /// Hum frequency: 50 or 60 Hz.
    pub dehum: Option<u32>,
    pub deess: Option<f32>,
    /// Clarity (Dialogue).
    pub dynamics: Option<f32>,
    pub eq: Option<usize>,
    /// Creative: reverb preset and amount 0–10.
    pub reverb: Option<(usize, f32)>,
    /// Ambience stereo width (%), SFX pan.
    pub width: Option<f32>,
    pub pan: Option<f32>,
    /// Ducking (Music, Ambience): duck against these types.
    pub duck_against: [bool; 4],
    pub duck_sensitivity: f32,
    pub duck_reduce: f32,
    pub duck_fade_ms: f32,
    /// How many effects at the end of the clip's rack belong to this panel.
    pub fx_count: usize,
}

impl Essential {
    pub fn new(kind: SoundType) -> Self {
        Essential {
            kind,
            target: kind.default_target(),
            noise: None,
            rumble: None,
            dehum: None,
            deess: None,
            dynamics: None,
            eq: None,
            reverb: None,
            width: None,
            pan: None,
            duck_against: [true, false, false, false],
            duck_sensitivity: 6.0,
            duck_reduce: 15.0,
            duck_fade_ms: 800.0,
            fx_count: 0,
        }
    }

    pub fn to_text(&self) -> String {
        let o = |v: Option<f32>| v.map(|x| x.to_string()).unwrap_or_default();
        let duck: String = self.duck_against.iter().map(|b| if *b { '1' } else { '0' }).collect();
        format!(
            "kind={};target={};noise={};rumble={};dehum={};deess={};dyn={};eq={};rev={};width={};pan={};duck={duck};sens={};reduce={};fade={};fx={}",
            self.kind.code(),
            self.target,
            o(self.noise),
            o(self.rumble),
            self.dehum.map(|v| v.to_string()).unwrap_or_default(),
            o(self.deess),
            o(self.dynamics),
            self.eq.map(|v| v.to_string()).unwrap_or_default(),
            self.reverb.map(|(p, a)| format!("{p}/{a}")).unwrap_or_default(),
            o(self.width),
            o(self.pan),
            self.duck_sensitivity,
            self.duck_reduce,
            self.duck_fade_ms,
            self.fx_count
        )
    }

    pub fn from_text(s: &str) -> Option<Self> {
        let get = |k: &str| s.split(';').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v.to_string()));
        let kind = SoundType::ALL.into_iter().find(|t| Some(t.code().to_string()) == get("kind"))?;
        let f = |k: &str| get(k).and_then(|v| v.parse::<f32>().ok());
        let mut e = Essential::new(kind);
        e.target = f("target").unwrap_or(e.target);
        e.noise = f("noise");
        e.rumble = f("rumble");
        e.dehum = get("dehum").and_then(|v| v.parse().ok());
        e.deess = f("deess");
        e.dynamics = f("dyn");
        e.eq = get("eq").and_then(|v| v.parse().ok());
        e.reverb = get("rev").and_then(|v| v.split_once('/').and_then(|(p, a)| Some((p.parse().ok()?, a.parse().ok()?))));
        e.width = f("width");
        e.pan = f("pan");
        if let Some(d) = get("duck") {
            for (i, ch) in d.chars().take(4).enumerate() {
                e.duck_against[i] = ch == '1';
            }
        }
        e.duck_sensitivity = f("sens").unwrap_or(e.duck_sensitivity);
        e.duck_reduce = f("reduce").unwrap_or(e.duck_reduce);
        e.duck_fade_ms = f("fade").unwrap_or(e.duck_fade_ms);
        e.fx_count = get("fx").and_then(|v| v.parse().ok()).unwrap_or(0);
        Some(e)
    }

    /// The effects these settings call for, in processing order.
    pub fn build_fx(&self, effects: &[crate::dsp::effects::EffectDef], sr: u32) -> Vec<FxSlot> {
        let mut out = Vec::new();
        let mut add = |id: &str, preset: Option<&str>, set: &[(&'static str, Value)]| {
            let Some(def) = effects.iter().find(|e| e.id == id) else { return };
            let mut p = preset.and_then(|n| def.preset_params(n)).unwrap_or_else(|| def.default_params());
            for (k, v) in set {
                p.set(k, v.clone());
            }
            if let Some(slot) = FxSlot::new(def.id, p, sr) {
                out.push(slot);
            }
        };
        let k = self.kind;
        if k == SoundType::Dialogue {
            if let Some(a) = self.rumble {
                add("parametric_eq", None, &[("hp_on", Value::B(true)), ("hp_f", Value::F(50.0 + a * 10.0))]);
            }
            if let Some(hz) = self.dehum {
                let mut set: Vec<(&'static str, Value)> = vec![("width", Value::C(0))];
                for (n, key) in [(1, ("n1_on", "n1_f", "n1_d")), (2, ("n2_on", "n2_f", "n2_d")), (3, ("n3_on", "n3_f", "n3_d")), (4, ("n4_on", "n4_f", "n4_d"))] {
                    set.push((key.0, Value::B(true)));
                    set.push((key.1, Value::F((hz * n) as f32)));
                    set.push((key.2, Value::F(40.0)));
                }
                add("notch_filter", None, &set);
            }
            if let Some(a) = self.noise {
                add("noise_gate", None, &[("threshold", Value::F(-72.0 + a * 3.5)), ("range", Value::F(6.0 + a * 2.4)), ("release", Value::F(180.0))]);
            }
            if let Some(a) = self.deess {
                add("deesser", None, &[("threshold", Value::F(-18.0 - a * 2.4))]);
            }
            if let Some(a) = self.dynamics {
                add("single_band_compressor", None, &[("threshold", Value::F(-12.0 - a * 2.4)), ("ratio", Value::F(2.0 + a * 0.4)), ("output", Value::F(a * 0.8))]);
            }
            if let Some(i) = self.eq {
                add("parametric_eq", EQ_PRESETS.get(i).copied(), &[]);
            }
        }
        if k == SoundType::Sfx {
            if let Some(pan) = self.pan {
                add("pan", None, &[("pan", Value::F(pan))]);
            }
        }
        if k == SoundType::Ambience {
            if let Some(w) = self.width {
                add("stereo_expander", None, &[("width", Value::F(w))]);
            }
        }
        if k != SoundType::Music {
            if let Some((preset, a)) = self.reverb {
                add("studio_reverb", REVERB_PRESETS.get(preset).copied(), &[("wet", Value::F(-30.0 + a * 2.4))]);
            }
        }
        out
    }
}

/// Replace a clip's Essential Sound effects with ones built from `es`.
pub fn apply_fx(s: &mut Session, clip: u64, mut es: Essential, effects: &[crate::dsp::effects::EffectDef]) {
    let sr = s.sample_rate;
    let new = es.build_fx(effects, sr);
    let Some(c) = s.clip_mut(clip) else { return };
    let keep = c.fx.len().saturating_sub(es.fx_count.min(c.fx.len()));
    c.fx.truncate(keep);
    es.fx_count = new.len();
    c.fx.extend(new);
    c.fx_on = true;
    c.es = Some(Box::new(es));
}

/// Volume automation on `clip`'s track that dips while the chosen kinds
/// of clips play. Returns how many dips were written.
pub fn generate_ducking(s: &mut Session, clip: u64) -> usize {
    let Some(c) = s.clip(clip).cloned() else { return 0 };
    let Some(es) = c.es.clone() else { return 0 };
    let Some((ti, _)) = s.find_clip(clip) else { return 0 };
    let sr = s.sample_rate as usize;
    let (a, b) = (c.start, c.end());
    // The other clips that trigger ducking, summed (mono) over this clip's span.
    let mut key = vec![0.0f32; b - a];
    for t in &s.tracks {
        for k in t.clips.iter().filter(|k| k.id != clip && !k.mute) {
            let Some(kes) = &k.es else { continue };
            let idx = SoundType::ALL.iter().position(|x| *x == kes.kind).unwrap_or(0);
            if !es.duck_against[idx] || k.end() <= a || k.start >= b {
                continue;
            }
            let Some(audio) = s.clip_audio(k.id) else { continue };
            for (ch, chan) in audio.iter().enumerate() {
                let _ = ch;
                for (i, v) in chan.iter().enumerate() {
                    let pos = k.start + i;
                    if pos >= a && pos < b {
                        key[pos - a] += v / audio.len() as f32;
                    }
                }
            }
        }
    }
    // Active where the 50 ms RMS is above the sensitivity threshold.
    let win = (sr / 20).max(1);
    let thr_db = -60.0 + es.duck_sensitivity.clamp(0.0, 10.0) * 4.0;
    let thr = 10f32.powf(thr_db / 20.0);
    let active: Vec<bool> = key.chunks(win).map(|w| (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt() > thr).collect();
    let fade = (es.duck_fade_ms.max(10.0) as usize * sr) / 1000;
    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < active.len() {
        if !active[k] {
            k += 1;
            continue;
        }
        let start = k;
        while k < active.len() && active[k] {
            k += 1;
        }
        let (ra, rb) = (a + start * win, (a + k * win).min(b));
        match regions.last_mut() {
            Some(last) if ra <= last.1 + 2 * fade => last.1 = rb,
            _ => regions.push((ra, rb)),
        }
    }
    regions.retain(|(x, y)| y - x >= sr / 10);
    s.push_undo("Generate Ducking");
    let t = &mut s.tracks[ti];
    let base = t.volume_db;
    let low = (base - es.duck_reduce.abs()).max(crate::session::VOL_ENV_MIN);
    t.vol_env.points.retain(|(p, _)| *p < a.saturating_sub(fade) || *p > b + fade);
    if !regions.is_empty() && t.vol_env.points.is_empty() {
        t.vol_env.points.push((0, base));
    }
    for (x, y) in &regions {
        t.vol_env.insert(x.saturating_sub(fade), base);
        t.vol_env.insert(*x, low);
        t.vol_env.insert(*y, low);
        t.vol_env.insert(y + fade, base);
    }
    t.show_env = true;
    t.lane_view = 0;
    s.touch();
    regions.len()
}

impl App {
    /// The Essential Sound panel (Multitrack editor, selected clips).
    pub fn essential_panel(&mut self, ui: &mut Ui) {
        let Some(si) = self.active_session.filter(|&i| i < self.sessions.len() && self.mode == crate::app::Mode::Multitrack) else {
            ui.label(RichText::new("Essential Sound works on clips in the Multitrack editor: select one or more clips there.").color(TEXT_DIM()));
            return;
        };
        let effects = self.effects.clone();
        let sel: Vec<u64> = {
            let s = &self.sessions[si];
            let mut v: Vec<u64> = s.selected_clips.iter().copied().filter(|id| s.clip(*id).is_some()).collect();
            v.sort_unstable();
            v
        };
        if sel.is_empty() {
            ui.label(RichText::new("Select one or more clips, then choose what they are.").color(TEXT_DIM()));
            return;
        }
        let first = self.sessions[si].clip(sel[0]).and_then(|c| c.es.clone()).map(|b| *b);
        let mut new_kind: Option<Option<SoundType>> = None;
        ui.label(RichText::new(format!("{} clip(s) selected", sel.len())).color(TEXT_DIM()).size(11.0));
        ui.horizontal_wrapped(|ui| {
            for k in SoundType::ALL {
                if ui.selectable_label(first.as_ref().map(|e| e.kind) == Some(k), k.label()).clicked() {
                    new_kind = Some(Some(k));
                }
            }
            if first.is_some() && ui.small_button("Clear Audio Type").clicked() {
                new_kind = Some(None);
            }
        });
        if let Some(k) = new_kind {
            let s = &mut self.sessions[si];
            s.push_undo("Audio Type");
            for id in &sel {
                match k {
                    Some(kind) => {
                        let es = Essential::new(kind);
                        let old = s.clip(*id).and_then(|c| c.es.as_ref().map(|e| e.fx_count)).unwrap_or(0);
                        apply_fx(s, *id, Essential { fx_count: old, ..es }, &effects);
                    }
                    None => {
                        if let Some(c) = s.clip_mut(*id) {
                            let n = c.es.as_ref().map(|e| e.fx_count).unwrap_or(0).min(c.fx.len());
                            let keep = c.fx.len() - n;
                            c.fx.truncate(keep);
                            c.es = None;
                        }
                    }
                }
            }
            s.touch();
            return;
        }
        let Some(mut es) = first else {
            ui.add_space(8.0);
            ui.label(RichText::new("Choose a type to see its controls.").color(TEXT_DIM()));
            return;
        };
        let before = es.clone();
        let mut auto_match = false;
        let mut duck_now = false;
        let mut remix: Option<(f32, bool)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Loudness");
            ui.horizontal(|ui| {
                ui.label("Target");
                ui.add(egui::DragValue::new(&mut es.target).speed(0.1).range(-40.0..=-10.0).fixed_decimals(1).suffix(" LUFS"));
                if ui.button("Auto-Match").on_hover_text("Set the clip gain so the clip measures the target loudness").clicked() {
                    auto_match = true;
                }
            });
            let opt = |ui: &mut Ui, label: &str, v: &mut Option<f32>, default: f32, range: std::ops::RangeInclusive<f32>, suffix: &str| {
                ui.horizontal(|ui| {
                    let mut on = v.is_some();
                    if ui.checkbox(&mut on, label).changed() {
                        *v = if on { Some(default) } else { None };
                    }
                    if let Some(x) = v.as_mut() {
                        ui.add(egui::Slider::new(x, range).suffix(suffix).fixed_decimals(1));
                    }
                });
            };
            match es.kind {
                SoundType::Dialogue => {
                    section(ui, "Repair");
                    opt(ui, "Reduce Noise", &mut es.noise, 5.0, 0.0..=10.0, "");
                    opt(ui, "Reduce Rumble", &mut es.rumble, 5.0, 0.0..=10.0, "");
                    ui.horizontal(|ui| {
                        let mut on = es.dehum.is_some();
                        if ui.checkbox(&mut on, "DeHum").changed() {
                            es.dehum = on.then_some(50);
                        }
                        if let Some(hz) = es.dehum.as_mut() {
                            ui.radio_value(hz, 50, "50 Hz");
                            ui.radio_value(hz, 60, "60 Hz");
                        }
                    });
                    opt(ui, "DeEss", &mut es.deess, 5.0, 0.0..=10.0, "");
                    section(ui, "Clarity");
                    opt(ui, "Dynamics", &mut es.dynamics, 5.0, 0.0..=10.0, "");
                    ui.horizontal(|ui| {
                        let mut on = es.eq.is_some();
                        if ui.checkbox(&mut on, "EQ").changed() {
                            es.eq = on.then_some(0);
                        }
                        if let Some(i) = es.eq.as_mut() {
                            egui::ComboBox::from_id_source("es_eq").width(170.0).selected_text(EQ_PRESETS[(*i).min(EQ_PRESETS.len() - 1)]).show_ui(ui, |ui| {
                                for (k, n) in EQ_PRESETS.iter().enumerate() {
                                    ui.selectable_value(i, k, *n);
                                }
                            });
                        }
                    });
                }
                SoundType::Music => {
                    section(ui, "Duration");
                    let len_s = self.sessions[si].clip(sel[0]).map(|c| c.len as f32 / self.sessions[si].sample_rate as f32).unwrap_or(0.0);
                    let id = ui.id().with("es_dur");
                    let mut target: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(len_s);
                    ui.horizontal(|ui| {
                        ui.label("Target");
                        ui.add(egui::DragValue::new(&mut target).speed(0.5).range(1.0..=3600.0).fixed_decimals(1).suffix(" s"));
                        ui.label(RichText::new(format!("(now {len_s:.1} s)")).color(TEXT_DIM()).size(11.0));
                    });
                    ui.data_mut(|d| d.insert_temp(id, target));
                    ui.horizontal(|ui| {
                        if ui.button("Remix").on_hover_text("Cut or repeat whole beats where the music sounds alike, keeping tempo and pitch").clicked() {
                            remix = Some((target, true));
                        }
                        if ui.button("Stretch").on_hover_text("Time-stretch to the length (changes the tempo)").clicked() {
                            remix = Some((target, false));
                        }
                    });
                }
                SoundType::Sfx => {
                    section(ui, "Pan");
                    opt(ui, "Pan", &mut es.pan, 0.0, -100.0..=100.0, "");
                }
                SoundType::Ambience => {
                    section(ui, "Stereo Width");
                    opt(ui, "Width", &mut es.width, 150.0, 0.0..=300.0, " %");
                }
            }
            if es.kind != SoundType::Music {
                section(ui, "Creative");
                ui.horizontal(|ui| {
                    let mut on = es.reverb.is_some();
                    if ui.checkbox(&mut on, "Reverb").changed() {
                        es.reverb = on.then_some((0, 4.0));
                    }
                    if let Some((p, a)) = es.reverb.as_mut() {
                        egui::ComboBox::from_id_source("es_rev").width(110.0).selected_text(REVERB_PRESETS[(*p).min(REVERB_PRESETS.len() - 1)]).show_ui(ui, |ui| {
                            for (k, n) in REVERB_PRESETS.iter().enumerate() {
                                ui.selectable_value(p, k, *n);
                            }
                        });
                        ui.add(egui::Slider::new(a, 0.0..=10.0).fixed_decimals(1));
                    }
                });
            }
            if matches!(es.kind, SoundType::Music | SoundType::Ambience) {
                section(ui, "Ducking");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Duck against");
                    for (i, k) in SoundType::ALL.into_iter().enumerate() {
                        ui.checkbox(&mut es.duck_against[i], k.label());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Sensitivity");
                    ui.add(egui::Slider::new(&mut es.duck_sensitivity, 0.0..=10.0).fixed_decimals(1));
                });
                ui.horizontal(|ui| {
                    ui.label("Reduce by");
                    ui.add(egui::Slider::new(&mut es.duck_reduce, 0.0..=40.0).suffix(" dB").fixed_decimals(1));
                });
                ui.horizontal(|ui| {
                    ui.label("Fades");
                    ui.add(egui::Slider::new(&mut es.duck_fade_ms, 50.0..=3000.0).suffix(" ms").fixed_decimals(0));
                });
                if ui.button("Generate Keyframes").on_hover_text("Write volume automation on this clip's track that dips under the chosen clip types").clicked() {
                    duck_now = true;
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("These controls add effects to the end of each clip's own effects rack (Clip Effects…), where they can be fine-tuned.").color(TEXT_DIM()).size(10.5));
        });
        let s = &mut self.sessions[si];
        if es != before {
            if self.mt.fx_undo_for != Some(u64::MAX - sel[0]) {
                s.push_undo("Essential Sound");
                self.mt.fx_undo_for = Some(u64::MAX - sel[0]);
            }
            for id in &sel {
                let old = s.clip(*id).and_then(|c| c.es.as_ref().map(|e| e.fx_count)).unwrap_or(0);
                // Each clip keeps its own type; the others follow the controls.
                let kind = s.clip(*id).and_then(|c| c.es.as_ref().map(|e| e.kind)).unwrap_or(es.kind);
                if kind == es.kind {
                    apply_fx(s, *id, Essential { fx_count: old, ..es.clone() }, &effects);
                }
            }
            s.touch();
        }
        if auto_match {
            self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::Match { mode: 0, target: es.target }));
        }
        if duck_now {
            let n: usize = sel.iter().map(|id| generate_ducking(&mut self.sessions[si], *id)).sum();
            self.set_status(if n == 0 { "No dialogue (or other chosen types) found under the clip: mark those clips in Essential Sound first.".to_string() } else { format!("Wrote {n} volume dip(s)") });
        }
        if let Some((secs, beats)) = remix {
            self.remix_clip(si, sel[0], secs, beats);
        }
    }

    /// Render a clip at a new length: Remix (beats) or Stretch.
    fn remix_clip(&mut self, si: usize, clip: u64, secs: f32, beats: bool) {
        let s = &self.sessions[si];
        let Some(c) = s.clip(clip) else { return };
        let Some(src) = s.sources.get(&c.source) else { return };
        let sr = s.sample_rate;
        let target = (secs.max(0.5) * sr as f32) as usize;
        let n = src.audio.len().clamp(1, 2);
        let audio: Vec<Vec<f32>> = (0..n).map(|ch| (0..c.len).map(|i| c.source_index(i, src.audio[ch].len()).map(|k| src.audio[ch][k]).unwrap_or(0.0)).collect()).collect();
        let effects = self.effects.clone();
        let percent = target as f32 / c.len.max(1) as f32 * 100.0;
        let session_id = s.id;
        let label = if beats { "Remix" } else { "Stretch Clip" };
        self.spawn_job(label.into(), JobKind::MtStretch { session_id, clips: vec![(clip, n)], percent, remixed: beats }, move || {
            let mut out = if beats {
                crate::dsp::remix::remix_beats(&audio, sr, target).ok_or("Remix found no steady beat in this clip; use Stretch instead.")?
            } else {
                audio
            };
            // A small stretch takes it to the exact length (all of it for Stretch).
            let len = out[0].len();
            if len != target {
                let ratio = target as f32 / len as f32 * 100.0;
                if beats && !(85.0..=115.0).contains(&ratio) {
                    return Err("Remix couldn't get close enough to that length; try a length nearer the original, or Stretch.".into());
                }
                let idx = effects.iter().position(|e| e.id == "stretch_pitch").ok_or("Stretch isn't available")?;
                let mut p = effects[idx].default_params();
                p.set("stretch", Value::F(ratio.clamp(25.0, 400.0)));
                let ctx = crate::dsp::effects::Ctx { sample_rate: sr, channels: out.len(), noise_print: None, sound_model: None };
                out = effects[idx].run(&out, &ctx, &p)?;
                for ch in out.iter_mut() {
                    ch.resize(target, 0.0);
                }
            }
            Ok(out)
        });
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(TEXT()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_build_effects() {
        let mut e = Essential::new(SoundType::Dialogue);
        e.noise = Some(4.0);
        e.dehum = Some(60);
        e.eq = Some(1);
        e.reverb = Some((2, 3.5));
        e.duck_against = [false, true, true, false];
        e.fx_count = 4;
        assert_eq!(Essential::from_text(&e.to_text()), Some(e.clone()));
        let reg = crate::dsp::effects::registry();
        let fx = e.build_fx(&reg, 48000);
        let ids: Vec<&str> = fx.iter().map(|f| f.effect).collect();
        assert_eq!(ids, vec!["notch_filter", "noise_gate", "parametric_eq", "studio_reverb"]);
    }

    #[test]
    fn ducking_dips_under_dialogue() {
        use std::sync::Arc;
        let mut s = Session::new(1, "T".into(), 1000, 2);
        let music = s.add_source("m".into(), Arc::new(vec![vec![0.3; 10_000]]));
        let voice = s.add_source("v".into(), Arc::new(vec![vec![0.5; 2_000]]));
        let m = s.insert_clip(0, music, 0).unwrap();
        let v = s.insert_clip(1, voice, 4_000).unwrap();
        let reg = crate::dsp::effects::registry();
        apply_fx(&mut s, m, Essential::new(SoundType::Music), &reg);
        apply_fx(&mut s, v, Essential::new(SoundType::Dialogue), &reg);
        assert_eq!(generate_ducking(&mut s, m), 1);
        let env = &s.tracks[0].vol_env;
        assert!(env.value_at(5_000).unwrap() <= -14.9, "ducked under the voice");
        assert!(env.value_at(1_000).unwrap().abs() < 0.01 && env.value_at(9_500).unwrap().abs() < 0.01);
    }
}
