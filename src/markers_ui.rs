//! Markers panel (point and range markers, marker types, merge, export),
//! the Playlist panel, marker CSV import/export, exporting range markers
//! as files, and zero-crossing helpers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, RichText, Ui};

use crate::app::{Action, App, Marker, MarkerKind};
use crate::dsp::util::{format_time, format_time_as, parse_time_as, slice_range, TimeStyle};
use crate::theme::*;

/// Playback tag of the playlist (not a document id).
pub const PLAYLIST_TAG: u64 = u64::MAX - 7;

/// One entry of a file's playlist: a range marker (by name), played `loops` times.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistItem {
    pub name: String,
    pub loops: u32,
}

#[derive(Default)]
pub struct MarkerState {
    /// Selected rows of the Markers panel, for document `doc`.
    pub sel: BTreeSet<usize>,
    pub doc: u64,
    /// Selected playlist row.
    pub pl_sel: Option<usize>,
}

// ------------------------------------------------------------------ zero crossings

/// The zero crossing nearest `pos`: searching left (`dir` < 0), right
/// (`dir` > 0) or either way (0), within a quarter of a second. The sum of
/// the active channels is used; `pos` itself if none is found.
pub fn zero_crossing(audio: &[Vec<f32>], active: &[bool], pos: usize, dir: i32, sr: u32) -> usize {
    let len = audio.first().map(|c| c.len()).unwrap_or(0);
    if len < 2 {
        return pos.min(len);
    }
    let pos = pos.min(len - 1);
    let chans: Vec<&Vec<f32>> = audio.iter().enumerate().filter(|(c, _)| active.get(*c).copied().unwrap_or(true)).map(|(_, ch)| ch).collect();
    let chans = if chans.is_empty() { audio.iter().collect() } else { chans };
    let s = |i: usize| chans.iter().map(|c| c[i]).sum::<f32>();
    // A crossing at i: the signal changes sign between i - 1 and i (or is 0 at i).
    let crosses = |i: usize| {
        let (p, q) = (s(i - 1), s(i));
        q == 0.0 || (p < 0.0) != (q < 0.0)
    };
    let win = (sr as usize / 4).max(64);
    let right = || (pos.max(1)..=(pos + win).min(len - 1)).find(|&i| crosses(i));
    let left = || (pos.saturating_sub(win).max(1)..=pos.max(1)).rev().find(|&i| crosses(i));
    match dir.signum() {
        1 => right().unwrap_or(pos),
        -1 => left().unwrap_or(pos),
        _ => match (left(), right()) {
            (Some(l), Some(r)) => {
                if pos - l <= r - pos {
                    l
                } else {
                    r
                }
            }
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => pos,
        },
    }
}

/// Edit > Zero Crossings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroMode {
    Inward,
    Outward,
    LeftToLeft,
    LeftToRight,
    RightToLeft,
    RightToRight,
}

impl ZeroMode {
    pub const ALL: [ZeroMode; 6] = [ZeroMode::Inward, ZeroMode::Outward, ZeroMode::LeftToLeft, ZeroMode::LeftToRight, ZeroMode::RightToLeft, ZeroMode::RightToRight];

    pub fn label(self) -> &'static str {
        match self {
            ZeroMode::Inward => "Adjust Selection Inward",
            ZeroMode::Outward => "Adjust Selection Outward",
            ZeroMode::LeftToLeft => "Adjust Left Side to Left",
            ZeroMode::LeftToRight => "Adjust Left Side to Right",
            ZeroMode::RightToLeft => "Adjust Right Side to Left",
            ZeroMode::RightToRight => "Adjust Right Side to Right",
        }
    }

    /// Directions for the (left, right) edges; 0 = leave.
    fn dirs(self) -> (i32, i32) {
        match self {
            ZeroMode::Inward => (1, -1),
            ZeroMode::Outward => (-1, 1),
            ZeroMode::LeftToLeft => (-1, 0),
            ZeroMode::LeftToRight => (1, 0),
            ZeroMode::RightToLeft => (0, -1),
            ZeroMode::RightToRight => (0, 1),
        }
    }
}

// ------------------------------------------------------------------ CSV

/// Markers in Audition's CSV layout (tab-separated, decimal times).
pub fn markers_to_csv(markers: &[Marker], sr: u32) -> String {
    let t = |s: usize| format_time_as(TimeStyle::Decimal, s as f64, sr);
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut out = String::from("Name\tStart\tDuration\tTime Format\tType\tDescription\n");
    for m in markers {
        out += &format!("{}\t{}\t{}\tdecimal\t{}\t{}\n", clean(&m.name), t(m.pos), t(m.len), m.kind.csv_name(), clean(&m.description));
    }
    out
}

