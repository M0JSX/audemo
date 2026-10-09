//! Multitrack Editor and Mixer: track headers, clips (move, trim, fade,
//! split), time selection, recording onto armed tracks, mixdown, and the
//! session commands behind the Multitrack menu.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Layout, Rect, RichText, Rounding, Sense, Shape, Stroke, Ui};

use crate::app::{Action, App, Dialog, Document, JobKind, LoadIntent, Mode};
use crate::dsp::peaks::PeakCache;
use crate::dsp::resample::{remap_channels, resample_channels};
use crate::dsp::util::{db_to_lin, format_time};
use crate::editor::{ruler_label, RULER_STEPS};
use crate::engine::Buffer;
use crate::io::{self, WavFormat};
use crate::session::{self, crossfade_pairs, mix_range, Clip, Envelope, Session, Source, Track, SESSION_EXT, VOL_ENV_MAX, VOL_ENV_MIN};
use crate::theme::*;

const HEADER_W: f32 = 214.0;
const RULER_H: f32 = 24.0;
const MASTER_H: f32 = 34.0;
const EDGE_PX: f32 = 6.0;
const CLIP_BAR: f32 = 15.0;
const SNAP_PX: f32 = 8.0;

/// Drag payload for an open file dragged from the Files panel.
#[derive(Clone, Copy)]
pub struct DocDrag(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MtView {
    #[default]
    Editor,
    Mixer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Body,
    Left,
    Right,
    FadeIn,
    FadeOut,
}

pub enum MtDrag {
    /// Move the grabbed clip and every other selected clip with it.
    Move { grabbed: u64, grab: f64, track0: usize, items: Vec<(u64, usize, usize)> },
    TrimStart(u64),
    TrimEnd(u64),
    FadeIn(u64),
    FadeOut(u64),
    Select(usize),
    /// An automation point: (track, pan envelope?, point index).
    EnvPoint(usize, bool, usize),
}

#[derive(Clone, Copy, PartialEq)]
enum EnvHit {
    Point(usize),
    Line,
}

pub struct MtRec {
    pub session_id: u64,
    pub track_id: u64,
    pub start: usize,
}

#[derive(Default)]
pub struct MtState {
    pub view: MtView,
    pub drag: Option<MtDrag>,
    /// (session id, clips with their track offset from the first one)
    pub clipboard: Option<(u64, Vec<(usize, Clip)>)>,
    pub rec: Option<MtRec>,
    /// Session sources that could not be found when opening.
    pub missing: Vec<String>,
    pub sent_version: u64,
    context_clip: Option<u64>,
    context_track: Option<usize>,
    /// Right-clicked automation: (track, pan?, point)
    context_env: Option<(usize, bool, Option<usize>)>,
    /// Per-track meter levels in dB (L, R), decayed.
    pub meters: Vec<[f32; 2]>,
    meter_at: Option<std::time::Instant>,
}

/// Where the automation lines are drawn inside a lane (below the clip names).
fn env_area(lane: Rect) -> Rect {
    Rect::from_min_max(pos2(lane.left(), lane.top() + CLIP_BAR + 2.0), pos2(lane.right(), lane.bottom() - 3.0))
}

fn env_y(pan: bool, v: f32, area: Rect) -> f32 {
    let frac = if pan { (v + 100.0) / 200.0 } else { 1.0 - (v - VOL_ENV_MIN) / (VOL_ENV_MAX - VOL_ENV_MIN) };
    area.top() + frac.clamp(0.0, 1.0) * area.height()
}

fn env_value(pan: bool, y: f32, area: Rect) -> f32 {
    let frac = ((y - area.top()) / area.height().max(1.0)).clamp(0.0, 1.0);
    if pan {
        (frac * 200.0 - 100.0).round()
    } else {
        let db = VOL_ENV_MAX - frac * (VOL_ENV_MAX - VOL_ENV_MIN);
        (db * 10.0).round() / 10.0
    }
}

fn env_of(t: &Track, pan: bool) -> &Envelope {
    if pan {
        &t.pan_env
    } else {
        &t.vol_env
    }
}

fn env_of_mut(t: &mut Track, pan: bool) -> &mut Envelope {
    if pan {
        &mut t.pan_env
    } else {
        &mut t.vol_env
    }
}

const VOL_ENV_COL: Color32 = Color32::from_rgb(0xf2, 0xc2, 0x30);
const PAN_ENV_COL: Color32 = Color32::from_rgb(0x4d, 0xa3, 0xff);

/// Vertical stereo peak meter (levels in dB).
fn draw_meter(p: &egui::Painter, r: Rect, db: [f32; 2]) {
    p.rect_filled(r, 1.0, Color32::from_rgb(0x0c, 0x0e, 0x12));
    let w = (r.width() - 1.0) / 2.0;
    for (c, d) in db.iter().enumerate() {
        let frac = ((d + 60.0) / 60.0).clamp(0.0, 1.0);
        if frac <= 0.0 {
            continue;
        }
        let x0 = r.left() + c as f32 * (w + 1.0);
        let top = r.bottom() - frac * r.height();
        let col = if *d > -0.1 { RECORD } else if *d > -6.0 { Color32::from_rgb(0xe8, 0xd2, 0x3a) } else { Color32::from_rgb(0x2f, 0xd4, 0x8c) };
        p.rect_filled(Rect::from_min_max(pos2(x0, top), pos2(x0 + w, r.bottom())), 0.0, col);
    }
}

/// Session-rate, at most stereo audio for a document (shared when possible).
fn source_audio(doc: &Document, rate: u32) -> (Buffer, Arc<PeakCache>) {
    if doc.sample_rate == rate && doc.n_ch() <= 2 {
        return (doc.audio.clone(), doc.peaks.clone());
    }
    let mut a = remap_channels(&doc.audio, doc.n_ch().clamp(1, 2));
    if doc.sample_rate != rate {
        a = resample_channels(&a, rate as f64 / doc.sample_rate as f64);
    }
    let peaks = Arc::new(PeakCache::build(&a));
    (Arc::new(a), peaks)
}

fn colour(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn safe_file_name(s: &str) -> String {
    let stem = std::path::Path::new(s).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|| s.to_string());
    let v: String = stem.chars().map(|c| if c.is_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
    if v.trim().is_empty() {
        "Audio".into()
    } else {
        v.trim().to_string()
    }
}

#[derive(Clone, Copy)]
struct TView {
    left: f32,
    width: f32,
    start: f64,
    end: f64,
}

impl TView {
    fn x(&self, s: f64) -> f32 {
        self.left + ((s - self.start) / (self.end - self.start)) as f32 * self.width
    }
    fn s(&self, x: f32) -> f64 {
        self.start + ((x - self.left) / self.width) as f64 * (self.end - self.start)
    }
    fn spp(&self) -> f64 {
        (self.end - self.start) / self.width as f64
    }
}

impl App {
    pub fn session(&self) -> Option<&Session> {
        self.active_session.and_then(|i| self.sessions.get(i))
    }

    fn in_multitrack(&self) -> bool {
        self.mode == Mode::Multitrack && self.session().is_some()
    }

    /// (position, sample rate, playing) for the transport display.
    pub fn transport_info(&self) -> Option<(f64, u32, bool)> {
        if self.mode != Mode::Multitrack {
            return None;
        }
        let s = self.session()?;
        let st = self.engine.status();
        let playing = st.playing && st.tag == s.id;
        let pos = match &self.mt.rec {
            Some(r) if r.session_id == s.id && self.engine.is_recording() => {
                let ratio = s.sample_rate as f64 / self.engine.recording_format().map(|f| f.0).unwrap_or(s.sample_rate) as f64;
                r.start as f64 + self.engine.recorded_frames() as f64 * ratio
            }
            _ if playing => st.pos,
            _ => s.cursor as f64,
        };
        Some((pos, s.sample_rate, playing))
    }

    // ------------------------------------------------------------ sessions

    pub fn create_session(&mut self, name: String, rate: u32, tracks: usize) {
        let id = self.new_id();
        let s = Session::new(id, name.clone(), rate, tracks.clamp(1, 64));
        self.sessions.push(s);
        self.active_session = Some(self.sessions.len() - 1);
        self.mode = Mode::Multitrack;
        self.set_status(format!("Created {name}: drag files from the Files panel onto a track, or arm a track (R) and record."));
    }

    /// The id of the open document for `path`, opening `dec` as one if needed.
    pub fn open_or_find_doc(&mut self, path: &std::path::Path, dec: io::Decoded) -> u64 {
        if let Some(d) = self.docs.iter().find(|d| d.path.as_deref() == Some(path)) {
            return d.id;
        }
        let id = self.new_id();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        let doc = Document::new(id, name, Some(path.to_path_buf()), dec.channels, dec.sample_rate, dec.bits, "Open");
        self.docs.push(doc);
        if self.active.is_none() {
            self.active = Some(self.docs.len() - 1);
        }
        id
    }

    /// Place open document `doc_id` on a track of session `session_id`;
    /// returns the new clip's id.
    pub fn mt_insert_doc(&mut self, session_id: u64, track: usize, at: usize, doc_id: u64) -> Option<u64> {
        let doc = self.docs.iter().find(|d| d.id == doc_id)?;
        let si = self.sessions.iter().position(|s| s.id == session_id)?;
        if doc.len() == 0 {
            self.set_status(format!("{} is empty.", doc.name));
            return None;
        }
        let rate = self.sessions[si].sample_rate;
        let existing = self.sessions[si].sources.values().find(|s| s.doc_id == Some(doc_id)).map(|s| s.id);
        let (name, path, version) = (doc.name.clone(), doc.path.clone(), doc.version);
        let src_id = match existing {
            Some(id) => id,
            None => {
                let (audio, peaks) = source_audio(doc, rate);
                let s = &mut self.sessions[si];
                let id = s.new_id();
                s.sources.insert(id, Arc::new(Source { id, name: name.clone(), doc_id: Some(doc_id), doc_version: version, path, audio, peaks }));
                id
            }
        };
        let s = &mut self.sessions[si];
        while s.tracks.len() <= track {
            s.add_track();
        }
        let clip = s.insert_clip(track, src_id, at);
        if clip.is_some() {
            s.selected_track = track;
            let end = s.end() as f64;
            if end > s.view_end && s.tracks.iter().map(|t| t.clips.len()).sum::<usize>() == 1 {
                s.view_start = 0.0;
                s.view_end = end * 1.1;
            }
            let tname = s.tracks[track].name.clone();
            self.set_status(format!("Inserted {name} on {tname}"));
        }
        self.active_session = Some(si);
        self.mode = Mode::Multitrack;
        clip
    }

    /// A source referenced by a session file has loaded as `doc_id`.
    pub fn mt_link_source(&mut self, session_id: u64, source_id: u64, doc_id: u64) {
        let Some(doc) = self.docs.iter().find(|d| d.id == doc_id) else { return };
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) else { return };
        let (audio, peaks) = source_audio(doc, s.sample_rate);
        let dirty = s.dirty;
        s.set_source_audio(source_id, audio, peaks, Some(doc_id), doc.version);
        s.dirty = dirty;
    }

    pub fn open_session(&mut self, path: &std::path::Path) {
        if let Some(i) = self.sessions.iter().position(|s| s.path.as_deref() == Some(path)) {
            self.active_session = Some(i);
            self.mode = Mode::Multitrack;
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.dialog = Some(Dialog::Message { title: "Couldn't open session".into(), text: format!("{}\n\n{e}", path.display()) });
                return;
            }
        };
        let id = self.new_id();
        let parsed = match session::from_text(id, &text, path.parent()) {
            Ok(p) => p,
            Err(e) => {
                self.dialog = Some(Dialog::Message { title: "Couldn't open session".into(), text: e });
                return;
            }
        };
        let mut s = parsed.session;
        s.path = Some(path.to_path_buf());
        s.name = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or(s.name);
        let mut loads = Vec::new();
        for (sid, p, name) in parsed.sources {
            s.sources.insert(
                sid,
                Arc::new(Source { id: sid, name, doc_id: None, doc_version: 0, path: Some(p.clone()), audio: Arc::new(vec![Vec::new()]), peaks: Arc::new(PeakCache::empty()) }),
            );
            loads.push((sid, p));
        }
        s.dirty = false;
        let session_id = s.id;
        self.sessions.push(s);
        self.active_session = Some(self.sessions.len() - 1);
        self.mode = Mode::Multitrack;
        self.mt.missing.clear();
        for (sid, p) in loads {
            if let Some(doc_id) = self.docs.iter().find(|d| d.path.as_deref() == Some(p.as_path())).map(|d| d.id) {
                self.mt_link_source(session_id, sid, doc_id);
            } else if p.is_file() {
                self.load_async(p, LoadIntent::SessionSource { session_id, source_id: sid });
            } else {
                let name = self.sessions.last().and_then(|s| s.sources.get(&sid)).map(|s| s.name.clone()).unwrap_or_default();
                self.mt.missing.push(format!("{name} ({})", p.display()));
            }
        }
        if !self.mt.missing.is_empty() {
            self.dialog = Some(Dialog::Message {
                title: "Missing files".into(),
                text: format!("These files used by the session could not be found; their clips will be silent:\n\n{}", self.mt.missing.join("\n")),
            });
        }
        self.prefs.add_recent(path.to_path_buf());
        self.prefs.save();
        self.set_status(format!("Opened session {}", path.display()));
    }

    /// Save the active session. Audio not already in a saved file is written
    /// to a "<session> Files" folder beside it.
    pub fn save_session(&mut self, force_dialog: bool) {
        let Some(si) = self.active_session else { return };
        let s = &self.sessions[si];
        let path = match (&s.path, force_dialog) {
            (Some(p), false) => p.clone(),
            _ => {
                let mut dlg = rfd::FileDialog::new().add_filter("Audemo session", &[SESSION_EXT]).set_file_name(format!("{}.{SESSION_EXT}", safe_file_name(&s.name)));
                if let Some(dir) = s.path.as_ref().and_then(|p| p.parent()) {
                    dlg = dlg.set_directory(dir);
                }
                match dlg.save_file() {
                    Some(p) if p.extension().is_none() => p.with_extension(SESSION_EXT),
                    Some(p) => p,
                    None => return,
                }
            }
        };
        let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Session".into());
        let files_dir = base.join(format!("{stem} Files"));
        let used: std::collections::HashSet<u64> = s.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.source)).chain(s.undo.iter().flat_map(|u| u.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.source)))).collect();
        let mut paths: BTreeMap<u64, PathBuf> = BTreeMap::new();
        let mut written: Vec<(u64, PathBuf)> = Vec::new();
        let mut taken: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for (id, src) in &s.sources {
            if !used.contains(id) {
                continue;
            }
            let doc = src.doc_id.and_then(|d| self.docs.iter().find(|x| x.id == d));
            let on_disk = match doc {
                Some(d) if !d.dirty => d.path.clone(),
                Some(_) => None,
                None => src.path.clone().filter(|p| p.is_file()),
            };
            if let Some(p) = on_disk {
                paths.insert(*id, p);
                continue;
            }
            if src.len() == 0 {
                // Missing or still loading: keep the reference rather than lose it.
                if let Some(p) = src.path.clone() {
                    paths.insert(*id, p);
                }
                continue;
            }
            if let Err(e) = std::fs::create_dir_all(&files_dir) {
                self.dialog = Some(Dialog::Message { title: "Save failed".into(), text: format!("{}\n\n{e}", files_dir.display()) });
                return;
            }
            let base_name = safe_file_name(&src.name);
            let mut p = files_dir.join(format!("{base_name}.wav"));
            let mut n = 2;
            while taken.contains(&p) || (p.exists() && src.path.as_deref() != Some(p.as_path())) {
                p = files_dir.join(format!("{base_name} {n}.wav"));
                n += 1;
            }
            taken.insert(p.clone());
            if let Err(e) = io::save_wav(&p, &src.audio, s.sample_rate, WavFormat::Float32, false) {
                self.dialog = Some(Dialog::Message { title: "Save failed".into(), text: e });
                return;
            }
            paths.insert(*id, p.clone());
            written.push((*id, p));
        }
        let text = session::to_text(s, &paths, Some(&base));
        if let Err(e) = std::fs::write(&path, text) {
            self.dialog = Some(Dialog::Message { title: "Save failed".into(), text: e.to_string() });
            return;
        }
        // Unsaved files now live in the session folder.
        let rate = s.sample_rate;
        for (sid, p) in &written {
            let doc_id = self.sessions[si].sources.get(sid).and_then(|s| s.doc_id);
            if let Some(d) = doc_id.and_then(|id| self.docs.iter_mut().find(|d| d.id == id)) {
                if d.path.is_none() && d.sample_rate == rate && d.n_ch() <= 2 {
                    d.path = Some(p.clone());
                    d.name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    d.dirty = false;
                    d.source_bits = None;
                }
            }
        }
        let s = &mut self.sessions[si];
        for (sid, p) in written {
            if let Some(src) = s.sources.get(&sid) {
                let ns = Source { id: sid, name: src.name.clone(), doc_id: src.doc_id, doc_version: src.doc_version, path: Some(p), audio: src.audio.clone(), peaks: src.peaks.clone() };
                s.sources.insert(sid, Arc::new(ns));
            }
        }
        s.path = Some(path.clone());
        s.name = stem.clone();
        s.dirty = false;
        self.prefs.add_recent(path);
        self.prefs.save();
        self.set_status(format!("Saved session {stem}"));
    }

    /// Per-frame upkeep: follow edited files, feed edits to the live mix,
    /// follow the playhead.
    pub fn sync_sessions(&mut self) {
        // Sources follow their documents.
        let mut updates = Vec::new();
        for (si, s) in self.sessions.iter().enumerate() {
            for src in s.sources.values() {
                if let Some(doc_id) = src.doc_id {
                    if let Some(d) = self.docs.iter().find(|d| d.id == doc_id) {
                        if d.version != src.doc_version && !d.recording {
                            let (audio, peaks) = source_audio(d, s.sample_rate);
                            updates.push((si, src.id, audio, peaks, doc_id, d.version));
                        }
                    }
                }
            }
        }
        for (si, sid, audio, peaks, doc_id, ver) in updates {
            let s = &mut self.sessions[si];
            s.set_source_audio(sid, audio, peaks, Some(doc_id), ver);
            s.dirty = true;
        }
        let st = self.engine.status();
        let session_ids: Vec<u64> = self.sessions.iter().map(|s| s.id).collect();
        let playing_session = st.playing && session_ids.contains(&st.tag);
        if self.mode != Mode::Multitrack {
            if playing_session && !self.engine.is_recording() {
                self.engine.stop();
            }
            return;
        }
        // Track meters fall at 24 dB/s between peaks.
        let now = std::time::Instant::now();
        let dt = self.mt.meter_at.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        self.mt.meter_at = Some(now);
        let n_tracks = self.session().map(|s| s.tracks.len()).unwrap_or(0);
        let active_playing = self.session().map(|s| st.playing && st.tag == s.id).unwrap_or(false);
        let peaks = if active_playing { self.engine.take_track_peaks() } else { Vec::new() };
        self.mt.meters.resize(n_tracks, [-120.0; 2]);
        for (i, m) in self.mt.meters.iter_mut().enumerate() {
            for c in 0..2 {
                let db = peaks.get(i).map(|p| crate::dsp::util::lin_to_db(p[c])).unwrap_or(-120.0).max(-120.0);
                m[c] = db.max(m[c] - 24.0 * dt);
            }
        }
        let follow = self.follow;
        let recording = self.engine.is_recording();
        let rec_head = self.transport_info().map(|t| t.0);
        let Some(si) = self.active_session.filter(|&i| i < self.sessions.len()) else { return };
        let s = &mut self.sessions[si];
        if st.playing && st.tag == s.id {
            let (v, end, sel) = (s.version, s.end(), s.sel_range());
            if v != self.mt.sent_version {
                self.engine.set_mix(Arc::new(s.mix_state()));
                self.mt.sent_version = v;
                if sel.is_none() && !recording {
                    self.engine.set_end(end as f64);
                }
            }
        }
        let head = if recording { rec_head } else if st.playing && st.tag == s.id { Some(st.pos) } else { None };
        if let (true, Some(h)) = (follow, head) {
            let span = s.view_end - s.view_start;
            if h > s.view_end - span * 0.02 || h < s.view_start {
                s.view_start = h - span * 0.05;
                s.view_end = s.view_start + span;
                if recording {
                    let max = s.view_end;
                    s.view_end = max;
                } else {
                    s.clamp_view();
                }
            }
        }
    }

    fn mt_play_toggle(&mut self) {
        let Some(si) = self.active_session else { return };
        let st = self.engine.status();
        let s = &mut self.sessions[si];
        if st.playing && st.tag == s.id {
            // Space leaves the cursor where playback stopped.
            self.engine.stop();
            s.cursor = st.pos.max(0.0) as usize;
            return;
        }
        let end_all = s.end();
        let (mut start, mut end, mut loop_start) = match s.sel_range() {
            Some((a, b)) if s.cursor >= a && s.cursor < b => (s.cursor, b, a),
            Some((a, b)) => (a, b, a),
            None => (s.cursor, end_all, s.cursor),
        };
        if start >= end {
            if end_all == 0 {
                self.set_status("The session is empty: drag a file from the Files panel onto a track.");
                return;
            }
            start = 0;
            loop_start = 0;
            end = end_all;
        }
        self.engine.play_mix(Arc::new(s.mix_state()), loop_start as f64, end as f64, self.looping, s.id);
        if start != loop_start {
            self.engine.seek(start as f64);
        }
        self.mt.sent_version = s.version;
        self.play_origin = start;
    }

    fn mt_toggle_record(&mut self) {
        if self.engine.is_recording() {
            self.mt_stop_recording();
            return;
        }
        let Some(si) = self.active_session else { return };
        let s = &mut self.sessions[si];
        if s.tracks.is_empty() {
            s.add_track();
        }
        let ti = match s.tracks.iter().position(|t| t.arm) {
            Some(i) => i,
            None => {
                let i = s.selected_track.min(s.tracks.len() - 1);
                s.tracks[i].arm = true;
                i
            }
        };
        let (track_id, start, session_id) = (s.tracks[ti].id, s.cursor, s.id);
        let state = Arc::new(s.mix_state());
        let end = s.end();
        self.engine.stop();
        match self.engine.start_recording() {
            Ok((rate, _)) => {
                // Play the other tracks while recording (overdub).
                if end > start {
                    self.engine.play_mix(state, start as f64, end as f64, false, session_id);
                }
                self.mt.rec = Some(MtRec { session_id, track_id, start });
                let tname = self.sessions[si].tracks[ti].name.clone();
                self.set_status(format!("Recording onto {tname} at {rate} Hz. Press Space or Stop to finish."));
            }
            Err(e) => {
                self.dialog = Some(Dialog::Message {
                    title: "Can't record".into(),
                    text: format!("{e}\n\nCheck the input device in Edit > Preferences > Audio Hardware. On macOS, allow microphone access for Audemo in System Settings > Privacy & Security > Microphone."),
                });
            }
        }
    }

    fn mt_stop_recording(&mut self) {
        let rec = self.mt.rec.take();
        self.engine.stop();
        let Some((audio, rate)) = self.engine.stop_recording() else { return };
        let Some(rec) = rec else { return };
        if audio.first().map(|c| c.is_empty()).unwrap_or(true) {
            self.set_status("Recording was empty.");
            return;
        }
        let target = self.sessions.iter().position(|s| s.id == rec.session_id).and_then(|si| self.sessions[si].tracks.iter().position(|t| t.id == rec.track_id).map(|ti| (si, ti)));
        let tname = target.map(|(si, ti)| self.sessions[si].tracks[ti].name.clone()).unwrap_or_else(|| "Recording".into());
        let n = self.docs.iter().filter(|d| d.name.starts_with(&tname)).count() + 1;
        let id = self.new_id();
        let secs = audio[0].len() as f64 / rate as f64;
        let mut doc = Document::new(id, format!("{tname}_{n:03}"), None, audio, rate, None, "Record");
        doc.dirty = true;
        self.docs.push(doc);
        if self.active.is_none() {
            self.active = Some(self.docs.len() - 1);
        }
        match target {
            Some((si, ti)) => {
                if let Some(cid) = self.mt_insert_doc(rec.session_id, ti, rec.start, id) {
                    // Latency compensation: the take arrived late, so move it earlier.
                    let shift = (self.prefs.rec_offset_ms as f64 * self.sessions[si].sample_rate as f64 / 1000.0) as usize;
                    if let Some(c) = self.sessions[si].clip_mut(cid).filter(|_| shift > 0) {
                        if c.start >= shift {
                            c.start -= shift;
                        } else {
                            let d = (shift - c.start).min(c.len - 1);
                            c.start = 0;
                            c.offset += d;
                            c.len -= d;
                        }
                    }
                }
                self.set_status(format!("Recorded {secs:.1} s onto {tname}"));
            }
            // The track went away: keep the take as a file rather than lose it.
            None => self.set_status(format!("Recorded {secs:.1} s. Its track is gone, so it was kept as the file {tname}_{n:03} in the Files panel.")),
        }
    }

    fn mt_mixdown(&mut self, selection: bool) {
        let Some(s) = self.session() else { return };
        let (a, b) = match (selection, s.sel_range()) {
            (true, Some(r)) => r,
            _ => (0, s.end()),
        };
        if b <= a {
            self.set_status("Nothing to mix down: the session is empty.");
            return;
        }
        let state = Arc::new(s.mix_state());
        let name = format!("{} Mixdown", s.name);
        let rate = s.sample_rate;
        self.spawn_job("Mixdown".into(), JobKind::NewDoc { name, rate }, move || Ok(mix_range(&state, a, b)));
    }

    fn mt_close_session(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        let id = self.sessions[i].id;
        if self.engine.status().tag == id {
            self.engine.stop();
        }
        self.sessions.remove(i);
        self.active_session = if self.sessions.is_empty() { None } else { Some(i.min(self.sessions.len() - 1)) };
    }

    /// Intercept actions that mean something different in the Multitrack
    /// editor. Returns the action when it should run as usual.
    pub fn mt_perform(&mut self, a: Action, _ctx: &egui::Context) -> Option<Action> {
        match a {
            Action::SetMode(m) => {
                self.mode = m;
                if m == Mode::Multitrack && self.sessions.is_empty() {
                    self.actions.push(Action::NewSession);
                }
                return None;
            }
            Action::NewSession => {
                let n = self.sessions.len() + 1;
                let rate = self.doc().map(|d| d.sample_rate).unwrap_or(48000);
                self.dialog = Some(Dialog::NewSession { name: format!("Untitled Session {n}"), rate, tracks: 6 });
                return None;
            }
            Action::MtCloseSession(i) => {
                let rec_session = self.mt.rec.as_ref().map(|r| r.session_id);
                if self.engine.is_recording() && rec_session.is_some() && rec_session == self.sessions.get(i).map(|s| s.id) {
                    self.set_status("Stop recording first.");
                    return None;
                }
                if self.sessions.get(i).map(|s| s.dirty).unwrap_or(false) {
                    self.dialog = Some(Dialog::ConfirmCloseSession { session: i });
                } else {
                    self.mt_close_session(i);
                }
                return None;
            }
            Action::MtInsertDoc(doc_id) => {
                let Some((sid, track, at)) = self.session().map(|s| (s.id, s.selected_track, s.cursor)) else {
                    self.set_status("Create or open a multitrack session first (File > New > Multitrack Session).");
                    return None;
                };
                self.mt_insert_doc(sid, track, at, doc_id);
                return None;
            }
            Action::MtInsertAt { session_id, doc_id, track, at } => {
                self.mt_insert_doc(session_id, track, at, doc_id);
                return None;
            }
            Action::MtEditSource(clip_id) => {
                let doc_id = self.session().and_then(|s| s.clip(clip_id)).and_then(|c| self.session().unwrap().sources.get(&c.source)).and_then(|src| src.doc_id);
                match doc_id.and_then(|id| self.docs.iter().position(|d| d.id == id)) {
                    Some(i) => {
                        self.engine.stop();
                        self.active = Some(i);
                        self.mode = Mode::Waveform;
                    }
                    None => self.set_status("That clip's file is not open."),
                }
                return None;
            }
            Action::MtInsertFiles | Action::MtDropFiles(_) => {
                let Some((sid, track, at)) = self.session().map(|s| (s.id, s.selected_track, s.cursor)) else {
                    if let Action::MtDropFiles(p) = a {
                        return Some(Action::OpenPaths(p));
                    }
                    self.set_status("Create or open a multitrack session first.");
                    return None;
                };
                let paths = match a {
                    Action::MtDropFiles(p) => p,
                    _ => rfd::FileDialog::new().add_filter("Audio files", io::OPEN_EXTENSIONS).pick_files().unwrap_or_default(),
                };
                let mut t = track;
                for p in paths {
                    if p.extension().map(|e| e.eq_ignore_ascii_case(SESSION_EXT)).unwrap_or(false) {
                        self.open_session(&p);
                        continue;
                    }
                    // Files already open go straight in; others load first.
                    if let Some(doc_id) = self.docs.iter().find(|d| d.path.as_deref() == Some(p.as_path())).map(|d| d.id) {
                        self.mt_insert_doc(sid, t, at, doc_id);
                    } else {
                        self.load_async(p, LoadIntent::MtInsert { session_id: sid, track: t, at });
                    }
                    t += 1;
                }
                return None;
            }
            _ => {}
        }
        if matches!(a, Action::MtSplit) && self.mode != Mode::Multitrack {
            return None;
        }
        let session_cmd = matches!(a, Action::MtAddTrack | Action::MtDeleteTrack | Action::MtSplit | Action::MtMixdown(_));
        if session_cmd && self.session().is_none() {
            self.set_status("Create or open a multitrack session first (File > New > Multitrack Session).");
            return None;
        }
        if !session_cmd && !self.in_multitrack() {
            return Some(a);
        }
        let recording_here = self.engine.is_recording() && self.mt.rec.is_some();
        let si = self.active_session.unwrap();
        let structural = matches!(a, Action::Undo | Action::Redo | Action::JumpHistory(_) | Action::MtDeleteTrack | Action::Close(usize::MAX) | Action::Delete | Action::Cut | Action::Paste);
        if structural && self.mt.drag.is_some() {
            return None;
        }
        if structural && recording_here {
            self.set_status("Stop recording first.");
            return None;
        }
        let sr = self.sessions[si].sample_rate as f64;
        let st = self.engine.status();
        let playing = st.playing && st.tag == self.sessions[si].id;
        match a {
            Action::MtAddTrack => {
                let s = &mut self.sessions[si];
                let i = s.add_track();
                s.selected_track = i;
            }
            Action::MtDeleteTrack => {
                let s = &mut self.sessions[si];
                let i = s.selected_track;
                s.remove_track(i);
            }
            Action::MtSplit => {
                let pos = if playing { st.pos as usize } else { self.sessions[si].cursor };
                let (n, skipped) = self.sessions[si].split_at(pos);
                if n == 0 && skipped > 0 {
                    self.set_status("Can't split inside a clip's fade: shorten the fade or move the playhead.");
                } else if n == 0 {
                    self.set_status("Put the cursor over a clip (select it, or select its track) to split it.");
                }
            }
            Action::MtMixdown(sel) => self.mt_mixdown(sel),
            Action::PlayToggle | Action::Stop if recording_here => self.mt_stop_recording(),
            Action::PlayToggle => self.mt_play_toggle(),
            Action::Pause => {
                if playing {
                    self.mt_play_toggle();
                } else {
                    self.mt_play_toggle();
                }
            }
            Action::Stop => {
                self.paused = None;
                self.engine.stop();
            }
            Action::Record => self.mt_toggle_record(),
            Action::Undo | Action::Redo => {
                let s = &mut self.sessions[si];
                let l = if matches!(a, Action::Undo) { s.undo() } else { s.redo() };
                if let Some(l) = l {
                    self.set_status(format!("{} {l}", if matches!(a, Action::Undo) { "Undo" } else { "Redo" }));
                }
            }
            Action::JumpHistory(target) => {
                let s = &mut self.sessions[si];
                while s.undo.len() > target && s.undo().is_some() {}
                while s.undo.len() < target && s.redo().is_some() {}
            }
            Action::Delete => {
                let s = &mut self.sessions[si];
                let ids: Vec<u64> = s.selected_clips.iter().copied().collect();
                if s.delete_clips(&ids) == 0 {
                    self.set_status("Select a clip to delete.");
                }
            }
            Action::Copy | Action::Cut => {
                let s = &self.sessions[si];
                let mut items: Vec<(usize, Clip)> = s
                    .tracks
                    .iter()
                    .enumerate()
                    .flat_map(|(t, tr)| tr.clips.iter().filter(|c| s.selected_clips.contains(&c.id)).map(move |c| (t, c.clone())))
                    .collect();
                if items.is_empty() {
                    self.set_status("Select one or more clips first.");
                    return None;
                }
                let t0 = items.iter().map(|i| i.0).min().unwrap();
                let s0 = items.iter().map(|i| i.1.start).min().unwrap();
                for it in items.iter_mut() {
                    it.0 -= t0;
                    it.1.start -= s0;
                }
                let n = items.len();
                self.mt.clipboard = Some((s.id, items));
                if matches!(a, Action::Cut) {
                    let ids: Vec<u64> = self.sessions[si].selected_clips.iter().copied().collect();
                    self.sessions[si].delete_clips(&ids);
                }
                self.set_status(format!("{} {n} clip(s)", if matches!(a, Action::Cut) { "Cut" } else { "Copied" }));
            }
            Action::Paste => {
                let Some((sid, items)) = self.mt.clipboard.clone() else {
                    self.set_status("No clips on the clipboard.");
                    return None;
                };
                let s = &mut self.sessions[si];
                if sid != s.id {
                    self.set_status("Clips can only be pasted into the session they came from.");
                    return None;
                }
                s.push_undo("Paste Clips");
                s.selected_clips.clear();
                let (t0, at) = (s.selected_track, s.cursor);
                let mut end = at;
                for (dt, mut c) in items {
                    let t = (t0 + dt).min(s.tracks.len().saturating_sub(1));
                    c.id = s.new_id();
                    c.start += at;
                    end = end.max(c.end());
                    s.selected_clips.insert(c.id);
                    if let Some(tr) = s.tracks.get_mut(t) {
                        tr.clips.push(c);
                    }
                }
                s.cursor = end;
                s.touch();
            }
            Action::SelectAll => {
                let s = &mut self.sessions[si];
                s.selected_clips = s.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
            }
            Action::Deselect => {
                let s = &mut self.sessions[si];
                s.selected_clips.clear();
                s.sel = None;
            }
            Action::ToStart | Action::ToEnd => {
                let s = &mut self.sessions[si];
                s.cursor = if matches!(a, Action::ToStart) { 0 } else { s.end() };
                let span = s.view_end - s.view_start;
                s.view_start = s.cursor as f64 - span * 0.1;
                s.view_end = s.view_start + span;
                s.clamp_view();
                if playing {
                    self.engine.stop();
                    self.mt_play_toggle();
                }
            }
            Action::PrevMarker | Action::NextMarker => {
                let next = matches!(a, Action::NextMarker);
                let s = &mut self.sessions[si];
                let pos = if playing { st.pos as usize } else { s.cursor };
                let mut stops = s.snap_points(None);
                stops.push(s.end());
                stops.sort_unstable();
                stops.dedup();
                let t = if next { stops.into_iter().find(|&p| p > pos) } else { stops.into_iter().rev().find(|&p| p < pos) };
                if let Some(t) = t {
                    s.cursor = t;
                    if playing {
                        self.engine.stop();
                        self.mt_play_toggle();
                    }
                }
            }
            Action::ZoomIn | Action::ZoomOut => {
                let s = &mut self.sessions[si];
                let pos = if playing { st.pos } else { s.cursor as f64 };
                let anchor = if pos >= s.view_start && pos <= s.view_end { pos } else { (s.view_start + s.view_end) / 2.0 };
                s.zoom(if matches!(a, Action::ZoomIn) { 0.5 } else { 2.0 }, anchor);
            }
            Action::ZoomFull => {
                let s = &mut self.sessions[si];
                s.view_start = 0.0;
                s.view_end = s.max_span();
            }
            Action::ZoomSel => {
                let s = &mut self.sessions[si];
                if let Some((a, b)) = s.sel_range() {
                    let pad = (b - a) as f64 * 0.05;
                    s.view_start = a as f64 - pad;
                    s.view_end = b as f64 + pad;
                    s.clamp_view();
                }
            }
            Action::Nudge(secs) => {
                let s = &mut self.sessions[si];
                if playing {
                    self.engine.seek(st.pos + secs * sr);
                } else {
                    s.cursor = (s.cursor as f64 + secs * sr).max(0.0) as usize;
                }
            }
            Action::SetCursor(p) => {
                self.sessions[si].cursor = p;
                if playing {
                    self.engine.stop();
                    self.mt_play_toggle();
                }
            }
            Action::ClearHistory => {
                let s = &mut self.sessions[si];
                s.undo.clear();
                s.redo.clear();
            }
            Action::Save => self.save_session(false),
            Action::SaveAs => self.save_session(true),
            Action::Close(usize::MAX) => self.actions.push(Action::MtCloseSession(si)),
            Action::AmpIn | Action::AmpOut => {}
            Action::OpenEffect(_)
            | Action::ApplyEffect(..)
            | Action::ApplyFavorite(..)
            | Action::RepeatLast
            | Action::ApplyRack
            | Action::Crop
            | Action::CaptureNoise
            | Action::Convert(..)
            | Action::AddMarker
            | Action::CopyToNew
            | Action::PasteNew
            | Action::MixPaste { .. }
            | Action::SaveSelectionAs
            | Action::AmplitudeStatistics => {
                self.set_status("That works on files: double-click a clip to open its file in the Waveform editor (or press 9).");
            }
            other => return Some(other),
        }
        None
    }

    // ------------------------------------------------------------ UI

    pub fn multitrack_ui(&mut self, ui: &mut Ui) {
        self.mt_tabs(ui);
        let Some(si) = self.active_session.filter(|&i| i < self.sessions.len()) else {
            let rect = ui.available_rect_before_wrap();
            ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x2b, 0x2b, 0x2b));
            ui.allocate_ui_at_rect(rect, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(rect.height() * 0.38);
                    ui.label(RichText::new("No multitrack session open").color(TEXT_DIM));
                    ui.add_space(6.0);
                    if ui.button("New Multitrack Session…").clicked() {
                        self.actions.push(Action::NewSession);
                    }
                });
            });
            return;
        };
        match self.mt.view {
            MtView::Editor => self.mt_editor(ui, si),
            MtView::Mixer => self.mt_mixer(ui, si),
        }
    }

    fn mt_tabs(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
        ui.painter().rect_filled(header, 0.0, BG_HEADER);
        let mut switch = None;
        let mut close = false;
        ui.allocate_ui_at_rect(header, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (view, label) in [(MtView::Editor, "Editor"), (MtView::Mixer, "Mixer")] {
                    let title = if view == MtView::Editor {
                        match self.session() {
                            Some(s) => format!("Editor: {}", s.display_name()),
                            None => "Editor".to_string(),
                        }
                    } else {
                        label.to_string()
                    };
                    let active = self.mt.view == view;
                    let font = if active { bold(12.0) } else { FontId::proportional(12.0) };
                    let w = ui.fonts(|f| f.layout_no_wrap(title.clone(), font.clone(), TEXT).size().x) + 16.0;
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
                    if active {
                        ui.painter().rect_filled(Rect::from_min_max(pos2(r.left(), r.top() + 1.0), pos2(r.right(), header.bottom())), Rounding { nw: 3.0, ne: 3.0, sw: 0.0, se: 0.0 }, BG_PANEL);
                    }
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, &title, font, if active { Color32::WHITE } else { TEXT_DIM });
                    if resp.clicked() {
                        self.mt.view = view;
                    }
                    if view == MtView::Editor && !self.sessions.is_empty() {
                        ui.menu_button(RichText::new("▾").color(TEXT), |ui| {
                            for (i, s) in self.sessions.iter().enumerate() {
                                if ui.selectable_label(Some(i) == self.active_session, s.display_name()).clicked() {
                                    switch = Some(i);
                                    ui.close_menu();
                                }
                            }
                        });
                        if ui.small_button("×").on_hover_text("Close session").clicked() {
                            close = true;
                        }
                    }
                }
            });
        });
        ui.add_space(24.0);
        if let Some(i) = switch {
            self.engine.stop();
            self.active_session = Some(i);
        }
        if close {
            if let Some(i) = self.active_session {
                self.actions.push(Action::MtCloseSession(i));
            }
        }
    }

    fn mt_editor(&mut self, ui: &mut Ui, si: usize) {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        if full.width() < 300.0 || full.height() < 100.0 {
            return;
        }
        let painter = ui.painter_at(full);
        painter.rect_filled(full, 0.0, Color32::from_rgb(0x2b, 0x2b, 0x2b));
        let ruler = Rect::from_min_max(pos2(full.left() + HEADER_W, full.top()), pos2(full.right(), full.top() + RULER_H));
        let master = Rect::from_min_max(pos2(full.left(), full.bottom() - MASTER_H), full.max);
        let body = Rect::from_min_max(pos2(full.left(), ruler.bottom() + 1.0), pos2(full.right(), master.top() - 1.0));
        let lanes_x = body.left() + HEADER_W;
        let lanes = Rect::from_min_max(pos2(lanes_x, body.top()), body.max);

        let st = self.engine.status();
        let mods = ui.input(|i| i.modifiers);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let recording = self.engine.is_recording() && self.mt.rec.as_ref().map(|r| r.session_id == self.sessions[si].id).unwrap_or(false);
        let rec_info = if recording {
            let r = self.mt.rec.as_ref().unwrap();
            let in_rate = self.engine.recording_format().map(|f| f.0).unwrap_or(48000);
            Some((r.track_id, r.start, in_rate))
        } else {
            None
        };
        let s = &mut self.sessions[si];
        let sr = s.sample_rate as f64;
        let playing = st.playing && st.tag == s.id;
        if s.view_end <= s.view_start {
            s.view_end = s.view_start + sr * 60.0;
        }
        let v = TView { left: lanes.left(), width: lanes.width(), start: s.view_start, end: s.view_end };

        // ------------------------------------------------ ruler
        painter.rect_filled(Rect::from_min_max(full.min, pos2(full.right(), ruler.bottom())), 0.0, Color32::from_rgb(0x2b, 0x2b, 0x2b));
        painter.line_segment([pos2(full.left(), ruler.bottom()), pos2(full.right(), ruler.bottom())], Stroke::new(1.0_f32, BORDER));
        painter.text(pos2(full.left() + 6.0, ruler.center().y), Align2::LEFT_CENTER, "hms", FontId::proportional(11.0), TEXT_DIM);
        {
            let px_per_s = v.width as f64 / ((v.end - v.start) / sr);
            let step = *RULER_STEPS.iter().find(|&&x| x * px_per_s >= 84.0).unwrap_or(&3600.0);
            let first = (v.start / sr / step).floor() as i64;
            let last = (v.end / sr / step).ceil() as i64;
            for i in first.max(0)..=last {
                let t = i as f64 * step;
                let x = v.x(t * sr);
                if x >= lanes.left() - 0.5 && x <= lanes.right() + 0.5 {
                    painter.line_segment([pos2(x, ruler.bottom() - 9.0), pos2(x, ruler.bottom())], Stroke::new(1.0_f32, TEXT_DIM));
                    painter.text(pos2(x + 3.0, ruler.top() + 3.0), Align2::LEFT_TOP, ruler_label(t, step), FontId::monospace(10.5), TEXT_DIM);
                    // Faint grid line down through the tracks.
                    painter.line_segment([pos2(x, body.top()), pos2(x, body.bottom())], Stroke::new(1.0_f32, Color32::from_rgb(0x30, 0x30, 0x30)));
                }
                for k in 1..5 {
                    let xm = v.x((t + step * k as f64 / 5.0) * sr);
                    if xm >= lanes.left() && xm <= lanes.right() {
                        painter.line_segment([pos2(xm, ruler.bottom() - 4.0), pos2(xm, ruler.bottom())], Stroke::new(1.0_f32, BORDER));
                    }
                }
            }
            let resp = ui.interact(ruler, ui.id().with(("mt_ruler", s.id)), Sense::click_and_drag());
            if resp.clicked() || resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let pos = v.s(p.x.clamp(lanes.left(), lanes.right())).max(0.0) as usize;
                    s.cursor = pos;
                    if resp.clicked() || resp.drag_stopped() {
                        self.actions.push(Action::SetCursor(pos));
                    }
                }
            }
        }

        // ------------------------------------------------ track geometry
        let total_h: f32 = s.tracks.iter().map(|t| t.height).sum();
        let max_scroll = (total_h - body.height() + 20.0).max(0.0);
        s.scroll_y = s.scroll_y.clamp(0.0, max_scroll);
        let mut rows: Vec<Rect> = Vec::with_capacity(s.tracks.len());
        let mut y = body.top() - s.scroll_y;
        for t in &s.tracks {
            rows.push(Rect::from_min_max(pos2(body.left(), y), pos2(body.right(), y + t.height - 1.0)));
            y += t.height;
        }
        let track_at = |py: f32| -> Option<usize> { rows.iter().position(|r| py >= r.top() && py < r.bottom() + 1.0) };

        // ------------------------------------------------ lanes background + clips
        let clip_painter = painter.with_clip_rect(lanes.intersect(body));
        let sel = s.sel_range();
        for (ti, t) in s.tracks.iter().enumerate() {
            let row = rows[ti];
            if row.bottom() < body.top() || row.top() > body.bottom() {
                continue;
            }
            let lane = Rect::from_min_max(pos2(lanes_x, row.top()), row.max);
            let bg = if ti == s.selected_track { Color32::from_rgb(0x1a, 0x20, 0x26) } else { LANE_BG };
            clip_painter.rect_filled(lane, 0.0, bg);
            if let Some((a, b)) = sel {
                let x0 = v.x(a as f64).max(lane.left());
                let x1 = v.x(b as f64).min(lane.right());
                if x1 > x0 {
                    clip_painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, lane.y_range()), 0.0, Color32::from_white_alpha(18));
                }
            }
            for c in &t.clips {
                let x0 = v.x(c.start as f64);
                let x1 = v.x(c.end() as f64);
                if x1 < lane.left() || x0 > lane.right() {
                    continue;
                }
                let r = Rect::from_min_max(pos2(x0, lane.top() + 1.0), pos2(x1.max(x0 + 2.0), lane.bottom() - 1.0));
                let selected = s.selected_clips.contains(&c.id);
                draw_clip(&clip_painter, r, c, s.sources.get(&c.source).map(|a| a.as_ref()), colour(t.colour), selected, t.mute || c.mute, &v, lane);
            }
            // Automatic crossfades where clips overlap.
            for (ia, ib) in crossfade_pairs(&t.clips) {
                let (ca, cb) = (&t.clips[ia], &t.clips[ib]);
                let (x0, x1) = (v.x(cb.start as f64), v.x(ca.end() as f64));
                if x1 < lane.left() || x0 > lane.right() || x1 - x0 < 2.0 {
                    continue;
                }
                let area = Rect::from_min_max(pos2(x0, lane.top() + CLIP_BAR + 1.0), pos2(x1, lane.bottom() - 1.0));
                clip_painter.rect_filled(area, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 70));
                let n = 20;
                let curve = |up: bool| -> Vec<egui::Pos2> {
                    (0..=n)
                        .map(|k| {
                            let f = k as f32 / n as f32;
                            let g = if up { (std::f32::consts::FRAC_PI_2 * f).sin() } else { (std::f32::consts::FRAC_PI_2 * (1.0 - f)).sin() };
                            pos2(area.left() + f * area.width(), area.bottom() - g * area.height())
                        })
                        .collect()
                };
                clip_painter.add(Shape::line(curve(true), Stroke::new(1.0_f32, VOL_ENV_COL)));
                clip_painter.add(Shape::line(curve(false), Stroke::new(1.0_f32, VOL_ENV_COL)));
            }
            // Automation lines.
            if t.show_env {
                let area = env_area(lane);
                for pan in [true, false] {
                    let env = env_of(t, pan);
                    let col = if pan { PAN_ENV_COL } else { VOL_ENV_COL };
                    let constant = if pan { t.pan } else { t.volume_db };
                    if env.is_empty() {
                        let y = env_y(pan, constant, area);
                        clip_painter.line_segment([pos2(lane.left(), y), pos2(lane.right(), y)], Stroke::new(1.0_f32, col.linear_multiply(0.6)));
                        continue;
                    }
                    let mut pts = vec![pos2(lane.left(), env_y(pan, env.points[0].1, area))];
                    for (p, val) in &env.points {
                        pts.push(pos2(v.x(*p as f64), env_y(pan, *val, area)));
                    }
                    pts.push(pos2(lane.right(), env_y(pan, env.points.last().unwrap().1, area)));
                    clip_painter.add(Shape::line(pts, Stroke::new(1.5_f32, col)));
                    for (p, val) in &env.points {
                        let c = pos2(v.x(*p as f64), env_y(pan, *val, area));
                        if c.x >= lane.left() - 4.0 && c.x <= lane.right() + 4.0 {
                            clip_painter.rect_filled(Rect::from_center_size(c, vec2(7.0, 7.0)), 1.0, col);
                            clip_painter.rect_stroke(Rect::from_center_size(c, vec2(7.0, 7.0)), 1.0, Stroke::new(1.0_f32, Color32::BLACK));
                        }
                    }
                }
            }
            // Live recording on this track.
            if let Some((rec_track, rec_start, in_rate)) = rec_info {
                if rec_track == t.id {
                    if let Some(rv) = self.engine.rec_view() {
                        let ratio = in_rate as f64 / sr;
                        let frames = rv.frames();
                        let x0 = v.x(rec_start as f64);
                        let x1 = v.x(rec_start as f64 + frames as f64 / ratio);
                        let r = Rect::from_min_max(pos2(x0, lane.top() + 1.0), pos2(x1.max(x0 + 2.0), lane.bottom() - 1.0));
                        clip_painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(0xd8, 0x32, 0x2c, 70));
                        clip_painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, RECORD));
                        let body_r = Rect::from_min_max(pos2(r.left(), r.top() + CLIP_BAR), r.max);
                        let mid = body_r.center().y;
                        let amp = body_r.height() * 0.48;
                        let mut shapes = Vec::new();
                        let spp_in = v.spp() * ratio;
                        let mut x = x0.max(lane.left()).floor();
                        while x < x1.min(lane.right()) {
                            let f0 = (v.s(x) - rec_start as f64) * ratio;
                            let ia = (f0.max(0.0) as usize).saturating_sub(1);
                            let ib = ((f0 + spp_in.max(1.0)).ceil() as usize).min(frames).max(ia + 1);
                            if ia < frames {
                                let (lo, hi) = rv.min_max(0, ia, ib);
                                shapes.push(Shape::line_segment([pos2(x + 0.5, mid - hi * amp), pos2(x + 0.5, (mid - lo * amp).max(mid - hi * amp + 1.0))], Stroke::new(1.0_f32, Color32::from_rgb(0xff, 0xb0, 0xa8))));
                            }
                            x += 1.0;
                        }
                        clip_painter.extend(shapes);
                        clip_painter.text(pos2(r.left() + 4.0, r.top() + 2.0), Align2::LEFT_TOP, "Recording…", FontId::proportional(10.5), Color32::WHITE);
                    }
                }
            }
        }

        // ------------------------------------------------ lane interaction
        let lane_area = lanes.intersect(body);
        let resp = ui.interact(lane_area, ui.id().with(("mt_lanes", s.id)), Sense::click_and_drag());
        let hit = |p: egui::Pos2, s: &Session| -> Option<(usize, Option<(u64, Zone)>)> {
            let ti = track_at(p.y)?;
            let row = rows[ti];
            let mut best: Option<(u64, Zone)> = None;
            // Topmost (last drawn) clip wins.
            for c in s.tracks[ti].clips.iter().rev() {
                let x0 = v.x(c.start as f64);
                let x1 = v.x(c.end() as f64);
                if p.x < x0 - 2.0 || p.x > x1 + 2.0 {
                    continue;
                }
                let fi = x0 + (c.fade_in as f64 / v.spp()) as f32;
                let fo = x1 - (c.fade_out as f64 / v.spp()) as f32;
                let near_top = p.y < row.top() + CLIP_BAR + 9.0;
                let zone = if near_top && (p.x - fi).abs() <= 6.0 && x1 - x0 > 24.0 {
                    Zone::FadeIn
                } else if near_top && (p.x - fo).abs() <= 6.0 && x1 - x0 > 24.0 {
                    Zone::FadeOut
                } else if (p.x - x0).abs() <= EDGE_PX && x1 - x0 > 12.0 {
                    Zone::Left
                } else if (p.x - x1).abs() <= EDGE_PX && x1 - x0 > 12.0 {
                    Zone::Right
                } else if p.x >= x0 && p.x <= x1 {
                    Zone::Body
                } else {
                    continue;
                };
                best = Some((c.id, zone));
                break;
            }
            Some((ti, best))
        };
        // Automation points and lines take priority over clips when shown.
        let env_hit = |p: egui::Pos2, s: &Session| -> Option<(usize, bool, EnvHit)> {
            let ti = track_at(p.y)?;
            let t = &s.tracks[ti];
            if !t.show_env {
                return None;
            }
            let area = env_area(Rect::from_min_max(pos2(lanes_x, rows[ti].top()), rows[ti].max));
            for pan in [false, true] {
                for (i, (pp, val)) in env_of(t, pan).points.iter().enumerate() {
                    let c = pos2(v.x(*pp as f64), env_y(pan, *val, area));
                    if (c.x - p.x).abs() <= 6.0 && (c.y - p.y).abs() <= 6.0 {
                        return Some((ti, pan, EnvHit::Point(i)));
                    }
                }
            }
            let over_clip = matches!(hit(p, s), Some((_, Some(_))));
            for pan in [false, true] {
                let env = env_of(t, pan);
                // An empty envelope's flat line only catches clicks away from clips,
                // so selecting a clip doesn't create automation by accident.
                if env.is_empty() && over_clip {
                    continue;
                }
                let val = env.value_at(v.s(p.x).max(0.0) as usize).unwrap_or(if pan { t.pan } else { t.volume_db });
                if (env_y(pan, val, area) - p.y).abs() <= 5.0 {
                    return Some((ti, pan, EnvHit::Line));
                }
            }
            None
        };
        let env_area_of = |ti: usize| env_area(Rect::from_min_max(pos2(lanes_x, rows[ti].top()), rows[ti].max));
        // Cursor shape.
        if let Some(p) = pointer.filter(|p| lane_area.contains(*p)) {
            if self.mt.drag.is_none() && env_hit(p, s).is_some() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            } else if self.mt.drag.is_none() {
                match hit(p, s) {
                    Some((_, Some((_, Zone::Left | Zone::Right)))) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                    Some((_, Some((_, Zone::FadeIn | Zone::FadeOut)))) => ui.ctx().set_cursor_icon(CursorIcon::PointingHand),
                    Some((_, Some((_, Zone::Body)))) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                    _ => ui.ctx().set_cursor_icon(CursorIcon::Text),
                }
            }
        }
        let snap = |x_sample: f64, s: &Session, exclude: Option<u64>| -> f64 {
            if mods.alt {
                return x_sample;
            }
            let px = v.x(x_sample);
            let mut best = (SNAP_PX, x_sample);
            for p in s.snap_points(exclude) {
                let d = (v.x(p as f64) - px).abs();
                if d < best.0 {
                    best = (d, p as f64);
                }
            }
            best.1
        };
        if resp.drag_started() {
            // egui reports a drag only after the pointer has moved a few
            // pixels, so hit-test where the button went down, not where the
            // pointer is now; otherwise small targets (fade handles, clip
            // edges, automation points) are missed and the clip moves instead.
            let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            if let Some(p) = origin {
                let pos = v.s(p.x).max(0.0);
                let env = env_hit(p, s);
                if let Some((ti, pan, h)) = env {
                    s.selected_track = ti;
                    s.push_undo(if pan { "Pan Automation" } else { "Volume Automation" });
                    let idx = match h {
                        EnvHit::Point(i) => i,
                        EnvHit::Line => {
                            let val = env_value(pan, p.y, env_area_of(ti));
                            env_of_mut(&mut s.tracks[ti], pan).insert(pos as usize, val)
                        }
                    };
                    s.touch();
                    self.mt.drag = Some(MtDrag::EnvPoint(ti, pan, idx));
                }
                match if env.is_some() { None } else { hit(p, s) } {
                    Some((ti, Some((id, zone)))) => {
                        s.selected_track = ti;
                        if !s.selected_clips.contains(&id) {
                            if !(mods.command || mods.shift) {
                                s.selected_clips.clear();
                            }
                            s.selected_clips.insert(id);
                        }
                        let label = match zone {
                            Zone::Body => "Move Clip",
                            Zone::Left | Zone::Right => "Trim Clip",
                            Zone::FadeIn | Zone::FadeOut => "Clip Fade",
                        };
                        s.push_undo(label);
                        self.mt.drag = Some(match zone {
                            Zone::Body => {
                                let c = s.clip(id).unwrap();
                                let items = s
                                    .selected_clips
                                    .iter()
                                    .filter_map(|cid| s.find_clip(*cid).map(|(t, ci)| (*cid, s.tracks[t].clips[ci].start, t)))
                                    .collect();
                                MtDrag::Move { grabbed: id, grab: pos - c.start as f64, track0: ti, items }
                            }
                            Zone::Left => MtDrag::TrimStart(id),
                            Zone::Right => MtDrag::TrimEnd(id),
                            Zone::FadeIn => MtDrag::FadeIn(id),
                            Zone::FadeOut => MtDrag::FadeOut(id),
                        });
                    }
                    Some((ti, None)) => {
                        s.selected_track = ti;
                        if !(mods.command || mods.shift) {
                            s.selected_clips.clear();
                        }
                        let a = snap(pos, s, None) as usize;
                        s.cursor = a;
                        s.sel = None;
                        self.mt.drag = Some(MtDrag::Select(a));
                    }
                    None => {}
                }
            }
        }
        if resp.dragged() {
            if let (Some(p), Some(drag)) = (resp.interact_pointer_pos(), self.mt.drag.as_ref()) {
                let pos = v.s(p.x.clamp(lanes.left() - 40.0, lanes.right() + 40.0)).max(0.0);
                match drag {
                    MtDrag::Move { grabbed, grab, track0, items } => {
                        let orig = items.iter().find(|i| i.0 == *grabbed).map(|i| i.1).unwrap_or(0) as f64;
                        let mut start = (pos - grab).max(0.0);
                        // Snap either edge of the grabbed clip.
                        let len = s.clip(*grabbed).map(|c| c.len).unwrap_or(0) as f64;
                        let snapped_start = snap(start, s, Some(*grabbed));
                        if snapped_start != start {
                            start = snapped_start;
                        } else {
                            let e = snap(start + len, s, Some(*grabbed));
                            if e != start + len {
                                start = (e - len).max(0.0);
                            }
                        }
                        let delta = start - orig;
                        let n_tracks = s.tracks.len() as i64;
                        let min_track = (items.iter().map(|i| i.2).min().unwrap_or(0) as i64).min(n_tracks - 1);
                        let max_track = (items.iter().map(|i| i.2).max().unwrap_or(0) as i64).clamp(min_track, n_tracks - 1);
                        let tdelta = (track_at(p.y).map(|t| t as i64).unwrap_or(*track0 as i64) - *track0 as i64).clamp(-min_track, n_tracks - 1 - max_track);
                        let moves: Vec<(u64, usize, usize)> = items.iter().map(|(id, st0, t0)| (*id, (*st0 as f64 + delta).max(0.0) as usize, (*t0 as i64 + tdelta) as usize)).collect();
                        for (id, st0, t) in moves {
                            s.move_clip(id, st0, t);
                        }
                    }
                    MtDrag::TrimStart(id) => {
                        let id = *id;
                        let x = snap(pos, s, Some(id)) as usize;
                        s.trim_start(id, x);
                    }
                    MtDrag::TrimEnd(id) => {
                        let id = *id;
                        let x = snap(pos, s, Some(id)) as usize;
                        s.trim_end(id, x);
                    }
                    MtDrag::FadeIn(id) | MtDrag::FadeOut(id) => {
                        let fade_in = matches!(drag, MtDrag::FadeIn(_));
                        let id = *id;
                        if let Some(c) = s.clip_mut(id) {
                            if fade_in {
                                c.fade_in = (pos as i64 - c.start as i64).clamp(0, (c.len - c.fade_out) as i64) as usize;
                            } else {
                                c.fade_out = (c.end() as i64 - pos as i64).clamp(0, (c.len - c.fade_in) as i64) as usize;
                            }
                        }
                        s.touch();
                    }
                    MtDrag::EnvPoint(ti, pan, idx) => {
                        let (ti, pan, idx) = (*ti, *pan, *idx);
                        if ti < s.tracks.len() {
                            let area = env_area_of(ti);
                            let val = env_value(pan, p.y, area);
                            env_of_mut(&mut s.tracks[ti], pan).move_point(idx, pos as usize, val);
                            s.touch();
                            let label = if pan {
                                if val.abs() < 0.5 { "C".to_string() } else if val < 0.0 { format!("L {:.0}", -val) } else { format!("R {val:.0}") }
                            } else if val <= VOL_ENV_MIN {
                                "-∞ dB".to_string()
                            } else {
                                format!("{val:+.1} dB")
                            };
                            painter.text(p + vec2(12.0, -12.0), Align2::LEFT_BOTTOM, label, FontId::monospace(11.0), if pan { PAN_ENV_COL } else { VOL_ENV_COL });
                        }
                    }
                    MtDrag::Select(anchor) => {
                        let b = snap(pos, s, None) as usize;
                        let (lo, hi) = if b < *anchor { (b, *anchor) } else { (*anchor, b) };
                        s.sel = if hi > lo { Some((lo, hi)) } else { None };
                        s.cursor = lo;
                    }
                }
            }
        }
        if resp.drag_stopped() {
            let was_edit = !matches!(self.mt.drag, Some(MtDrag::Select(_)) | None);
            if was_edit && s.undo.last().map(|u| u.tracks == s.tracks).unwrap_or(false) {
                s.undo.pop();
            }
            if matches!(self.mt.drag, Some(MtDrag::Select(_))) && playing {
                self.actions.push(Action::SetCursor(s.cursor));
            }
            self.mt.drag = None;
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let env = env_hit(p, s);
                match if env.is_some() { None } else { hit(p, s) } {
                    Some((ti, Some((id, _)))) => {
                        s.selected_track = ti;
                        if mods.command || mods.shift {
                            if !s.selected_clips.remove(&id) {
                                s.selected_clips.insert(id);
                            }
                        } else {
                            s.selected_clips.clear();
                            s.selected_clips.insert(id);
                        }
                    }
                    Some((ti, None)) => {
                        s.selected_track = ti;
                        s.selected_clips.clear();
                        s.sel = None;
                        let pos = v.s(p.x).max(0.0) as usize;
                        s.cursor = pos;
                        self.actions.push(Action::SetCursor(pos));
                    }
                    None => {}
                }
            }
        }
        if resp.double_clicked() {
            let p = resp.interact_pointer_pos();
            let env = p.and_then(|p| env_hit(p, s));
            if let Some((ti, pan, EnvHit::Point(i))) = env {
                s.push_undo("Delete Automation Point");
                env_of_mut(&mut s.tracks[ti], pan).points.remove(i);
                s.touch();
            } else if let (Some((ti, pan, EnvHit::Line)), Some(p)) = (env, p) {
                s.push_undo(if pan { "Pan Automation" } else { "Volume Automation" });
                let val = env_value(pan, p.y, env_area_of(ti));
                env_of_mut(&mut s.tracks[ti], pan).insert(v.s(p.x).max(0.0) as usize, val);
                s.touch();
            } else if env.is_none() {
                if let Some((_, Some((id, _)))) = resp.interact_pointer_pos().and_then(|p| hit(p, s)) {
                    self.actions.push(Action::MtEditSource(id));
                }
            }
        }
        if resp.secondary_clicked() {
            let env = resp.interact_pointer_pos().and_then(|p| env_hit(p, s));
            self.mt.context_env = env.map(|(ti, pan, h)| (ti, pan, if let EnvHit::Point(i) = h { Some(i) } else { None }));
            let h = if env.is_some() { None } else { resp.interact_pointer_pos().and_then(|p| hit(p, s)) };
            self.mt.context_clip = h.and_then(|h| h.1.map(|c| c.0));
            self.mt.context_track = h.map(|h| h.0);
            if let Some(id) = self.mt.context_clip {
                if !s.selected_clips.contains(&id) {
                    s.selected_clips.clear();
                    s.selected_clips.insert(id);
                }
            }
            if let Some(t) = self.mt.context_track {
                s.selected_track = t;
            }
        }
        // Files dragged in from the Files panel.
        if let Some(p) = pointer.filter(|p| lane_area.contains(*p)) {
            if resp.dnd_hover_payload::<DocDrag>().is_some() {
                if let Some(ti) = track_at(p.y) {
                    let row = rows[ti];
                    let x = p.x.max(lanes.left());
                    painter.line_segment([pos2(x, row.top()), pos2(x, row.bottom())], Stroke::new(2.0_f32, HOT));
                    painter.rect_stroke(Rect::from_min_max(pos2(lanes.left(), row.top()), row.max), 0.0, Stroke::new(1.0_f32, HOT));
                }
            }
            if let Some(d) = resp.dnd_release_payload::<DocDrag>() {
                let ti = track_at(p.y).unwrap_or(s.tracks.len());
                let at = snap(v.s(p.x).max(0.0), s, None) as usize;
                let (sid, doc_id) = (s.id, d.0);
                self.actions.push(Action::MtInsertAt { session_id: sid, doc_id, track: ti, at });
            }
        }
        // Wheel: zoom time; Shift scrolls time; Alt (or over the headers) scrolls tracks.
        if let Some(p) = pointer.filter(|p| body.contains(*p)) {
            let scroll = ui.input(|i| i.raw_scroll_delta);
            if scroll != egui::Vec2::ZERO {
                if p.x < lanes.left() || mods.alt {
                    s.scroll_y = (s.scroll_y - scroll.y).clamp(0.0, max_scroll);
                } else if mods.shift || scroll.x.abs() > scroll.y.abs() {
                    let d = if scroll.x != 0.0 { scroll.x } else { scroll.y };
                    let shift = -(d as f64) * v.spp() * 1.5;
                    s.view_start += shift;
                    s.view_end += shift;
                    s.clamp_view();
                } else {
                    let f = 0.85f64.powf(scroll.y as f64 / 40.0);
                    s.zoom(f, v.s(p.x));
                }
            }
        }

        // ------------------------------------------------ cursor, playhead, selection edges
        let cx = v.x(s.cursor as f64);
        if cx >= lanes.left() && cx <= lanes.right() {
            painter.line_segment([pos2(cx, ruler.top()), pos2(cx, body.bottom())], Stroke::new(1.0_f32, CURSOR));
            painter.add(Shape::convex_polygon(vec![pos2(cx - 5.0, ruler.top()), pos2(cx + 5.0, ruler.top()), pos2(cx, ruler.top() + 7.0)], CURSOR, Stroke::NONE));
        }
        let head = if recording { rec_info.map(|(_, start, in_rate)| start as f64 + self.engine.recorded_frames() as f64 * sr / in_rate as f64) } else if playing { Some(st.pos) } else { None };
        if let Some(h) = head {
            let hx = v.x(h);
            if hx >= lanes.left() && hx <= lanes.right() {
                let col = if recording { RECORD } else { PLAYHEAD };
                painter.line_segment([pos2(hx, ruler.top()), pos2(hx, body.bottom())], Stroke::new(1.5_f32, col));
                painter.add(Shape::convex_polygon(vec![pos2(hx - 6.0, ruler.top()), pos2(hx + 6.0, ruler.top()), pos2(hx, ruler.top() + 9.0)], col, Stroke::NONE));
            }
        }
        if let Some((a, b)) = sel {
            for e in [a, b] {
                let x = v.x(e as f64);
                if x >= lanes.left() && x <= lanes.right() {
                    painter.line_segment([pos2(x, ruler.top()), pos2(x, ruler.bottom())], Stroke::new(1.0_f32, Color32::WHITE));
                }
            }
            let (x0, x1) = (v.x(a as f64).max(lanes.left()), v.x(b as f64).min(lanes.right()));
            if x1 > x0 {
                painter.rect_filled(Rect::from_min_max(pos2(x0, ruler.top()), pos2(x1, ruler.bottom())), 0.0, Color32::from_white_alpha(22));
            }
        }

        // ------------------------------------------------ track headers
        let header_clip = Rect::from_min_max(body.min, pos2(lanes_x, body.bottom()));
        let mut delete_track: Option<usize> = None;
        let mut add_track = false;
        let n_tracks = s.tracks.len();
        let mut changed = false;
        let mut toggles: Vec<(usize, bool)> = Vec::new(); // (track, true = mute / false = solo)
        let mut view_changed = false;
        for ti in 0..n_tracks {
            let row = rows[ti];
            if row.bottom() < body.top() || row.top() > body.bottom() {
                continue;
            }
            let hr = Rect::from_min_max(row.min, pos2(lanes_x - 1.0, row.bottom()));
            let selected = ti == s.selected_track;
            let hp = painter.with_clip_rect(header_clip);
            hp.rect_filled(hr, 0.0, if selected { Color32::from_rgb(0x46, 0x46, 0x46) } else { BG_PANEL });
            hp.rect_filled(Rect::from_min_size(hr.min, vec2(4.0, hr.height())), 0.0, colour(s.tracks[ti].colour));
            let hresp = ui.interact(hr, ui.id().with(("mt_head", s.id, ti)), Sense::click());
            if hresp.clicked() {
                s.selected_track = ti;
            }
            hresp.context_menu(|ui| {
                if ui.button("Add Audio Track").clicked() {
                    add_track = true;
                    ui.close_menu();
                }
                if ui.button("Delete Track").clicked() {
                    delete_track = Some(ti);
                    ui.close_menu();
                }
            });
            // Track meter down the right-hand edge.
            let meter_r = Rect::from_min_max(pos2(hr.right() - 10.0, hr.top() + 4.0), pos2(hr.right() - 3.0, hr.bottom() - 4.0));
            draw_meter(&hp, meter_r, self.mt.meters.get(ti).copied().unwrap_or([-120.0; 2]));
            let inner = Rect::from_min_max(pos2(hr.left() + 10.0, hr.top() + 4.0), pos2(hr.right() - 13.0, hr.bottom() - 2.0));
            let mut child = ui.child_ui(inner, Layout::top_down(Align::Min), None);
            child.set_clip_rect(inner.intersect(header_clip));
            let t = &mut s.tracks[ti];
            child.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let r = ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(88.0).font(FontId::proportional(12.0)));
                if r.changed() {
                    changed = true;
                }
                for (label, on, tip, col) in [
                    ("M", t.mute, "Mute", Color32::from_rgb(0x4d, 0xa3, 0xff)),
                    ("S", t.solo, "Solo", Color32::from_rgb(0xe8, 0xc4, 0x3a)),
                    ("R", t.arm, "Arm for Record", RECORD),
                    ("A", t.show_env, "Show volume (yellow) and pan (blue) automation", VOL_ENV_COL),
                ] {
                    let btn = egui::Button::new(RichText::new(label).font(bold(11.0)).color(if on { Color32::BLACK } else { TEXT }))
                        .fill(if on { col } else { Color32::from_rgb(0x2c, 0x2c, 0x2c) })
                        .min_size(vec2(20.0, 18.0));
                    if ui.add(btn).on_hover_text(tip).clicked() {
                        match label {
                            "M" => toggles.push((ti, true)),
                            "S" => toggles.push((ti, false)),
                            "A" => {
                                t.show_env = !t.show_env;
                                view_changed = true;
                            }
                            _ => t.arm = !t.arm,
                        }
                    }
                }
            });
            if inner.height() > 40.0 {
                child.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let auto_tip = "Automated: edit the line on the track (A shows it), or right-click the line to clear it";
                    ui.label(RichText::new("Vol").color(TEXT_DIM).size(11.0));
                    if t.vol_env.is_empty() {
                        let r = hot_drag(ui, egui::DragValue::new(&mut t.volume_db).speed(0.2).range(-96.0..=12.0).fixed_decimals(1).suffix(" dB"));
                        if r.double_clicked() {
                            t.volume_db = 0.0;
                        }
                        if r.changed() || r.double_clicked() {
                            changed = true;
                        }
                    } else {
                        ui.label(RichText::new("Auto").color(VOL_ENV_COL).size(11.5)).on_hover_text(auto_tip);
                    }
                    ui.label(RichText::new("Pan").color(TEXT_DIM).size(11.0));
                    if t.pan_env.is_empty() {
                        let r = hot_drag(ui, egui::DragValue::new(&mut t.pan).speed(0.5).range(-100.0..=100.0).fixed_decimals(0));
                        if r.double_clicked() {
                            t.pan = 0.0;
                        }
                        if r.changed() || r.double_clicked() {
                            changed = true;
                        }
                    } else {
                        ui.label(RichText::new("Auto").color(PAN_ENV_COL).size(11.5)).on_hover_text(auto_tip);
                    }
                });
            }
            // Drag the bottom edge to resize the track.
            let edge = Rect::from_min_max(pos2(row.left(), row.bottom() - 3.0), pos2(lanes_x, row.bottom() + 2.0));
            let er = ui.interact(edge, ui.id().with(("mt_resize", s.id, ti)), Sense::drag()).on_hover_cursor(CursorIcon::ResizeVertical);
            if er.dragged() {
                let t = &mut s.tracks[ti];
                t.height = (t.height + er.drag_delta().y).clamp(44.0, 400.0);
            }
        }
        // Mute/solo are undoable; level and name changes just mark the session.
        for (ti, mute) in toggles {
            s.push_undo(if mute { "Mute Track" } else { "Solo Track" });
            let t = &mut s.tracks[ti];
            if mute {
                t.mute = !t.mute;
            } else {
                t.solo = !t.solo;
            }
            changed = true;
        }
        if changed {
            s.touch();
        }
        if view_changed {
            s.dirty = true;
        }
        // Empty area under the last track.
        let below = Rect::from_min_max(pos2(body.left(), y.max(body.top())), pos2(lanes_x - 1.0, body.bottom()));
        if below.height() > 8.0 {
            let r = ui.interact(below, ui.id().with(("mt_below", s.id)), Sense::click());
            r.context_menu(|ui| {
                if ui.button("Add Audio Track").clicked() {
                    add_track = true;
                    ui.close_menu();
                }
            });
            if r.double_clicked() {
                add_track = true;
            }
        }

        // ------------------------------------------------ master row
        painter.rect_filled(master, 0.0, BG_PANEL);
        painter.line_segment([pos2(master.left(), master.top()), pos2(master.right(), master.top())], Stroke::new(1.0_f32, BORDER));
        {
            let inner = Rect::from_min_max(pos2(master.left() + 10.0, master.top() + 6.0), pos2(lanes_x + 260.0, master.bottom() - 4.0));
            let mut child = ui.child_ui(inner, Layout::left_to_right(Align::Center), None);
            child.label(RichText::new("Mix").font(bold(12.0)).color(Color32::WHITE));
            child.add_space(60.0);
            child.label(RichText::new("Master").color(TEXT_DIM).size(11.0));
            let r = hot_drag(&mut child, egui::DragValue::new(&mut s.master_db).speed(0.2).range(-96.0..=12.0).fixed_decimals(1).suffix(" dB"));
            if r.changed() {
                s.touch();
            }
            let end = s.end();
            child.add_space(16.0);
            child.label(RichText::new(format!("{} tracks · {} · {} Hz", s.tracks.len(), format_time(end as f64, s.sample_rate), s.sample_rate)).color(TEXT_DIM).size(11.0));
        }

        // ------------------------------------------------ clip context menu
        let ctx_clip = self.mt.context_clip;
        let ctx_env = self.mt.context_env;
        let mut menu_action: Option<Action> = None;
        resp.context_menu(|ui| {
            if let Some((ti, pan, point)) = ctx_env.filter(|e| e.0 < s.tracks.len()) {
                let what = if pan { "Pan" } else { "Volume" };
                if let Some(i) = point {
                    if ui.button("Delete Point").clicked() {
                        s.push_undo("Delete Automation Point");
                        let e = env_of_mut(&mut s.tracks[ti], pan);
                        if i < e.points.len() {
                            e.points.remove(i);
                        }
                        s.touch();
                        ui.close_menu();
                    }
                }
                if ui.button(format!("Clear {what} Automation")).clicked() {
                    s.push_undo(if pan { "Clear Pan Automation" } else { "Clear Volume Automation" });
                    env_of_mut(&mut s.tracks[ti], pan).points.clear();
                    s.touch();
                    ui.close_menu();
                }
                if ui.button("Hide Automation").clicked() {
                    s.tracks[ti].show_env = false;
                    s.dirty = true;
                    ui.close_menu();
                }
            } else if let Some(id) = ctx_clip {
                if ui.button("Split at Playhead            Ctrl+K").clicked() {
                    menu_action = Some(Action::MtSplit);
                    ui.close_menu();
                }
                if ui.button("Delete                              Del").clicked() {
                    menu_action = Some(Action::Delete);
                    ui.close_menu();
                }
                if let Some(c) = s.clip_mut(id) {
                    ui.separator();
                    let mut mute = c.mute;
                    if ui.checkbox(&mut mute, "Mute Clip").changed() {
                        c.mute = mute;
                        s.touch();
                    }
                    let c = s.clip_mut(id).unwrap();
                    let mut g = c.gain_db;
                    let mut gain_changed = false;
                    ui.horizontal(|ui| {
                        ui.label("Clip Gain");
                        gain_changed = hot_drag(ui, egui::DragValue::new(&mut g).speed(0.2).range(-48.0..=24.0).fixed_decimals(1).suffix(" dB")).changed();
                    });
                    if gain_changed {
                        c.gain_db = g;
                        s.touch();
                    }
                    if ui.button("Remove Fades").clicked() {
                        if let Some(c) = s.clip_mut(id) {
                            c.fade_in = 0;
                            c.fade_out = 0;
                        }
                        s.touch();
                        ui.close_menu();
                    }
                }
                ui.separator();
                if ui.button("Edit Source File").clicked() {
                    menu_action = Some(Action::MtEditSource(id));
                    ui.close_menu();
                }
            } else {
                if ui.button("Insert Files…").clicked() {
                    menu_action = Some(Action::MtInsertFiles);
                    ui.close_menu();
                }
                if ui.button("Add Audio Track").clicked() {
                    add_track = true;
                    ui.close_menu();
                }
                if ui.button("Paste").clicked() {
                    menu_action = Some(Action::Paste);
                    ui.close_menu();
                }
            }
        });
        if let Some(a) = menu_action {
            self.actions.push(a);
        }
        if add_track {
            self.actions.push(Action::MtAddTrack);
        }
        if let Some(i) = delete_track {
            s.selected_track = i;
            self.actions.push(Action::MtDeleteTrack);
        }
    }

    fn mt_mixer(&mut self, ui: &mut Ui, si: usize) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x2b, 0x2b, 0x2b));
        let master_meter = self.meter_db;
        let meters = self.mt.meters.clone();
        let s = &mut self.sessions[si];
        let mut changed = false;
        let mut toggles: Vec<(usize, bool)> = Vec::new();
        let fader_h = (rect.height() - 230.0).max(80.0);
        #[allow(clippy::too_many_arguments)]
        let strip = |ui: &mut Ui,
                     name: &mut String,
                     col: Color32,
                     vol: &mut f32,
                     vol_auto: bool,
                     pan: Option<(&mut f32, bool)>,
                     flags: Option<(bool, bool, &mut bool)>,
                     meter: [f32; 2],
                     changed: &mut bool|
         -> (bool, bool) {
            let mut clicked = (false, false);
            egui::Frame::none().fill(BG_PANEL).inner_margin(egui::Margin::same(6.0)).rounding(3.0).show(ui, |ui| {
                ui.set_width(96.0);
                ui.set_min_height(rect.height() - 24.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(92.0, 4.0), Sense::hover());
                    ui.painter().rect_filled(r, 1.0, col);
                    if ui.add(egui::TextEdit::singleline(name).desired_width(90.0).horizontal_align(Align::Center)).changed() {
                        *changed = true;
                    }
                    match pan {
                        Some((p, auto)) => {
                            ui.label(RichText::new("Pan").color(TEXT_DIM).size(10.5));
                            if auto {
                                ui.label(RichText::new("Auto").color(PAN_ENV_COL));
                            } else {
                                let r = hot_drag(ui, egui::DragValue::new(p).speed(0.5).range(-100.0..=100.0).fixed_decimals(0));
                                if r.double_clicked() {
                                    *p = 0.0;
                                }
                                *changed |= r.changed() || r.double_clicked();
                            }
                        }
                        None => {
                            ui.add_space(34.0);
                        }
                    }
                    match flags {
                        Some((m, so, arm)) => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                ui.add_space(6.0);
                                for (label, on, c) in [("M", m, Color32::from_rgb(0x4d, 0xa3, 0xff)), ("S", so, Color32::from_rgb(0xe8, 0xc4, 0x3a)), ("R", *arm, RECORD)] {
                                    let b = egui::Button::new(RichText::new(label).font(bold(11.0)).color(if on { Color32::BLACK } else { TEXT })).fill(if on { c } else { Color32::from_rgb(0x2c, 0x2c, 0x2c) }).min_size(vec2(26.0, 20.0));
                                    if ui.add(b).clicked() {
                                        match label {
                                            "M" => clicked.0 = true,
                                            "S" => clicked.1 = true,
                                            _ => *arm = !*arm,
                                        }
                                    }
                                }
                            });
                        }
                        None => {
                            ui.add_space(24.0);
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.spacing_mut().slider_width = fader_h;
                        let r = ui.add_enabled(!vol_auto, egui::Slider::new(vol, -60.0..=12.0).vertical().show_value(false));
                        if r.double_clicked() {
                            *vol = 0.0;
                        }
                        *changed |= r.changed() || r.double_clicked();
                        let (mr, _) = ui.allocate_exact_size(vec2(12.0, fader_h), Sense::hover());
                        draw_meter(ui.painter(), mr, meter);
                    });
                    if vol_auto {
                        ui.label(RichText::new("Auto").color(VOL_ENV_COL));
                    } else {
                        let r = hot_drag(ui, egui::DragValue::new(vol).speed(0.2).range(-96.0..=12.0).fixed_decimals(1).suffix(" dB"));
                        *changed |= r.changed();
                    }
                });
            });
            clicked
        };
        ui.allocate_ui_at_rect(rect.shrink(6.0), |ui| {
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (ti, t) in s.tracks.iter_mut().enumerate() {
                        let col = colour(t.colour);
                        let (va, pa) = (!t.vol_env.is_empty(), !t.pan_env.is_empty());
                        let (mute_c, solo_c) = strip(
                            ui,
                            &mut t.name,
                            col,
                            &mut t.volume_db,
                            va,
                            Some((&mut t.pan, pa)),
                            Some((t.mute, t.solo, &mut t.arm)),
                            meters.get(ti).copied().unwrap_or([-120.0; 2]),
                            &mut changed,
                        );
                        if mute_c {
                            toggles.push((ti, true));
                        }
                        if solo_c {
                            toggles.push((ti, false));
                        }
                    }
                    ui.add_space(12.0);
                    let mut name = "Mix".to_string();
                    strip(ui, &mut name, Color32::WHITE, &mut s.master_db, false, None, None, master_meter, &mut changed);
                });
            });
        });
        for (ti, mute) in toggles {
            s.push_undo(if mute { "Mute Track" } else { "Solo Track" });
            let t = &mut s.tracks[ti];
            if mute {
                t.mute = !t.mute;
            } else {
                t.solo = !t.solo;
            }
            changed = true;
        }
        if changed {
            s.touch();
        }
    }
}

