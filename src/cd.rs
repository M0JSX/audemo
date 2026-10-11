//! CD Layout: arrange open files (or their CD Track markers) as the tracks
//! of an audio CD with pauses and CD-Text, and write a disc image (one
//! 16-bit 44.1 kHz stereo WAV and a cue sheet) for disc-burning software.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};

use crate::app::App;
use crate::dsp::resample::{remap_channels, resample_channels};
use crate::dsp::util::slice_range;
use crate::theme::*;

/// CD audio: 44.1 kHz; positions in frames of 1/75 s (588 samples).
pub const CD_RATE: u32 = 44100;
const FRAME: usize = 588;

#[derive(Clone, Debug, PartialEq)]
pub struct CdTrack {
    pub doc_id: u64,
    /// Part of the file (None = all of it).
    pub range: Option<(usize, usize)>,
    pub title: String,
    pub performer: String,
    /// Silence before the track, in seconds (2 s is the Red Book default
    /// before the first track).
    pub pause: f32,
}

#[derive(Default)]
pub struct CdLayout {
    pub show: bool,
    pub title: String,
    pub performer: String,
    pub tracks: Vec<CdTrack>,
    pub rx: Option<std::sync::mpsc::Receiver<String>>,
}

/// mm:ss:ff from a position in samples at 44.1 kHz.
pub fn msf(samples: usize) -> String {
    let frames = samples / FRAME;
    format!("{:02}:{:02}:{:02}", frames / (75 * 60), (frames / 75) % 60, frames % 75)
}

fn quote(s: &str) -> String {
    s.replace('"', "'")
}

/// The cue sheet for tracks starting at `starts` (sample offsets of INDEX 01
/// in the image), each with its pause before it.
pub fn cue_sheet(wav_name: &str, title: &str, performer: &str, tracks: &[(String, String, usize, usize)]) -> String {
    let mut o = String::new();
    if !performer.is_empty() {
        o += &format!("PERFORMER \"{}\"\n", quote(performer));
    }
    if !title.is_empty() {
        o += &format!("TITLE \"{}\"\n", quote(title));
    }
    o += &format!("FILE \"{}\" WAVE\n", quote(wav_name));
    for (i, (t, p, start, pause)) in tracks.iter().enumerate() {
        o += &format!("  TRACK {:02} AUDIO\n", i + 1);
        if !t.is_empty() {
            o += &format!("    TITLE \"{}\"\n", quote(t));
        }
        if !p.is_empty() {
            o += &format!("    PERFORMER \"{}\"\n", quote(p));
        }
        if *pause > 0 && i > 0 {
            o += &format!("    INDEX 00 {}\n", msf(start - pause));
        }
        o += &format!("    INDEX 01 {}\n", msf(*start));
    }
    o
}