/// Markers from a CSV written by Audemo or Audition (tab- or comma-separated;
/// times as decimal m:ss.ddd, or samples when the time format says Hz).
pub fn markers_from_csv(text: &str, sr: u32) -> Vec<Marker> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let sep = if line.contains('\t') { '\t' } else { ',' };
        let f: Vec<&str> = line.split(sep).map(|x| x.trim().trim_matches('"')).collect();
        if f.len() < 2 || (i == 0 && f[0].eq_ignore_ascii_case("name")) {
            continue;
        }
        let format = f.get(3).copied().unwrap_or("decimal").to_ascii_lowercase();
        let time = |s: &str| -> Option<usize> {
            if format.contains("hz") || format.contains("sample") {
                s.parse::<f64>().ok().map(|v| v.max(0.0) as usize)
            } else {
                parse_time_as(TimeStyle::Decimal, s, sr).map(|v| v.max(0.0) as usize)
            }
        };
        let Some(pos) = time(f[1]) else { continue };
        let len = f.get(2).and_then(|d| time(d)).unwrap_or(0);
        let kind = match f.get(4).map(|k| k.to_ascii_lowercase()) {
            Some(k) if k.starts_with("track") || k.starts_with("cd") => MarkerKind::CdTrack,
            Some(k) if k.starts_with("cart") => MarkerKind::CartTimer,
            Some(k) if k.starts_with("subclip") => MarkerKind::Subclip,
            _ => MarkerKind::Cue,
        };
        out.push(Marker { pos, len, name: f[0].to_string(), kind, description: f.get(5).map(|s| s.to_string()).unwrap_or_default() });
    }
    out
}

/// `dir/name.ext`, or `name (2).ext` etc. if that exists.
pub fn unique_path(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let base = crate::mt_ui::safe_file_name(name);
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}

// ------------------------------------------------------------------ panels

impl App {
    /// Selected marker rows of the active file (cleared when it changes).
    fn marker_selection(&mut self) -> Vec<usize> {
        let Some(doc) = self.doc() else { return Vec::new() };
        let (id, n) = (doc.id, doc.markers.len());
        if self.marks.doc != id {
            self.marks.doc = id;
            self.marks.sel.clear();
        }
        self.marks.sel.retain(|&i| i < n);
        self.marks.sel.iter().copied().collect()
    }

