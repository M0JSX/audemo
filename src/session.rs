//! Multitrack sessions: tracks of clips that point into source audio, the
//! mix of them, undo, and the `.audemo` session file format.
//!
//! Clips never own audio. Each refers to a [`Source`] by id; a source is
//! usually linked to an open file (document), so editing that file in the
//! Waveform editor updates every clip that uses it, as in Audition.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dsp::peaks::PeakCache;
use crate::dsp::util::db_to_lin;
use crate::engine::Buffer;

pub const MAX_SESSION_UNDO: usize = 100;
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
            g *= (std::f32::consts::FRAC_PI_2 * i as f32 / self.fade_in as f32).sin();
        }
        if self.fade_out > 0 && i + self.fade_out > self.len {
            let r = (self.len - i) as f32 / self.fade_out as f32;
            g *= (std::f32::consts::FRAC_PI_2 * r.min(1.0)).sin();
        }
        g
    }
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
}

impl Track {
    pub fn new(id: u64, name: String, colour: [u8; 3]) -> Self {
        Track { id, name, colour, volume_db: 0.0, pan: 0.0, mute: false, solo: false, arm: false, height: 96.0, clips: Vec::new() }
    }

    /// Left/right gains for the pan position: the centre is unity and
    /// panning attenuates the opposite side (Audition's default pan law).
    pub fn pan_gains(&self) -> (f32, f32) {
        let p = (self.pan / 100.0).clamp(-1.0, 1.0);
        let cut = |x: f32| (std::f32::consts::FRAC_PI_2 * x).cos();
        if p >= 0.0 {
            (cut(p), 1.0)
        } else {
            (1.0, cut(-p))
        }
    }
}

#[derive(Clone)]
pub struct SessionSnapshot {
    pub label: String,
    pub tracks: Vec<Track>,
    pub master_db: f32,
}

/// Everything the mixer needs, cheap to clone and share with a render thread.
#[derive(Clone)]
pub struct MixState {
    pub sample_rate: u32,
    pub tracks: Vec<Track>,
    pub sources: BTreeMap<u64, Arc<Source>>,
    pub master_db: f32,
}

pub struct Session {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub sample_rate: u32,
    pub tracks: Vec<Track>,
    pub sources: BTreeMap<u64, Arc<Source>>,
    pub master_db: f32,
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
        self.undo.push(SessionSnapshot { label: label.to_string(), tracks: self.tracks.clone(), master_db: self.master_db });
        if self.undo.len() > MAX_SESSION_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, s: SessionSnapshot) -> SessionSnapshot {
        let cur = SessionSnapshot { label: s.label.clone(), tracks: std::mem::replace(&mut self.tracks, s.tracks), master_db: self.master_db };
        self.master_db = s.master_db;
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
        if len == 0 || track >= self.tracks.len() {
            return None;
        }
        let name = self.sources[&source].name.clone();
        self.push_undo("Insert Clip");
        let id = self.new_id();
        self.tracks[track].clips.push(Clip { id, source, name, start: at, offset: 0, len, gain_db: 0.0, fade_in: 0, fade_out: 0, mute: false });
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
        MixState { sample_rate: self.sample_rate, tracks: self.tracks.clone(), sources: self.sources.clone(), master_db: self.master_db }
    }
}

// ---------------------------------------------------------------- mixing

/// Render frames [a, b) of the stereo mix.
pub fn mix_range(m: &MixState, a: usize, b: usize) -> Vec<Vec<f32>> {
    let mut out = vec![Vec::new(), Vec::new()];
    mix_into(m, a, b, &mut out);
    out
}

/// Render frames [a, b) of the mix into `out` (two channels), reusing its
/// allocation: this runs inside the audio callback during playback.
pub fn mix_into(m: &MixState, a: usize, b: usize, out: &mut [Vec<f32>]) {
    let n = b.saturating_sub(a);
    for ch in out.iter_mut() {
        ch.clear();
        ch.resize(n, 0.0);
    }
    let any_solo = m.tracks.iter().any(|t| t.solo);
    let master = db_to_lin(m.master_db);
    for t in &m.tracks {
        if t.mute || (any_solo && !t.solo) {
            continue;
        }
        let (pl, pr) = t.pan_gains();
        let tg = db_to_lin(t.volume_db) * master;
        for c in &t.clips {
            if c.mute || c.end() <= a || c.start >= b {
                continue;
            }
            let Some(src) = m.sources.get(&c.source) else { continue };
            let sl = &src.audio[0];
            let sr = &src.audio[src.audio.len().min(2) - 1];
            let cg = db_to_lin(c.gain_db) * tg;
            let s0 = c.start.max(a);
            let s1 = c.end().min(b);
            for t_pos in s0..s1 {
                let i = t_pos - c.start;
                let idx = c.offset + i;
                if idx >= sl.len() {
                    break;
                }
                let g = cg * c.envelope(i);
                let k = t_pos - a;
                out[0][k] += sl[idx] * g * pl;
                out[1][k] += sr[idx] * g * pr;
            }
        }
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
    o.push_str(&format!("name\t{}\nrate\t{}\nmaster\t{}\n", clean(&s.name), s.sample_rate, s.master_db));
    for (id, src) in &s.sources {
        if let Some(p) = paths.get(id) {
            o.push_str(&format!("source\t{id}\t{}\t{}\n", rel_path(p, base), clean(&src.name)));
        }
    }
    for t in &s.tracks {
        o.push_str(&format!(
            "track\t{}\t{}\t{}\t{}\t{}\t{}\t{},{},{}\t{}\n",
            t.id, clean(&t.name), t.volume_db, t.pan, t.mute as u8, t.solo as u8, t.colour[0], t.colour[1], t.colour[2], t.height
        ));
        for c in &t.clips {
            o.push_str(&format!(
                "clip\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                c.id, c.source, c.start, c.offset, c.len, c.gain_db, c.fade_in, c.fade_out, c.mute as u8, clean(&c.name)
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

pub fn from_text(id: u64, text: &str, base: Option<&Path>) -> Result<ParsedSession, String> {
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
                };
                let mut c = c;
                c.len = c.len.max(1);
                c.fade_in = c.fade_in.min(c.len);
                c.fade_out = c.fade_out.min(c.len - c.fade_in);
                max_id = max_id.max(c.id);
                t.clips.push(c);
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
        let mut p = from_text(1, text, None).unwrap();
        let k = p.session.tracks[0].clips[0].clone();
        assert_eq!((k.len, k.fade_in, k.fade_out), (1, 1, 0));
        p.session.trim_start(k.id, 0);
    }

    #[test]
    fn session_text_round_trip() {
        let (mut s, src) = session_with(vec![vec![0.1; 50]]);
        let c = s.insert_clip(1, src, 7).unwrap();
        {
            let k = s.clip_mut(c).unwrap();
            k.gain_db = -3.5;
            k.fade_in = 4;
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
        let p = from_text(9, &text, Some(&base)).unwrap();
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
        assert!(from_text(1, "nope", None).is_err());
    }
}
