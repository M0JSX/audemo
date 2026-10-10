//! Analysis panels: Frequency Analysis, Phase Meter, Match Loudness and the
//! Amplitude Statistics window.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Shape, Stroke, Ui};

use crate::app::{App, Dialog, JobKind};
use crate::dsp::analysis::{self, AmplitudeStats};
use crate::dsp::loudness::{self, Loudness};
use crate::dsp::params::Value;
use crate::dsp::util::{format_time, slice_range};
use crate::theme::*;

pub const FFT_SIZES: [usize; 6] = [1024, 2048, 4096, 8192, 16384, 32768];
const RIGHT_COL: Color32 = Color32::from_rgb(0x4d, 0xa3, 0xff);

pub const LOUDNESS_PRESETS: [(&str, f32, f32); 4] = [
    ("EBU R128 (-23 LUFS)", -23.0, -1.0),
    ("ATSC A/85 (-24 LUFS)", -24.0, -2.0),
    ("Podcast (-16 LUFS)", -16.0, -1.0),
    ("Streaming (-14 LUFS)", -14.0, -1.0),
];

pub struct FaScan {
    pub doc_id: u64,
    pub version: u64,
    pub n: usize,
    pub label: String,
    pub spectra: Vec<Vec<f32>>,
}

pub struct AnalysisState {
    pub fft_idx: usize,
    smooth: Vec<Vec<f32>>,
    pub scan: Option<FaScan>,
    scan_rx: Option<Receiver<FaScan>>,
    corr: f32,
    // Match Loudness
    pub ml_target: f32,
    pub ml_tp: f32,
    pub ml_excluded: HashSet<u64>,
    ml_cache: HashMap<u64, (u64, Loudness)>,
    ml_scanning: HashSet<(u64, u64)>,
    ml_tx: Option<Sender<(u64, u64, Loudness)>>,
    ml_rx: Option<Receiver<(u64, u64, Loudness)>>,
    pub ml_queue: VecDeque<u64>,
    ml_total: usize,
}

impl Default for AnalysisState {
    fn default() -> Self {
        AnalysisState {
            fft_idx: 3,
            smooth: Vec::new(),
            scan: None,
            scan_rx: None,
            corr: 0.0,
            ml_target: -16.0,
            ml_tp: -1.0,
            ml_excluded: HashSet::new(),
            ml_cache: HashMap::new(),
            ml_scanning: HashSet::new(),
            ml_tx: None,
            ml_rx: None,
            ml_queue: VecDeque::new(),
            ml_total: 0,
        }
    }
}

fn fmt_db(v: f64) -> String {
    if v <= -199.0 {
        "-inf".into()
    } else {
        format!("{v:.2}")
    }
}

fn opt(v: Option<f64>, unit: &str) -> String {
    v.map(|x| format!("{x:.1} {unit}")).unwrap_or_else(|| "—".into())
}

impl App {
    /// Background results and the Match Loudness run queue.
    pub fn poll_analysis(&mut self) {
        if let Some(rx) = &self.analysis.scan_rx {
            if let Ok(s) = rx.try_recv() {
                self.analysis.scan = Some(s);
                self.analysis.scan_rx = None;
            }
        }
        if let Some(rx) = &self.analysis.ml_rx {
            while let Ok((id, ver, l)) = rx.try_recv() {
                self.analysis.ml_scanning.remove(&(id, ver));
                self.analysis.ml_cache.insert(id, (ver, l));
            }
        }
        if !self.analysis.ml_queue.is_empty() && self.job.is_none() {
            let id = self.analysis.ml_queue.pop_front().unwrap();
            self.run_match_loudness(id);
        }
    }

