//! Application state, documents, undo history and the action dispatcher.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::dsp::effects::{self, Ctx, EffectDef, NoiseProfile};
use crate::dsp::params::Params;
use crate::dsp::peaks::PeakCache;
use crate::dsp::resample::{remap_channels, resample_channels};
use crate::dsp::util::{db_to_lin, format_time, slice_range};
use crate::engine::{Buffer, Engine, StreamBuf, BROWSER_TAG, PREVIEW_TAG};
use crate::liverack::{LiveRack, RackSettings};
use crate::io::{self, WavFormat};
use crate::prefs::Prefs;

pub const MAX_UNDO: usize = 60;
pub const SAMPLE_RATES: [u32; 10] = [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 192000];

#[derive(Clone, Debug)]
pub struct Marker {
    pub pos: usize,
    pub name: String,
}

#[derive(Clone)]
pub struct Snapshot {
    pub label: String,
    pub audio: Buffer,
    pub peaks: Arc<PeakCache>,
    pub sample_rate: u32,
    pub sel: Option<(usize, usize)>,
    pub cursor: usize,
    pub markers: Vec<Marker>,
}

pub struct SpecTex {
    pub key: (u64, i64, i64, usize),
    pub tex: egui::TextureHandle,
}

pub struct Document {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub origin: String,
    pub audio: Buffer,
    pub peaks: Arc<PeakCache>,
    pub sample_rate: u32,
    pub source_bits: Option<u32>,
    pub version: u64,
    pub undo: Vec<Snapshot>,
    pub redo: Vec<Snapshot>,
    pub view_start: f64,
    pub view_end: f64,
    pub amp_zoom: f32,
    pub cursor: usize,
    pub sel: Option<(usize, usize)>,
    pub markers: Vec<Marker>,
    pub dirty: bool,
    pub active_ch: Vec<bool>,
    pub spec: Vec<Option<SpecTex>>,
    /// Length (in this file's samples) of a recording in progress.
    pub pending: usize,
    pub recording: bool,
}

impl Document {
    pub fn new(id: u64, name: String, path: Option<PathBuf>, audio: Vec<Vec<f32>>, sample_rate: u32, bits: Option<u32>, origin: &str) -> Self {
        let n_ch = audio.len().max(1);
        let audio = if audio.is_empty() { vec![Vec::new()] } else { audio };
        let peaks = Arc::new(PeakCache::build(&audio));
        let len = audio[0].len();
        Document {
            id,
            name,
            path,
            origin: origin.to_string(),
            audio: Arc::new(audio),
            peaks,
            sample_rate,
            source_bits: bits,
            version: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            view_start: 0.0,
            view_end: (len.max(sample_rate as usize)) as f64,
            amp_zoom: 1.0,
            cursor: 0,
            sel: None,
            markers: Vec::new(),
            dirty: false,
            active_ch: vec![true; n_ch],
            spec: Vec::new(),
            pending: 0,
            recording: false,
        }
    }

