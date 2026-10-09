//! Diagnostics (find and fix clicks, clipping, silence; mark audio) and
//! Batch Process (run a chain over many files and export them).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use eframe::egui::{self, vec2, Align, Color32, Layout, RichText, Ui};

use crate::app::{apply_edit, run_rack, App, Marker, FAVORITES};
use crate::dsp::effects::diagnose::{self, Finding};
use crate::dsp::effects::Ctx;
use crate::dsp::params::{Params, Value};
use crate::dsp::resample::resample_channels;
use crate::dsp::util::{format_time, slice_range};
use crate::io::{self, WavFormat};
use crate::theme::*;

pub const DIAG_KINDS: [&str; 4] = ["Click/Pop Eliminator", "DeClipper", "Delete Silence", "Mark Audio"];

pub struct DiagResults {
    pub doc_id: u64,
    pub version: u64,
    pub kind: usize,
    pub items: Vec<Finding>,
}

pub struct DiagState {
    pub kind: usize,
    pub sensitivity: f32,
    pub max_ms: f32,
    pub clip_db: f32,
    pub silence_db: f32,
    pub silence_ms: f32,
    pub audio_db: f32,
    pub audio_ms: f32,
    pub results: Option<DiagResults>,
    pub selected: Option<usize>,
    rx: Option<Receiver<DiagResults>>,
}