    fn run_match_loudness(&mut self, doc_id: u64) {
        let Some(idx) = self.find_effect("match_loudness") else { return };
        let Some(doc) = self.docs.iter().find(|d| d.id == doc_id) else { return };
        if doc.len() == 0 {
            return;
        }
        let effects = self.effects.clone();
        let mut params = effects[idx].default_params();
        params.set("target", Value::F(self.analysis.ml_target));
        params.set("ceiling", Value::F(self.analysis.ml_tp));
        params.set("limit", Value::B(true));
        let input = (*doc.audio).clone();
        let (sr, n_ch, len) = (doc.sample_rate, doc.n_ch(), doc.len());
        let done = self.analysis.ml_total - self.analysis.ml_queue.len();
        let label = format!("Match Loudness ({done} of {})", self.analysis.ml_total);
        self.engine.stop();
        self.spawn_job(label, JobKind::Edit { doc_id, range: (0, len), active: vec![true; n_ch], new_rate: None, select: false }, move || {
            let ctx = crate::dsp::effects::Ctx { sample_rate: sr, channels: n_ch, noise_print: None };
            effects[idx].run(&input, &ctx, &params)
        });
    }

    // ------------------------------------------------------------ Frequency

    pub fn frequency_analysis(&mut self, ui: &mut Ui) {
        let Some(doc) = self.doc() else {
            ui.label(RichText::new("No file").color(TEXT_DIM()));
            return;
        };
        let (doc_id, version, sr, n_ch) = (doc.id, doc.version, doc.sample_rate, doc.n_ch());
        let sel = doc.sel_range();
        let pos = self.display_pos() as usize;
        let playing = self.engine.status().playing;
        let n = FFT_SIZES[self.analysis.fft_idx.min(FFT_SIZES.len() - 1)];

        // Controls.
        let mut scan_now = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("FFT Size:").color(TEXT_DIM()));
            egui::ComboBox::from_id_source("fa_size").width(70.0).selected_text(format!("{n}")).show_ui(ui, |ui| {
                for (i, s) in FFT_SIZES.iter().enumerate() {
                    ui.selectable_value(&mut self.analysis.fft_idx, i, format!("{s}"));
                }
            });
            let busy = self.analysis.scan_rx.is_some();
            let label = if sel.is_some() { "Scan Selection" } else { "Scan File" };
            if ui.add_enabled(!busy, egui::Button::new(if busy { "Scanning…" } else { label })).on_hover_text("Average the spectrum over the selection (or whole file)").clicked() {
                scan_now = true;
            }
            if self.analysis.scan.is_some() && ui.small_button("Clear").clicked() {
                self.analysis.scan = None;
            }
        });
        if scan_now {
            let doc = self.doc().unwrap();
            let (a, b) = sel.unwrap_or((0, doc.len()));
            let audio = doc.audio.clone();
            let label = if sel.is_some() { "Selection average".to_string() } else { "File average".to_string() };
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let spectra = audio.iter().map(|c| analysis::spectrum_average(c, a, b, n)).collect();
                let _ = tx.send(FaScan { doc_id, version, n, label, spectra });
            });
            self.analysis.scan_rx = Some(rx);
        }

        // Live spectrum at the playhead / cursor.
        let doc = self.doc().unwrap();
        let live: Vec<Vec<f32>> = if doc.len() == 0 { Vec::new() } else { doc.audio.iter().map(|c| analysis::spectrum_at(c, pos, n)).collect() };
        let sm = &mut self.analysis.smooth;
        if sm.len() != live.len() || sm.first().map(|v| v.len()) != live.first().map(|v| v.len()) || !playing {
            *sm = live.clone();
        } else {
            for (s, l) in sm.iter_mut().zip(&live) {
                for (a, b) in s.iter_mut().zip(l) {
                    *a = if *b > *a { *b } else { (*a - 1.2).max(*b) };
                }
            }
        }
        let shown = self.analysis.smooth.clone();
        let scan = self.analysis.scan.as_ref().filter(|s| s.doc_id == doc_id && s.version == version);

        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::hover());
        if rect.height() < 30.0 || rect.width() < 60.0 {
            return;
        }
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, LANE_BG());
        let plot = Rect::from_min_max(pos2(rect.left() + 30.0, rect.top() + 4.0), pos2(rect.right() - 6.0, rect.bottom() - 14.0));
        let fmin = 20.0f32;
        let fmax = sr as f32 / 2.0;
        let (db_lo, db_hi) = (-108.0f32, 0.0f32);
        let fx = |f: f32| plot.left() + ((f / fmin).ln() / (fmax / fmin).ln()) * plot.width();
        let xf = |x: f32| fmin * (fmax / fmin).powf(((x - plot.left()) / plot.width()).clamp(0.0, 1.0));
        let dy = |db: f32| plot.top() + (db_hi - db.clamp(db_lo, db_hi)) / (db_hi - db_lo) * plot.height();
        for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
            if f >= fmax {
                continue;
            }
            let x = fx(f);
            p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0_f32, LANE_GRID()));
            let l = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
            p.text(pos2(x, plot.bottom() + 1.0), Align2::CENTER_TOP, l, FontId::monospace(9.0), TEXT_DIM());
        }
        let mut db = 0.0;
        while db >= db_lo {
            let y = dy(db);
            p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0_f32, LANE_GRID()));
            p.text(pos2(plot.left() - 3.0, y), Align2::RIGHT_CENTER, format!("{}", db as i32), FontId::monospace(9.0), TEXT_DIM());
            db -= if plot.height() > 160.0 { 12.0 } else { 24.0 };
        }
        // Spectrum → one point per pixel column (max of the bins it spans).
        let curve = |spec: &[f32], n: usize| -> Vec<egui::Pos2> {
            let cols = plot.width().max(2.0) as usize;
            let bin_hz = sr as f32 / n as f32;
            (0..cols)
                .map(|i| {
                    let x0 = plot.left() + i as f32;
                    let (f0, f1) = (xf(x0), xf(x0 + 1.0));
                    let (k0, k1) = ((f0 / bin_hz).floor() as usize, (f1 / bin_hz).ceil() as usize);
                    let v = if k1 > k0 + 1 {
                        spec[k0.min(spec.len() - 1)..k1.min(spec.len())].iter().cloned().fold(-200.0, f32::max)
                    } else {
                        let k = f0 / bin_hz;
                        let i0 = (k.floor() as usize).min(spec.len() - 1);
                        let i1 = (i0 + 1).min(spec.len() - 1);
                        let t = k - i0 as f32;
                        spec[i0] * (1.0 - t) + spec[i1] * t
                    };
                    pos2(x0, dy(v))
                })
                .collect()
        };
        if let Some(s) = scan {
            for (c, spec) in s.spectra.iter().enumerate() {
                let col = if c == 0 { WAVE() } else { RIGHT_COL };
                let pts = curve(spec, s.n);
                let mut mesh = Vec::with_capacity(pts.len() * 2);
                for w in pts.windows(2) {
                    mesh.push(Shape::convex_polygon(
                        vec![w[0], w[1], pos2(w[1].x, plot.bottom()), pos2(w[0].x, plot.bottom())],
                        col.linear_multiply(0.12),
                        Stroke::NONE,
                    ));
                }
                p.extend(mesh);
                p.add(Shape::line(pts, Stroke::new(1.0_f32, col.linear_multiply(0.6))));
            }
            p.text(pos2(plot.right() - 4.0, plot.top() + 2.0), Align2::RIGHT_TOP, &s.label, FontId::proportional(10.5), TEXT_DIM());
        }
        for (c, spec) in shown.iter().enumerate() {
            let col = if c == 0 { WAVE() } else { RIGHT_COL };
            p.add(Shape::line(curve(spec, n), Stroke::new(1.4_f32, col)));
        }
        if n_ch > 1 {
            p.text(pos2(plot.left() + 4.0, plot.top() + 2.0), Align2::LEFT_TOP, "L", FontId::monospace(10.0), WAVE());
            p.text(pos2(plot.left() + 14.0, plot.top() + 2.0), Align2::LEFT_TOP, "R", FontId::monospace(10.0), RIGHT_COL);
        }
        if let Some(h) = resp.hover_pos().filter(|h| plot.contains(*h)) {
            let f = xf(h.x);
            let k = ((f / (sr as f32 / n as f32)).round() as usize).min(shown.first().map(|s| s.len() - 1).unwrap_or(0));
            let vals: Vec<String> = shown.iter().map(|s| format!("{:.1} dB", s.get(k).copied().unwrap_or(-200.0))).collect();
            p.line_segment([pos2(h.x, plot.top()), pos2(h.x, plot.bottom())], Stroke::new(1.0_f32, TEXT_DIM()));
            let label = if f >= 1000.0 { format!("{:.2} kHz  {}", f / 1000.0, vals.join(" / ")) } else { format!("{f:.0} Hz  {}", vals.join(" / ")) };
            p.text(pos2(h.x + 6.0, plot.top() + 14.0), Align2::LEFT_TOP, label, FontId::monospace(10.0), HOT());
        }
    }

    // ------------------------------------------------------------ Phase

    pub fn phase_meter(&mut self, ui: &mut Ui) {
        let Some(audio) = self.doc().map(|d| d.audio.clone()) else {
            ui.label(RichText::new("No file").color(TEXT_DIM()));
            return;
        };
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        if rect.height() < 30.0 || rect.width() < 80.0 {
            return;
        }
        let p = ui.painter_at(rect);
        if audio.len() < 2 {
            p.text(rect.center(), Align2::CENTER_CENTER, "Mono file — phase metering needs two channels", FontId::proportional(12.0), TEXT_DIM());
            return;
        }
        let pos = self.display_pos() as usize;
        let n = 2048usize;
        let len = audio[0].len();
        let a = pos.saturating_sub(n / 2).min(len.saturating_sub(n));
        let b = (a + n).min(len);
        let (l, r) = (&audio[0][a..b], &audio[1][a..b]);
        let c = analysis::correlation(l, r);
        let playing = self.engine.status().playing;
        self.analysis.corr = if playing { self.analysis.corr * 0.8 + c * 0.2 } else { c };
        let corr = self.analysis.corr;

        // Goniometer (mid up, side across).
        let g = rect.height().min(rect.width() * 0.45) - 8.0;
        let gr = Rect::from_min_size(pos2(rect.left() + 4.0, rect.top() + 4.0), vec2(g, g));
        p.rect_filled(gr, 2.0, LANE_BG());
        let cen = gr.center();
        let half = g * 0.5 - 4.0;
        for (d, lab) in [((-1.0f32, -1.0f32), "L"), ((1.0, -1.0), "R")] {
            let e = pos2(cen.x + d.0 * half * 0.7071, cen.y + d.1 * half * 0.7071);
            p.line_segment([pos2(2.0 * cen.x - e.x, 2.0 * cen.y - e.y), e], Stroke::new(1.0_f32, LANE_GRID()));
            p.text(e, Align2::CENTER_BOTTOM, lab, FontId::monospace(9.0), TEXT_DIM());
        }
        p.line_segment([pos2(cen.x, gr.top() + 2.0), pos2(cen.x, gr.bottom() - 2.0)], Stroke::new(1.0_f32, LANE_GRID()));
        p.line_segment([pos2(gr.left() + 2.0, cen.y), pos2(gr.right() - 2.0, cen.y)], Stroke::new(1.0_f32, LANE_GRID()));
        let pk = l.iter().chain(r).fold(0.0f32, |m, v| m.max(v.abs())).max(0.05);
        let scale = half / (pk * 1.414);
        let dots: Vec<Shape> = l
            .iter()
            .zip(r)
            .step_by(2)
            .map(|(&lv, &rv)| {
                let x = (rv - lv) * 0.7071 * scale;
                let y = (lv + rv) * 0.7071 * scale;
                Shape::rect_filled(Rect::from_center_size(pos2(cen.x + x, cen.y - y), vec2(1.5, 1.5)), 0.0, WAVE().linear_multiply(0.7))
            })
            .collect();
        p.extend(dots);

        // Correlation bar.
        let bx = gr.right() + 14.0;
        let bar = Rect::from_min_max(pos2(bx, rect.center().y - 7.0), pos2(rect.right() - 10.0, rect.center().y + 7.0));
        if bar.width() > 40.0 {
            p.rect_filled(bar, 2.0, Color32::from_rgb(0x0c, 0x0e, 0x12));
            let xc = |v: f32| bar.left() + (v + 1.0) * 0.5 * bar.width();
            let col = if corr < 0.0 { RECORD() } else if corr < 0.3 { WARN() } else { WAVE() };
            let (x0, x1) = (xc(0.0).min(xc(corr)), xc(0.0).max(xc(corr)));
            p.rect_filled(Rect::from_min_max(pos2(x0, bar.top() + 2.0), pos2(x1.max(x0 + 2.0), bar.bottom() - 2.0)), 1.0, col);
            for (v, s) in [(-1.0, "-1"), (-0.5, "-0.5"), (0.0, "0"), (0.5, "+0.5"), (1.0, "+1")] {
                let x = xc(v);
                p.line_segment([pos2(x, bar.bottom()), pos2(x, bar.bottom() + 4.0)], Stroke::new(1.0_f32, TEXT_DIM()));
                p.text(pos2(x, bar.bottom() + 5.0), Align2::CENTER_TOP, s, FontId::monospace(9.0), TEXT_DIM());
            }
            p.text(pos2(bar.left(), bar.top() - 4.0), Align2::LEFT_BOTTOM, "Phase correlation", FontId::proportional(11.0), TEXT_DIM());
            p.text(pos2(bar.right(), bar.top() - 4.0), Align2::RIGHT_BOTTOM, format!("{corr:+.2}"), bold(12.0), col);
        }
    }

    // ------------------------------------------------------------ Match Loudness

    pub fn match_loudness_panel(&mut self, ui: &mut Ui) {
        if self.analysis.ml_tx.is_none() {
            let (tx, rx) = channel();
            self.analysis.ml_tx = Some(tx);
            self.analysis.ml_rx = Some(rx);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Target:").color(TEXT_DIM()));
            let st = &mut self.analysis;
            let current = LOUDNESS_PRESETS.iter().find(|p| p.1 == st.ml_target && p.2 == st.ml_tp).map(|p| p.0).unwrap_or("Custom");
            egui::ComboBox::from_id_source("ml_preset").width(ui.available_width() - 4.0).selected_text(current).show_ui(ui, |ui| {
                for (name, t, tp) in LOUDNESS_PRESETS {
                    if ui.selectable_label(current == name, name).clicked() {
                        st.ml_target = t;
                        st.ml_tp = tp;
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Loudness").color(TEXT_DIM()));
            hot_drag(ui, egui::DragValue::new(&mut self.analysis.ml_target).speed(0.1).range(-40.0..=-5.0).fixed_decimals(1).suffix(" LUFS"));
            ui.label(RichText::new("True peak").color(TEXT_DIM()));
            hot_drag(ui, egui::DragValue::new(&mut self.analysis.ml_tp).speed(0.1).range(-9.0..=0.0).fixed_decimals(1).suffix(" dBTP"));
        });
        ui.add_space(2.0);

        let rows: Vec<(u64, String, u64, bool)> = self.docs.iter().map(|d| (d.id, d.name.clone(), d.version, d.len() > 0)).collect();
        let mut scan: Vec<u64> = Vec::new();
        let table_h = (ui.available_height() - 30.0).max(40.0);
        egui::Frame::none().fill(BG_LIST()).inner_margin(egui::Margin::same(3.0)).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(table_h).min_scrolled_height(table_h).auto_shrink([false, false]).show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(RichText::new("Open files appear here.").color(TEXT_DIM()));
                    return;
                }
                egui::Grid::new("ml_table").num_columns(5).spacing([8.0, 3.0]).striped(true).show(ui, |ui| {
                    for h in ["", "File", "LUFS", "dBTP", "LRA"] {
                        ui.label(RichText::new(h).color(TEXT_DIM()).size(11.0));
                    }
                    ui.end_row();
                    for (id, name, ver, has_audio) in &rows {
                        let mut inc = !self.analysis.ml_excluded.contains(id);
                        if ui.checkbox(&mut inc, "").changed() {
                            if inc {
                                self.analysis.ml_excluded.remove(id);
                            } else {
                                self.analysis.ml_excluded.insert(*id);
                            }
                        }
                        ui.label(RichText::new(name).color(if inc { TEXT() } else { TEXT_DIM() }));
                        let m = self.analysis.ml_cache.get(id).filter(|(v, _)| v == ver).map(|(_, l)| *l);
                        let scanning = self.analysis.ml_scanning.contains(&(*id, *ver));
                        match m {
                            Some(l) => {
                                let off = l.integrated.map(|x| (x - self.analysis.ml_target as f64).abs() > 0.5).unwrap_or(false);
                                ui.label(RichText::new(opt(l.integrated, "")).monospace().color(if off { WARN() } else { TEXT() }));
                                let over = l.true_peak_db > self.analysis.ml_tp as f64 + 0.05;
                                ui.label(RichText::new(fmt_db(l.true_peak_db)).monospace().color(if over { RECORD() } else { TEXT() }));
                                ui.label(RichText::new(opt(l.range, "")).monospace());
                            }
                            None => {
                                let t = if scanning { "…" } else { "—" };
                                for _ in 0..3 {
                                    ui.label(RichText::new(t).color(TEXT_DIM()));
                                }
                                if inc && *has_audio && !scanning {
                                    scan.push(*id);
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
            });
        });
        let included: Vec<u64> = rows.iter().filter(|r| r.3 && !self.analysis.ml_excluded.contains(&r.0)).map(|r| r.0).collect();
        let mut do_scan = false;
        let mut do_run = false;
        ui.horizontal(|ui| {
            let running = !self.analysis.ml_queue.is_empty() || (self.busy() && self.job.as_ref().map(|j| j.label.starts_with("Match Loudness")).unwrap_or(false));
            if ui.add_enabled(!scan.is_empty(), egui::Button::new("Scan")).on_hover_text("Measure loudness of the checked files").clicked() {
                do_scan = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if running {
                    if ui.button("Stop").clicked() {
                        self.analysis.ml_queue.clear();
                    }
                } else if ui.add_enabled(!included.is_empty() && !self.busy(), egui::Button::new("Run")).on_hover_text("Match every checked file to the target").clicked() {
                    do_run = true;
                }
            });
        });
        if do_scan {
            let tx = self.analysis.ml_tx.clone().unwrap();
            for id in scan {
                let Some(d) = self.docs.iter().find(|d| d.id == id) else { continue };
                let (audio, sr, ver) = (d.audio.clone(), d.sample_rate, d.version);
                self.analysis.ml_scanning.insert((id, ver));
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send((id, ver, loudness::measure(&audio, sr)));
                });
            }
        }
        if do_run {
            self.analysis.ml_queue = included.into_iter().collect();
            self.analysis.ml_total = self.analysis.ml_queue.len();
        }
    }

    // ------------------------------------------------------------ Amplitude Statistics

    pub fn open_amplitude_stats(&mut self) {
        let Some(doc) = self.doc() else {
            self.set_status("Open or create a file first.");
            return;
        };
        let (a, b) = doc.sel_range().unwrap_or((0, doc.len()));
        let scope = if doc.sel_range().is_some() {
            format!("Selection {} – {}", format_time(a as f64, doc.sample_rate), format_time(b as f64, doc.sample_rate))
        } else {
            "Entire file".to_string()
        };
        let title = format!("{} — {scope}", doc.name);
        let input = slice_range(&doc.audio, a, b);
        let sr = doc.sample_rate;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(analysis::amplitude_stats(&input, sr));
        });
        self.dialog = Some(Dialog::AmplitudeStats { title, rx: Some(rx), stats: None });
    }

    /// Draw the Amplitude Statistics window; returns the dialog to keep showing.
    pub fn amp_stats_dialog(&mut self, ctx: &egui::Context, title: String, mut rx: Option<Receiver<AmplitudeStats>>, mut stats: Option<AmplitudeStats>) -> Option<Dialog> {
        if let Some(r) = &rx {
            if let Ok(s) = r.try_recv() {
                stats = Some(s);
                rx = None;
            }
        }
        let mut open = true;
        let mut close = false;
        egui::Window::new("Amplitude Statistics")
            .collapsible(false)
            .resizable(false)
            .default_pos(ctx.screen_rect().center() - vec2(200.0, 200.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label(RichText::new(&title).color(TEXT_DIM()));
                ui.add_space(4.0);
                match &stats {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Scanning…");
                        });
                        ctx.request_repaint();
                    }
                    Some(s) => {
                        let text = stats_rows(s);
                        let stereo = s.channels.len() > 1;
                        egui::Frame::none().fill(BG_LIST()).inner_margin(egui::Margin::same(8.0)).rounding(2.0).show(ui, |ui| {
                            egui::Grid::new("ampstats").num_columns(if stereo { 3 } else { 2 }).spacing([18.0, 4.0]).striped(true).show(ui, |ui| {
                                ui.label("");
                                if stereo {
                                    ui.label(RichText::new("Left").color(TEXT_DIM()));
                                    ui.label(RichText::new("Right").color(TEXT_DIM()));
                                } else {
                                    ui.label(RichText::new("Mono").color(TEXT_DIM()));
                                }
                                ui.end_row();
                                for (label, vals) in &text {
                                    ui.label(*label);
                                    for v in vals {
                                        ui.label(RichText::new(v).monospace().color(TEXT()));
                                    }
                                    if vals.len() == 1 && stereo {
                                        ui.label("");
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("Copy to Clipboard").clicked() {
                                let mut out = format!("Amplitude Statistics: {title}\n");
                                for (label, vals) in &text {
                                    out.push_str(&format!("{label}\t{}\n", vals.join("\t")));
                                }
                                ctx.output_mut(|o| o.copied_text = out);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("   Close   ").clicked() {
                                    close = true;
                                }
                            });
                        });
                    }
                }
            });
        if !open || close {
            None
        } else {
            Some(Dialog::AmplitudeStats { title, rx, stats })
        }
    }
}