    pub fn len(&self) -> usize {
        self.audio.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn n_ch(&self) -> usize {
        self.audio.len()
    }

    pub fn sel_range(&self) -> Option<(usize, usize)> {
        self.sel.filter(|(a, b)| b > a)
    }

    /// Selection, or the whole file when nothing is selected.
    pub fn target_range(&self) -> (usize, usize) {
        self.sel_range().unwrap_or((0, self.len()))
    }

    fn snapshot(&self, label: String) -> Snapshot {
        Snapshot {
            label,
            audio: self.audio.clone(),
            peaks: self.peaks.clone(),
            sample_rate: self.sample_rate,
            sel: self.sel,
            cursor: self.cursor,
            markers: self.markers.clone(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.audio = s.audio;
        self.peaks = s.peaks;
        self.sample_rate = s.sample_rate;
        self.sel = s.sel;
        self.cursor = s.cursor;
        self.markers = s.markers;
        self.version += 1;
        self.dirty = true;
        self.fix_channels();
        self.clamp_view();
    }

    pub fn push_undo(&mut self, label: &str) {
        self.undo.push(self.snapshot(label.to_string()));
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self) -> Option<String> {
        let s = self.undo.pop()?;
        let label = s.label.clone();
        self.redo.push(self.snapshot(label.clone()));
        self.restore(s);
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let s = self.redo.pop()?;
        let label = s.label.clone();
        self.undo.push(self.snapshot(label.clone()));
        self.restore(s);
        Some(label)
    }

    pub fn set_audio(&mut self, audio: Vec<Vec<f32>>) {
        self.peaks = Arc::new(PeakCache::build(&audio));
        self.audio = Arc::new(audio);
        self.version += 1;
        self.dirty = true;
        self.fix_channels();
        self.clamp_view();
    }

    fn fix_channels(&mut self) {
        let n = self.n_ch();
        if self.active_ch.len() != n {
            self.active_ch = vec![true; n];
        }
    }

    pub fn max_span(&self) -> f64 {
        let base = (self.len() + self.pending).max(64) as f64;
        if self.recording {
            // Keep a steady 30 s window while recording into a short file.
            base.max(self.sample_rate as f64 * 30.0)
        } else {
            base
        }
    }

    pub fn clamp_view(&mut self) {
        let len = self.len();
        let max = self.max_span();
        let mut span = (self.view_end - self.view_start).clamp(16.0, max);
        if !span.is_finite() {
            span = max;
        }
        let mut start = self.view_start.max(0.0);
        if start + span > max {
            start = (max - span).max(0.0);
        }
        self.view_start = start;
        self.view_end = start + span;
        self.cursor = self.cursor.min(len);
        if let Some((a, b)) = self.sel {
            let (a, b) = (a.min(len), b.min(len));
            self.sel = if b > a { Some((a, b)) } else { None };
        }
        for m in self.markers.iter_mut() {
            m.pos = m.pos.min(len);
        }
    }

    pub fn zoom(&mut self, factor: f64, anchor: f64) {
        let span = self.view_end - self.view_start;
        let new_span = (span * factor).clamp(16.0, self.max_span());
        let t = if span > 0.0 { (anchor - self.view_start) / span } else { 0.5 };
        self.view_start = anchor - new_span * t;
        self.view_end = self.view_start + new_span;
        self.clamp_view();
    }

    pub fn zoom_full(&mut self) {
        self.view_start = 0.0;
        self.view_end = self.max_span();
    }

    pub fn zoom_to(&mut self, a: usize, b: usize) {
        let pad = ((b - a) as f64 * 0.05).max(8.0);
        self.view_start = a as f64 - pad;
        self.view_end = b as f64 + pad;
        self.clamp_view();
    }

    pub fn display_name(&self) -> String {
        if self.dirty {
            format!("{} *", self.name)
        } else {
            self.name.clone()
        }
    }

    pub fn format_label(&self) -> String {
        let bits = match self.source_bits {
            Some(b) => format!("{b}-bit"),
            None => "32-bit (float)".into(),
        };
        let ch = match self.n_ch() {
            1 => "Mono".to_string(),
            2 => "Stereo".to_string(),
            n => format!("{n} ch"),
        };
        format!("{} Hz • {} • {}", self.sample_rate, bits, ch)
    }

    /// Shift markers after `at` by `delta` samples (for inserts/deletes).
    pub fn shift_markers(&mut self, at: usize, removed: usize, inserted: usize) {
        self.markers.retain(|m| !(m.pos > at && m.pos < at + removed));
        for m in self.markers.iter_mut() {
            if m.pos >= at + removed {
                m.pos = m.pos - removed + inserted;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Selection,
    Hand,
}

/// Panels in the workspace, grouped into tabbed frames like Audition's
/// default workspace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Files,
    Favorites,
    MediaBrowser,
    EffectsRack,
    Markers,
    Properties,
    History,
    MatchLoudness,
    Levels,
    FrequencyAnalysis,
    PhaseMeter,
}

impl Panel {
    pub const TOP: [Panel; 2] = [Panel::Files, Panel::Favorites];
    pub const MIDDLE: [Panel; 4] = [Panel::MediaBrowser, Panel::EffectsRack, Panel::Markers, Panel::Properties];
    pub const BOTTOM: [Panel; 2] = [Panel::History, Panel::MatchLoudness];
    pub const METERS: [Panel; 3] = [Panel::Levels, Panel::FrequencyAnalysis, Panel::PhaseMeter];
    /// Every panel, in Window-menu order.
    pub const ALL: [Panel; 11] = [
        Panel::EffectsRack,
        Panel::Favorites,
        Panel::Files,
        Panel::FrequencyAnalysis,
        Panel::History,
        Panel::Levels,
        Panel::Markers,
        Panel::MatchLoudness,
        Panel::MediaBrowser,
        Panel::PhaseMeter,
        Panel::Properties,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Panel::Files => "Files",
            Panel::Favorites => "Favorites",
            Panel::MediaBrowser => "Media Browser",
            Panel::EffectsRack => "Effects Rack",
            Panel::Markers => "Markers",
            Panel::Properties => "Properties",
            Panel::History => "History",
            Panel::MatchLoudness => "Match Loudness",
            Panel::Levels => "Levels",
            Panel::FrequencyAnalysis => "Frequency Analysis",
            Panel::PhaseMeter => "Phase Meter",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RackSlot {
    pub idx: usize,
    pub params: Params,
    pub on: bool,
}

pub const RACK_SLOTS: usize = 16;

/// Where a recording in progress will land.
pub struct RecTarget {
    pub doc_id: u64,
    pub range: (usize, usize),
}

#[derive(Clone, Debug)]
pub enum LoadIntent {
    Open,
    Append(u64),
    Audition,
}

pub struct EffectDialog {
    pub idx: usize,
    pub params: Params,
    pub preset: String,
    pub previewing: bool,
    pub bypass: bool,
    pub rendered: Option<Params>,
    pub last_change: Instant,
    pub drag_handle: Option<usize>,
    pub dry: Option<Buffer>,
    pub wet: Option<Buffer>,
    pub error: Option<String>,
    /// Editing a slot of the Effects Rack rather than applying directly.
    pub rack_slot: Option<usize>,
}

pub enum Dialog {
    Effect(EffectDialog),
    /// New Audio File; `then_record` starts recording into it on OK.
    NewFile { name: String, rate: u32, channels: usize, bits: Option<u32>, seconds: f32, then_record: bool },
    Export { format: WavFormat, dither: bool, path: Option<PathBuf>, selection: bool },
    Preferences { input: Option<String>, output: Option<String>, inputs: Vec<String>, outputs: Vec<String> },
    MixPaste { mode: usize, clip_db: f32, orig_db: f32 },
    Convert { rate: u32, channels: usize },
    Shortcuts,
    About,
    Message { title: String, text: String },
    ConfirmClose { doc: usize },
    ConfirmQuit,
    AmplitudeStats { title: String, rx: Option<Receiver<crate::dsp::analysis::AmplitudeStats>>, stats: Option<crate::dsp::analysis::AmplitudeStats> },
}

pub enum JobKind {
    /// Replace `range` of a document with the result.
    Edit { doc_id: u64, range: (usize, usize), active: Vec<bool>, new_rate: Option<u32>, select: bool },
    /// Render an effect preview.
    Preview { params: Params },
}

pub struct Job {
    pub rx: Receiver<Result<Vec<Vec<f32>>, String>>,
    pub kind: JobKind,
    pub label: String,
    pub started: Instant,
}

pub enum Action {
    New,
    Open,
    OpenPaths(Vec<PathBuf>),
    Save,
    SaveAs,
    Close(usize),
    ForceClose(usize),
    Quit,
    Undo,
    Redo,
    JumpHistory(usize),
    Cut,
    Copy,
    Paste,
    PasteNew,
    Delete,
    Crop,
    SelectAll,
    Deselect,
    OpenEffect(usize),
    ApplyEffect(usize, Params),
    ApplyFavorite(&'static str, &'static str),
    ApplyFade { fade_in: bool, len: usize },
    ApplyGain(f32),
    CaptureNoise,
    Convert(u32, usize),
    AddMarker,
    PlayToggle,
    Stop,
    Record,
    ToStart,
    ToEnd,
    PrevMarker,
    NextMarker,
    ZoomIn,
    ZoomOut,
    ZoomFull,
    ZoomSel,
    AmpIn,
    AmpOut,
    SetCursor(usize),
    Status(String),
    CloseAll,
    SaveAll,
    SaveSelectionAs,
    CopyToNew,
    MixPaste { mode: usize, clip_db: f32, orig_db: f32 },
    RepeatLast,
    OpenAppend,
    OpenRecent(PathBuf),
    Preferences,
    ApplyRack,
    Audition(PathBuf),
    Pause,
    /// Move the playhead by this many seconds (Rewind / Fast Forward).
    Nudge(f64),
    ClearHistory,
    AmplitudeStatistics,
}

pub const FAVORITES: &[(&str, &str, &str)] = &[
    ("Fade In", "fade_in", ""),
    ("Fade Out", "fade_out", ""),
    ("Normalize to -0.1 dB", "normalize", "Normalize to -0.1 dB"),
    ("Normalize to -3 dB", "normalize", "Normalize to -3 dB"),
    ("Hard Limit to -1 dB", "hard_limiter", "Limit to -1 dB"),
    ("Remove 50 Hz Hum", "dehummer", "50 Hz, 4 harmonics"),
    ("Remove 60 Hz Hum", "dehummer", "60 Hz, 6 harmonics"),
    ("Repair Clipping", "declipper", ""),
    ("Vocal Presence EQ", "parametric_eq", "Vocal Presence"),
    ("Remove Rumble", "parametric_eq", "Remove Rumble"),
    ("Broadcast Compression", "dynamics", "Broadcast"),
];

pub struct App {
    pub docs: Vec<Document>,
    pub active: Option<usize>,
    next_id: u64,
    pub engine: Engine,
    pub effects: Arc<Vec<EffectDef>>,
    pub clipboard: Option<(Vec<Vec<f32>>, u32)>,
    pub noise_print: Option<NoiseProfile>,
    pub dialog: Option<Dialog>,
    pub job: Option<Job>,
    pub loads: Vec<(PathBuf, LoadIntent, Receiver<Result<io::Decoded, String>>)>,
    pub status: String,
    pub status_at: Instant,
    pub show_spectral: bool,
    pub show_left: bool,
    pub show_bottom: bool,
    pub show_meters: bool,
    pub tab_top: Panel,
    pub tab_mid: Panel,
    pub tab_bot: Panel,
    pub tab_meter: Panel,
    pub analysis: crate::analysis_ui::AnalysisState,
    pub prefs: Prefs,
    pub rack: Vec<RackSlot>,
    pub rack_mix: f32,
    pub rack_in_db: f32,
    pub rack_out_db: f32,
    /// Master power: transport playback is heard through the rack.
    pub rack_on: bool,
    pub live: Option<LiveRack>,
    /// Effects Rack "Process": false = selection only, true = entire file.
    pub rack_entire: bool,
    pub rack_version: u64,
    pub rack_changed: Instant,
    pub last_effect: Option<(usize, Params)>,
    pub rec_target: Option<RecTarget>,
    pub browser_dir: PathBuf,
    pub browser_entries: Option<(PathBuf, Vec<(PathBuf, bool, u64)>)>,
    pub browser_selected: Option<PathBuf>,
    /// (document id, position) of a paused playback.
    pub paused: Option<(u64, f64)>,
    pub free_cache: std::cell::Cell<Option<(Instant, Option<u64>)>>,
    #[cfg(any(target_os = "macos", audemo_check_menu))]
    pub native_menu: Option<crate::macmenu::NativeMenu>,
    pub tool: Tool,
    pub looping: bool,
    pub follow: bool,
    pub meter_db: [f32; 2],
    pub meter_hold: [(f32, Instant); 2],
    pub actions: Vec<Action>,
    pub hud_gain: f32,
    pub drag_anchor: Option<usize>,
    pub hand_anchor: Option<(f32, f64, f64)>,
    pub fade_drag: Option<(bool, usize)>,
    pub play_origin: usize,
    pub allow_quit: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Vec<PathBuf>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        let prefs = Prefs::load();
        let engine = Engine::new(prefs.output_device.clone(), prefs.input_device.clone());
        let browser_dir = prefs
            .browser_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(crate::prefs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut app = App {
            docs: Vec::new(),
            active: None,
            next_id: 1,
            status: match &engine.error {
                Some(e) => format!("Audio output unavailable: {e}"),
                None => format!("Output: {} @ {} Hz", engine.device_name, engine.out_rate),
            },
            engine,
            effects: Arc::new(effects::registry()),
            clipboard: None,
            noise_print: None,
            dialog: None,
            job: None,
            loads: Vec::new(),
            status_at: Instant::now(),
            show_spectral: false,
            show_left: true,
            show_bottom: true,
            show_meters: true,
            tab_top: Panel::Files,
            tab_mid: Panel::EffectsRack,
            tab_bot: Panel::History,
            tab_meter: Panel::Levels,
            analysis: Default::default(),
            prefs,
            rack: Vec::new(),
            rack_mix: 100.0,
            rack_in_db: 0.0,
            rack_out_db: 0.0,
            rack_on: false,
            live: None,
            rack_entire: false,
            rack_version: 1,
            rack_changed: Instant::now(),
            last_effect: None,
            rec_target: None,
            browser_dir,
            browser_entries: None,
            browser_selected: None,
            paused: None,
            free_cache: std::cell::Cell::new(None),
            #[cfg(any(target_os = "macos", audemo_check_menu))]
            native_menu: None,
            tool: Tool::Selection,
            looping: false,
            follow: true,
            meter_db: [-120.0; 2],
            meter_hold: [(-120.0, Instant::now()); 2],
            actions: Vec::new(),
            hud_gain: 0.0,
            drag_anchor: None,
            hand_anchor: None,
            fade_drag: None,
            play_origin: 0,
            allow_quit: false,
        };
        if !open.is_empty() {
            app.actions.push(Action::OpenPaths(open));
        }
        #[cfg(any(target_os = "macos", audemo_check_menu))]
        app.init_native_menu();
        app
    }

    pub fn doc(&self) -> Option<&Document> {
        self.active.and_then(|i| self.docs.get(i))
    }

    pub fn doc_mut(&mut self) -> Option<&mut Document> {
        match self.active {
            Some(i) => self.docs.get_mut(i),
            None => None,
        }
    }

    /// Free space on the drive holding the active file (cached for 10 s).
    pub fn free_space(&self) -> Option<u64> {
        if let Some((at, v)) = self.free_cache.get() {
            if at.elapsed() < Duration::from_secs(10) {
                return v;
            }
        }
        let dir = self
            .doc()
            .and_then(|d| d.path.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())))
            .or_else(crate::prefs::home_dir)?;
        let v = crate::prefs::disk_free(&dir);
        self.free_cache.set(Some((Instant::now(), v)));
        v
    }

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_at = Instant::now();
    }

    pub fn busy(&self) -> bool {
        matches!(self.job, Some(Job { kind: JobKind::Edit { .. }, .. }))
    }

    fn add_doc(&mut self, doc: Document) {
        self.docs.push(doc);
        self.active = Some(self.docs.len() - 1);
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn find_effect(&self, id: &str) -> Option<usize> {
        self.effects.iter().position(|e| e.id == id)
    }

    /// Playhead if this document is playing, otherwise its cursor.
    pub fn display_pos(&self) -> f64 {
        match self.doc() {
            Some(d) => {
                let st = self.engine.status();
                if st.playing && st.tag == d.id {
                    st.pos
                } else {
                    d.cursor as f64
                }
            }
            None => 0.0,
        }
    }

    // ---------------------------------------------------------------- frame

    pub fn frame(&mut self, ctx: &egui::Context) {
        #[cfg(any(target_os = "macos", audemo_check_menu))]
        self.poll_native_menu(ctx);
        self.poll_loads();
        self.poll_job();
        self.handle_dropped_files(ctx);
        self.handle_shortcuts(ctx);
        self.update_meters(ctx);
        self.follow_playhead();
        self.track_recording();
        self.maybe_render_preview();
        self.sync_live_rack();
        self.poll_analysis();
    }

    /// Keep the target file's live length and view in step with the input.
    fn track_recording(&mut self) {
        self.engine.poll_recording();
        let Some(t) = &self.rec_target else { return };
        let Some((rate, _)) = self.engine.recording_format() else { return };
        let frames = self.engine.recorded_frames();
        let doc_id = t.doc_id;
        let start = t.range.0;
        let follow = self.follow;
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == doc_id) {
            d.recording = true;
            d.pending = (frames as f64 * d.sample_rate as f64 / rate as f64) as usize;
            let head = (start + d.pending) as f64;
            let span = d.view_end - d.view_start;
            if follow && head > d.view_end - span * 0.05 {
                d.view_end = d.max_span().min(head + span * 0.25).max(span);
                d.view_start = (d.view_end - span).max(0.0);
            }
        }
    }

    pub fn finish_frame(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.perform(a, ctx);
        }
        let st = self.engine.status();
        if st.playing || self.job.is_some() || self.engine.is_recording() || !self.loads.is_empty() || self.meter_db.iter().any(|d| *d > -100.0) {
            ctx.request_repaint();
        } else if matches!(self.dialog, Some(Dialog::Effect(ref d)) if d.previewing) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_quit && self.docs.iter().any(|d| d.dirty) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::ConfirmQuit);
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.actions.push(Action::OpenPaths(dropped));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut as KS, Modifiers as M};
        let cmd = M::COMMAND;
        let cs = M::COMMAND | M::SHIFT;
        let mut acts = Vec::new();
        ctx.input_mut(|i| {
            // Longer modifier combos first: egui matches extra Shift loosely.
            if i.consume_shortcut(&KS::new(cs, Key::S)) { acts.push(Action::SaveAs); }
            if i.consume_shortcut(&KS::new(cmd, Key::N)) { acts.push(Action::New); }
            if i.consume_shortcut(&KS::new(cmd, Key::O)) { acts.push(Action::Open); }
            if i.consume_shortcut(&KS::new(cmd, Key::S)) { acts.push(Action::Save); }
            if i.consume_shortcut(&KS::new(cmd, Key::Q)) { acts.push(Action::Quit); }
        });
        let typing = ctx.wants_keyboard_input();
        let modal = matches!(self.dialog, Some(ref d) if !matches!(d, Dialog::Effect(_)));
        if !typing {
            ctx.input_mut(|i| {
                if i.consume_shortcut(&KS::new(cs, Key::Z)) { acts.push(Action::Redo); }
                if i.consume_shortcut(&KS::new(cmd | M::ALT, Key::V)) { acts.push(Action::PasteNew); }
                if i.consume_shortcut(&KS::new(cs, Key::V)) {
                    acts.push(Action::Status("__mix_paste".into()));
                }
                if i.consume_key(M::ALT | M::SHIFT, Key::C) { acts.push(Action::CopyToNew); }
                if i.consume_key(M::SHIFT, Key::R) { acts.push(Action::RepeatLast); }
                if i.consume_shortcut(&KS::new(cmd, Key::Z)) { acts.push(Action::Undo); }
                if i.consume_shortcut(&KS::new(cmd, Key::Y)) { acts.push(Action::Redo); }
                if i.consume_shortcut(&KS::new(cmd, Key::X)) { acts.push(Action::Cut); }
                if i.consume_shortcut(&KS::new(cmd, Key::C)) { acts.push(Action::Copy); }
                if i.consume_shortcut(&KS::new(cmd, Key::V)) { acts.push(Action::Paste); }
                if i.consume_shortcut(&KS::new(cmd, Key::A)) { acts.push(Action::SelectAll); }
                if i.consume_shortcut(&KS::new(cmd, Key::T)) { acts.push(Action::Crop); }
                if i.consume_shortcut(&KS::new(cmd, Key::W)) { acts.push(Action::Close(usize::MAX)); }
                if i.consume_key(M::SHIFT, Key::Space) { acts.push(Action::Record); }
                if i.consume_key(M::SHIFT, Key::P) { acts.push(Action::CaptureNoise); }
                if i.consume_key(M::SHIFT, Key::D) { acts.push(Action::Status("__toggle_spectral".into())); }
                if i.consume_key(M::NONE, Key::Space) { acts.push(Action::PlayToggle); }
                if i.consume_key(M::NONE, Key::Delete) || i.consume_key(M::NONE, Key::Backspace) { acts.push(Action::Delete); }
                if i.consume_key(M::NONE, Key::Escape) { acts.push(Action::Deselect); }
                if i.consume_key(M::NONE, Key::Home) { acts.push(Action::ToStart); }
                if i.consume_key(M::NONE, Key::End) { acts.push(Action::ToEnd); }
                if i.consume_key(M::NONE, Key::M) { acts.push(Action::AddMarker); }
                if i.consume_key(M::NONE, Key::Equals) || i.consume_key(M::NONE, Key::Plus) { acts.push(Action::ZoomIn); }
                if i.consume_key(M::NONE, Key::Minus) { acts.push(Action::ZoomOut); }
                if i.consume_key(M::NONE, Key::Backslash) { acts.push(Action::ZoomFull); }
                if i.consume_key(M::NONE, Key::ArrowLeft) { acts.push(Action::PrevMarker); }
                if i.consume_key(M::NONE, Key::ArrowRight) { acts.push(Action::NextMarker); }
            });
        }
        for a in acts {
            // In an effect dialog, Space toggles the preview and Esc closes it.
            if let Some(Dialog::Effect(d)) = &mut self.dialog {
                match a {
                    Action::PlayToggle => {
                        d.previewing = !d.previewing;
                        if !d.previewing {
                            self.engine.stop();
                        }
                        d.rendered = None;
                        continue;
                    }
                    Action::Deselect => {
                        self.close_effect_dialog();
                        continue;
                    }
                    _ => {}
                }
            }
            if modal && !matches!(a, Action::Quit) {
                if matches!(a, Action::Deselect) {
                    self.dialog = None;
                }
                continue;
            }
            if let Action::Status(s) = &a {
                if s == "__toggle_spectral" {
                    self.show_spectral = !self.show_spectral;
                    continue;
                }
                if s == "__mix_paste" {
                    self.dialog = Some(Dialog::MixPaste { mode: 1, clip_db: 0.0, orig_db: 0.0 });
                    continue;
                }
            }
            self.actions.push(a);
        }
    }

    fn update_meters(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let peaks = self.engine.take_peaks();
        for c in 0..2 {
            let db = crate::dsp::util::lin_to_db(peaks[c]).max(-120.0);
            let fall = self.meter_db[c] - 24.0 * dt;
            self.meter_db[c] = db.max(fall).max(-120.0);
            let (hold, at) = self.meter_hold[c];
            if db >= hold || at.elapsed() > Duration::from_millis(1500) {
                self.meter_hold[c] = (db.max(if at.elapsed() > Duration::from_millis(1500) { -120.0 } else { hold }), Instant::now());
            }
        }
    }

    fn follow_playhead(&mut self) {
        let st = self.engine.status();
        if !self.follow || !st.playing {
            return;
        }
        if let Some(d) = self.doc_mut() {
            if st.tag != d.id {
                return;
            }
            let span = d.view_end - d.view_start;
            if st.pos > d.view_end || st.pos < d.view_start {
                d.view_start = st.pos - span * 0.02;
                d.view_end = d.view_start + span;
                d.clamp_view();
            }
        }
    }

    // ---------------------------------------------------------------- loading

    fn poll_loads(&mut self) {
        let mut done = Vec::new();
        for (i, (_, _, rx)) in self.loads.iter().enumerate() {
            if let Ok(res) = rx.try_recv() {
                done.push((i, res));
            }
        }
        for (i, res) in done.into_iter().rev() {
            let (path, intent, _) = self.loads.remove(i);
            let dec = match res {
                Ok(d) => d,
                Err(e) => {
                    if !matches!(intent, LoadIntent::Audition) {
                        self.dialog = Some(Dialog::Message { title: "Couldn't open file".into(), text: format!("{}\n\n{e}", path.display()) });
                    }
                    continue;
                }
            };
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
            match intent {
                LoadIntent::Open => {
                    let id = self.new_id();
                    let len = dec.channels[0].len();
                    let doc = Document::new(id, name.clone(), Some(path.clone()), dec.channels, dec.sample_rate, dec.bits, "Open");
                    self.add_doc(doc);
                    self.prefs.add_recent(path.clone());
                    self.prefs.save();
                    self.set_status(format!("Opened {name} ({})", format_time(len as f64, dec.sample_rate)));
                }
                LoadIntent::Append(doc_id) => {
                    if let Some(d) = self.docs.iter_mut().find(|d| d.id == doc_id) {
                        let mut audio = dec.channels;
                        if dec.sample_rate != d.sample_rate {
                            audio = resample_channels(&audio, d.sample_rate as f64 / dec.sample_rate as f64);
                        }
                        let audio = remap_channels(&audio, d.n_ch());
                        let end = d.len();
                        let active = vec![true; d.n_ch()];
                        apply_edit(d, "Append", (end, end), &active, audio, None, true);
                        self.set_status(format!("Appended {name}"));
                    }
                }
                LoadIntent::Audition => {
                    if self.browser_selected.as_deref() == Some(path.as_path()) {
                        let len = dec.channels[0].len() as f64;
                        self.engine.play(Arc::new(dec.channels), dec.sample_rate, 0.0, len, false, BROWSER_TAG);
                    }
                }
            }
        }
    }

    fn load_async(&mut self, p: PathBuf, intent: LoadIntent) {
        let (tx, rx) = channel();
        let path = p.clone();
        std::thread::spawn(move || {
            let _ = tx.send(io::load(&path));
        });
        if !matches!(intent, LoadIntent::Audition) {
            self.set_status(format!("Opening {}…", p.display()));
        }
        self.loads.push((p, intent, rx));
    }

    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(p.as_path())) {
                self.active = Some(i);
                continue;
            }
            self.load_async(p, LoadIntent::Open);
        }
    }

    // ---------------------------------------------------------------- jobs

    pub(crate) fn spawn_job(&mut self, label: String, kind: JobKind, f: impl FnOnce() -> Result<Vec<Vec<f32>>, String> + Send + 'static) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .unwrap_or_else(|_| Err("The effect failed unexpectedly.".into()));
            let _ = tx.send(r);
        });
        self.job = Some(Job { rx, kind, label, started: Instant::now() });
    }

    fn poll_job(&mut self) {
        let res = match &self.job {
            Some(j) => match j.rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => Err("The background task stopped.".into()),
            },
            None => return,
        };
        let job = self.job.take().unwrap();
        match job.kind {
            JobKind::Preview { params } => {
                if let Some(Dialog::Effect(d)) = &mut self.dialog {
                    match res {
                        Ok(out) => {
                            let sr = self.docs.get(self.active.unwrap_or(0)).map(|d| d.sample_rate).unwrap_or(48000);
                            let len = out.first().map(|c| c.len()).unwrap_or(0) as f64;
                            let wet: Buffer = Arc::new(out);
                            d.wet = Some(wet.clone());
                            d.rendered = Some(params);
                            d.error = None;
                            if d.previewing {
                                let buf = if d.bypass { d.dry.clone().unwrap_or(wet) } else { wet };
                                if self.engine.is_playing_tag(PREVIEW_TAG) {
                                    self.engine.replace_buffer(buf, sr, 0.0, len);
                                } else {
                                    self.engine.play(buf, sr, 0.0, len, true, PREVIEW_TAG);
                                }
                            }
                        }
                        Err(e) => {
                            d.error = Some(e);
                            d.rendered = Some(params);
                        }
                    }
                }
            }
            JobKind::Edit { doc_id, range, active, new_rate, select } => {
                let out = match res {
                    Ok(o) => o,
                    Err(e) => {
                        self.dialog = Some(Dialog::Message { title: job.label.clone(), text: e });
                        return;
                    }
                };
                let Some(di) = self.docs.iter().position(|d| d.id == doc_id) else { return };
                let doc = &mut self.docs[di];
                let elapsed = job.started.elapsed().as_secs_f32();
                apply_edit(doc, &job.label, range, &active, out, new_rate, select);
                self.set_status(format!("{} applied in {:.2} s", job.label, elapsed));
            }
        }
    }

    fn maybe_render_preview(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(Dialog::Effect(d)) = &self.dialog else { return };
        if !d.previewing || d.rendered.as_ref() == Some(&d.params) || d.last_change.elapsed() < Duration::from_millis(220) {
            return;
        }
        let Some(doc) = self.doc() else { return };
        let def_idx = d.idx;
        let params = d.params.clone();
        let generator = self.effects[def_idx].generator;
        let (a, b) = doc.target_range();
        let b = b.min(a + doc.sample_rate as usize * 20);
        let input = slice_range(&doc.audio, a, b);
        let sr = doc.sample_rate;
        let n_ch = doc.n_ch();
        let np = self.noise_print.clone();
        let effects = self.effects.clone();
        let dry: Buffer = Arc::new(input.clone());
        if let Some(Dialog::Effect(d)) = &mut self.dialog {
            d.dry = Some(dry);
        }
        let p2 = params.clone();
        self.spawn_job("Preview".into(), JobKind::Preview { params }, move || {
            let ctx = Ctx { sample_rate: sr, channels: n_ch, noise_print: np.as_ref() };
            let input = if generator { vec![Vec::new(); n_ch] } else { input };
            let mut out = effects[def_idx].run(&input, &ctx, &p2)?;
            // Keep previews short for generators too.
            for c in out.iter_mut() {
                c.truncate(sr as usize * 20);
            }
            Ok(out)
        });
    }

    fn rack_inputs(&self) -> Option<(Vec<(usize, Params)>, (usize, usize), Vec<Vec<f32>>, u32, usize)> {
        let doc = self.doc()?;
        let slots = self.rack.iter().filter(|s| s.on).map(|s| (s.idx, s.params.clone())).collect();
        let (a, b) = if self.rack_entire { (0, doc.len()) } else { doc.target_range() };
        Some((slots, (a, b), Vec::new(), doc.sample_rate, doc.n_ch()))
    }

    fn rack_settings(&self) -> RackSettings {
        RackSettings {
            slots: self.rack.iter().filter(|s| s.on).map(|s| (s.idx, s.params.clone())).collect(),
            mix: self.rack_mix,
            in_db: self.rack_in_db,
            out_db: self.rack_out_db,
        }
    }

    /// The rendered stream for document `id`, when the rack is live for it.
    pub fn live_stream(&self, id: u64) -> Option<Arc<StreamBuf>> {
        self.live.as_ref().filter(|l| l.doc_id == id).map(|l| l.sb.clone())
    }

    /// Keep the real-time rack renderer in step with the power switch, the
    /// active file and the rack settings.
    fn sync_live_rack(&mut self) {
        let settings = self.rack_settings();
        let want = self.rack_on && !settings.slots.is_empty() && !self.engine.is_recording();
        let target = self.doc().filter(|d| d.len() > 0).map(|d| (d.id, d.audio.clone(), d.sample_rate, d.cursor));
        let st = self.engine.status();
        let Some((id, audio, sr, cursor)) = target.filter(|_| want) else {
            if self.live.take().is_some() && self.engine.has_stream() {
                self.engine.set_stream(None);
            }
            return;
        };
        let stale = match &self.live {
            Some(l) => l.doc_id != id || !Arc::ptr_eq(&l.audio, &audio),
            None => true,
        };
        if stale {
            let pos = if st.playing && st.tag == id { st.pos as usize } else { cursor };
            let live = LiveRack::start(self.effects.clone(), audio, sr, self.noise_print.clone(), settings, id, pos);
            let sb = live.sb.clone();
            self.live = Some(live);
            if st.playing && st.tag == id {
                self.engine.set_stream(Some(sb));
            } else if self.engine.has_stream() {
                self.engine.set_stream(None);
            }
        } else if let Some(l) = &mut self.live {
            l.update(settings);
            if !(st.playing && st.tag == id) {
                // Pre-render from wherever playback will start.
                l.sb.set_pos(cursor as f64);
            }
        }
        if let Some(e) = self.live.as_ref().and_then(|l| l.take_error()) {
            self.rack_on = false;
            self.live = None;
            if self.engine.has_stream() {
                self.engine.set_stream(None);
            }
            self.set_status(format!("Effects Rack turned off: {e}"));
        }
    }

    pub fn rack_touched(&mut self) {
        self.rack_version += 1;
        self.rack_changed = Instant::now();
    }

    pub fn apply_rack(&mut self) {
        if self.busy() {
            return;
        }
        let Some((slots, (a, b), _, sr, n_ch)) = self.rack_inputs() else {
            self.set_status("Open or create a file first.");
            return;
        };
        if slots.is_empty() {
            self.set_status("The Effects Rack has no enabled effects.");
            return;
        }
        if b <= a {
            self.set_status("The file is empty.");
            return;
        }
        self.rack_on = false;
        self.engine.stop();
        if matches!(self.job, Some(Job { kind: JobKind::Preview { .. }, .. })) {
            self.job = None;
        }
        let doc = self.doc().unwrap();
        let input = slice_range(&doc.audio, a, b);
        let (doc_id, active) = (doc.id, doc.active_ch.clone());
        let (effects, np) = (self.effects.clone(), self.noise_print.clone());
        let (mix, gi, go) = (self.rack_mix, self.rack_in_db, self.rack_out_db);
        self.spawn_job("Effects Rack".into(), JobKind::Edit { doc_id, range: (a, b), active, new_rate: None, select: true }, move || {
            run_rack(&effects, &slots, input, sr, n_ch, np.as_ref(), mix, gi, go)
        });
    }

    fn mix_paste(&mut self, mode: usize, clip_db: f32, orig_db: f32) {
        let Some((clip, rate)) = self.clipboard.clone() else {
            self.set_status("The clipboard is empty.");
            return;
        };
        self.engine.stop();
        let Some(d) = self.doc_mut() else { return };
        let mut audio = clip;
        if rate != d.sample_rate {
            audio = resample_channels(&audio, d.sample_rate as f64 / rate as f64);
        }
        let audio = remap_channels(&audio, d.n_ch());
        let cg = db_to_lin(clip_db);
        let og = db_to_lin(orig_db);
        let clen = audio[0].len();
        let at = d.sel_range().map(|s| s.0).unwrap_or(d.cursor).min(d.len());
        let all = vec![true; d.n_ch()];
        match mode {
            0 => {
                let out = audio.iter().map(|c| c.iter().map(|v| v * cg).collect()).collect();
                apply_edit(d, "Mix Paste (Insert)", (at, at), &all, out, None, true);
            }
            2 => {
                let end = (at + clen).min(d.len());
                let out = audio.iter().map(|c| c.iter().map(|v| v * cg).collect()).collect();
                apply_edit(d, "Mix Paste (Replace)", (at, end), &all, out, None, true);
            }
            _ => {
                let end = (at + clen).min(d.len());
                let existing = slice_range(&d.audio, at, end);
                let out: Vec<Vec<f32>> = (0..d.n_ch())
                    .map(|c| {
                        (0..clen)
                            .map(|i| {
                                let o = existing[c].get(i).copied().unwrap_or(0.0) * og;
                                let w = audio[c][i] * cg;
                                if mode == 3 { o * w } else { o + w }
                            })
                            .collect()
                    })
                    .collect();
                let label = if mode == 3 { "Mix Paste (Modulate)" } else { "Mix Paste (Overlap)" };
                apply_edit(d, label, (at, end), &all, out, None, true);
            }
        }
    }

    pub fn close_effect_dialog(&mut self) {
        if self.engine.is_playing_tag(PREVIEW_TAG) || matches!(self.engine.status().tag, PREVIEW_TAG) {
            self.engine.stop();
        }
        if matches!(self.job, Some(Job { kind: JobKind::Preview { .. }, .. })) {
            self.job = None;
        }
        self.dialog = None;
    }

    /// Run an effect on the selection (or the whole file) in the background.
    pub fn run_effect(&mut self, idx: usize, params: Params) {
        if self.busy() {
            return;
        }
        if matches!(self.job, Some(Job { kind: JobKind::Preview { .. }, .. })) {
            self.job = None;
        }
        let effects = self.effects.clone();
        let def = &effects[idx];
        let Some(doc) = self.doc() else {
            self.set_status("Open or create a file first.");
            return;
        };
        let (a, b) = if def.generator {
            doc.sel_range().unwrap_or((doc.cursor, doc.cursor))
        } else {
            doc.target_range()
        };
        if !def.generator && b <= a {
            self.set_status("The file is empty.");
            return;
        }
        if def.stereo_only && doc.n_ch() < 2 {
            self.dialog = Some(Dialog::Message {
                title: def.name.into(),
                text: format!("{} needs a stereo file. Use Edit > Convert Sample Type to make this file stereo first.", def.name),
            });
            return;
        }
        let input = if def.generator { vec![Vec::new(); doc.n_ch()] } else { slice_range(&doc.audio, a, b) };
        let sr = doc.sample_rate;
        let n_ch = doc.n_ch();
        let active = doc.active_ch.clone();
        let doc_id = doc.id;
        let np = self.noise_print.clone();
        let label = def.name.to_string();
        let effects2 = effects.clone();
        if !def.generator || def.params.iter().any(|p| p.key == "duration") {
            self.last_effect = Some((idx, params.clone()));
        }
        self.spawn_job(label, JobKind::Edit { doc_id, range: (a, b), active, new_rate: None, select: true }, move || {
            let ctx = Ctx { sample_rate: sr, channels: n_ch, noise_print: np.as_ref() };
            effects2[idx].run(&input, &ctx, &params)
        });
    }

    // ---------------------------------------------------------------- playback

    pub fn play_toggle(&mut self) {
        let Some(doc) = self.doc() else { return };
        let st = self.engine.status();
        if st.playing && st.tag == doc.id {
            self.engine.stop();
            return;
        }
        let len = doc.len();
        if let Some((pid, pos)) = self.paused {
            if pid == doc.id {
                let (buf, sr, id) = (doc.audio.clone(), doc.sample_rate, doc.id);
                let end = doc.sel_range().filter(|(a, b)| pos >= *a as f64 && pos < *b as f64).map(|s| s.1).unwrap_or(len);
                let start = doc.sel_range().map(|s| s.0).filter(|&a| (a as f64) <= pos).unwrap_or(0);
                self.paused = None;
                let stream = self.live_stream(id);
                self.engine.play_ex(buf, stream, sr, start as f64, end as f64, self.looping, id);
                self.engine.seek(pos);
                return;
            }
        }
        let (mut start, mut end) = match doc.sel_range() {
            Some((a, b)) if doc.cursor >= a && doc.cursor < b => (doc.cursor, b),
            Some((a, b)) => (a, b),
            None => (doc.cursor, len),
        };
        if start >= end {
            start = 0;
            end = len;
        }
        // When looping a selection, wrap back to its start rather than the cursor.
        let loop_start = doc.sel_range().map(|s| s.0).filter(|&a| a <= start).unwrap_or(start);
        let (buf, sr, id) = (doc.audio.clone(), doc.sample_rate, doc.id);
        self.play_origin = start;
        let stream = self.live_stream(id);
        self.engine.play_ex(buf, stream, sr, loop_start as f64, end as f64, self.looping, id);
        if start != loop_start {
            self.engine.seek(start as f64);
        }
    }

    pub fn restart_from_cursor_if_playing(&mut self) {
        let Some(doc) = self.doc() else { return };
        let st = self.engine.status();
        if st.playing && st.tag == doc.id {
            self.engine.stop();
            self.play_toggle();
        }
    }

    pub fn stop_recording(&mut self) {
        let target = self.rec_target.take();
        let Some((rec, rate)) = self.engine.stop_recording() else { return };
        for d in self.docs.iter_mut() {
            d.recording = false;
            d.pending = 0;
        }
        let Some(t) = target else { return };
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == t.doc_id) else {
            self.set_status("The file being recorded into was closed; the recording was discarded.");
            return;
        };
        if rec.first().map(|c| c.is_empty()).unwrap_or(true) {
            self.set_status("Recording was empty.");
            return;
        }
        let secs = rec[0].len() as f64 / rate as f64;
        let mut audio = rec;
        if rate != doc.sample_rate {
            audio = resample_channels(&audio, doc.sample_rate as f64 / rate as f64);
        }
        let audio = remap_channels(&audio, doc.n_ch());
        let active = vec![true; doc.n_ch()];
        apply_edit(doc, "Record", t.range, &active, audio, None, true);
        doc.clamp_view();
        self.set_status(format!("Recorded {secs:.1} s"));
    }

    fn toggle_record(&mut self) {
        if self.engine.is_recording() {
            self.stop_recording();
            return;
        }
        // Like Audition, recording needs a file: ask for its format first.
        if self.doc().is_none() {
            let name = format!("Untitled {}", self.docs.len() + 1);
            self.dialog = Some(Dialog::NewFile { name, rate: 48000, channels: 2, bits: None, seconds: 0.0, then_record: true });
            return;
        }
        self.engine.stop();
        let (rate, channels) = match self.engine.start_recording() {
            Ok(f) => f,
            Err(e) => {
                self.dialog = Some(Dialog::Message {
                    title: "Can't record".into(),
                    text: format!(
                        "{e}\n\nCheck the input device in Edit > Preferences > Audio Hardware. On macOS, allow microphone access for Audemo in System Settings > Privacy & Security > Microphone."
                    ),
                });
                return;
            }
        };
        let _ = channels;
        let d = self.doc_mut().unwrap();
        let range = d.sel_range().unwrap_or((d.cursor, d.cursor));
        d.sel = None;
        d.recording = true;
        d.pending = 0;
        self.rec_target = Some(RecTarget { doc_id: d.id, range });
        self.set_status(format!("Recording at {rate} Hz. Press Stop, Space or Shift+Space to finish."));
    }

    // ---------------------------------------------------------------- saving

    fn save(&mut self, force_dialog: bool) {
        let Some(doc) = self.doc() else { return };
        let is_wav = doc
            .path
            .as_ref()
            .and_then(|p| p.extension())
            .map(|e| e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("wave"))
            .unwrap_or(false);
        if force_dialog || !is_wav {
            let fmt = WavFormat::from_bits(doc.source_bits);
            self.dialog = Some(Dialog::Export { format: fmt, dither: fmt == WavFormat::Pcm16, path: None, selection: false });
            return;
        }
        let path = doc.path.clone().unwrap();
        let fmt = WavFormat::from_bits(doc.source_bits);
        self.write_wav(path, fmt, fmt == WavFormat::Pcm16, false);
    }

    /// Write the active file (or just its selection) as WAV.
    pub fn write_wav(&mut self, path: PathBuf, fmt: WavFormat, dither: bool, selection: bool) {
        let Some(doc) = self.doc() else { return };
        let res = if selection {
            let (a, b) = doc.target_range();
            io::save_wav(&path, &slice_range(&doc.audio, a, b), doc.sample_rate, fmt, dither)
        } else {
            io::save_wav(&path, &doc.audio, doc.sample_rate, fmt, dither)
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match res {
            Ok(()) if selection => {
                self.prefs.add_recent(path.clone());
                self.prefs.save();
                self.set_status(format!("Saved selection as {name}"));
            }
            Ok(()) => {
                let doc = self.doc_mut().unwrap();
                doc.path = Some(path.clone());
                doc.name = name.clone();
                doc.dirty = false;
                doc.source_bits = match fmt {
                    WavFormat::Pcm16 => Some(16),
                    WavFormat::Pcm24 => Some(24),
                    WavFormat::Pcm32 => Some(32),
                    WavFormat::Float32 => None,
                };
                self.prefs.add_recent(path);
                self.prefs.save();
                self.set_status(format!("Saved {name} ({})", fmt.label()));
            }
            Err(e) => self.dialog = Some(Dialog::Message { title: "Save failed".into(), text: e }),
        }
    }

    pub fn pick_save_path(&self) -> Option<PathBuf> {
        let doc = self.doc()?;
        let stem = Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        let mut dlg = rfd::FileDialog::new().add_filter("WAV audio", &["wav"]).set_file_name(format!("{stem}.wav"));
        if let Some(dir) = doc.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        dlg.save_file().map(|p| if p.extension().is_none() { p.with_extension("wav") } else { p })
    }

    // ---------------------------------------------------------------- dispatcher

    pub fn perform(&mut self, action: Action, ctx: &egui::Context) {
        let editing = !matches!(
            action,
            Action::New
                | Action::Open
                | Action::OpenPaths(_)
                | Action::OpenRecent(_)
                | Action::Quit
                | Action::Status(_)
                | Action::Stop
                | Action::Close(_)
                | Action::ForceClose(_)
                | Action::Preferences
                | Action::Audition(_)
        );
        if editing && self.busy() {
            self.set_status("Please wait for the current effect to finish.");
            return;
        }
        match action {
            Action::New => {
                let (rate, channels) = self.doc().map(|d| (d.sample_rate, d.n_ch())).unwrap_or((48000, 2));
                let name = format!("Untitled {}", self.docs.len() + 1);
                self.dialog = Some(Dialog::NewFile { name, rate, channels, bits: None, seconds: 0.0, then_record: false });
            }
            Action::Open => {
                let picked = rfd::FileDialog::new()
                    .add_filter("Audio files", io::OPEN_EXTENSIONS)
                    .add_filter("All files", &["*"])
                    .pick_files();
                if let Some(p) = picked {
                    self.open_paths(p);
                }
            }
            Action::OpenPaths(p) => self.open_paths(p),
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::Close(i) => {
                let i = if i == usize::MAX { match self.active { Some(a) => a, None => return } } else { i };
                if self.docs.get(i).map(|d| d.dirty).unwrap_or(false) {
                    self.dialog = Some(Dialog::ConfirmClose { doc: i });
                } else {
                    self.actions.push(Action::ForceClose(i));
                }
            }
            Action::ForceClose(i) => {
                if i < self.docs.len() {
                    if self.engine.status().tag == self.docs[i].id {
                        self.engine.stop();
                    }
                    self.docs.remove(i);
                    self.active = if self.docs.is_empty() { None } else { Some(i.min(self.docs.len() - 1)) };
                }
            }
            Action::Quit => {
                if self.docs.iter().any(|d| d.dirty) && !self.allow_quit {
                    self.dialog = Some(Dialog::ConfirmQuit);
                } else {
                    self.allow_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Undo => {
                self.engine.stop();
                if let Some(l) = self.doc_mut().and_then(|d| d.undo()) {
                    self.set_status(format!("Undo {l}"));
                }
            }
            Action::Redo => {
                self.engine.stop();
                if let Some(l) = self.doc_mut().and_then(|d| d.redo()) {
                    self.set_status(format!("Redo {l}"));
                }
            }
            Action::JumpHistory(target) => {
                self.engine.stop();
                if let Some(d) = self.doc_mut() {
                    while d.undo.len() > target {
                        if d.undo().is_none() {
                            break;
                        }
                    }
                    while d.undo.len() < target {
                        if d.redo().is_none() {
                            break;
                        }
                    }
                }
            }
            Action::Copy => {
                let res = self.doc().map(|d| d.sel_range().map(|(a, b)| (slice_range(&d.audio, a, b), d.sample_rate, b - a)));
                match res {
                    Some(Some((clip, sr, n))) => {
                        self.clipboard = Some((clip, sr));
                        self.set_status(format!("Copied {}", format_time(n as f64, sr)));
                    }
                    Some(None) => self.set_status("Select something to copy."),
                    None => {}
                }
            }
            Action::Cut => {
                self.actions.push(Action::Copy);
                self.actions.push(Action::Delete);
            }
            Action::Delete => {
                self.engine.stop();
                if let Some(d) = self.doc_mut() {
                    if let Some((a, b)) = d.sel_range() {
                        let active = vec![true; d.n_ch()];
                        let empty = vec![Vec::new(); d.n_ch()];
                        apply_edit(d, "Delete", (a, b), &active, empty, None, false);
                        d.cursor = a;
                    }
                }
            }
            Action::Crop => {
                self.engine.stop();
                if let Some(d) = self.doc_mut() {
                    if let Some((a, b)) = d.sel_range() {
                        d.push_undo("Crop");
                        let audio = slice_range(&d.audio, a, b);
                        d.markers.retain(|m| m.pos >= a && m.pos <= b);
                        for m in d.markers.iter_mut() {
                            m.pos -= a;
                        }
                        d.set_audio(audio);
                        d.sel = None;
                        d.cursor = 0;
                        d.zoom_full();
                    }
                }
            }
            Action::Paste => {
                self.engine.stop();
                let Some((clip, rate)) = self.clipboard.clone() else {
                    self.set_status("The clipboard is empty.");
                    return;
                };
                if let Some(d) = self.doc_mut() {
                    let mut audio = clip;
                    if rate != d.sample_rate {
                        audio = resample_channels(&audio, d.sample_rate as f64 / rate as f64);
                    }
                    let audio = remap_channels(&audio, d.n_ch());
                    let range = d.sel_range().unwrap_or((d.cursor, d.cursor));
                    let active = vec![true; d.n_ch()];
                    apply_edit(d, "Paste", range, &active, audio, None, true);
                } else {
                    self.actions.push(Action::PasteNew);
                }
            }
            Action::PasteNew => {
                let Some((clip, rate)) = self.clipboard.clone() else {
                    self.set_status("The clipboard is empty.");
                    return;
                };
                let id = self.new_id();
                let n = self.docs.len() + 1;
                let mut doc = Document::new(id, format!("Untitled {n}"), None, clip, rate, None, "Paste to New");
                doc.dirty = true;
                self.add_doc(doc);
            }
            Action::SelectAll => {
                if let Some(d) = self.doc_mut() {
                    let len = d.len();
                    d.sel = if len > 0 { Some((0, len)) } else { None };
                }
            }
            Action::Deselect => {
                if let Some(d) = self.doc_mut() {
                    d.sel = None;
                }
            }
            Action::OpenEffect(idx) => {
                if self.doc().is_none() {
                    self.set_status("Open or create a file first.");
                    return;
                }
                self.close_effect_dialog();
                if self.effects[idx].params.is_empty() {
                    self.run_effect(idx, Params::default());
                    return;
                }
                let params = self.effects[idx].default_params();
                self.dialog = Some(Dialog::Effect(EffectDialog {
                    idx,
                    params,
                    preset: "(Default)".into(),
                    previewing: false,
                    bypass: false,
                    rendered: None,
                    last_change: Instant::now(),
                    drag_handle: None,
                    dry: None,
                    wet: None,
                    error: None,
                    rack_slot: None,
                }));
            }
            Action::ApplyEffect(idx, params) => {
                self.close_effect_dialog();
                self.run_effect(idx, params);
            }
            Action::ApplyFavorite(id, preset) => {
                if let Some(idx) = self.find_effect(id) {
                    let def = &self.effects[idx];
                    let params = if preset.is_empty() { def.default_params() } else { def.preset_params(preset).unwrap_or_else(|| def.default_params()) };
                    self.run_effect(idx, params);
                }
            }
            Action::ApplyFade { fade_in, len } => {
                if let Some(d) = self.doc_mut() {
                    let total = d.len();
                    let len = len.min(total);
                    if len < 2 {
                        return;
                    }
                    let (a, b) = if fade_in { (0, len) } else { (total - len, total) };
                    let seg = slice_range(&d.audio, a, b);
                    let out = effects::fade(&seg, fade_in, 2);
                    let active = d.active_ch.clone();
                    let (sel, cur) = (d.sel, d.cursor);
                    apply_edit(d, if fade_in { "Fade In" } else { "Fade Out" }, (a, b), &active, out, None, false);
                    d.sel = sel;
                    d.cursor = cur;
                }
            }
            Action::ApplyGain(db) => {
                if let Some(idx) = self.find_effect("amplify") {
                    let mut p = self.effects[idx].default_params();
                    p.set("left", crate::dsp::params::Value::F(db));
                    self.run_effect(idx, p);
                }
            }
            Action::CaptureNoise => {
                let Some(d) = self.doc() else { return };
                let Some((a, b)) = d.sel_range() else {
                    self.set_status("Select a noise-only region first, then capture the noise print (Shift+P).");
                    return;
                };
                match effects::capture_noise_print(&slice_range(&d.audio, a, b), d.sample_rate) {
                    Ok(np) => {
                        let secs = (b - a) as f64 / d.sample_rate as f64;
                        self.noise_print = Some(np);
                        self.set_status(format!("Noise print captured ({secs:.2} s). Now select the audio to clean and run Noise Reduction."));
                    }
                    Err(e) => self.set_status(e),
                }
            }
            Action::Convert(rate, channels) => {
                let Some(d) = self.doc() else { return };
                let audio = d.audio.clone();
                let old = d.sample_rate;
                let len = d.len();
                let kind = JobKind::Edit { doc_id: d.id, range: (0, len), active: vec![true; d.n_ch()], new_rate: Some(rate), select: false };
                self.engine.stop();
                self.spawn_job("Convert Sample Type".into(), kind, move || {
                    let mut out = remap_channels(&audio, channels);
                    if rate != old {
                        out = resample_channels(&out, rate as f64 / old as f64);
                    }
                    Ok(out)
                });
            }
            Action::AddMarker => {
                let pos = self.display_pos() as usize;
                if let Some(d) = self.doc_mut() {
                    let n = d.markers.len() + 1;
                    d.markers.push(Marker { pos, name: format!("Marker {n:02}") });
                    d.markers.sort_by_key(|m| m.pos);
                    d.dirty = true;
                }
            }
            Action::PlayToggle if self.engine.is_recording() => self.stop_recording(),
            Action::Stop if self.engine.is_recording() => self.stop_recording(),
            Action::PlayToggle => self.play_toggle(),
            Action::Stop => {
                self.paused = None;
                self.engine.stop();
            }
            Action::Record => self.toggle_record(),
            Action::ToStart => {
                if let Some(d) = self.doc_mut() {
                    d.cursor = 0;
                    let span = d.view_end - d.view_start;
                    d.view_start = 0.0;
                    d.view_end = span;
                    d.clamp_view();
                }
                self.restart_from_cursor_if_playing();
            }
            Action::ToEnd => {
                if let Some(d) = self.doc_mut() {
                    d.cursor = d.len();
                    let span = d.view_end - d.view_start;
                    d.view_end = d.max_span();
                    d.view_start = d.view_end - span;
                    d.clamp_view();
                }
                self.engine.stop();
            }
            Action::PrevMarker | Action::NextMarker => {
                let next = matches!(action, Action::NextMarker);
                let pos = self.display_pos() as usize;
                if let Some(d) = self.doc_mut() {
                    let mut stops: Vec<usize> = d.markers.iter().map(|m| m.pos).collect();
                    stops.push(0);
                    stops.push(d.len());
                    if let Some((a, b)) = d.sel_range() {
                        stops.push(a);
                        stops.push(b);
                    }
                    stops.sort_unstable();
                    stops.dedup();
                    let t = if next { stops.into_iter().find(|&p| p > pos) } else { stops.into_iter().rev().find(|&p| p < pos) };
                    if let Some(t) = t {
                        d.cursor = t;
                        if (t as f64) < d.view_start || (t as f64) > d.view_end {
                            let span = d.view_end - d.view_start;
                            d.view_start = t as f64 - span / 2.0;
                            d.view_end = d.view_start + span;
                            d.clamp_view();
                        }
                    }
                }
                self.restart_from_cursor_if_playing();
            }
            Action::ZoomIn | Action::ZoomOut => {
                let pos = self.display_pos();
                let f = if matches!(action, Action::ZoomIn) { 0.5 } else { 2.0 };
                if let Some(d) = self.doc_mut() {
                    let anchor = if pos >= d.view_start && pos <= d.view_end { pos } else { (d.view_start + d.view_end) / 2.0 };
                    d.zoom(f, anchor);
                }
            }
            Action::ZoomFull => {
                if let Some(d) = self.doc_mut() {
                    d.zoom_full();
                }
            }
            Action::ZoomSel => {
                if let Some(d) = self.doc_mut() {
                    if let Some((a, b)) = d.sel_range() {
                        d.zoom_to(a, b);
                    }
                }
            }
            Action::AmpIn => {
                if let Some(d) = self.doc_mut() {
                    d.amp_zoom = (d.amp_zoom * 1.5).min(64.0);
                }
            }
            Action::AmpOut => {
                if let Some(d) = self.doc_mut() {
                    d.amp_zoom = (d.amp_zoom / 1.5).max(1.0);
                }
            }
            Action::SetCursor(p) => {
                self.paused = None;
                if let Some(d) = self.doc_mut() {
                    d.cursor = p.min(d.len());
                }
                self.restart_from_cursor_if_playing();
            }
            Action::Status(s) if s.starts_with("__confirm_close:") => {
                let id: u64 = s["__confirm_close:".len()..].parse().unwrap_or(0);
                if let Some(i) = self.docs.iter().position(|d| d.id == id) {
                    self.dialog = Some(Dialog::ConfirmClose { doc: i });
                }
            }
            Action::Status(s) => self.set_status(s),
            Action::CloseAll => {
                let dirty: Vec<usize> = (0..self.docs.len()).filter(|&i| self.docs[i].dirty).collect();
                for i in (0..self.docs.len()).rev() {
                    if !self.docs[i].dirty {
                        self.actions.push(Action::ForceClose(i));
                    }
                }
                if let Some(&first) = dirty.first() {
                    // Indices shift as clean files close; confirm the dirty ones next frame.
                    let id = self.docs[first].id;
                    self.actions.push(Action::Status(format!("__confirm_close:{id}")));
                }
            }
            Action::SaveAll => {
                let mut saved = 0;
                let mut skipped = 0;
                for d in self.docs.iter_mut().filter(|d| d.dirty) {
                    let wav = d.path.as_ref().and_then(|p| p.extension()).map(|e| e.eq_ignore_ascii_case("wav")).unwrap_or(false);
                    if !wav {
                        skipped += 1;
                        continue;
                    }
                    let fmt = WavFormat::from_bits(d.source_bits);
                    if io::save_wav(d.path.as_ref().unwrap(), &d.audio, d.sample_rate, fmt, fmt == WavFormat::Pcm16).is_ok() {
                        d.dirty = false;
                        saved += 1;
                    }
                }
                self.set_status(if skipped > 0 {
                    format!("Saved {saved} file(s). {skipped} need Save As first (new or non-WAV files).")
                } else {
                    format!("Saved {saved} file(s).")
                });
            }
            Action::SaveSelectionAs => {
                if let Some(d) = self.doc() {
                    let fmt = WavFormat::from_bits(d.source_bits);
                    self.dialog = Some(Dialog::Export { format: fmt, dither: fmt == WavFormat::Pcm16, path: None, selection: true });
                }
            }
            Action::CopyToNew => {
                let res = self.doc().map(|d| {
                    let (a, b) = d.target_range();
                    (slice_range(&d.audio, a, b), d.sample_rate)
                });
                if let Some((audio, sr)) = res {
                    let id = self.new_id();
                    let n = self.docs.len() + 1;
                    let mut doc = Document::new(id, format!("Untitled {n}"), None, audio, sr, None, "Copy to New");
                    doc.dirty = true;
                    self.add_doc(doc);
                }
            }
            Action::MixPaste { mode, clip_db, orig_db } => self.mix_paste(mode, clip_db, orig_db),
            Action::RepeatLast => match self.last_effect.clone() {
                Some((idx, p)) => self.run_effect(idx, p),
                None => self.set_status("No effect has been applied yet."),
            },
            Action::OpenAppend => {
                let Some(doc_id) = self.doc().map(|d| d.id) else {
                    self.actions.push(Action::Open);
                    return;
                };
                if let Some(paths) = rfd::FileDialog::new().add_filter("Audio files", io::OPEN_EXTENSIONS).pick_files() {
                    for p in paths {
                        self.load_async(p, LoadIntent::Append(doc_id));
                    }
                }
            }
            Action::OpenRecent(p) => {
                if p.is_file() {
                    self.open_paths(vec![p]);
                } else {
                    self.prefs.recent.retain(|r| r != &p);
                    self.prefs.save();
                    self.dialog = Some(Dialog::Message { title: "File not found".into(), text: format!("{} no longer exists.", p.display()) });
                }
            }
            Action::Preferences => {
                let (inputs, outputs) = crate::engine::list_devices();
                self.dialog = Some(Dialog::Preferences {
                    input: self.prefs.input_device.clone(),
                    output: self.prefs.output_device.clone(),
                    inputs,
                    outputs,
                });
            }
            Action::ApplyRack => self.apply_rack(),
            Action::Pause => {
                let st = self.engine.status();
                match (self.doc().map(|d| d.id), self.paused) {
                    (Some(id), Some((pid, _))) if pid == id => self.play_toggle(),
                    (Some(id), _) if st.playing && st.tag == id => {
                        self.paused = Some((id, st.pos));
                        self.engine.stop();
                    }
                    _ => {}
                }
            }
            Action::Nudge(secs) => {
                let st = self.engine.status();
                if let Some(d) = self.doc_mut() {
                    let delta = secs * d.sample_rate as f64;
                    if st.playing && st.tag == d.id {
                        self.engine.seek(st.pos + delta);
                    } else {
                        let len = d.len() as f64;
                        d.cursor = (d.cursor as f64 + delta).clamp(0.0, len) as usize;
                        if (d.cursor as f64) < d.view_start || (d.cursor as f64) > d.view_end {
                            let span = d.view_end - d.view_start;
                            d.view_start = d.cursor as f64 - span / 2.0;
                            d.view_end = d.view_start + span;
                            d.clamp_view();
                        }
                    }
                }
            }
            Action::AmplitudeStatistics => self.open_amplitude_stats(),
            Action::ClearHistory => {
                if let Some(d) = self.doc_mut() {
                    d.undo.clear();
                    d.redo.clear();
                }
                self.set_status("History cleared.");
            }
            Action::Audition(p) => {
                self.browser_selected = Some(p.clone());
                if self.engine.is_playing_tag(BROWSER_TAG) {
                    self.engine.stop();
                }
                if self.prefs.browser_autoplay {
                    self.load_async(p, LoadIntent::Audition);
                }
            }
        }
    }

    pub fn create_new(&mut self, name: &str, rate: u32, channels: usize, bits: Option<u32>, seconds: f32) {
        let id = self.new_id();
        let len = (seconds.max(0.0) as f64 * rate as f64) as usize;
        let name = if name.trim().is_empty() { format!("Untitled {}", self.docs.len() + 1) } else { name.trim().to_string() };
        let mut doc = Document::new(id, name, None, vec![vec![0.0; len]; channels.max(1)], rate, bits, "New Audio File");
        doc.dirty = true;
        self.add_doc(doc);
    }
}

/// Run an Effects Rack chain: input gain, each enabled effect in order,
/// dry/wet mix, output gain.
#[allow(clippy::too_many_arguments)]
pub fn run_rack(
    effects: &[EffectDef],
    slots: &[(usize, Params)],
    input: Vec<Vec<f32>>,
    sr: u32,
    n_ch: usize,
    np: Option<&NoiseProfile>,
    mix: f32,
    in_db: f32,
    out_db: f32,
) -> Result<Vec<Vec<f32>>, String> {
    let ctx = Ctx { sample_rate: sr, channels: n_ch, noise_print: np };
    let gi = db_to_lin(in_db);
    let go = db_to_lin(out_db);
    let mut x: Vec<Vec<f32>> = input.iter().map(|c| c.iter().map(|s| s * gi).collect()).collect();
    for (idx, p) in slots {
        x = effects[*idx].run(&x, &ctx, p).map_err(|e| format!("{}: {e}", effects[*idx].name))?;
    }
    let m = (mix / 100.0).clamp(0.0, 1.0);
    let same = x.first().map(|c| c.len()) == input.first().map(|c| c.len());
    Ok(x.into_iter()
        .enumerate()
        .map(|(c, ch)| {
            ch.into_iter()
                .enumerate()
                .map(|(i, w)| {
                    if same {
                        (input[c.min(input.len() - 1)][i] * (1.0 - m) + w * m) * go
                    } else {
                        w * go
                    }
                })
                .collect()
        })
        .collect())
}

/// Replace `range` with `out`, record undo, keep markers and selection sane.
pub fn apply_edit(
    doc: &mut Document,
    label: &str,
    range: (usize, usize),
    active: &[bool],
    out: Vec<Vec<f32>>,
    new_rate: Option<u32>,
    select: bool,
) {
    doc.push_undo(label);
    let (a, b) = (range.0.min(doc.len()), range.1.min(doc.len()));
    let out_len = out.first().map(|c| c.len()).unwrap_or(0);
    let whole = new_rate.is_some() || (out.len() != doc.n_ch() && a == 0 && b == doc.len());
    if whole {
        // Channel count or sample rate changed: replace everything.
        let ratio = new_rate.map(|r| r as f64 / doc.sample_rate as f64).unwrap_or(1.0);
        if let Some(r) = new_rate {
            doc.sample_rate = r;
            doc.source_bits = None;
        }
        for m in doc.markers.iter_mut() {
            m.pos = (m.pos as f64 * ratio) as usize;
        }
        doc.cursor = (doc.cursor as f64 * ratio) as usize;
        doc.sel = None;
        doc.set_audio(out);
        doc.active_ch = vec![true; doc.n_ch()];
        doc.zoom_full();
        return;
    }
    let same_len = out_len == b - a;
    let mut audio: Vec<Vec<f32>> = (*doc.audio).clone();
    for (c, ch) in audio.iter_mut().enumerate() {
        if same_len && !active.get(c).copied().unwrap_or(true) {
            continue;
        }
        let src = &out[c.min(out.len().saturating_sub(1))];
        ch.splice(a..b, src.iter().copied());
    }
    doc.shift_markers(a, b - a, out_len);
    doc.set_audio(audio);
    doc.sel = if select && out_len > 0 { Some((a, a + out_len)) } else { None };
    doc.cursor = a;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(len: usize) -> Document {
        let a: Vec<f32> = (0..len).map(|i| i as f32).collect();
        Document::new(1, "t.wav".into(), None, vec![a.clone(), a], 1000, Some(16), "Open")
    }

    #[test]
    fn delete_insert_undo_redo() {
        let mut d = doc(100);
        let all = vec![true, true];
        apply_edit(&mut d, "Delete", (10, 20), &all, vec![vec![], vec![]], None, false);
        assert_eq!(d.len(), 90);
        assert_eq!(d.audio[0][10], 20.0);
        apply_edit(&mut d, "Paste", (5, 5), &all, vec![vec![-1.0; 3], vec![-1.0; 3]], None, true);
        assert_eq!(d.len(), 93);
        assert_eq!(d.sel, Some((5, 8)));
        assert_eq!(d.undo.len(), 2);
        d.undo();
        assert_eq!(d.len(), 90);
        d.undo();
        assert_eq!(d.len(), 100);
        assert_eq!(d.audio[0][15], 15.0);
        d.redo();
        assert_eq!(d.len(), 90);
        assert_eq!(d.redo.len(), 1);
    }

    #[test]
    fn inactive_channel_untouched_for_same_length_edits() {
        let mut d = doc(50);
        apply_edit(&mut d, "Silence", (0, 50), &[true, false], vec![vec![0.0; 50], vec![0.0; 50]], None, true);
        assert_eq!(d.audio[0][30], 0.0);
        assert_eq!(d.audio[1][30], 30.0);
    }

    #[test]
    fn markers_follow_edits() {
        let mut d = doc(100);
        d.markers = vec![Marker { pos: 5, name: "a".into() }, Marker { pos: 15, name: "b".into() }, Marker { pos: 60, name: "c".into() }];
        let all = vec![true, true];
        apply_edit(&mut d, "Delete", (10, 20), &all, vec![vec![], vec![]], None, false);
        let pos: Vec<usize> = d.markers.iter().map(|m| m.pos).collect();
        assert_eq!(pos, vec![5, 50]);
        d.undo();
        assert_eq!(d.markers.len(), 3);
    }

    #[test]
    fn convert_replaces_whole_file() {
        let mut d = doc(1000);
        let mono = remap_channels(&d.audio, 1);
        let out = resample_channels(&mono, 2.0);
        apply_edit(&mut d, "Convert Sample Type", (0, 1000), &[true, true], out, Some(2000), false);
        assert_eq!(d.n_ch(), 1);
        assert_eq!(d.sample_rate, 2000);
        assert_eq!(d.len(), 2000);
        assert_eq!(d.active_ch.len(), 1);
        d.undo();
        assert_eq!((d.n_ch(), d.sample_rate, d.len()), (2, 1000, 1000));
    }

    #[test]
    fn rack_chain_gain_and_mix() {
        let effects = crate::dsp::effects::registry();
        let amp = effects.iter().position(|e| e.id == "amplify").unwrap();
        let inv = effects.iter().position(|e| e.id == "invert").unwrap();
        let mut p = effects[amp].default_params();
        p.set("left", crate::dsp::params::Value::F(6.0206));
        let input = vec![vec![0.25f32; 100]];
        // +6 dB then invert, fully wet, -6 dB output: back to -0.25.
        let slots = vec![(amp, p.clone()), (inv, Params::default())];
        let out = run_rack(&effects, &slots, input.clone(), 1000, 1, None, 100.0, 0.0, -6.0206).unwrap();
        assert!((out[0][10] + 0.25).abs() < 1e-3, "{}", out[0][10]);
        // 50% mix of dry 0.25 and wet -0.25 cancels.
        let out = run_rack(&effects, &slots[1..], input, 1000, 1, None, 50.0, 0.0, 0.0).unwrap();
        assert!(out[0][10].abs() < 1e-6);
    }

    #[test]
    fn recording_extends_view_span() {
        let mut d = doc(0);
        d.recording = true;
        d.pending = 500;
        assert!(d.max_span() >= 30_000.0);
        d.recording = false;
        d.pending = 0;
        assert_eq!(d.max_span(), 64.0);
    }

    #[test]
    fn view_clamps_and_zooms() {
        let mut d = doc(10_000);
        d.zoom(0.1, 5000.0);
        assert!((d.view_end - d.view_start - 1000.0).abs() < 1.0);
        d.view_start = 9500.0;
        d.view_end = 10_500.0;
        d.clamp_view();
        assert!(d.view_end <= 10_000.0 + 1e-6);
        d.zoom(1e9, 0.0);
        assert_eq!((d.view_start, d.view_end), (0.0, 10_000.0));
    }
}
