//! Multitrack sessions: tracks of clips that point into source audio, the
//! mix of them, undo, and the `.audemo` session file format.
//!
//! Clips never own audio. Each refers to a [`Source`] by id; a source is
//! usually linked to an open file (document), so editing that file in the
//! Waveform editor updates every clip that uses it, as in Audition.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::dsp::effects::fade_shape;
use crate::dsp::effects::rt::{self, RtEffect};
use crate::dsp::effects::EffectDef;
use crate::dsp::params::Params;
use std::sync::Mutex;
use crate::dsp::peaks::PeakCache;
use crate::dsp::util::db_to_lin;
use crate::engine::Buffer;

pub const SESSION_EXT: &str = "audemo";

/// Track colours, cycled as tracks are added.
pub const TRACK_COLOURS: [[u8; 3]; 8] = [
    [0x3f, 0xdc, 0x9b],
    [0x4d, 0xa3, 0xff],
    [0xe8, 0xa1, 0x3a],
    [0xc7, 0x7d, 0xff],
    [0xf2, 0x6d, 0x6d],
    [0x5c, 0xd6, 0xd6],
    [0xd9, 0xd2, 0x4a],
    [0xff, 0x8f, 0xc8],
];

pub struct Source {
    pub id: u64,
    pub name: String,
    /// Open document this audio comes from (kept in sync with its edits).
    pub doc_id: Option<u64>,
    /// Document version the audio was taken from.
    pub doc_version: u64,
    /// File on disk, once known.
    pub path: Option<PathBuf>,
    /// Audio at the session's sample rate (1 or 2 channels).
    pub audio: Buffer,
    pub peaks: Arc<PeakCache>,
}

impl Source {
    #[cfg(test)]
    pub fn new(id: u64, name: String, audio: Buffer) -> Self {
        let peaks = Arc::new(PeakCache::build(&audio));
        Source { id, name, doc_id: None, doc_version: 0, path: None, audio, peaks }
    }

    pub fn len(&self) -> usize {
        self.audio.first().map(|c| c.len()).unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: u64,
    pub source: u64,
    pub name: String,
    /// Position on the timeline (session samples).
    pub start: usize,
    /// Where in the source the clip begins.
    pub offset: usize,
    pub len: usize,
    pub gain_db: f32,
    pub fade_in: usize,
    pub fade_out: usize,
    /// Fade shapes: -1..1 (0 = linear), or a cosine S-curve.
    pub fade_in_curve: f32,
    pub fade_out_curve: f32,
    pub fade_in_cos: bool,
    pub fade_out_cos: bool,
    pub mute: bool,
}

impl Clip {
    pub fn end(&self) -> usize {
        self.start + self.len
    }

    /// Fade/gain multiplier at `i` samples into the clip.
    pub fn envelope(&self, i: usize) -> f32 {
        let mut g = 1.0f32;
        if self.fade_in > 0 && i < self.fade_in {
            g *= fade_shape(i as f32 / self.fade_in as f32, self.fade_in_curve, self.fade_in_cos);
        }
        if self.fade_out > 0 && i + self.fade_out > self.len {
            let r = (self.len - i) as f32 / self.fade_out as f32;
            g *= fade_shape(r.min(1.0), self.fade_out_curve, self.fade_out_cos);
        }
        g
    }
}

/// Volume automation below this many dB means silence.
pub const VOL_ENV_MIN: f32 = -60.0;
pub const VOL_ENV_MAX: f32 = 12.0;

/// Automation: (timeline position, value) points, kept sorted by position.
/// Between points the value is interpolated linearly; before the first and
/// after the last it holds.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Envelope {
    pub points: Vec<(usize, f32)>,
}

impl Envelope {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn value_at(&self, pos: usize) -> Option<f32> {
        let p = &self.points;
        if p.is_empty() {
            return None;
        }
        let i = p.partition_point(|q| q.0 <= pos);
        if i == 0 {
            return Some(p[0].1);
        }
        if i == p.len() {
            return Some(p[i - 1].1);
        }
        let ((x0, y0), (x1, y1)) = (p[i - 1], p[i]);
        if x1 <= x0 {
            return Some(y1);
        }
        let t = (pos - x0) as f32 / (x1 - x0) as f32;
        Some(y0 + (y1 - y0) * t)
    }

    /// Add a point; returns its index.
    pub fn insert(&mut self, pos: usize, v: f32) -> usize {
        let i = self.points.partition_point(|q| q.0 < pos);
        self.points.insert(i, (pos, v));
        i
    }

    /// Move point `i`, keeping it between its neighbours.
    pub fn move_point(&mut self, i: usize, pos: usize, v: f32) {
        if i >= self.points.len() {
            return;
        }
        let lo = if i > 0 { self.points[i - 1].0 } else { 0 };
        let hi = self.points.get(i + 1).map(|q| q.0).unwrap_or(usize::MAX);
        self.points[i] = (pos.clamp(lo, hi), v);
    }

    fn to_text(&self) -> String {
        self.points.iter().map(|(p, v)| format!("{p}:{v}")).collect::<Vec<_>>().join(",")
    }

    fn parse(s: &str) -> Self {
        let mut points: Vec<(usize, f32)> = s
            .split(',')
            .filter_map(|kv| {
                let (k, v) = kv.split_once(':')?;
                Some((k.trim().parse().ok()?, v.trim().parse().ok()?))
            })
            .collect();
        points.sort_by_key(|p| p.0);
        Envelope { points }
    }
}

/// Pan gains for -100 (left) .. +100 (right): the centre is unity and panning
/// attenuates the opposite side (Audition's default pan law).
// Settings from Preferences > Multitrack and Multitrack Clips.
static EQUAL_POWER_PAN: AtomicBool = AtomicBool::new(false);
static AUTO_CROSSFADE: AtomicBool = AtomicBool::new(true);
static EQUAL_POWER_XFADE: AtomicBool = AtomicBool::new(true);
static CLIP_FADE_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_mix_prefs(equal_power_pan: bool, auto_crossfade: bool, equal_power_crossfade: bool, clip_fade_ms: f32) {
    EQUAL_POWER_PAN.store(equal_power_pan, Ordering::Relaxed);
    AUTO_CROSSFADE.store(auto_crossfade, Ordering::Relaxed);
    EQUAL_POWER_XFADE.store(equal_power_crossfade, Ordering::Relaxed);
    CLIP_FADE_MS.store(clip_fade_ms.max(0.0).to_bits(), Ordering::Relaxed);
}

/// Fade shape of automatic crossfades (see `fade_shape`): ≈ equal power, or linear.
fn crossfade_curve() -> f32 {
    if EQUAL_POWER_XFADE.load(Ordering::Relaxed) {
        0.233
    } else {
        0.0
    }
}

pub fn pan_gains(pan: f32) -> (f32, f32) {
    let p = (pan / 100.0).clamp(-1.0, 1.0);
    if EQUAL_POWER_PAN.load(Ordering::Relaxed) {
        // Sinusoidal: constant power, −3 dB each side at centre.
        let th = (p + 1.0) * std::f32::consts::FRAC_PI_4;
        return (th.cos(), th.sin());
    }
    let cut = |x: f32| (std::f32::consts::FRAC_PI_2 * x).cos();
    if p >= 0.0 {
        (cut(p), 1.0)
    } else {
        (1.0, cut(-p))
    }
}

/// Linear gain for an automation volume in dB (silence at the bottom).
pub fn env_gain(db: f32) -> f32 {
    if db <= VOL_ENV_MIN {
        0.0
    } else {
        db_to_lin(db)
    }
}