/// Labelled rows for the statistics table: per-channel values, or a single
/// value for whole-file loudness figures.
fn stats_rows(s: &AmplitudeStats) -> Vec<(&'static str, Vec<String>)> {
    let per = |f: &dyn Fn(&analysis::ChannelStats) -> String| s.channels.iter().map(f).collect::<Vec<_>>();
    vec![
        ("Peak Amplitude", per(&|c| format!("{} dB", fmt_db(c.peak_db)))),
        ("True Peak", per(&|c| format!("{} dBTP", fmt_db(c.true_peak_db)))),
        ("Maximum Sample Value", per(&|c| format!("{:.4}", c.max_sample))),
        ("Minimum Sample Value", per(&|c| format!("{:.4}", c.min_sample))),
        ("Possibly Clipped Samples", per(&|c| format!("{}", c.clipped))),
        ("Total RMS Amplitude", per(&|c| format!("{} dB", fmt_db(c.total_rms_db)))),
        ("Maximum RMS Amplitude", per(&|c| format!("{} dB", fmt_db(c.max_rms_db)))),
        ("Minimum RMS Amplitude", per(&|c| format!("{} dB", fmt_db(c.min_rms_db)))),
        ("Average RMS Amplitude", per(&|c| format!("{} dB", fmt_db(c.avg_rms_db)))),
        ("DC Offset", per(&|c| format!("{:.3} %", c.dc_offset_pct))),
        ("Integrated Loudness", vec![opt(s.integrated_lufs, "LUFS")]),
        ("Loudness Range", vec![opt(s.loudness_range, "LU")]),
        ("Duration", vec![format!("{:.3} s", s.duration_s)]),
    ]
}