/// A clip: name bar, waveform (per channel), fade curves, selection outline.
#[allow(clippy::too_many_arguments)]
fn draw_clip(p: &egui::Painter, r: Rect, c: &Clip, src: Option<&Source>, track_col: Color32, selected: bool, muted: bool, v: &TView, lane: Rect) {
    let base = if muted { Color32::from_rgb(0x3a, 0x3a, 0x3a) } else { track_col };
    let fill = if selected { base.linear_multiply(0.42) } else { base.linear_multiply(0.24) };
    p.rect_filled(r, 3.0, fill);
    let bar = Rect::from_min_max(r.min, pos2(r.right(), r.top() + CLIP_BAR));
    p.rect_filled(bar, Rounding { nw: 3.0, ne: 3.0, sw: 0.0, se: 0.0 }, if selected { base.linear_multiply(0.85) } else { base.linear_multiply(0.5) });
    let text_x = r.left().max(lane.left()) + 4.0;
    if r.right() - text_x > 20.0 {
        let label = if c.gain_db.abs() > 0.05 { format!("{}  ({:+.1} dB)", c.name, c.gain_db) } else { c.name.clone() };
        p.with_clip_rect(bar.intersect(lane)).text(pos2(text_x, bar.center().y), Align2::LEFT_CENTER, label, FontId::proportional(10.5), if selected { Color32::BLACK } else { Color32::WHITE });
    }
    let wave = Rect::from_min_max(pos2(r.left(), bar.bottom()), r.max);
    let wave_col = if muted { Color32::from_rgb(0x70, 0x70, 0x70) } else if selected { Color32::WHITE } else { base.linear_multiply(1.0) };
    if let Some(src) = src.filter(|s| s.len() > 0) {
        let n_ch = src.audio.len().clamp(1, 2);
        let h = wave.height() / n_ch as f32;
        let gain = db_to_lin(c.gain_db);
        let x_from = r.left().max(lane.left()).floor();
        let x_to = r.right().min(lane.right());
        let mut shapes = Vec::new();
        for ch in 0..n_ch {
            let mid = wave.top() + h * (ch as f32 + 0.5);
            let amp = h * 0.48;
            let mut x = x_from;
            while x < x_to {
                let t0 = v.s(x) - c.start as f64;
                let t1 = t0 + v.spp();
                if t1 > 0.0 && t0 < c.len as f64 {
                    let a = c.offset + t0.max(0.0) as usize;
                    let b = (c.offset + (t1.ceil() as usize).min(c.len)).max(a + 1).min(src.len());
                    if a < b {
                        let (lo, hi) = src.peaks.min_max(&src.audio, ch, a, b);
                        let g = gain * c.envelope(t0.max(0.0) as usize);
                        let y0 = (mid - hi * g * amp).clamp(wave.top(), wave.bottom());
                        let y1 = (mid - lo * g * amp).clamp(wave.top(), wave.bottom()).max(y0 + 1.0);
                        shapes.push(Shape::line_segment([pos2(x + 0.5, y0), pos2(x + 0.5, y1)], Stroke::new(1.0_f32, wave_col)));
                    }
                }
                x += 1.0;
            }
        }
        p.extend(shapes);
    } else {
        p.text(wave.center(), Align2::CENTER_CENTER, "Offline", FontId::proportional(10.5), TEXT_DIM);
    }
    // Fade curves and handles.
    let spp = v.spp();
    for fade_in in [true, false] {
        let len = if fade_in { c.fade_in } else { c.fade_out };
        let hx = if fade_in { r.left() + (len as f64 / spp) as f32 } else { r.right() - (len as f64 / spp) as f32 };
        if len > 0 {
            let n = 24;
            let pts: Vec<_> = (0..=n)
                .map(|k| {
                    let f = k as f32 / n as f32;
                    let x = if fade_in { r.left() + (hx - r.left()) * f } else { hx + (r.right() - hx) * f };
                    let g = if fade_in { (std::f32::consts::FRAC_PI_2 * f).sin() } else { (std::f32::consts::FRAC_PI_2 * (1.0 - f)).sin() };
                    pos2(x, wave.bottom() - g * wave.height())
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(1.0_f32, Color32::from_rgb(0xf2, 0xc2, 0x30))));
        }
        if r.width() > 24.0 {
            let hr = Rect::from_center_size(pos2(hx.clamp(r.left() + 4.0, r.right() - 4.0), bar.bottom() + 1.0), vec2(8.0, 8.0));
            p.rect_filled(hr, 1.0, Color32::from_rgb(0xf2, 0xc2, 0x30));
        }
    }
    p.rect_stroke(r, 3.0, Stroke::new(if selected { 1.5_f32 } else { 1.0_f32 }, if selected { Color32::WHITE } else { base.linear_multiply(0.7) }));
}