/// Overlapping clips on one track cross-fade automatically, as in Audition:
/// where clip B starts inside clip A and runs past its end, A fades out and
/// B fades in across the overlap. Returns the clips with those fades applied.
pub fn effective_clips(clips: &[Clip]) -> Vec<Clip> {
    let mut v = clips.to_vec();
    let curve = crossfade_curve();
    for (a, b) in crossfade_pairs(clips) {
        let x = v[a].end() - v[b].start;
        if x > v[a].fade_out {
            v[a].fade_out = x;
            v[a].fade_out_curve = curve;
            v[a].fade_out_cos = false;
        }
        if x > v[b].fade_in {
            v[b].fade_in = x;
            v[b].fade_in_curve = curve;
            v[b].fade_in_cos = false;
        }
    }
    for c in v.iter_mut() {
        let total = c.fade_in + c.fade_out;
        if total > c.len && total > 0 {
            c.fade_in = (c.fade_in as u64 * c.len as u64 / total as u64) as usize;
            c.fade_out = c.len - c.fade_in;
        }
    }
    v
}

/// (earlier, later) index pairs of partially overlapping, unmuted clips.
pub fn crossfade_pairs(clips: &[Clip]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if !AUTO_CROSSFADE.load(Ordering::Relaxed) {
        return out;
    }
    for (i, a) in clips.iter().enumerate() {
        for (j, b) in clips.iter().enumerate() {
            if i == j || a.mute || b.mute {
                continue;
            }
            let b_after = b.start > a.start || (b.start == a.start && j > i);
            if b_after && b.start < a.end() && b.end() > a.end() {
                out.push((i, j));
            }
        }
    }
    out
}

/// A live effect instance shared between the session and the audio thread.
pub type SharedFx = Arc<Mutex<Box<dyn RtEffect>>>;

/// One slot of a track, bus or master effects rack.
#[derive(Clone)]
pub struct FxSlot {
    /// Stable identity (effect windows find their slot by it).
    pub id: u64,
    pub effect: &'static str,
    pub params: Params,
    pub on: bool,
    pub rt: SharedFx,
    /// A plug-in that isn't available: it passes audio through and keeps
    /// its settings so saving the session doesn't lose them. Shared with the
    /// slot's undo copies, like `rt`.
    pub missing: Arc<std::sync::atomic::AtomicBool>,
}

impl FxSlot {
    pub fn new(effect: &'static str, params: Params, sample_rate: u32) -> Option<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let e = rt::make(effect, &params, sample_rate)?;
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(FxSlot { id, effect, params, on: true, rt: Arc::new(Mutex::new(e)), missing: Arc::new(false.into()) })
    }

    /// A slot for a plug-in that can't be loaded right now.
    pub fn placeholder(effect: &'static str, params: Params) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 40);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        FxSlot { id, effect, params, on: true, rt: Arc::new(Mutex::new(Box::new(rt::Passthrough))), missing: Arc::new(true.into()) }
    }