impl App {
    pub fn cd_add_files(&mut self) {
        let ids: Vec<(u64, String, String)> = self.docs.iter().map(|d| (d.id, d.name.clone(), d.meta.get(crate::export::Tag::Artist).to_string())).collect();
        for (id, name, artist) in ids {
            if self.cd.tracks.iter().any(|t| t.doc_id == id && t.range.is_none()) {
                continue;
            }
            let title = Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name);
            let pause = if self.cd.tracks.is_empty() { 2.0 } else { 2.0 };
            self.cd.tracks.push(CdTrack { doc_id: id, range: None, title, performer: artist, pause });
        }
    }

    /// One track per CD Track marker of the active file (each up to the next).
    pub fn cd_add_markers(&mut self) {
        let Some(d) = self.doc() else { return };
        let mut starts: Vec<(usize, String)> = d.markers.iter().filter(|m| m.kind == crate::app::MarkerKind::CdTrack).map(|m| (m.pos, m.name.clone())).collect();
        if starts.is_empty() {
            self.set_status("The file has no CD Track markers (Edit > Marker > Add CD Track Marker, Shift+M).");
            return;
        }
        starts.sort_by_key(|s| s.0);
        let (id, len, artist) = (d.id, d.len(), d.meta.get(crate::export::Tag::Artist).to_string());
        for (k, (a, name)) in starts.iter().enumerate() {
            let b = starts.get(k + 1).map(|s| s.0).unwrap_or(len);
            if b > *a {
                self.cd.tracks.push(CdTrack { doc_id: id, range: Some((*a, b)), title: name.clone(), performer: artist.clone(), pause: if k == 0 && self.cd.tracks.is_empty() { 2.0 } else { 0.0 } });
            }
        }
    }

    pub fn cd_window(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.cd.rx {
            if let Ok(msg) = rx.try_recv() {
                self.cd.rx = None;
                self.set_status(msg);
            }
        }
        if !self.cd.show {
            return;
        }
        let mut open = true;
        let mut add_files = false;
        let mut add_markers = false;
        let mut export = false;
        let names: Vec<(u64, String, u32, usize)> = self.docs.iter().map(|d| (d.id, d.name.clone(), d.sample_rate, d.len())).collect();
        egui::Window::new("CD Layout").open(&mut open).default_size([680.0, 420.0]).show(ctx, |ui| {
            egui::Grid::new("cd_album").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label("Album");
                ui.add(egui::TextEdit::singleline(&mut self.cd.title).desired_width(200.0));
                ui.label("Performer");
                ui.add(egui::TextEdit::singleline(&mut self.cd.performer).desired_width(200.0));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button("Add Open Files").clicked() {
                    add_files = true;
                }
                if ui.button("Add CD Track Markers").on_hover_text("One track per CD Track marker of the active file").clicked() {
                    add_markers = true;
                }
                if ui.button("Clear").clicked() {
                    self.cd.tracks.clear();
                }
            });
            ui.separator();
            let mut op: Option<(usize, i32)> = None;
            let mut total = 0usize;
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("cd_tracks").num_columns(7).striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
                    for h in ["#", "Title", "Performer", "Pause", "Length", "Source", ""] {
                        ui.label(RichText::new(h).color(TEXT_DIM()).size(11.0));
                    }
                    ui.end_row();
                    let n = self.cd.tracks.len();
                    for (i, t) in self.cd.tracks.iter_mut().enumerate() {
                        ui.label(format!("{:02}", i + 1));
                        ui.add(egui::TextEdit::singleline(&mut t.title).desired_width(150.0));
                        ui.add(egui::TextEdit::singleline(&mut t.performer).desired_width(110.0));
                        ui.add(egui::DragValue::new(&mut t.pause).speed(0.1).range(0.0..=10.0).suffix(" s"));
                        let info = names.iter().find(|x| x.0 == t.doc_id);
                        let len = info.map(|(_, _, sr, l)| {
                            let (a, b) = t.range.unwrap_or((0, *l));
                            ((b.saturating_sub(a)) as f64 * CD_RATE as f64 / *sr as f64) as usize
                        });
                        total += len.unwrap_or(0) + (t.pause * CD_RATE as f32) as usize;
                        ui.label(RichText::new(len.map(msf).unwrap_or_else(|| "missing".into())).monospace().size(11.0));
                        ui.label(RichText::new(info.map(|x| x.1.clone()).unwrap_or_else(|| "(closed)".into())).color(TEXT_DIM()).size(11.0));
                        ui.horizontal(|ui| {
                            if ui.add_enabled(i > 0, egui::Button::new("▲").small()).clicked() {
                                op = Some((i, -1));
                            }
                            if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small()).clicked() {
                                op = Some((i, 1));
                            }
                            if ui.small_button("×").clicked() {
                                op = Some((i, 0));
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some((i, d)) = op {
                if d == 0 {
                    self.cd.tracks.remove(i);
                } else {
                    self.cd.tracks.swap(i, (i as i32 + d) as usize);
                }
            }
            ui.separator();
            let over = total > CD_RATE as usize * 80 * 60;
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} tracks · {}{}", self.cd.tracks.len(), msf(total), if over { " — longer than an 80-minute disc" } else { "" })).color(if over { WARN() } else { TEXT_DIM() }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!self.cd.tracks.is_empty() && self.cd.rx.is_none(), egui::Button::new("Export Disc Image…")).on_hover_text("A 16-bit 44.1 kHz WAV and a cue sheet, for disc-burning software").clicked() {
                        export = true;
                    }
                });
            });
            ui.label(RichText::new("Burn the exported .cue with your disc software (for example Finder/Music on macOS, ImgBurn on Windows, Brasero or cdrdao on Linux).").color(TEXT_DIM()).size(10.5));
        });
        if !open {
            self.cd.show = false;
        }
        if add_files {
            self.cd_add_files();
        }
        if add_markers {
            self.cd_add_markers();
        }
        if export {
            self.cd_export();
        }
    }

    fn cd_export(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Cue sheet", &["cue"]).set_file_name(format!("{}.cue", if self.cd.title.is_empty() { "Disc" } else { &self.cd.title })).save_file() else { return };
        let cue_path = if path.extension().is_none() { path.with_extension("cue") } else { path };
        let wav_path: PathBuf = cue_path.with_extension("wav");
        // Gather each track's audio now (on this thread: it's shared, not copied).
        let mut parts = Vec::new();
        for t in &self.cd.tracks {
            let Some(d) = self.docs.iter().find(|d| d.id == t.doc_id) else {
                self.set_status(format!("\"{}\" is no longer open.", t.title));
                return;
            };
            parts.push((d.audio.clone(), d.sample_rate, t.clone()));
        }
        let (title, performer) = (self.cd.title.clone(), self.cd.performer.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        self.cd.rx = Some(rx);
        self.set_status("Writing the disc image…");
        std::thread::spawn(move || {
            let mut out: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
            let mut cue_tracks = Vec::new();
            for (audio, sr, t) in parts {
                // Tracks start on whole CD frames.
                let pad = (t.pause.max(0.0) * CD_RATE as f32) as usize;
                let pad = pad.div_ceil(FRAME) * FRAME;
                let before = out[0].len();
                let aligned = before.div_ceil(FRAME) * FRAME;
                for ch in out.iter_mut() {
                    ch.resize(aligned + pad, 0.0);
                }
                let start = out[0].len();
                let (a, b) = t.range.unwrap_or((0, audio[0].len()));
                let mut seg = remap_channels(&slice_range(&audio, a, b), 2);
                if sr != CD_RATE {
                    seg = resample_channels(&seg, CD_RATE as f64 / sr as f64);
                }
                for (o, s) in out.iter_mut().zip(&seg) {
                    o.extend_from_slice(s);
                }
                cue_tracks.push((t.title.clone(), t.performer.clone(), start, pad));
            }
            // The disc ends on a whole frame (CDs hold at least four seconds per track).
            let end = out[0].len().div_ceil(FRAME) * FRAME;
            for ch in out.iter_mut() {
                ch.resize(end, 0.0);
            }
            let settings = crate::export::ExportSettings { container: crate::export::Container::Wav, wav: crate::export::WavFormat::Pcm16, dither: true, include_meta: false, ..Default::default() };
            let res = crate::export::save(&wav_path, &out, CD_RATE, &settings, &crate::export::Metadata::default(), &[], &crate::export::Progress::default()).and_then(|_| {
                let wav_name = wav_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                std::fs::write(&cue_path, cue_sheet(&wav_name, &title, &performer, &cue_tracks)).map_err(|e| e.to_string())
            });
            let _ = tx.send(match res {
                Ok(()) => format!("Disc image written: {}", cue_path.display()),
                Err(e) => format!("Couldn't write the disc image: {e}"),
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_sheet_lists_tracks_in_cd_frames() {
        let cue = cue_sheet("disc.wav", "Album", "Band", &[("One".into(), "".into(), 88200, 88200), ("Two".into(), "Guest".into(), 44100 * 200, 44100 * 2)]);
        assert!(cue.contains("FILE \"disc.wav\" WAVE"));
        assert!(cue.contains("TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 01 00:02:00"));
        assert!(cue.contains("TRACK 02 AUDIO\n    TITLE \"Two\"\n    PERFORMER \"Guest\"\n    INDEX 00 03:18:00\n    INDEX 01 03:20:00"));
        assert_eq!(msf(44100 + 588 * 3), "00:01:03");
    }
}