    pub fn markers_tab(&mut self, ui: &mut Ui) {
        if self.mode == crate::app::Mode::Multitrack {
            if let Some(si) = self.active_session {
                return self.session_markers_tab(ui, si);
            }
        }
        let sel = self.marker_selection();
        let has_doc = self.doc().is_some();
        let any_range_sel = self.doc().map(|d| sel.iter().any(|&i| d.markers[i].is_range())).unwrap_or(false);
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(has_doc, egui::Button::new("+ Marker")).on_hover_text("Add a cue marker at the playhead, or a range marker over the selection (M)").clicked() {
                self.actions.push(Action::AddMarker);
            }
            if ui.add_enabled(sel.len() >= 2, egui::Button::new("Merge")).on_hover_text("Merge the selected markers into one range marker").clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::Merge));
            }
            if ui.add_enabled(!sel.is_empty(), egui::Button::new("Delete")).clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::Delete));
            }
            if ui.add_enabled(any_range_sel, egui::Button::new("Insert into Playlist")).clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::ToPlaylist));
            }
            if ui.add_enabled(any_range_sel, egui::Button::new("Export Audio...")).on_hover_text("Save each selected range marker as its own file").clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::ExportAudio));
            }
            ui.menu_button("More", |ui| {
                if ui.add_enabled(has_doc, egui::Button::new("Export Markers (CSV)...")).clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ExportCsv));
                    ui.close_menu();
                }
                if ui.add_enabled(has_doc, egui::Button::new("Import Markers (CSV)...")).clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ImportCsv));
                    ui.close_menu();
                }
                ui.separator();
                if ui.add_enabled(!sel.is_empty(), egui::Button::new("Convert to Range (Selection)")).on_hover_text("Make the selected marker a range over the current selection").clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ToRange));
                    ui.close_menu();
                }
                if ui.add_enabled(any_range_sel, egui::Button::new("Convert to Point")).clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ToPoint));
                    ui.close_menu();
                }
                if ui.add_enabled(has_doc, egui::Button::new("Select All Markers")).clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::SelectAll));
                    ui.close_menu();
                }
            });
        });
        ui.add_space(4.0);
        let Some(di) = self.active else {
            ui.label(RichText::new("Open a file to add markers.").color(TEXT_DIM()));
            return;
        };
        let doc = &mut self.docs[di];
        if doc.markers.is_empty() {
            ui.label(RichText::new("No markers. Press M at the playhead, or with a selection for a range marker.").color(TEXT_DIM()));
            return;
        }
        let sr = doc.sample_rate;
        let mods = ui.input(|i| i.modifiers);
        let mut clicked_row = None;
        let mut changed = false;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("markers").num_columns(6).striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
                for h in ["", "Name", "Start", "End", "Duration", "Type"] {
                    ui.label(RichText::new(h).color(TEXT_DIM()).size(11.0));
                }
                ui.end_row();
                for (i, m) in doc.markers.iter_mut().enumerate() {
                    let selected = self.marks.sel.contains(&i);
                    let icon = if m.is_range() { "▭" } else { "◆" };
                    if ui.selectable_label(selected, RichText::new(icon).color(MARKER())).clicked() {
                        clicked_row = Some(i);
                    }
                    if ui.add(egui::TextEdit::singleline(&mut m.name).desired_width(110.0)).changed() {
                        changed = true;
                    }
                    if ui.link(RichText::new(format_time(m.pos as f64, sr)).monospace().size(11.0)).clicked() {
                        clicked_row = Some(i);
                    }
                    if m.is_range() {
                        ui.label(RichText::new(format_time(m.end() as f64, sr)).monospace().size(11.0));
                        ui.label(RichText::new(format_time(m.len as f64, sr)).monospace().size(11.0));
                    } else {
                        ui.label("");
                        ui.label("");
                    }
                    let before = m.kind;
                    egui::ComboBox::from_id_source(("mk", i)).width(84.0).selected_text(m.kind.label()).show_ui(ui, |ui| {
                        for k in MarkerKind::ALL {
                            ui.selectable_value(&mut m.kind, k, k.label());
                        }
                    });
                    changed |= before != m.kind;
                    ui.end_row();
                }
            });
            // Description of the single selected marker.
            if let [i] = self.marks.sel.iter().copied().collect::<Vec<_>>()[..] {
                if let Some(m) = doc.markers.get_mut(i) {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Description").color(TEXT_DIM()).size(11.0));
                    if ui.add(egui::TextEdit::multiline(&mut m.description).desired_rows(2).desired_width(f32::INFINITY)).changed() {
                        changed = true;
                    }
                }
            }
        });
        if changed {
            doc.dirty = true;
        }
        if let Some(i) = clicked_row {
            if mods.command {
                if !self.marks.sel.remove(&i) {
                    self.marks.sel.insert(i);
                }
            } else if mods.shift && !self.marks.sel.is_empty() {
                let a = *self.marks.sel.iter().next().unwrap();
                let (lo, hi) = (a.min(i), a.max(i));
                self.marks.sel = (lo..=hi).collect();
            } else {
                self.marks.sel = std::iter::once(i).collect();
                // Clicking a marker moves the playhead there; a range selects its audio.
                let m = &doc.markers[i];
                if m.is_range() {
                    doc.sel = Some((m.pos, m.end()));
                    doc.cursor = m.pos;
                } else {
                    self.actions.push(Action::SetCursor(m.pos));
                }
            }
        }
    }

    pub fn playlist_panel(&mut self, ui: &mut Ui) {
        let Some(di) = self.active else {
            ui.label(RichText::new("Open a file and add range markers to build a playlist.").color(TEXT_DIM()));
            return;
        };
        let playing = self.engine.status().playing && self.engine.status().tag == PLAYLIST_TAG;
        ui.horizontal(|ui| {
            let can = !self.docs[di].playlist.is_empty();
            if ui.add_enabled(can, egui::Button::new(if playing { "■ Stop" } else { "▶ Play" })).clicked() {
                self.actions.push(Action::MarkerCmd(if playing { MarkerCmd::StopPlaylist } else { MarkerCmd::PlayPlaylist }));
            }
            if ui.add_enabled(can, egui::Button::new("Export as File...")).on_hover_text("Join the playlist (with its loops) into a new file").clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::PlaylistToFile));
            }
        });
        ui.add_space(4.0);
        let doc = &mut self.docs[di];
        if doc.playlist.is_empty() {
            ui.label(RichText::new("Select range markers in the Markers panel and choose Insert into Playlist.").color(TEXT_DIM()));
            return;
        }
        let sr = doc.sample_rate;
        let mut action: Option<(usize, i32)> = None;
        let mut total = 0usize;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("playlist").num_columns(5).striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
                for h in ["#", "Range marker", "Loops", "Duration", ""] {
                    ui.label(RichText::new(h).color(TEXT_DIM()).size(11.0));
                }
                ui.end_row();
                let n = doc.playlist.len();
                for (i, it) in doc.playlist.iter_mut().enumerate() {
                    let m = doc.markers.iter().find(|m| m.is_range() && m.name == it.name);
                    if ui.selectable_label(self.marks.pl_sel == Some(i), format!("{}", i + 1)).clicked() {
                        self.marks.pl_sel = Some(i);
                    }
                    ui.label(if m.is_some() { RichText::new(&it.name) } else { RichText::new(format!("{} (missing)", it.name)).color(TEXT_DIM()) });
                    ui.add(egui::DragValue::new(&mut it.loops).range(1..=99));
                    let len = m.map(|m| m.len * it.loops as usize).unwrap_or(0);
                    total += len;
                    ui.label(RichText::new(format_time(len as f64, sr)).monospace().size(11.0));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(i > 0, egui::Button::new("▲").small()).clicked() {
                            action = Some((i, -1));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small()).clicked() {
                            action = Some((i, 1));
                        }
                        if ui.small_button("×").clicked() {
                            action = Some((i, 0));
                        }
                    });
                    ui.end_row();
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Total {}", format_time(total as f64, sr))).color(TEXT_DIM()).size(11.0));
        });
        if let Some((i, d)) = action {
            match d {
                0 => {
                    doc.playlist.remove(i);
                }
                d => doc.playlist.swap(i, (i as i32 + d) as usize),
            }
            doc.dirty = true;
        }
    }

    /// The playlist joined into one buffer (each range repeated its loop count).
    fn playlist_audio(&self) -> Option<(Vec<Vec<f32>>, u32)> {
        let doc = self.doc()?;
        let mut out: Vec<Vec<f32>> = vec![Vec::new(); doc.n_ch()];
        for it in &doc.playlist {
            let Some(m) = doc.markers.iter().find(|m| m.is_range() && m.name == it.name) else { continue };
            let part = slice_range(&doc.audio, m.pos, m.end().min(doc.len()));
            for _ in 0..it.loops.max(1) {
                for (o, p) in out.iter_mut().zip(&part) {
                    o.extend_from_slice(p);
                }
            }
        }
        (out[0].len() > 0).then(|| (out, doc.sample_rate))
    }

    pub fn marker_cmd(&mut self, cmd: MarkerCmd) {
        let sel = self.marker_selection();
        match cmd {
            MarkerCmd::Delete => {
                if let Some(d) = self.doc_mut() {
                    for &i in sel.iter().rev() {
                        d.markers.remove(i);
                    }
                    d.dirty = true;
                }
                self.marks.sel.clear();
            }
            MarkerCmd::SelectAll => {
                let n = self.doc().map(|d| d.markers.len()).unwrap_or(0);
                self.marks.sel = (0..n).collect();
            }
            MarkerCmd::Merge => {
                if let Some(d) = self.doc_mut() {
                    if sel.len() >= 2 {
                        let a = sel.iter().map(|&i| d.markers[i].pos).min().unwrap_or(0);
                        let b = sel.iter().map(|&i| d.markers[i].end()).max().unwrap_or(0);
                        let first = d.markers[sel[0]].clone();
                        for &i in sel.iter().rev() {
                            d.markers.remove(i);
                        }
                        d.markers.push(Marker { pos: a, len: b - a, ..first });
                        d.markers.sort_by_key(|m| m.pos);
                        d.dirty = true;
                    }
                }
                self.marks.sel.clear();
            }
            MarkerCmd::ToRange => {
                if let Some(d) = self.doc_mut() {
                    match (d.sel_range(), sel.first()) {
                        (Some((a, b)), Some(&i)) => {
                            d.markers[i].pos = a;
                            d.markers[i].len = b - a;
                            d.markers.sort_by_key(|m| m.pos);
                            d.dirty = true;
                        }
                        _ => self.set_status("Select some audio first: the marker becomes a range over it."),
                    }
                }
            }
            MarkerCmd::ToPoint => {
                if let Some(d) = self.doc_mut() {
                    for &i in &sel {
                        d.markers[i].len = 0;
                    }
                    d.dirty = true;
                }
            }
            MarkerCmd::ToPlaylist => {
                if let Some(d) = self.doc_mut() {
                    let names: Vec<String> = sel.iter().map(|&i| &d.markers[i]).filter(|m| m.is_range()).map(|m| m.name.clone()).collect();
                    for name in names {
                        d.playlist.push(PlaylistItem { name, loops: 1 });
                    }
                    d.dirty = true;
                }
                self.show_panel(crate::app::Panel::Playlist);
            }
            MarkerCmd::PlayPlaylist => {
                if let Some((audio, sr)) = self.playlist_audio() {
                    let len = audio[0].len() as f64;
                    self.engine.stop();
                    self.prepare_output_rate(sr);
                    self.engine.play(Arc::new(audio), sr, 0.0, len, false, PLAYLIST_TAG);
                    self.set_status("Playing the playlist");
                }
            }
            MarkerCmd::StopPlaylist => self.engine.stop(),
            MarkerCmd::PlaylistToFile => {
                if let Some((audio, sr)) = self.playlist_audio() {
                    let name = format!("{} Playlist", self.doc().map(|d| d.name.clone()).unwrap_or_default());
                    self.new_document_from(name, audio, sr, "Playlist");
                }
            }
            MarkerCmd::ExportCsv => {
                let Some(d) = self.doc() else { return };
                let csv = markers_to_csv(&d.markers, d.sample_rate);
                let stem = Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Markers".into());
                let mut dlg = rfd::FileDialog::new().add_filter("Markers (CSV)", &["csv"]).set_file_name(format!("{stem} Markers.csv"));
                if let Some(dir) = d.path.as_deref().and_then(|p| p.parent()) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.save_file() {
                    match std::fs::write(&p, csv) {
                        Ok(()) => self.set_status(format!("Exported {} marker(s) to {}", d.markers.len(), p.display())),
                        Err(e) => self.set_status(format!("Couldn't write {}: {e}", p.display())),
                    }
                }
            }
            MarkerCmd::ImportCsv => {
                let Some(p) = rfd::FileDialog::new().add_filter("Markers (CSV)", &["csv", "txt"]).pick_file() else { return };
                let text = match std::fs::read_to_string(&p) {
                    Ok(t) => t,
                    Err(e) => return self.set_status(format!("Couldn't read {}: {e}", p.display())),
                };
                if let Some(d) = self.doc_mut() {
                    let len = d.len();
                    let new: Vec<Marker> = markers_from_csv(&text, d.sample_rate)
                        .into_iter()
                        .filter(|m| m.pos <= len)
                        .map(|mut m| {
                            m.len = m.len.min(len - m.pos);
                            m
                        })
                        .collect();
                    let n = new.len();
                    d.markers.extend(new);
                    d.markers.sort_by_key(|m| m.pos);
                    d.dirty = true;
                    self.set_status(format!("Imported {n} marker(s)"));
                }
            }
            MarkerCmd::ExportAudio => self.export_range_audio(&sel),
        }
    }

    /// Save each selected range marker as its own file, named after the
    /// marker, in the file's current format, beside nothing existing.
    fn export_range_audio(&mut self, sel: &[usize]) {
        let Some(d) = self.doc() else { return };
        let ranges: Vec<Marker> = sel.iter().map(|&i| d.markers[i].clone()).filter(|m| m.is_range()).collect();
        if ranges.is_empty() {
            return;
        }
        let mut dlg = rfd::FileDialog::new().set_title("Folder for the exported ranges");
        if let Some(dir) = d.path.as_deref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let Some(dir) = dlg.pick_folder() else { return };
        let mut settings = d.save_as_settings();
        settings.dither = self.prefs.dither;
        let (audio, sr, meta) = (d.audio.clone(), d.sample_rate, d.meta.clone());
        let stem = Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = settings.container.ext();
        let copy_markers = self.prefs.copy_markers;
        let all_markers = d.markers.clone();
        let n = ranges.len();
        // Names are chosen now so two ranges with one name don't collide.
        let mut jobs = Vec::new();
        let mut taken: Vec<PathBuf> = Vec::new();
        for m in ranges {
            let base = if m.name.trim().is_empty() { format!("{stem} {}", jobs.len() + 1) } else { format!("{stem} {}", m.name.trim()) };
            let mut p = unique_path(&dir, &base, ext);
            let mut k = 2;
            while taken.contains(&p) {
                p = unique_path(&dir, &format!("{base} {k}"), ext);
                k += 1;
            }
            taken.push(p.clone());
            jobs.push((p, m));
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut errs = Vec::new();
            for (path, m) in jobs {
                let part = slice_range(&audio, m.pos, m.end().min(audio[0].len()));
                let markers: Vec<Marker> = if copy_markers { all_markers.iter().filter(|x| x != &&m).filter_map(|x| x.within(m.pos, m.end())).collect() } else { Vec::new() };
                if let Err(e) = crate::export::save(&path, &part, sr, &settings, &meta, &markers, &crate::export::Progress::default()) {
                    errs.push(format!("{}: {e}", path.display()));
                }
            }
            let _ = tx.send(if errs.is_empty() { format!("Exported {n} range(s) to {}", dir.display()) } else { format!("Export failed: {}", errs.join("; ")) });
        });
        self.marker_export = Some(rx);
        self.set_status(format!("Exporting {n} range(s)..."));
    }

    /// Report a finished range export. Call once per frame.
    pub fn poll_marker_export(&mut self) {
        if let Some(rx) = &self.marker_export {
            if let Ok(msg) = rx.try_recv() {
                self.marker_export = None;
                self.set_status(msg);
            }
        }
    }

    /// Edit > Zero Crossings on the selection (or the cursor without one).
    pub fn zero_cross(&mut self, mode: ZeroMode) {
        let Some(d) = self.doc_mut() else { return };
        let sr = d.sample_rate;
        let (dl, dr) = mode.dirs();
        match d.sel_range() {
            Some((a, b)) => {
                let a2 = if dl != 0 { zero_crossing(&d.audio, &d.active_ch, a, dl, sr) } else { a };
                let b2 = if dr != 0 { zero_crossing(&d.audio, &d.active_ch, b, dr, sr) } else { b };
                d.sel = (b2 > a2).then_some((a2, b2));
                d.cursor = a2;
            }
            None => {
                let dir = if dl != 0 { dl } else { dr };
                d.cursor = zero_crossing(&d.audio, &d.active_ch, d.cursor, dir, sr);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerCmd {
    Delete,
    SelectAll,
    Merge,
    ToRange,
    ToPoint,
    ToPlaylist,
    PlayPlaylist,
    StopPlaylist,
    PlaylistToFile,
    ExportCsv,
    ImportCsv,
    ExportAudio,
}

// ---------------------------------------------------------------- session markers

impl App {
    /// M in the Multitrack editor: a marker at the playhead, or a range over
    /// the time selection when stopped.
    pub fn mt_add_marker(&mut self, si: usize, kind: MarkerKind) {
        let st = self.engine.status();
        let marker_name = self.prefs.marker_name.clone();
        let s = &mut self.sessions[si];
        let playing = st.playing && st.tag == s.id;
        let pos = if playing { st.pos.max(0.0) as usize } else { s.cursor };
        let name = format!("{} {:02}", marker_name.trim(), s.markers.len() + 1);
        let mut m = match s.sel_range().filter(|(a, b)| !playing && b > a) {
            Some((a, b)) => Marker::range(a, b - a, name),
            None => Marker::point(pos, name),
        };
        m.kind = kind;
        s.push_undo("Add Marker");
        s.markers.push(m.clone());
        s.markers.sort_by_key(|m| m.pos);
        s.dirty = true;
        if let Some(i) = s.markers.iter().position(|x| *x == m) {
            self.mt.marker_sel = std::iter::once(i).collect();
        }
    }

    /// Markers panel commands for the active session.
    pub fn mt_marker_cmd(&mut self, si: usize, cmd: MarkerCmd) {
        let sel: Vec<usize> = self.mt.marker_sel.iter().copied().filter(|&i| i < self.sessions[si].markers.len()).collect();
        let s = &mut self.sessions[si];
        match cmd {
            MarkerCmd::Delete if !sel.is_empty() => {
                s.push_undo("Delete Markers");
                for &i in sel.iter().rev() {
                    s.markers.remove(i);
                }
                s.dirty = true;
                self.mt.marker_sel.clear();
            }
            MarkerCmd::SelectAll => self.mt.marker_sel = (0..s.markers.len()).collect(),
            MarkerCmd::Merge if sel.len() >= 2 => {
                s.push_undo("Merge Markers");
                let a = sel.iter().map(|&i| s.markers[i].pos).min().unwrap_or(0);
                let b = sel.iter().map(|&i| s.markers[i].end()).max().unwrap_or(0);
                let first = s.markers[sel[0]].clone();
                for &i in sel.iter().rev() {
                    s.markers.remove(i);
                }
                s.markers.push(Marker { pos: a, len: b - a, ..first });
                s.markers.sort_by_key(|m| m.pos);
                s.dirty = true;
                self.mt.marker_sel.clear();
            }
            MarkerCmd::ToRange => match (s.sel_range(), sel.first()) {
                (Some((a, b)), Some(&i)) if b > a => {
                    s.push_undo("Convert Marker");
                    s.markers[i].pos = a;
                    s.markers[i].len = b - a;
                    s.markers.sort_by_key(|m| m.pos);
                    s.dirty = true;
                }
                _ => self.set_status("Select a time range first: the marker becomes a range over it."),
            },
            MarkerCmd::ToPoint if !sel.is_empty() => {
                s.push_undo("Convert Markers");
                for &i in &sel {
                    s.markers[i].len = 0;
                }
                s.dirty = true;
            }
            MarkerCmd::ExportCsv => {
                let csv = markers_to_csv(&s.markers, s.sample_rate);
                let n = s.markers.len();
                let mut dlg = rfd::FileDialog::new().add_filter("Markers (CSV)", &["csv"]).set_file_name(format!("{} Markers.csv", s.name));
                if let Some(dir) = s.path.as_deref().and_then(|p| p.parent()) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.save_file() {
                    match std::fs::write(&p, csv) {
                        Ok(()) => self.set_status(format!("Exported {n} marker(s) to {}", p.display())),
                        Err(e) => self.set_status(format!("Couldn't write {}: {e}", p.display())),
                    }
                }
            }
            MarkerCmd::ImportCsv => {
                let Some(p) = rfd::FileDialog::new().add_filter("Markers (CSV)", &["csv", "txt"]).pick_file() else { return };
                let text = match std::fs::read_to_string(&p) {
                    Ok(t) => t,
                    Err(e) => return self.set_status(format!("Couldn't read {}: {e}", p.display())),
                };
                let s = &mut self.sessions[si];
                let new = markers_from_csv(&text, s.sample_rate);
                let n = new.len();
                s.push_undo("Import Markers");
                s.markers.extend(new);
                s.markers.sort_by_key(|m| m.pos);
                s.dirty = true;
                self.set_status(format!("Imported {n} marker(s)"));
            }
            MarkerCmd::Delete | MarkerCmd::Merge | MarkerCmd::ToPoint => {}
            _ => self.set_status("Playlists and range export work on files: open one in the Waveform editor (9)."),
        }
    }

    /// The Markers panel while the Multitrack editor is showing.
    fn session_markers_tab(&mut self, ui: &mut Ui, si: usize) {
        let sel: Vec<usize> = self.mt.marker_sel.iter().copied().collect();
        ui.horizontal_wrapped(|ui| {
            if ui.button("+ Marker").on_hover_text("Add a marker at the playhead, or a range marker over the time selection (M)").clicked() {
                self.actions.push(Action::AddMarker);
            }
            if ui.add_enabled(sel.len() >= 2, egui::Button::new("Merge")).on_hover_text("Merge the selected markers into one range marker").clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::Merge));
            }
            if ui.add_enabled(!sel.is_empty(), egui::Button::new("Delete")).clicked() {
                self.actions.push(Action::MarkerCmd(MarkerCmd::Delete));
            }
            ui.menu_button("More", |ui| {
                if ui.button("Export Markers (CSV)...").clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ExportCsv));
                    ui.close_menu();
                }
                if ui.button("Import Markers (CSV)...").clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ImportCsv));
                    ui.close_menu();
                }
                ui.separator();
                if ui.add_enabled(!sel.is_empty(), egui::Button::new("Convert to Range (Selection)")).clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ToRange));
                    ui.close_menu();
                }
                if ui.add_enabled(!sel.is_empty(), egui::Button::new("Convert to Point")).clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::ToPoint));
                    ui.close_menu();
                }
                if ui.button("Select All Markers").clicked() {
                    self.actions.push(Action::MarkerCmd(MarkerCmd::SelectAll));
                    ui.close_menu();
                }
            });
        });
        ui.add_space(4.0);
        let s = &mut self.sessions[si];
        if s.markers.is_empty() {
            ui.label(RichText::new("No markers in this session. Press M at the playhead, or with a time selection for a range marker.").color(TEXT_DIM()));
            return;
        }
        let sr = s.sample_rate;
        let mods = ui.input(|i| i.modifiers);
        let mut clicked_row = None;
        let mut changed = false;
        let mut kind_change = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("session_markers").num_columns(6).striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
                for h in ["", "Name", "Start", "End", "Duration", "Type"] {
                    ui.label(RichText::new(h).color(TEXT_DIM()).size(11.0));
                }
                ui.end_row();
                for (i, m) in s.markers.iter_mut().enumerate() {
                    let selected = self.mt.marker_sel.contains(&i);
                    let icon = if m.is_range() { "▭" } else { "◆" };
                    if ui.selectable_label(selected, RichText::new(icon).color(crate::mt_ui::marker_colour(m.kind))).clicked() {
                        clicked_row = Some(i);
                    }
                    if ui.add(egui::TextEdit::singleline(&mut m.name).desired_width(110.0)).changed() {
                        changed = true;
                    }
                    if ui.link(RichText::new(format_time(m.pos as f64, sr)).monospace().size(11.0)).clicked() {
                        clicked_row = Some(i);
                    }
                    if m.is_range() {
                        ui.label(RichText::new(format_time(m.end() as f64, sr)).monospace().size(11.0));
                        ui.label(RichText::new(format_time(m.len as f64, sr)).monospace().size(11.0));
                    } else {
                        ui.label("");
                        ui.label("");
                    }
                    let mut k = m.kind;
                    egui::ComboBox::from_id_source(("smk", i)).width(84.0).selected_text(m.kind.label()).show_ui(ui, |ui| {
                        for kk in MarkerKind::ALL {
                            ui.selectable_value(&mut k, kk, kk.label());
                        }
                    });
                    if k != m.kind {
                        kind_change = Some((i, k));
                    }
                    ui.end_row();
                }
            });
            if let [i] = self.mt.marker_sel.iter().copied().collect::<Vec<_>>()[..] {
                if let Some(m) = s.markers.get_mut(i) {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Description").color(TEXT_DIM()).size(11.0));
                    if ui.add(egui::TextEdit::multiline(&mut m.description).desired_rows(2).desired_width(f32::INFINITY)).changed() {
                        changed = true;
                    }
                }
            }
        });
        if let Some((i, k)) = kind_change {
            s.push_undo("Marker Type");
            s.markers[i].kind = k;
            changed = true;
        }
        if changed {
            s.dirty = true;
        }
        if let Some(i) = clicked_row {
            if mods.command {
                if !self.mt.marker_sel.remove(&i) {
                    self.mt.marker_sel.insert(i);
                }
            } else if mods.shift && !self.mt.marker_sel.is_empty() {
                let a = *self.mt.marker_sel.iter().next().unwrap();
                self.mt.marker_sel = (a.min(i)..=a.max(i)).collect();
            } else {
                self.mt.marker_sel = std::iter::once(i).collect();
                let m = s.markers[i].clone();
                if m.is_range() {
                    s.sel = Some((m.pos, m.end()));
                }
                s.cursor = m.pos;
                // Bring the marker into view.
                let span = s.view_end - s.view_start;
                if (m.pos as f64) < s.view_start || (m.pos as f64) > s.view_end {
                    s.view_start = m.pos as f64 - span * 0.2;
                    s.view_end = s.view_start + span;
                    s.clamp_view();
                }
                self.actions.push(Action::SetCursor(m.pos));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_zero_crossings() {
        // Crossings of a 100-sample-period sine at 0, 50, 100, ...
        let x: Vec<f32> = (0..1000).map(|i| ((i as f32 + 0.5) * std::f32::consts::TAU / 100.0).sin()).collect();
        let a = vec![x];
        assert_eq!(zero_crossing(&a, &[true], 120, 1, 48000), 150);
        assert_eq!(zero_crossing(&a, &[true], 120, -1, 48000), 100);
        assert_eq!(zero_crossing(&a, &[true], 140, 0, 48000), 150);
        assert_eq!(zero_crossing(&a, &[true], 110, 0, 48000), 100);
    }

    #[test]
    fn csv_round_trip() {
        let mut r = Marker::range(48000, 24000, "Chorus");
        r.kind = MarkerKind::Subclip;
        r.description = "take 2".into();
        let ms = vec![Marker::point(4800, "Intro"), r];
        let back = markers_from_csv(&markers_to_csv(&ms, 48000), 48000);
        assert_eq!(back, ms);
        // Audition's own export.
        let au = "Name\tStart\tDuration\tTime Format\tType\tDescription\nMarker 01\t0:01.500\t0:00.000\tdecimal\tCue\t\n";
        assert_eq!(markers_from_csv(au, 1000), vec![Marker::point(1500, "Marker 01")]);
    }

    #[test]
    fn range_markers_follow_edits() {
        let m = Marker::range(100, 100, "r");
        // Delete before it: moves back.
        assert_eq!(m.after_edit(10, 20, 0).map(|m| (m.pos, m.len)), Some((80, 100)));
        // Delete overlapping its start: shortens it.
        assert_eq!(m.after_edit(50, 100, 0).map(|m| (m.pos, m.len)), Some((50, 50)));
        // Delete covering it: removes it.
        assert_eq!(m.after_edit(50, 300, 0), None);
        // Insert inside it: lengthens it.
        assert_eq!(m.after_edit(150, 0, 30).map(|m| (m.pos, m.len)), Some((100, 130)));
        assert_eq!(m.within(150, 400).map(|m| (m.pos, m.len)), Some((0, 50)));
    }
}