    pub fn is_missing(&self) -> bool {
        self.missing.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Try to load a missing plug-in again (after a scan). True if it loaded.
    pub fn revive(&mut self, sample_rate: u32) -> bool {
        if !self.is_missing() {
            return false;
        }
        match rt::make(self.effect, &self.params, sample_rate) {
            Some(e) => {
                if let Ok(mut r) = self.rt.lock() {
                    *r = e;
                }
                self.missing.store(false, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// New parameters, applied to the running instance without resetting it.
    pub fn set_params(&mut self, p: Params) {
        if let Ok(mut e) = self.rt.lock() {
            e.set_params(&p);
        }
        self.params = p;
    }

    /// Push this slot's stored parameters to its instance (after undo).
    pub fn sync(&self) {
        if let Ok(mut e) = self.rt.lock() {
            e.set_params(&self.params);
        }
    }

    pub fn reset(&self) {
        if let Ok(mut e) = self.rt.lock() {
            e.reset();
        }
    }

    /// The same effect and settings with its own, fresh state (for mixdown).
    fn fresh(&self, sample_rate: u32) -> FxSlot {
        let mut s = self.clone();
        // A missing plug-in passes audio through in mixdowns too.
        if self.is_missing() {
            return s;
        }
        if let Some(e) = rt::make(self.effect, &self.params, sample_rate) {
            s.rt = Arc::new(Mutex::new(e));
        }
        s
    }
}

impl PartialEq for FxSlot {
    fn eq(&self, o: &Self) -> bool {
        self.effect == o.effect && self.params == o.params && self.on == o.on
    }
}

impl std::fmt::Debug for FxSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FxSlot({}, on={})", self.effect, self.on)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackKind {
    #[default]
    Audio,
    /// No clips: mixes the tracks routed or sent to it.
    Bus,
}

/// A send from a track to a bus.
#[derive(Clone, Debug, PartialEq)]
pub struct Send {
    pub bus: u64,
    pub level_db: f32,
    /// Taken before the track's fader (true) or after it.
    pub pre: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub colour: [u8; 3],
    pub volume_db: f32,
    /// -100 (left) .. +100 (right)
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub arm: bool,
    pub height: f32,
    pub clips: Vec<Clip>,
    /// Volume automation in dB (overrides `volume_db` when not empty).
    pub vol_env: Envelope,
    /// Pan automation (overrides `pan` when not empty).
    pub pan_env: Envelope,
    /// Show the automation lines over the track.
    pub show_env: bool,
    pub kind: TrackKind,
    /// Bus track this track feeds (None = the master).
    pub output: Option<u64>,
    pub sends: Vec<Send>,
    /// Effects rack (runs before the fader).
    pub fx: Vec<FxSlot>,
    /// Rack power.
    pub fx_on: bool,
}

impl Track {
    pub fn new(id: u64, name: String, colour: [u8; 3]) -> Self {
        Track {
            id,
            name,
            colour,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            arm: false,
            height: 96.0,
            clips: Vec::new(),
            vol_env: Envelope::default(),
            pan_env: Envelope::default(),
            show_env: false,
            kind: TrackKind::Audio,
            output: None,
            sends: Vec::new(),
            fx: Vec::new(),
            fx_on: true,
        }
    }

    pub fn is_bus(&self) -> bool {
        self.kind == TrackKind::Bus
    }

    pub fn pan_gains(&self) -> (f32, f32) {
        pan_gains(self.pan)
    }

    /// Volume (dB) at `pos`, from the automation when there is any.
    pub fn volume_at(&self, pos: usize) -> f32 {
        self.vol_env.value_at(pos).unwrap_or(self.volume_db)
    }

    pub fn pan_at(&self, pos: usize) -> f32 {
        self.pan_env.value_at(pos).unwrap_or(self.pan)
    }
}

#[derive(Clone)]
pub struct SessionSnapshot {
    pub label: String,
    pub tracks: Vec<Track>,
    pub master_db: f32,
    pub master_fx: Vec<FxSlot>,
    pub master_fx_on: bool,
}

/// Everything the mixer needs, cheap to clone and share with a render thread.
#[derive(Clone)]
pub struct MixState {
    pub sample_rate: u32,
    pub tracks: Vec<Track>,
    pub sources: BTreeMap<u64, Arc<Source>>,
    pub master_db: f32,
    pub master_fx: Vec<FxSlot>,
    pub master_fx_on: bool,
}

impl MixState {
    /// A copy whose effects have their own fresh state, so an offline render
    /// (mixdown) neither hears nor disturbs the live instances.
    pub fn with_fresh_fx(&self) -> MixState {
        let sr = self.sample_rate;
        let mut m = self.clone();
        for t in m.tracks.iter_mut() {
            t.fx = t.fx.iter().map(|s| s.fresh(sr)).collect();
        }
        m.master_fx = m.master_fx.iter().map(|s| s.fresh(sr)).collect();
        m
    }
}

pub struct Session {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub sample_rate: u32,
    pub tracks: Vec<Track>,
    pub sources: BTreeMap<u64, Arc<Source>>,
    pub master_db: f32,
    pub master_fx: Vec<FxSlot>,
    pub master_fx_on: bool,
    pub cursor: usize,
    pub sel: Option<(usize, usize)>,
    pub view_start: f64,
    pub view_end: f64,
    pub scroll_y: f32,
    pub selected_track: usize,
    pub selected_clips: HashSet<u64>,
    pub undo: Vec<SessionSnapshot>,
    pub redo: Vec<SessionSnapshot>,
    pub dirty: bool,
    /// Bumped whenever the mix could sound different.
    pub version: u64,
    next_id: u64,
}

impl Session {
    pub fn new(id: u64, name: String, sample_rate: u32, n_tracks: usize) -> Self {
        let mut s = Session {
            id,
            name,
            path: None,
            sample_rate,
            tracks: Vec::new(),
            sources: BTreeMap::new(),
            master_db: 0.0,
            master_fx: Vec::new(),
            master_fx_on: true,
            cursor: 0,
            sel: None,
            view_start: 0.0,
            view_end: sample_rate as f64 * 60.0,
            scroll_y: 0.0,
            selected_track: 0,
            selected_clips: HashSet::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            version: 1,
            next_id: 1,
        };
        for _ in 0..n_tracks {
            s.add_track_silent();
        }
        s
    }

    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Bump the id counter past ids loaded from a file.
    pub fn reserve_ids(&mut self, max: u64) {
        self.next_id = self.next_id.max(max);
    }

    pub fn touch(&mut self) {
        self.version += 1;
        self.dirty = true;
    }

    pub fn display_name(&self) -> String {
        if self.dirty {
            format!("{} *", self.name)
        } else {
            self.name.clone()
        }
    }

    /// End of the last clip.
    pub fn end(&self) -> usize {
        self.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.end()).max().unwrap_or(0)
    }

    /// Timeline extent shown and scrolled through.
    pub fn max_span(&self) -> f64 {
        (self.end() as f64 * 1.1).max(self.sample_rate as f64 * 60.0)
    }

    pub fn clamp_view(&mut self) {
        let max = self.max_span();
        let span = (self.view_end - self.view_start).clamp(64.0, max);
        let mut start = self.view_start.max(0.0);
        if start + span > max {
            start = (max - span).max(0.0);
        }
        self.view_start = start;
        self.view_end = start + span;
    }

    pub fn zoom(&mut self, factor: f64, anchor: f64) {
        let span = self.view_end - self.view_start;
        let new_span = (span * factor).clamp(64.0, self.max_span());
        let t = if span > 0.0 { (anchor - self.view_start) / span } else { 0.5 };
        self.view_start = anchor - new_span * t;
        self.view_end = self.view_start + new_span;
        self.clamp_view();
    }

    pub fn sel_range(&self) -> Option<(usize, usize)> {
        self.sel.filter(|(a, b)| b > a)
    }

    fn add_track_silent(&mut self) -> usize {
        let id = self.new_id();
        let n = self.tracks.len();
        self.tracks.push(Track::new(id, format!("Track {}", n + 1), TRACK_COLOURS[n % TRACK_COLOURS.len()]));
        n
    }

    // ------------------------------------------------------------ undo

    pub fn push_undo(&mut self, label: &str) {
        self.undo.push(SessionSnapshot {
            label: label.to_string(),
            tracks: self.tracks.clone(),
            master_db: self.master_db,
            master_fx: self.master_fx.clone(),
            master_fx_on: self.master_fx_on,
        });
        if self.undo.len() > crate::app::undo_levels() {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Record an undo step for a change already made, given the tracks as
    /// they were before it.
    pub fn push_undo_from(&mut self, label: &str, tracks_before: Vec<Track>) {
        self.undo.push(SessionSnapshot {
            label: label.to_string(),
            tracks: tracks_before,
            master_db: self.master_db,
            master_fx: self.master_fx.clone(),
            master_fx_on: self.master_fx_on,
        });
        if self.undo.len() > crate::app::undo_levels() {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, s: SessionSnapshot) -> SessionSnapshot {
        let cur = SessionSnapshot {
            label: s.label.clone(),
            tracks: std::mem::replace(&mut self.tracks, s.tracks),
            master_db: self.master_db,
            master_fx: std::mem::replace(&mut self.master_fx, s.master_fx),
            master_fx_on: self.master_fx_on,
        };
        self.master_db = s.master_db;
        self.master_fx_on = s.master_fx_on;
        // Live effect instances are shared with the snapshot: give them back
        // the restored settings.
        for slot in self.tracks.iter().flat_map(|t| t.fx.iter()).chain(self.master_fx.iter()) {
            slot.sync();
        }
        self.selected_track = self.selected_track.min(self.tracks.len().saturating_sub(1));
        self.selected_clips.retain(|id| self.tracks.iter().any(|t| t.clips.iter().any(|c| c.id == *id)));
        self.touch();
        cur
    }

    pub fn undo(&mut self) -> Option<String> {
        let s = self.undo.pop()?;
        let label = s.label.clone();
        let cur = self.restore(s);
        self.redo.push(cur);
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let s = self.redo.pop()?;
        let label = s.label.clone();
        let cur = self.restore(s);
        self.undo.push(cur);
        Some(label)
    }

    // ------------------------------------------------------------ edits

    pub fn add_track(&mut self) -> usize {
        self.push_undo("Add Track");
        let i = self.add_track_silent();
        self.touch();
        i
    }

    /// Index of the first audio (non-bus) track at or after `i`, adding one
    /// if there isn't any.
    pub fn audio_track_at_or_after(&mut self, i: usize) -> usize {
        if let Some(j) = (i..self.tracks.len()).chain(0..i.min(self.tracks.len())).find(|&j| !self.tracks[j].is_bus()) {
            return j;
        }
        let id = self.new_id();
        let n = self.tracks.len();
        self.tracks.push(Track::new(id, format!("Track {}", n + 1), TRACK_COLOURS[n % TRACK_COLOURS.len()]));
        self.touch();
        n
    }

    pub fn add_bus(&mut self) -> usize {
        self.push_undo("Add Bus Track");
        let id = self.new_id();
        let n = self.tracks.iter().filter(|t| t.is_bus()).count();
        let letter = (b'A' + (n % 26) as u8) as char;
        let mut t = Track::new(id, format!("Bus {letter}"), [0x9a, 0x9a, 0xa8]);
        t.kind = TrackKind::Bus;
        t.height = 64.0;
        self.tracks.push(t);
        self.touch();
        self.tracks.len() - 1
    }

    /// Clear every effect's state (before playback starts).
    pub fn reset_fx(&self) {
        for slot in self.tracks.iter().flat_map(|t| t.fx.iter()).chain(self.master_fx.iter()) {
            slot.reset();
        }
    }

    pub fn remove_track(&mut self, i: usize) {
        if i >= self.tracks.len() {
            return;
        }
        self.push_undo("Delete Track");
        self.tracks.remove(i);
        self.selected_track = self.selected_track.min(self.tracks.len().saturating_sub(1));
        self.touch();
    }

    #[cfg(test)]
    pub fn add_source(&mut self, name: String, audio: Buffer) -> u64 {
        let id = self.new_id();
        self.sources.insert(id, Arc::new(Source::new(id, name, audio)));
        id
    }

    pub fn source_len(&self, id: u64) -> usize {
        self.sources.get(&id).map(|s| s.len()).unwrap_or(0)
    }

    /// Place a whole source on `track` at `at`; returns the clip id.
    pub fn insert_clip(&mut self, track: usize, source: u64, at: usize) -> Option<u64> {
        let len = self.source_len(source);
        if len == 0 || track >= self.tracks.len() || self.tracks[track].is_bus() {
            return None;
        }
        let name = self.sources[&source].name.clone();
        self.push_undo("Insert Clip");
        let id = self.new_id();
        let fade = ((f32::from_bits(CLIP_FADE_MS.load(Ordering::Relaxed)) as f64 * self.sample_rate as f64 / 1000.0) as usize).min(len / 2);
        self.tracks[track].clips.push(Clip {
            id,
            source,
            name,
            start: at,
            offset: 0,
            len,
            gain_db: 0.0,
            fade_in: fade,
            fade_out: fade,
            fade_in_curve: 0.0,
            fade_out_curve: 0.0,
            fade_in_cos: false,
            fade_out_cos: false,
            mute: false,
        });
        self.selected_clips.clear();
        self.selected_clips.insert(id);
        self.touch();
        Some(id)
    }

    pub fn find_clip(&self, id: u64) -> Option<(usize, usize)> {
        self.tracks.iter().enumerate().find_map(|(t, tr)| tr.clips.iter().position(|c| c.id == id).map(|c| (t, c)))
    }

    pub fn clip(&self, id: u64) -> Option<&Clip> {
        self.find_clip(id).map(|(t, c)| &self.tracks[t].clips[c])
    }

    pub fn clip_mut(&mut self, id: u64) -> Option<&mut Clip> {
        let (t, c) = self.find_clip(id)?;
        Some(&mut self.tracks[t].clips[c])
    }

    /// Move a clip to `start` on `track` (no undo; callers push one per drag).
    pub fn move_clip(&mut self, id: u64, start: usize, track: usize) {
        let Some((t, c)) = self.find_clip(id) else { return };
        let track = track.min(self.tracks.len() - 1);
        // Clips can't live on bus tracks.
        let track = if self.tracks[track].is_bus() { t } else { track };
        if t == track {
            if self.tracks[t].clips[c].start != start {
                self.tracks[t].clips[c].start = start;
                self.touch();
            }
            return;
        }
        let mut clip = self.tracks[t].clips.remove(c);
        clip.start = start;
        self.tracks[track].clips.push(clip);
        self.touch();
    }

    /// Move the clip's left edge to `new_start`, keeping its audio in place.
    pub fn trim_start(&mut self, id: u64, new_start: usize) {
        let Some(c) = self.clip(id).cloned() else { return };
        let min_start = c.start.saturating_sub(c.offset);
        let new_start = new_start.clamp(min_start, c.end().saturating_sub(1));
        let clip = self.clip_mut(id).unwrap();
        if new_start >= clip.start {
            let d = new_start - clip.start;
            clip.offset += d;
            clip.len -= d;
        } else {
            let d = clip.start - new_start;
            clip.offset -= d;
            clip.len += d;
        }
        clip.start = new_start;
        clip.fade_in = clip.fade_in.min(clip.len);
        clip.fade_out = clip.fade_out.min(clip.len - clip.fade_in);
        self.touch();
    }

    /// Move the clip's right edge to `new_end`.
    pub fn trim_end(&mut self, id: u64, new_end: usize) {
        let Some(c) = self.clip(id).cloned() else { return };
        let src_len = self.source_len(c.source);
        let max_end = c.start + src_len.saturating_sub(c.offset);
        let new_end = new_end.clamp(c.start + 1, max_end.max(c.start + 1));
        let clip = self.clip_mut(id).unwrap();
        clip.len = new_end - clip.start;
        clip.fade_out = clip.fade_out.min(clip.len);
        clip.fade_in = clip.fade_in.min(clip.len - clip.fade_out);
        self.touch();
    }

    /// Split every selected clip (or, with none selected, every clip on the
    /// selected track) that spans `pos`. Clips whose fade covers `pos` are
    /// left alone (splitting would change the fade's shape). Returns
    /// (split, skipped because of a fade).
    pub fn split_at(&mut self, pos: usize) -> (usize, usize) {
        let targets: Vec<u64> = if self.selected_clips.is_empty() {
            self.tracks.get(self.selected_track).map(|t| t.clips.iter().map(|c| c.id).collect()).unwrap_or_default()
        } else {
            self.selected_clips.iter().copied().collect()
        };
        let spanning: Vec<u64> = targets.into_iter().filter(|id| self.clip(*id).map(|c| pos > c.start && pos < c.end()).unwrap_or(false)).collect();
        let in_fade = |c: &Clip| pos < c.start + c.fade_in || pos + c.fade_out > c.end();
        let hits: Vec<u64> = spanning.iter().copied().filter(|id| self.clip(*id).map(|c| !in_fade(c)).unwrap_or(false)).collect();
        let skipped = spanning.len() - hits.len();
        if hits.is_empty() {
            return (0, skipped);
        }
        self.push_undo("Split");
        for id in &hits {
            let (t, ci) = self.find_clip(*id).unwrap();
            let nid = self.new_id();
            let c = &mut self.tracks[t].clips[ci];
            let left = pos - c.start;
            let mut right = c.clone();
            right.id = nid;
            right.start = pos;
            right.offset = c.offset + left;
            right.len = c.len - left;
            right.fade_in = 0;
            c.len = left;
            c.fade_out = 0;
            self.tracks[t].clips.push(right);
            self.selected_clips.insert(nid);
        }
        self.touch();
        (hits.len(), skipped)
    }

    pub fn delete_clips(&mut self, ids: &[u64]) -> usize {
        let n: usize = self.tracks.iter().map(|t| t.clips.iter().filter(|c| ids.contains(&c.id)).count()).sum();
        if n == 0 {
            return 0;
        }
        self.push_undo(if n == 1 { "Delete Clip" } else { "Delete Clips" });
        for t in self.tracks.iter_mut() {
            t.clips.retain(|c| !ids.contains(&c.id));
        }
        self.selected_clips.retain(|id| !ids.contains(id));
        self.touch();
        n
    }

    /// Clip edges and the cursor, for snapping.
    pub fn snap_points(&self, exclude: Option<u64>) -> Vec<usize> {
        let mut v = vec![0, self.cursor];
        for t in &self.tracks {
            for c in &t.clips {
                if Some(c.id) != exclude {
                    v.push(c.start);
                    v.push(c.end());
                }
            }
        }
        if let Some((a, b)) = self.sel_range() {
            v.push(a);
            v.push(b);
        }
        v
    }

    /// Replace a source's audio (e.g. after its file was edited).
    pub fn set_source_audio(&mut self, id: u64, audio: Buffer, peaks: Arc<PeakCache>, doc_id: Option<u64>, doc_version: u64) {
        if let Some(s) = self.sources.get(&id) {
            let ns = Source { id, name: s.name.clone(), doc_id: doc_id.or(s.doc_id), doc_version, path: s.path.clone(), audio, peaks };
            let new_len = ns.len();
            self.sources.insert(id, Arc::new(ns));
            // Keep clips inside the (possibly shorter) source.
            for t in self.tracks.iter_mut() {
                for c in t.clips.iter_mut().filter(|c| c.source == id) {
                    if c.offset >= new_len {
                        c.offset = new_len.saturating_sub(1);
                    }
                    c.len = c.len.min(new_len - c.offset).max(1);
                    c.fade_in = c.fade_in.min(c.len);
                    c.fade_out = c.fade_out.min(c.len - c.fade_in);
                }
            }
            self.version += 1;
        }
    }

    pub fn mix_state(&self) -> MixState {
        let tracks = self
            .tracks
            .iter()
            .map(|t| {
                let mut t = t.clone();
                t.clips = effective_clips(&t.clips);
                t
            })
            .collect();
        MixState {
            sample_rate: self.sample_rate,
            tracks,
            sources: self.sources.clone(),
            master_db: self.master_db,
            master_fx: self.master_fx.clone(),
            master_fx_on: self.master_fx_on,
        }
    }
}

// ---------------------------------------------------------------- mixing

/// Render frames [a, b) of the stereo mix offline (mixdown), with fresh
/// effect state, in blocks.
pub fn mix_range(m: &MixState, a: usize, b: usize) -> Vec<Vec<f32>> {
    let fresh = m.with_fresh_fx();
    let mut out = vec![Vec::with_capacity(b.saturating_sub(a)), Vec::with_capacity(b.saturating_sub(a))];
    let mut tmp = vec![Vec::new(), Vec::new()];
    let mut scratch = MixScratch::default();
    let mut c = a;
    while c < b {
        let e = (c + 8192).min(b);
        mix_into(&fresh, c, e, &mut tmp, &mut [], &mut scratch);
        out[0].extend_from_slice(&tmp[0]);
        out[1].extend_from_slice(&tmp[1]);
        c = e;
    }
    out
}

/// Per-track working buffers, kept between blocks so mixing in the audio
/// callback doesn't allocate.
#[derive(Default)]
pub struct MixScratch {
    bufs: Vec<[Vec<f32>; 2]>,
}

impl MixScratch {
    /// Make room for `tracks` tracks of `n` frames (call off the audio thread).
    pub fn reserve(&mut self, tracks: usize, n: usize) {
        let want = tracks + 8;
        if self.bufs.len() < want {
            self.bufs.resize_with(want, || [Vec::new(), Vec::new()]);
        }
        for b in self.bufs.iter_mut() {
            for c in b.iter_mut() {
                if c.capacity() < n {
                    c.reserve(n - c.len());
                }
            }
        }
    }

    fn prepare(&mut self, tracks: usize, n: usize) {
        if self.bufs.len() < tracks {
            self.bufs.resize_with(tracks, || [Vec::new(), Vec::new()]);
        }
        for b in self.bufs[..tracks].iter_mut() {
            for c in b.iter_mut() {
                c.clear();
                c.resize(n, 0.0);
            }
        }
    }

    /// bufs[dst] += bufs[src] * g
    fn add(&mut self, src: usize, dst: usize, g: f32) {
        if src == dst {
            return;
        }
        let (s, d) = if src < dst {
            let (lo, hi) = self.bufs.split_at_mut(dst);
            (&lo[src], &mut hi[0])
        } else {
            let (lo, hi) = self.bufs.split_at_mut(src);
            (&hi[0], &mut lo[dst])
        };
        for c in 0..2 {
            for (o, i) in d[c].iter_mut().zip(&s[c]) {
                *o += i * g;
            }
        }
    }
}

/// Run a rack over a stereo block. A slot the UI is busy updating is
/// skipped for this block rather than waited for.
fn run_rack(fx: &[FxSlot], on: bool, l: &mut [f32], r: &mut [f32]) {
    if !on {
        return;
    }
    for slot in fx.iter().filter(|s| s.on) {
        // The UI only holds a slot's lock for a few microseconds while it
        // updates settings, so spin briefly rather than skip the effect.
        for _ in 0..20_000 {
            if let Ok(mut e) = slot.rt.try_lock() {
                e.process(l, r);
                break;
            }
            std::hint::spin_loop();
        }
    }
}

/// Apply a track's (or bus's) volume and pan, with automation.
fn fader(t: &Track, a: usize, l: &mut [f32], r: &mut [f32]) {
    if t.vol_env.is_empty() && t.pan_env.is_empty() {
        let g = db_to_lin(t.volume_db);
        let (pl, pr) = t.pan_gains();
        l.iter_mut().for_each(|s| *s *= g * pl);
        r.iter_mut().for_each(|s| *s *= g * pr);
        return;
    }
    for k in 0..l.len() {
        let pos = a + k;
        let g = if t.vol_env.is_empty() { db_to_lin(t.volume_db) } else { env_gain(t.volume_at(pos)) };
        let (pl, pr) = pan_gains(t.pan_at(pos));
        l[k] *= g * pl;
        r[k] *= g * pr;
    }
}

fn peak_into(peaks: &mut [[f32; 2]], ti: usize, l: &[f32], r: &[f32]) {
    if let Some(p) = peaks.get_mut(ti) {
        p[0] = l.iter().fold(p[0], |m, s| m.max(s.abs()));
        p[1] = r.iter().fold(p[1], |m, s| m.max(s.abs()));
    }
}

/// Render frames [a, b) of the mix into `out` (two channels): each audio
/// track's clips, its effects rack, sends, fader and routing; then each bus;
/// then the master rack and fader. Reuses `out` and `scratch` so it doesn't
/// allocate in the audio callback. Each track's post-fader peak (L, R) is
/// max-ed into `peaks[track]` when there is room.
pub fn mix_into(m: &MixState, a: usize, b: usize, out: &mut [Vec<f32>], peaks: &mut [[f32; 2]], scratch: &mut MixScratch) {
    let n = b.saturating_sub(a);
    for ch in out.iter_mut() {
        ch.clear();
        ch.resize(n, 0.0);
    }
    scratch.prepare(m.tracks.len(), n);
    let any_solo = m.tracks.iter().any(|t| !t.is_bus() && t.solo);
    let bus_index = |id: u64| m.tracks.iter().position(|t| t.id == id && t.is_bus());
    for (ti, t) in m.tracks.iter().enumerate() {
        if t.is_bus() {
            continue;
        }
        let silent = t.mute || (any_solo && !t.solo);
        // A silent track with no effects has nothing to keep running.
        if silent && (t.fx.is_empty() || !t.fx_on) {
            continue;
        }
        {
            let [bl, br] = &mut scratch.bufs[ti];
            for c in &t.clips {
                if c.mute || c.end() <= a || c.start >= b {
                    continue;
                }
                let Some(src) = m.sources.get(&c.source) else { continue };
                let sl = &src.audio[0];
                let sr = &src.audio[src.audio.len().min(2) - 1];
                let cg = db_to_lin(c.gain_db);
                for t_pos in c.start.max(a)..c.end().min(b) {
                    let i = t_pos - c.start;
                    let idx = c.offset + i;
                    if idx >= sl.len() {
                        break;
                    }
                    let g = cg * c.envelope(i);
                    let k = t_pos - a;
                    bl[k] += sl[idx] * g;
                    br[k] += sr[idx] * g;
                }
            }
            run_rack(&t.fx, t.fx_on, bl, br);
        }
        if silent {
            // Muted after its effects, as in Audition: tails continue, unheard.
            continue;
        }
        for s in t.sends.iter().filter(|s| s.pre) {
            if let Some(bi) = bus_index(s.bus) {
                scratch.add(ti, bi, db_to_lin(s.level_db));
            }
        }
        {
            let [bl, br] = &mut scratch.bufs[ti];
            fader(t, a, bl, br);
            peak_into(peaks, ti, bl, br);
        }
        for s in t.sends.iter().filter(|s| !s.pre) {
            if let Some(bi) = bus_index(s.bus) {
                scratch.add(ti, bi, db_to_lin(s.level_db));
            }
        }
        match t.output.and_then(bus_index) {
            Some(bi) => scratch.add(ti, bi, 1.0),
            None => {
                let [bl, br] = &scratch.bufs[ti];
                for (o, v) in out[0].iter_mut().zip(bl) {
                    *o += v;
                }
                for (o, v) in out[1].iter_mut().zip(br) {
                    *o += v;
                }
            }
        }
    }
    for (bi, t) in m.tracks.iter().enumerate() {
        if !t.is_bus() || t.mute {
            continue;
        }
        let [bl, br] = &mut scratch.bufs[bi];
        run_rack(&t.fx, t.fx_on, bl, br);
        fader(t, a, bl, br);
        peak_into(peaks, bi, bl, br);
        for (o, v) in out[0].iter_mut().zip(bl.iter()) {
            *o += v;
        }
        for (o, v) in out[1].iter_mut().zip(br.iter()) {
            *o += v;
        }
    }
    if let [ol, or, ..] = out {
        run_rack(&m.master_fx, m.master_fx_on, ol, or);
    }
    let master = db_to_lin(m.master_db);
    if master != 1.0 {
        out.iter_mut().for_each(|c| c.iter_mut().for_each(|s| *s *= master));
    }
}

// ---------------------------------------------------------------- file format

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn rel_path(p: &Path, base: Option<&Path>) -> String {
    if let Some(b) = base {
        if let Ok(r) = p.strip_prefix(b) {
            return r.to_string_lossy().to_string();
        }
    }
    p.to_string_lossy().to_string()
}

/// Serialise the session. `paths` gives the file used for each source.
pub fn to_text(s: &Session, paths: &BTreeMap<u64, PathBuf>, base: Option<&Path>) -> String {
    let mut o = String::from("AUDEMO-SESSION 1\n");
    o.push_str(&format!("name\t{}\nrate\t{}\nmaster\t{}\nmasterfx\t{}\n", clean(&s.name), s.sample_rate, s.master_db, s.master_fx_on as u8));
    let fx_line = |key: &str, f: &FxSlot| format!("{key}\t{}\t{}\t{}\n", f.effect, f.on as u8, f.params.to_text());
    for f in &s.master_fx {
        o.push_str(&fx_line("mfx", f));
    }
    for (id, src) in &s.sources {
        if let Some(p) = paths.get(id) {
            o.push_str(&format!("source\t{id}\t{}\t{}\n", rel_path(p, base), clean(&src.name)));
        }
    }
    for t in &s.tracks {
        o.push_str(&format!(
            "track\t{}\t{}\t{}\t{}\t{}\t{}\t{},{},{}\t{}\t{}\t{}\t{}\t{}\n",
            t.id,
            clean(&t.name),
            t.volume_db,
            t.pan,
            t.mute as u8,
            t.solo as u8,
            t.colour[0],
            t.colour[1],
            t.colour[2],
            t.height,
            t.show_env as u8,
            t.is_bus() as u8,
            t.fx_on as u8,
            t.output.unwrap_or(0)
        ));
        for f in &t.fx {
            o.push_str(&fx_line("fx", f));
        }
        for sd in &t.sends {
            o.push_str(&format!("send\t{}\t{}\t{}\n", sd.bus, sd.level_db, sd.pre as u8));
        }
        if !t.vol_env.is_empty() {
            o.push_str(&format!("venv\t{}\n", t.vol_env.to_text()));
        }
        if !t.pan_env.is_empty() {
            o.push_str(&format!("penv\t{}\n", t.pan_env.to_text()));
        }
        for c in &t.clips {
            o.push_str(&format!(
                "clip\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                c.id,
                c.source,
                c.start,
                c.offset,
                c.len,
                c.gain_db,
                c.fade_in,
                c.fade_out,
                c.mute as u8,
                clean(&c.name),
                c.fade_in_curve,
                c.fade_out_curve,
                c.fade_in_cos as u8,
                c.fade_out_cos as u8
            ));
        }
    }
    o
}

/// A parsed session file: the session (sources empty) and the files to load.
pub struct ParsedSession {
    pub session: Session,
    /// (source id, file, name)
    pub sources: Vec<(u64, PathBuf, String)>,
}

/// Parse a session file. `effects` (the effect registry) is used to read
/// effects-rack settings.
pub fn from_text(id: u64, text: &str, base: Option<&Path>, effects: &[EffectDef]) -> Result<ParsedSession, String> {
    let mut lines = text.lines();
    if lines.next().map(|l| l.trim()) != Some("AUDEMO-SESSION 1") {
        return Err("Not an Audemo session file.".into());
    }
    let mut s = Session::new(id, "Untitled Session".into(), 48000, 0);
    let mut sources = Vec::new();
    let mut max_id = 1u64;
    let bad = |l: &str| format!("Unreadable line in session file: {l}");
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        let num = |i: usize| -> Result<f64, String> { f.get(i).and_then(|v| v.trim().parse::<f64>().ok()).ok_or_else(|| bad(line)) };
        match f[0] {
            "name" => s.name = f.get(1).unwrap_or(&"").to_string(),
            "rate" => s.sample_rate = num(1)? as u32,
            "master" => s.master_db = num(1)? as f32,
            "source" => {
                let sid = num(1)? as u64;
                let p = PathBuf::from(f.get(2).ok_or_else(|| bad(line))?);
                let p = match base {
                    Some(b) if p.is_relative() => b.join(p),
                    _ => p,
                };
                let name = f.get(3).unwrap_or(&"").to_string();
                max_id = max_id.max(sid);
                sources.push((sid, p, name));
            }
            "track" => {
                let tid = num(1)? as u64;
                let mut t = Track::new(tid, f.get(2).unwrap_or(&"Track").to_string(), [0x3f, 0xdc, 0x9b]);
                t.volume_db = num(3)? as f32;
                t.pan = num(4)? as f32;
                t.mute = num(5)? != 0.0;
                t.solo = num(6)? != 0.0;
                if let Some(c) = f.get(7) {
                    let v: Vec<u8> = c.split(',').filter_map(|x| x.parse().ok()).collect();
                    if v.len() == 3 {
                        t.colour = [v[0], v[1], v[2]];
                    }
                }
                if let Ok(h) = num(8) {
                    t.height = (h as f32).clamp(40.0, 400.0);
                }
                if let Ok(v) = num(9) {
                    t.show_env = v != 0.0;
                }
                if num(10).map(|v| v != 0.0).unwrap_or(false) {
                    t.kind = TrackKind::Bus;
                }
                t.fx_on = num(11).map(|v| v != 0.0).unwrap_or(true);
                t.output = num(12).ok().map(|v| v as u64).filter(|v| *v != 0);
                max_id = max_id.max(tid);
                s.tracks.push(t);
            }
            "clip" => {
                let t = s.tracks.last_mut().ok_or_else(|| bad(line))?;
                let c = Clip {
                    id: num(1)? as u64,
                    source: num(2)? as u64,
                    start: num(3)? as usize,
                    offset: num(4)? as usize,
                    len: num(5)? as usize,
                    gain_db: num(6)? as f32,
                    fade_in: num(7)? as usize,
                    fade_out: num(8)? as usize,
                    mute: num(9)? != 0.0,
                    name: f.get(10).unwrap_or(&"").to_string(),
                    fade_in_curve: num(11).unwrap_or(0.0).clamp(-1.0, 1.0) as f32,
                    fade_out_curve: num(12).unwrap_or(0.0).clamp(-1.0, 1.0) as f32,
                    fade_in_cos: num(13).map(|v| v != 0.0).unwrap_or(false),
                    fade_out_cos: num(14).map(|v| v != 0.0).unwrap_or(false),
                };
                let mut c = c;
                c.len = c.len.max(1);
                c.fade_in = c.fade_in.min(c.len);
                c.fade_out = c.fade_out.min(c.len - c.fade_in);
                max_id = max_id.max(c.id);
                t.clips.push(c);
            }
            "venv" | "penv" => {
                let t = s.tracks.last_mut().ok_or_else(|| bad(line))?;
                let mut e = Envelope::parse(f.get(1).unwrap_or(&""));
                for p in e.points.iter_mut() {
                    p.1 = if f[0] == "venv" { p.1.clamp(VOL_ENV_MIN, VOL_ENV_MAX) } else { p.1.clamp(-100.0, 100.0) };
                }
                if f[0] == "venv" {
                    t.vol_env = e;
                } else {
                    t.pan_env = e;
                }
            }
            "masterfx" => s.master_fx_on = num(1).map(|v| v != 0.0).unwrap_or(true),
            "fx" | "mfx" => {
                // Built-in effects this version doesn't know are skipped;
                // plug-ins that can't load are kept as placeholders.
                let sr = s.sample_rate;
                let on = f.get(2).map(|v| *v != "0").unwrap_or(true);
                let text = f.get(3).unwrap_or(&"");
                let eid = f.get(1).copied().unwrap_or("");
                let def = effects.iter().find(|e| e.id == eid);
                let mut slot = def.and_then(|def| FxSlot::new(def.id, Params::from_text(&def.params, text), sr));
                if slot.is_none() && crate::plugin::split_id(eid).is_some() {
                    let id: &'static str = def.map(|d| d.id).unwrap_or_else(|| Box::leak(eid.to_string().into_boxed_str()));
                    slot = Some(FxSlot::placeholder(id, Params::from_text(&crate::plugin::param_defs(""), text)));
                }
                if let Some(sl) = slot.as_mut() {
                    sl.on = on;
                }
                if let Some(slot) = slot {
                    if f[0] == "mfx" {
                        s.master_fx.push(slot);
                    } else if let Some(t) = s.tracks.last_mut() {
                        t.fx.push(slot);
                    }
                }
            }
            "send" => {
                let t = s.tracks.last_mut().ok_or_else(|| bad(line))?;
                t.sends.push(Send { bus: num(1)? as u64, level_db: num(2)? as f32, pre: num(3).map(|v| v != 0.0).unwrap_or(false) });
            }
            "" => {}
            _ => {} // unknown keys from newer versions are ignored
        }
    }
    s.reserve_ids(max_id);
    s.view_end = s.max_span().min(s.sample_rate as f64 * 60.0).max(s.end() as f64 * 1.05);
    Ok(ParsedSession { session: s, sources })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with(audio: Vec<Vec<f32>>) -> (Session, u64) {
        let mut s = Session::new(1, "Test".into(), 1000, 2);
        let src = s.add_source("tone".into(), Arc::new(audio));
        (s, src)
    }

    #[test]
    fn mix_places_clip_with_gain_and_pan() {
        let (mut s, src) = session_with(vec![vec![0.5; 100]]);
        let c = s.insert_clip(0, src, 10).unwrap();
        s.tracks[0].volume_db = -6.0206;
        let m = mix_range(&s.mix_state(), 0, 200);
        assert_eq!(m[0][9], 0.0);
        assert!((m[0][10] - 0.25).abs() < 1e-4 && (m[1][50] - 0.25).abs() < 1e-4);
        assert_eq!(m[0][110], 0.0);
        // Hard right: left silent, right unchanged.
        s.tracks[0].pan = 100.0;
        let m = mix_range(&s.mix_state(), 0, 200);
        assert!(m[0][50].abs() < 1e-6 && (m[1][50] - 0.25).abs() < 1e-4);
        // Mute and solo.
        s.tracks[0].pan = 0.0;
        s.tracks[1].solo = true;
        assert!(mix_range(&s.mix_state(), 0, 200)[0].iter().all(|v| *v == 0.0));
        s.tracks[1].solo = false;
        s.clip_mut(c).unwrap().mute = true;
        assert!(mix_range(&s.mix_state(), 0, 200)[0].iter().all(|v| *v == 0.0));
    }

    #[test]
    fn mix_is_chunk_independent() {
        let audio: Vec<f32> = (0..500).map(|i| (i as f32 * 0.1).sin()).collect();
        let (mut s, src) = session_with(vec![audio.clone(), audio]);
        let c = s.insert_clip(1, src, 37).unwrap();
        s.clip_mut(c).unwrap().fade_in = 40;
        s.clip_mut(c).unwrap().fade_out = 60;
        let st = s.mix_state();
        let whole = mix_range(&st, 0, 700);
        let mut parts = vec![Vec::new(), Vec::new()];
        for (a, b) in [(0, 64), (64, 333), (333, 700)] {
            let p = mix_range(&st, a, b);
            parts[0].extend_from_slice(&p[0]);
            parts[1].extend_from_slice(&p[1]);
        }
        assert_eq!(whole, parts);
        // Fades start and end at silence.
        assert!(whole[0][37].abs() < 1e-6);
        assert!(whole[0][536].abs() < 0.03);
    }

    #[test]
    fn split_trim_move_and_undo() {
        let (mut s, src) = session_with(vec![(0..100).map(|i| i as f32).collect()]);
        let c = s.insert_clip(0, src, 50).unwrap();
        assert_eq!(s.split_at(80), (1, 0));
        let clips = &s.tracks[0].clips;
        assert_eq!(clips.len(), 2);
        let right = clips.iter().find(|x| x.id != c).unwrap().clone();
        assert_eq!((right.start, right.offset, right.len), (80, 30, 70));
        // Mixing the split pair equals the original clip.
        let m = mix_range(&s.mix_state(), 0, 200);
        for t in 50..150 {
            assert_eq!(m[0][t], (t - 50) as f32);
        }
        // Trim the right half's start back past its offset: clamps to source start.
        s.trim_start(right.id, 0);
        let r = s.clip(right.id).unwrap();
        assert_eq!((r.start, r.offset, r.len), (50, 0, 100));
        s.trim_end(right.id, 10_000);
        assert_eq!(s.clip(right.id).unwrap().end(), 150);
        s.move_clip(c, 300, 1);
        assert_eq!(s.find_clip(c), Some((1, 0)));
        assert_eq!(s.undo().as_deref(), Some("Split"));
        assert_eq!(s.tracks[0].clips.len(), 1);
        assert_eq!(s.tracks[0].clips[0].len, 100);
        assert_eq!(s.redo().as_deref(), Some("Split"));
        assert_eq!(s.delete_clips(&[c]), 1);
        assert!(s.find_clip(c).is_none());
    }

    #[test]
    fn split_skips_fades_and_shrinking_source_keeps_fades_valid() {
        let (mut s, src) = session_with(vec![vec![0.5; 100]]);
        let c = s.insert_clip(0, src, 0).unwrap();
        s.clip_mut(c).unwrap().fade_in = 40;
        s.clip_mut(c).unwrap().fade_out = 30;
        assert_eq!(s.split_at(20), (0, 1));
        assert_eq!(s.split_at(80), (0, 1));
        assert_eq!(s.split_at(50), (1, 0));
        // Source shrinks to 10 frames: every clip and fade must fit.
        s.set_source_audio(src, Arc::new(vec![vec![0.1; 10]]), Arc::new(PeakCache::empty()), None, 2);
        for t in &s.tracks {
            for k in &t.clips {
                assert!(k.offset + k.len <= 10 || k.len == 1, "{k:?}");
                assert!(k.fade_in + k.fade_out <= k.len, "{k:?}");
            }
        }
        // A corrupt file with a zero-length clip and oversized fades loads safely.
        let text = "AUDEMO-SESSION 1\nrate\t1000\ntrack\t2\tT\t0\t0\t0\t0\t1,2,3\t96\nclip\t3\t1\t100\t0\t0\t0\t50\t70\t0\tx\n";
        let mut p = from_text(1, text, None, &[]).unwrap();
        let k = p.session.tracks[0].clips[0].clone();
        assert_eq!((k.len, k.fade_in, k.fade_out), (1, 1, 0));
        p.session.trim_start(k.id, 0);
    }

    #[test]
    fn envelope_interpolates_and_automates_the_mix() {
        let mut e = Envelope::default();
        assert_eq!(e.value_at(5), None);
        e.insert(100, -6.0);
        e.insert(0, 0.0);
        assert_eq!(e.points, vec![(0, 0.0), (100, -6.0)]);
        assert_eq!(e.value_at(50), Some(-3.0));
        assert_eq!(e.value_at(500), Some(-6.0));
        e.move_point(1, 0, -6.0); // can't pass its neighbour
        assert_eq!(e.points[1].0, 0);
        assert_eq!(Envelope::parse(&e.to_text()), e);

        let (mut s, src) = session_with(vec![vec![0.5; 200]]);
        s.insert_clip(0, src, 0).unwrap();
        s.tracks[0].vol_env.insert(0, 0.0);
        s.tracks[0].vol_env.insert(100, VOL_ENV_MIN);
        s.tracks[0].pan_env.insert(0, 100.0);
        let mut peaks = [[0.0f32; 2]; 2];
        let mut out = vec![Vec::new(), Vec::new()];
        mix_into(&s.mix_state(), 0, 200, &mut out, &mut peaks, &mut MixScratch::default());
        assert!((out[1][0] - 0.5).abs() < 1e-5 && out[0][0].abs() < 1e-6, "hard right");
        assert!(out[1][150].abs() < 1e-9, "silent after the envelope reaches the bottom");
        assert!((peaks[0][1] - 0.5).abs() < 1e-5 && peaks[1] == [0.0, 0.0]);
    }

    #[test]
    fn overlapping_clips_crossfade() {
        let (mut s, src) = session_with(vec![vec![1.0; 100]]);
        let a = s.insert_clip(0, src, 0).unwrap();
        let b = s.insert_clip(0, src, 60).unwrap();
        let eff = effective_clips(&s.tracks[0].clips);
        let ea = eff.iter().find(|c| c.id == a).unwrap();
        let eb = eff.iter().find(|c| c.id == b).unwrap();
        assert_eq!((ea.fade_out, eb.fade_in), (40, 40));
        // Equal-power crossfade: the sum stays near unity (within the +3 dB bump).
        let m = mix_range(&s.mix_state(), 0, 160);
        for t in 60..100 {
            assert!(m[0][t] >= 0.99 && m[0][t] <= 1.42, "t={t}: {}", m[0][t]);
        }
        assert!((m[0][30] - 1.0).abs() < 1e-6 && (m[0][130] - 1.0).abs() < 1e-6);
        // Session file keeps automation.
        s.tracks[1].vol_env.insert(10, -3.0);
        s.tracks[1].show_env = true;
        let mut paths = BTreeMap::new();
        paths.insert(src, PathBuf::from("/x/tone.wav"));
        let p = from_text(2, &to_text(&s, &paths, None), None, &[]).unwrap();
        assert_eq!(p.session.tracks[1].vol_env, s.tracks[1].vol_env);
        assert!(p.session.tracks[1].show_env);
    }

    #[test]
    fn fade_shapes() {
        // Linear, bent up, bent down, cosine: all start at 0 and end at 1.
        for (c, cos) in [(0.0, false), (0.7, false), (-0.7, false), (0.0, true)] {
            assert!(fade_shape(0.0, c, cos).abs() < 1e-6 && (fade_shape(1.0, c, cos) - 1.0).abs() < 1e-6);
        }
        assert!((fade_shape(0.5, 0.0, false) - 0.5).abs() < 1e-6);
        assert!(fade_shape(0.5, 0.7, false) > 0.75, "up = fast rise");
        assert!(fade_shape(0.5, -0.7, false) < 0.25, "down = slow rise");
        assert!((fade_shape(0.5, 0.0, true) - 0.5).abs() < 1e-6);
        let (mut s, src) = session_with(vec![vec![1.0; 100]]);
        let c = s.insert_clip(0, src, 0).unwrap();
        {
            let k = s.clip_mut(c).unwrap();
            k.fade_out = 50;
            k.fade_out_curve = 1.0;
        }
        // A bent-up fade-out stays loud longer than a linear one.
        let m = mix_range(&s.mix_state(), 0, 100);
        assert!(m[0][75] > 0.85, "{}", m[0][75]);
    }

    #[test]
    fn track_racks_buses_and_sends() {
        let reg = crate::dsp::effects::registry();
        let def = |id: &str| reg.iter().find(|e| e.id == id).unwrap();
        let (mut s, src) = session_with(vec![vec![0.5; 400]]);
        s.insert_clip(0, src, 0).unwrap();
        // Track 0 rack: Amplify -6 dB.
        let mut p = def("amplify").default_params();
        p.set("left", crate::dsp::params::Value::F(-6.0206));
        s.tracks[0].fx.push(FxSlot::new("amplify", p, 1000).unwrap());
        let m = mix_range(&s.mix_state(), 0, 400);
        assert!((m[0][100] - 0.25).abs() < 1e-4, "{}", m[0][100]);
        // Route track 0 to a bus at -6 dB with its own Amplify +6 dB.
        let bi = s.add_bus();
        let bus_id = s.tracks[bi].id;
        s.tracks[0].output = Some(bus_id);
        s.tracks[bi].volume_db = -6.0206;
        let mut p = def("amplify").default_params();
        p.set("left", crate::dsp::params::Value::F(6.0206));
        s.tracks[bi].fx.push(FxSlot::new("amplify", p, 1000).unwrap());
        let m = mix_range(&s.mix_state(), 0, 400);
        assert!((m[0][100] - 0.25).abs() < 1e-4, "{}", m[0][100]);
        // A post-fader send to the bus doubles the signal when the track also goes to the master.
        s.tracks[0].output = None;
        s.tracks[bi].fx.clear();
        s.tracks[bi].volume_db = 0.0;
        s.tracks[0].sends.push(Send { bus: bus_id, level_db: 0.0, pre: false });
        let m = mix_range(&s.mix_state(), 0, 400);
        assert!((m[0][100] - 0.5).abs() < 1e-4, "{}", m[0][100]);
        // Muting the bus leaves only the direct path; clips can't go on a bus.
        s.tracks[bi].mute = true;
        assert!((mix_range(&s.mix_state(), 0, 400)[0][100] - 0.25).abs() < 1e-4);
        assert!(s.insert_clip(bi, src, 0).is_none());
        // Master rack.
        s.tracks[bi].mute = false;
        s.master_fx.push(FxSlot::new("amplify", def("amplify").default_params(), 1000).unwrap());
        // Round trip.
        let mut paths = BTreeMap::new();
        paths.insert(src, PathBuf::from("/x/a.wav"));
        let p2 = from_text(3, &to_text(&s, &paths, None), None, &reg).unwrap().session;
        assert_eq!(p2.tracks, s.tracks);
        assert_eq!(p2.master_fx, s.master_fx);
        // Undo restores rack settings to the live instance too.
        s.push_undo("x");
        let mut p = def("amplify").default_params();
        p.set("left", crate::dsp::params::Value::F(-40.0));
        s.tracks[0].fx[0].set_params(p);
        s.undo();
        let st = s.mix_state();
        let mut out = vec![Vec::new(), Vec::new()];
        mix_into(&st, 0, 400, &mut out, &mut [], &mut MixScratch::default());
        assert!((out[0][100] - 0.5).abs() < 1e-4, "live instance kept the undone setting: {}", out[0][100]);
    }

    #[test]
    fn live_blocks_match_mixdown_with_stateful_effects() {
        let reg = crate::dsp::effects::registry();
        let echo = reg.iter().find(|e| e.id == "echo").unwrap();
        let audio: Vec<f32> = (0..6000).map(|i| ((i as f32) * 0.05).sin() * if i < 500 { 1.0 } else { 0.0 }).collect();
        let (mut s, src) = session_with(vec![audio]);
        s.sample_rate = 8000;
        s.insert_clip(0, src, 100).unwrap();
        let mut p = echo.default_params();
        p.set("delay_ms", crate::dsp::params::Value::F(50.0));
        s.tracks[0].fx.push(FxSlot::new("echo", p, 8000).unwrap());
        let st = s.mix_state();
        let want = mix_range(&st, 0, 7000);
        // The live path: consecutive blocks through the shared instances.
        s.reset_fx();
        let mut scratch = MixScratch::default();
        scratch.reserve(st.tracks.len(), 1024);
        let mut out = vec![Vec::new(), Vec::new()];
        let mut got = vec![Vec::new(), Vec::new()];
        let mut a = 0;
        while a < 7000 {
            let b = (a + 1024).min(7000);
            mix_into(&st, a, b, &mut out, &mut [], &mut scratch);
            got[0].extend_from_slice(&out[0]);
            got[1].extend_from_slice(&out[1]);
            a = b;
        }
        assert_eq!(got, want);
        assert!(want[0][100 + 400 + 600].abs() > 0.01, "echo tail present");
    }

    #[test]
    fn session_text_round_trip() {
        let (mut s, src) = session_with(vec![vec![0.1; 50]]);
        let c = s.insert_clip(1, src, 7).unwrap();
        {
            let k = s.clip_mut(c).unwrap();
            k.gain_db = -3.5;
            k.fade_in = 4;
            k.fade_in_curve = 0.5;
            k.fade_out_cos = true;
            k.name = "Take\t1".into();
        }
        s.tracks[1].pan = -25.0;
        s.tracks[1].mute = true;
        s.master_db = -1.5;
        let base = PathBuf::from("/tmp/proj");
        let mut paths = BTreeMap::new();
        paths.insert(src, base.join("Test Files/tone.wav"));
        let text = to_text(&s, &paths, Some(&base));
        assert!(text.contains("source\t") && text.contains("Test Files/tone.wav"));
        let p = from_text(9, &text, Some(&base), &[]).unwrap();
        assert_eq!(p.session.tracks, s.tracks.iter().map(|t| {
            let mut t = t.clone();
            for c in t.clips.iter_mut() {
                c.name = c.name.replace('\t', " ");
            }
            t
        }).collect::<Vec<_>>());
        assert_eq!(p.session.master_db, -1.5);
        assert_eq!(p.sources, vec![(src, base.join("Test Files/tone.wav"), "tone".to_string())]);
        let mut s2 = p.session;
        assert!(s2.new_id() > c);
        assert!(from_text(1, "nope", None, &[]).is_err());
    }
}