impl Default for DiagState {
    fn default() -> Self {
        DiagState {
            kind: 0,
            sensitivity: 35.0,
            max_ms: 1.0,
            clip_db: -0.1,
            silence_db: -50.0,
            silence_ms: 500.0,
            audio_db: -40.0,
            audio_ms: 300.0,
            results: None,
            selected: None,
            rx: None,
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum BatchStatus {
    Pending,
    Done(String),
    Failed(String),
}

pub struct BatchState {
    pub files: Vec<(PathBuf, BatchStatus)>,
    /// 0 = no processing, 1 = Effects Rack chain, 2 = Match Loudness, 3.. = Favorites
    pub process: usize,
    pub format: WavFormat,
    pub rate: Option<u32>,
    pub out_dir: Option<PathBuf>,
    pub suffix: String,
    rx: Option<Receiver<(usize, Result<String, String>)>>,
    cancel: Arc<AtomicBool>,
    pub done: usize,
}

impl Default for BatchState {
    fn default() -> Self {
        BatchState {
            files: Vec::new(),
            process: 1,
            format: WavFormat::Pcm24,
            rate: None,
            out_dir: None,
            suffix: "_processed".into(),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            done: 0,
        }
    }
}

/// What a batch run does to each file.
enum BatchJob {
    None,
    Rack { slots: Vec<(usize, Params)>, mix: f32, in_db: f32, out_db: f32 },
    Effect(usize, Params),
}

impl App {
    pub fn poll_tools(&mut self) {
        if let Some(rx) = &self.diag.rx {
            if let Ok(r) = rx.try_recv() {
                let n = r.items.len();
                self.diag.results = Some(r);
                self.diag.rx = None;
                self.diag.selected = None;
                self.set_status(format!("Diagnostics: {n} found"));
            }
        }
        let mut finished = false;
        if let Some(rx) = &self.batch.rx {
            loop {
                match rx.try_recv() {
                    Ok((i, r)) => {
                        if let Some(f) = self.batch.files.get_mut(i) {
                            f.1 = match r {
                                Ok(name) => BatchStatus::Done(name),
                                Err(e) => BatchStatus::Failed(e),
                            };
                        }
                        self.batch.done += 1;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    // The worker has finished (or was stopped).
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.batch.rx = None;
            let ok = self.batch.files.iter().filter(|f| matches!(f.1, BatchStatus::Done(_))).count();
            self.set_status(format!("Batch finished: {ok} of {} file(s) processed", self.batch.files.len()));
        }
    }

    // ------------------------------------------------------------ Diagnostics

    fn diag_scan(&mut self) {
        let Some(doc) = self.doc() else { return };
        let (a, b) = doc.target_range();
        if b <= a {
            return;
        }
        let input = slice_range(&doc.audio, a, b);
        let (sr, doc_id, version) = (doc.sample_rate, doc.id, doc.version);
        let d = &self.diag;
        let (kind, sens, max_ms, clip_db, sil_db, sil_ms, aud_db, aud_ms) = (d.kind, d.sensitivity, d.max_ms, d.clip_db, d.silence_db, d.silence_ms, d.audio_db, d.audio_ms);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let items = match kind {
                0 => diagnose::find_clicks(&input, sr, sens, max_ms, a),
                1 => diagnose::find_clipping(&input, clip_db, a),
                2 => diagnose::find_silence(&input, sr, sil_db, sil_ms, a),
                _ => diagnose::find_audio(&input, sr, aud_db, aud_ms, a),
            };
            let _ = tx.send(DiagResults { doc_id, version, kind, items });
        });
        self.diag.rx = Some(rx);
        self.diag.results = None;
    }

    /// Repair / delete / mark the chosen findings (None = all of them).
    fn diag_apply(&mut self, which: Option<usize>) {
        let Some(r) = self.diag.results.take() else { return };
        let items: Vec<Finding> = match which {
            Some(i) => r.items.get(i).cloned().into_iter().collect(),
            None => r.items.clone(),
        };
        let rest: Vec<Finding> = match which {
            Some(i) => r.items.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, f)| f.clone()).collect(),
            None => Vec::new(),
        };
        let kind = r.kind;
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == r.doc_id) else { return };
        if items.is_empty() {
            return;
        }
        self.engine.stop();
        let n = items.len();
        let len = doc.len();
        let all = vec![true; doc.n_ch()];
        match kind {
            0 | 1 => {
                let mut audio = (*doc.audio).clone();
                if kind == 0 {
                    diagnose::repair_clicks(&mut audio, &items);
                } else {
                    diagnose::repair_clipping(&mut audio, &items);
                }
                let label = if kind == 0 { "Repair Clicks" } else { "Repair Clipping" };
                let (sel, cur) = (doc.sel, doc.cursor);
                apply_edit(doc, label, (0, len), &all, audio, None, false);
                doc.sel = sel;
                doc.cursor = cur;
                // Repaired items are gone; the rest are unchanged.
                self.diag.results = Some(DiagResults { doc_id: r.doc_id, version: doc.version, kind, items: rest });
                self.set_status(format!("Repaired {n} {}", if kind == 0 { "click(s)" } else { "clipped section(s)" }));
            }
            2 => {
                let audio = diagnose::delete_regions(&doc.audio, &items);
                // Move markers back by the silence removed before them.
                let mut regions: Vec<(usize, usize)> = items.iter().map(|f| (f.start, f.end())).collect();
                regions.sort_unstable();
                let markers: Vec<Marker> = doc
                    .markers
                    .iter()
                    .map(|m| {
                        let removed: usize = regions.iter().map(|&(a, b)| if m.pos >= b { b - a } else if m.pos > a { m.pos - a } else { 0 }).sum();
                        Marker { pos: m.pos - removed.min(m.pos), name: m.name.clone() }
                    })
                    .collect();
                doc.push_undo("Delete Silence");
                doc.set_audio(audio);
                doc.markers = markers;
                doc.sel = None;
                doc.clamp_view();
                self.set_status(format!("Deleted {n} silent section(s)"));
                // Positions after a deletion have moved: scan again.
                self.diag_scan();
            }
            _ => {
                doc.push_undo("Mark Audio");
                let base = doc.markers.len();
                for (k, f) in items.iter().enumerate() {
                    doc.markers.push(Marker { pos: f.start, name: format!("Audio {:02} ({})", base + k + 1, format_time(f.len as f64, doc.sample_rate)) });
                }
                doc.markers.sort_by_key(|m| m.pos);
                doc.dirty = true;
                self.diag.results = Some(DiagResults { doc_id: r.doc_id, version: doc.version, kind, items: rest });
                self.set_status(format!("Added {n} marker(s)"));
            }
        }
        self.diag.selected = None;
    }

    pub fn diagnostics_panel(&mut self, ui: &mut Ui) {
        let Some((doc_id, version, sr)) = self.doc().map(|d| (d.id, d.version, d.sample_rate)) else {
            ui.label(RichText::new("Open a file to scan it.").color(TEXT_DIM));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Effect:").color(TEXT_DIM));
            let before = self.diag.kind;
            egui::ComboBox::from_id_source("diag_kind").width(ui.available_width() - 4.0).selected_text(DIAG_KINDS[self.diag.kind]).show_ui(ui, |ui| {
                for (i, k) in DIAG_KINDS.iter().enumerate() {
                    ui.selectable_value(&mut self.diag.kind, i, *k);
                }
            });
            if before != self.diag.kind {
                self.diag.results = None;
            }
        });
        let d = &mut self.diag;
        egui::Grid::new("diag_params").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            let row = |ui: &mut Ui, label: &str, v: &mut f32, lo: f32, hi: f32, suffix: &str| {
                ui.label(RichText::new(label).color(TEXT_DIM).size(11.5));
                hot_drag(ui, egui::DragValue::new(v).speed(0.2).range(lo..=hi).fixed_decimals(1).suffix(suffix));
                ui.end_row();
            };
            match d.kind {
                0 => {
                    row(ui, "Sensitivity", &mut d.sensitivity, 1.0, 100.0, "");
                    row(ui, "Max click length", &mut d.max_ms, 0.05, 5.0, " ms");
                }
                1 => row(ui, "Clip level", &mut d.clip_db, -12.0, 0.0, " dBFS"),
                2 => {
                    row(ui, "Quieter than", &mut d.silence_db, -96.0, -10.0, " dB");
                    row(ui, "For at least", &mut d.silence_ms, 10.0, 10000.0, " ms");
                }
                _ => {
                    row(ui, "Louder than", &mut d.audio_db, -96.0, -3.0, " dB");
                    row(ui, "For at least", &mut d.audio_ms, 10.0, 10000.0, " ms");
                }
            }
        });
        let scanning = self.diag.rx.is_some();
        let has_sel = self.doc().and_then(|d| d.sel_range()).is_some();
        ui.horizontal(|ui| {
            let label = if scanning { "Scanning…" } else if has_sel { "Scan Selection" } else { "Scan" };
            if ui.add_enabled(!scanning, egui::Button::new(label)).clicked() {
                self.diag_scan();
            }
            let stale = self.diag.results.as_ref().map(|r| r.doc_id != doc_id || r.version != version).unwrap_or(false);
            if stale {
                ui.label(RichText::new("File changed: scan again").color(WARN).size(11.0));
            }
        });
        let results = self.diag.results.as_ref().filter(|r| r.doc_id == doc_id && r.version == version);
        let Some(r) = results else { return };
        let kind = r.kind;
        let items = r.items.clone();
        let mut select: Option<usize> = None;
        let list_h = (ui.available_height() - 30.0).max(40.0);
        egui::Frame::none().fill(BG_LIST).inner_margin(egui::Margin::same(3.0)).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                if items.is_empty() {
                    ui.label(RichText::new("Nothing found.").color(TEXT_DIM));
                    return;
                }
                egui::Grid::new("diag_list").num_columns(4).spacing([10.0, 2.0]).striped(true).show(ui, |ui| {
                    for h in ["#", "Start", "Length", if kind == 0 { "Height" } else { "Level" }] {
                        ui.label(RichText::new(h).color(TEXT_DIM).size(11.0));
                    }
                    ui.end_row();
                    for (i, f) in items.iter().enumerate().take(5000) {
                        let sel = self.diag.selected == Some(i);
                        if ui.selectable_label(sel, format!("{}", i + 1)).clicked() {
                            select = Some(i);
                        }
                        if ui.selectable_label(sel, RichText::new(format_time(f.start as f64, sr)).monospace()).clicked() {
                            select = Some(i);
                        }
                        ui.label(RichText::new(format!("{:.1} ms", f.len as f64 * 1000.0 / sr as f64)).monospace().size(11.5));
                        ui.label(RichText::new(format!("{:.1} dB", f.level_db)).monospace().size(11.5));
                        ui.end_row();
                    }
                });
            });
        });
        if let Some(i) = select {
            self.diag.selected = Some(i);
            let f = items[i].clone();
            if let Some(doc) = self.doc_mut() {
                // Select the finding and show it with some context.
                doc.sel = Some((f.start, f.end().min(doc.len())));
                doc.cursor = f.start;
                let pad = (f.len.max(doc.sample_rate as usize / 20)) as f64;
                doc.view_start = f.start as f64 - pad;
                doc.view_end = f.end() as f64 + pad;
                doc.clamp_view();
            }
        }
        let verb = match kind {
            0 | 1 => "Repair",
            2 => "Delete",
            _ => "Mark",
        };
        let mut apply: Option<Option<usize>> = None;
        ui.horizontal(|ui| {
            if ui.add_enabled(self.diag.selected.is_some(), egui::Button::new(verb)).clicked() {
                apply = Some(self.diag.selected);
            }
            if ui.add_enabled(!items.is_empty(), egui::Button::new(format!("{verb} All"))).clicked() {
                apply = Some(None);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("{} found", items.len())).color(TEXT_DIM).size(11.0));
            });
        });
        if let Some(w) = apply {
            self.diag_apply(w);
        }
    }

    // ------------------------------------------------------------ Batch

    fn batch_process_names(&self) -> Vec<String> {
        let n = self.rack.iter().filter(|s| s.on).count();
        let mut v = vec!["None (convert only)".to_string(), format!("Effects Rack chain ({n} effect{})", if n == 1 { "" } else { "s" }), format!("Match Loudness ({:.1} LUFS)", self.analysis.ml_target)];
        v.extend(FAVORITES.iter().map(|f| format!("Favorite: {}", f.0)));
        v
    }

    fn batch_run(&mut self) {
        let job = match self.batch.process {
            0 => BatchJob::None,
            1 => BatchJob::Rack { slots: self.rack.iter().filter(|s| s.on).map(|s| (s.idx, s.params.clone())).collect(), mix: self.rack_mix, in_db: self.rack_in_db, out_db: self.rack_out_db },
            2 => match self.find_effect("match_loudness") {
                Some(idx) => {
                    let mut p = self.effects[idx].default_params();
                    p.set("target", Value::F(self.analysis.ml_target));
                    p.set("ceiling", Value::F(self.analysis.ml_tp));
                    BatchJob::Effect(idx, p)
                }
                None => BatchJob::None,
            },
            k => {
                let (_, id, preset) = FAVORITES[(k - 3).min(FAVORITES.len() - 1)];
                match self.find_effect(id) {
                    Some(idx) => {
                        let def = &self.effects[idx];
                        let p = if preset.is_empty() { def.default_params() } else { def.preset_params(preset).unwrap_or_else(|| def.default_params()) };
                        BatchJob::Effect(idx, p)
                    }
                    None => BatchJob::None,
                }
            }
        };
        for f in self.batch.files.iter_mut() {
            f.1 = BatchStatus::Pending;
        }
        let files: Vec<PathBuf> = self.batch.files.iter().map(|f| f.0.clone()).collect();
        let (effects, fmt, rate, out_dir, suffix) = (self.effects.clone(), self.batch.format, self.batch.rate, self.batch.out_dir.clone(), self.batch.suffix.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.batch.cancel = cancel.clone();
        self.batch.done = 0;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut written: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
            for (i, path) in files.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let written_ref = &mut written;
                let files_ref = &files;
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<String, String> {
                    let dec = io::load(path)?;
                    let mut audio = dec.channels;
                    let mut sr = dec.sample_rate;
                    let n_ch = audio.len();
                    audio = match &job {
                        BatchJob::None => audio,
                        BatchJob::Rack { slots, mix, in_db, out_db } => run_rack(&effects, slots, audio, sr, n_ch, None, *mix, *in_db, *out_db)?,
                        BatchJob::Effect(idx, p) => {
                            let ctx = Ctx { sample_rate: sr, channels: n_ch, noise_print: None };
                            effects[*idx].run(&audio, &ctx, p)?
                        }
                    };
                    if let Some(r) = rate.filter(|r| *r != sr) {
                        audio = resample_channels(&audio, r as f64 / sr as f64);
                        sr = r;
                    }
                    let dir = out_dir.clone().or_else(|| path.parent().map(|p| p.to_path_buf())).unwrap_or_default();
                    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "audio".into());
                    let mut out = dir.join(format!("{stem}{suffix}.wav"));
                    let mut k = 2;
                    // Never overwrite a source file, another output of this run,
                    // or anything already on disk: number the name instead.
                    while out.exists() || written_ref.contains(&out) || files_ref.contains(&out) {
                        out = dir.join(format!("{stem}{suffix} {k}.wav"));
                        k += 1;
                    }
                    io::save_wav(&out, &audio, sr, fmt, fmt == WavFormat::Pcm16)?;
                    written_ref.insert(out.clone());
                    Ok(out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                }))
                .unwrap_or_else(|_| Err("failed unexpectedly".into()));
                if tx.send((i, r)).is_err() {
                    break;
                }
            }
        });
        self.batch.rx = Some(rx);
    }

    pub fn batch_panel(&mut self, ui: &mut Ui) {
        let running = self.batch.rx.is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if ui.add_enabled(!running, egui::Button::new("Add Files…")).clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Audio files", io::OPEN_EXTENSIONS).pick_files() {
                    for f in p {
                        if !self.batch.files.iter().any(|x| x.0 == f) {
                            self.batch.files.push((f, BatchStatus::Pending));
                        }
                    }
                }
            }
            if ui.add_enabled(!running, egui::Button::new("Add Open Files")).on_hover_text("Saved files that are open (the copy on disk is used)").clicked() {
                let paths: Vec<PathBuf> = self.docs.iter().filter_map(|d| d.path.clone()).collect();
                for f in paths {
                    if !self.batch.files.iter().any(|x| x.0 == f) {
                        self.batch.files.push((f, BatchStatus::Pending));
                    }
                }
            }
            if ui.add_enabled(!running && !self.batch.files.is_empty(), egui::Button::new("Clear")).clicked() {
                self.batch.files.clear();
            }
        });
        let list_h = (ui.available_height() - 150.0).max(50.0);
        let mut remove: Option<usize> = None;
        egui::Frame::none().fill(BG_LIST).inner_margin(egui::Margin::same(3.0)).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                if self.batch.files.is_empty() {
                    ui.label(RichText::new("Add files to process.").color(TEXT_DIM));
                }
                let next = self.batch.files.iter().position(|f| f.1 == BatchStatus::Pending);
                for (i, (p, st)) in self.batch.files.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let (mark, col) = match st {
                            BatchStatus::Done(_) => ("✔", WAVE),
                            BatchStatus::Failed(_) => ("✖", RECORD),
                            BatchStatus::Pending if running && next == Some(i) => ("…", HOT),
                            BatchStatus::Pending => ("•", TEXT_DIM),
                        };
                        ui.label(RichText::new(mark).color(col));
                        let r = ui.label(name).on_hover_text(p.display().to_string());
                        match st {
                            BatchStatus::Done(out) => {
                                r.on_hover_text(format!("Saved as {out}"));
                            }
                            BatchStatus::Failed(e) => {
                                ui.label(RichText::new(e).color(RECORD).size(10.5));
                            }
                            _ => {}
                        }
                        if !running {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("×").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                    });
                }
            });
        });
        if let Some(i) = remove {
            self.batch.files.remove(i);
        }
        let names = self.batch_process_names();
        egui::Grid::new("batch_opts").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Process").color(TEXT_DIM));
            egui::ComboBox::from_id_source("batch_proc").width(180.0).selected_text(names[self.batch.process.min(names.len() - 1)].clone()).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut self.batch.process, i, n);
                }
            });
            ui.end_row();
            ui.label(RichText::new("Format").color(TEXT_DIM));
            egui::ComboBox::from_id_source("batch_fmt").width(180.0).selected_text(format!("WAV {}", self.batch.format.label())).show_ui(ui, |ui| {
                for f in [WavFormat::Pcm16, WavFormat::Pcm24, WavFormat::Pcm32, WavFormat::Float32] {
                    ui.selectable_value(&mut self.batch.format, f, format!("WAV {}", f.label()));
                }
            });
            ui.end_row();
            ui.label(RichText::new("Sample rate").color(TEXT_DIM));
            let rate_label = self.batch.rate.map(|r| format!("{r} Hz")).unwrap_or_else(|| "Keep original".into());
            egui::ComboBox::from_id_source("batch_rate").width(180.0).selected_text(rate_label).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.batch.rate, None, "Keep original");
                for r in crate::app::SAMPLE_RATES {
                    ui.selectable_value(&mut self.batch.rate, Some(r), format!("{r} Hz"));
                }
            });
            ui.end_row();
            ui.label(RichText::new("Save to").color(TEXT_DIM));
            ui.horizontal(|ui| {
                let label = self.batch.out_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Same folder as each file".into());
                if ui.button(label).on_hover_text("Choose an output folder").clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.batch.out_dir = Some(d);
                    }
                }
                if self.batch.out_dir.is_some() && ui.small_button("×").on_hover_text("Use each file's own folder").clicked() {
                    self.batch.out_dir = None;
                }
            });
            ui.end_row();
            ui.label(RichText::new("Name suffix").color(TEXT_DIM));
            ui.add(egui::TextEdit::singleline(&mut self.batch.suffix).desired_width(120.0));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if running {
                let total = self.batch.files.len().max(1);
                ui.add(egui::ProgressBar::new(self.batch.done as f32 / total as f32).desired_width(ui.available_width() - 70.0).text(format!("{} / {}", self.batch.done, total)));
                if ui.button("Stop").clicked() {
                    self.batch.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                let n = self.batch.files.len();
                if ui.add_enabled(n > 0, egui::Button::new(RichText::new(format!("  Run ({n})  ")).color(Color32::WHITE)).min_size(vec2(90.0, 24.0))).clicked() {
                    self.batch_run();
                }
            }
        });
    }
}
