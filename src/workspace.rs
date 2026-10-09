//! Favorites, Media Browser, Effects Rack and Properties panels.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};

use crate::app::{Action, App, Dialog, EffectDialog, RackSlot, FAVORITES, RACK_SLOTS};
use crate::dsp::effects::Category;
use crate::dsp::util::{format_time, lin_to_db, peak_ch, rms_ch};
use crate::engine::{BROWSER_TAG, PREVIEW_TAG};
use crate::io::OPEN_EXTENSIONS;
use crate::theme::*;

fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| OPEN_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_073_741_824.0 {
        format!("{:.1} GB", b / 1_073_741_824.0)
    } else if b >= 1_048_576.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Folder shortcuts for the Media Browser location menu.
fn locations() -> Vec<(String, PathBuf)> {
    let mut v = Vec::new();
    if let Some(h) = crate::prefs::home_dir() {
        v.push(("Home".to_string(), h.clone()));
        for sub in ["Desktop", "Documents", "Music", "Downloads"] {
            let p = h.join(sub);
            if p.is_dir() {
                v.push((sub.to_string(), p));
            }
        }
    }
    if cfg!(target_os = "windows") {
        for letter in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", letter as char));
            if p.is_dir() {
                v.push((format!("Drive {}:", letter as char), p));
            }
        }
    } else {
        let mounts = if cfg!(target_os = "macos") { vec!["/Volumes"] } else { vec!["/media", "/mnt"] };
        for m in mounts {
            if let Ok(rd) = std::fs::read_dir(m) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        v.push((e.file_name().to_string_lossy().to_string(), p));
                    }
                }
            }
        }
        v.push(("Computer (/)".to_string(), PathBuf::from("/")));
    }
    v
}

fn list_dir(dir: &Path) -> Vec<(PathBuf, bool, u64)> {
    let mut v: Vec<(PathBuf, bool, u64)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter_map(|e| {
                    let p = e.path();
                    let md = e.metadata().ok()?;
                    if md.is_dir() {
                        Some((p, true, 0))
                    } else if is_audio(&p) {
                        Some((p, false, md.len()))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.file_name().cmp(&b.0.file_name())));
    v
}

impl App {
    pub fn favorites_panel(&mut self, ui: &mut Ui) {
        let has_doc = self.doc().is_some();
        ui.label(RichText::new("Click to apply to the selection, or the whole file.").color(TEXT_DIM).size(11.0));
        ui.add_space(2.0);
        let tips: Vec<String> = FAVORITES
            .iter()
            .map(|(_, id, preset)| match self.find_effect(id) {
                Some(i) if preset.is_empty() => self.effects[i].name.to_string(),
                Some(i) => format!("{}: {}", self.effects[i].name, preset),
                None => String::new(),
            })
            .collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for ((label, id, preset), tip) in FAVORITES.iter().zip(tips) {
                let r = ui.add_enabled(has_doc, egui::SelectableLabel::new(false, *label)).on_hover_text(tip);
                if r.clicked() {
                    self.actions.push(Action::ApplyFavorite(*id, *preset));
                }
            }
        });
    }

    pub fn media_browser(&mut self, ui: &mut Ui) {
        let mut go: Option<PathBuf> = None;
        ui.horizontal(|ui| {
            let here = self.browser_dir.clone();
            egui::ComboBox::from_id_source("mb_loc")
                .width(130.0)
                .selected_text(here.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| here.display().to_string()))
                .show_ui(ui, |ui| {
                    for (name, p) in locations() {
                        if ui.selectable_label(p == here, name).clicked() {
                            go = Some(p);
                        }
                    }
                });
            if ui.small_button("Up").on_hover_text("Parent folder").clicked() {
                if let Some(parent) = here.parent() {
                    go = Some(parent.to_path_buf());
                }
            }
            if ui.small_button("Refresh").clicked() {
                self.browser_entries = None;
            }
            if self.engine.is_playing_tag(BROWSER_TAG) && ui.small_button("Stop").clicked() {
                self.engine.stop();
            }
        });
        ui.horizontal(|ui| {
            let mut auto = self.prefs.browser_autoplay;
            if ui.checkbox(&mut auto, "Auto-Play").on_hover_text("Play files when you click them").changed() {
                self.prefs.browser_autoplay = auto;
                self.prefs.save();
            }
            ui.label(RichText::new(self.browser_dir.display().to_string()).color(TEXT_DIM).size(10.5));
        });
        ui.add_space(2.0);
        let stale = self.browser_entries.as_ref().map(|(d, _)| d != &self.browser_dir).unwrap_or(true);
        if stale {
            let dir = self.browser_dir.clone();
            self.browser_entries = Some((dir.clone(), list_dir(&dir)));
        }
        let entries = self.browser_entries.as_ref().map(|(_, e)| e.clone()).unwrap_or_default();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if entries.is_empty() {
                ui.label(RichText::new("No folders or audio files here.").color(TEXT_DIM));
            }
            egui::Grid::new("mb_list").num_columns(2).striped(true).spacing([8.0, 2.0]).show(ui, |ui| {
                for (p, is_dir, size) in &entries {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let selected = self.browser_selected.as_deref() == Some(p.as_path());
                    let label = if *is_dir {
                        RichText::new(format!("▸ {name}")).color(TEXT)
                    } else {
                        RichText::new(format!("   {name}")).color(if selected { WAVE_SEL } else { TEXT })
                    };
                    let r = ui.selectable_label(selected, label);
                    if *is_dir {
                        if r.clicked() {
                            go = Some(p.clone());
                        }
                        ui.label("");
                    } else {
                        let r = r.on_hover_text("Click to audition, double-click to open");
                        if r.double_clicked() {
                            self.actions.push(Action::OpenPaths(vec![p.clone()]));
                        } else if r.clicked() {
                            self.actions.push(Action::Audition(p.clone()));
                        }
                        ui.label(RichText::new(human_size(*size)).color(TEXT_DIM).size(10.5));
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(d) = go {
            self.browser_dir = d.clone();
            self.browser_entries = None;
            self.prefs.browser_dir = Some(d);
            self.prefs.save();
        }
    }

    pub fn effects_rack(&mut self, ui: &mut Ui) {
        let effects = self.effects.clone();
        let mut changed = false;
        let mut edit: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let scope = match self.doc().and_then(|d| d.sel_range()) {
            Some(_) => "Process: selection",
            None => "Process: entire file",
        };
        egui::ScrollArea::vertical().max_height((ui.available_height() - 120.0).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
            let n = self.rack.len();
            for (i, slot) in self.rack.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut slot.on, "").on_hover_text("Power").changed() {
                        changed = true;
                    }
                    ui.label(RichText::new(format!("{:>2}", i + 1)).monospace().color(TEXT_DIM));
                    let def = &effects[slot.idx];
                    let name = RichText::new(def.name).color(if slot.on { TEXT } else { TEXT_DIM });
                    let r = ui.selectable_label(false, name);
                    if r.clicked() && !def.params.is_empty() {
                        edit = Some(i);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                        if i + 1 < n && ui.small_button("↓").on_hover_text("Move down").clicked() {
                            swap = Some((i, i + 1));
                        }
                        if i > 0 && ui.small_button("↑").on_hover_text("Move up").clicked() {
                            swap = Some((i, i - 1));
                        }
                    });
                });
            }
            if self.rack.len() < RACK_SLOTS {
                ui.menu_button(RichText::new(format!("{:>2}  + Add effect", self.rack.len() + 1)).color(ACCENT), |ui| {
                    for cat in std::iter::once(Category::Basic).chain(Category::MENU_ORDER) {
                        if cat == Category::Generate {
                            continue;
                        }
                        ui.menu_button(cat.name(), |ui| {
                            for (i, e) in effects.iter().enumerate().filter(|(_, e)| e.category == cat && !e.changes_length) {
                                if ui.button(e.name).clicked() {
                                    ui.close_menu();
                                    self.rack.push(RackSlot { idx: i, params: e.default_params(), on: true });
                                    changed = true;
                                    if !e.params.is_empty() {
                                        edit = Some(self.rack.len() - 1);
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
        if let Some(i) = remove {
            self.rack.remove(i);
            changed = true;
        }
        if let Some((a, b)) = swap {
            self.rack.swap(a, b);
            changed = true;
        }
        ui.separator();
        egui::Grid::new("rack_io").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("Input");
            changed |= ui.add(egui::Slider::new(&mut self.rack_in_db, -24.0..=24.0).suffix(" dB").fixed_decimals(1)).changed();
            ui.end_row();
            ui.label("Output");
            changed |= ui.add(egui::Slider::new(&mut self.rack_out_db, -24.0..=24.0).suffix(" dB").fixed_decimals(1)).changed();
            ui.end_row();
            ui.label("Mix (wet)");
            changed |= ui.add(egui::Slider::new(&mut self.rack_mix, 0.0..=100.0).suffix(" %").fixed_decimals(0)).changed();
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let has_doc = self.doc().is_some();
            let label = if self.rack_previewing { "Stop Preview" } else { "Preview" };
            if ui.add_enabled(has_doc, egui::Button::new(label)).on_hover_text("Loop the selection through the rack").clicked() {
                self.rack_previewing = !self.rack_previewing;
                if self.rack_previewing {
                    self.rack_rendered = 0;
                    self.rack_changed = Instant::now().checked_sub(std::time::Duration::from_millis(500)).unwrap_or_else(Instant::now);
                } else if self.engine.is_playing_tag(PREVIEW_TAG) {
                    self.engine.stop();
                }
            }
            let any_on = self.rack.iter().any(|s| s.on);
            if ui
                .add_enabled(has_doc && any_on, egui::Button::new(RichText::new("Apply").color(Color32::WHITE)).fill(ACCENT))
                .on_hover_text("Process the audio with every enabled effect")
                .clicked()
            {
                self.actions.push(Action::ApplyRack);
            }
            if ui.add_enabled(!self.rack.is_empty(), egui::Button::new("Clear")).clicked() {
                self.rack.clear();
                changed = true;
            }
        });
        ui.label(RichText::new(scope).color(TEXT_DIM).size(11.0));
        if changed {
            self.rack_touched();
        }
        if let Some(i) = edit {
            let slot = self.rack[i].clone();
            self.close_effect_dialog();
            self.dialog = Some(Dialog::Effect(EffectDialog {
                idx: slot.idx,
                params: slot.params,
                preset: "(Custom)".into(),
                previewing: false,
                bypass: false,
                rendered: None,
                last_change: Instant::now(),
                drag_handle: None,
                dry: None,
                wet: None,
                error: None,
                rack_slot: Some(i),
            }));
        }
    }

    pub fn properties_panel(&mut self, ui: &mut Ui) {
        let Some(doc) = self.doc() else {
            ui.label(RichText::new("No file selected.").color(TEXT_DIM));
            return;
        };
        let id = egui::Id::new(("props", doc.id, doc.version));
        let stats: Option<(f32, f32)> = ui.data_mut(|d| d.get_temp::<(f32, f32)>(id));
        let (pk, rms) = match stats {
            Some(s) => s,
            None => {
                let pk = doc.audio.iter().map(|c| peak_ch(c)).fold(0.0, f32::max);
                let rms = doc.audio.iter().map(|c| rms_ch(c)).fold(0.0, f32::max);
                ui.data_mut(|d| d.insert_temp(id, (pk, rms)));
                (pk, rms)
            }
        };
        let format = doc
            .path
            .as_ref()
            .and_then(|p| p.extension())
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "Unsaved".into());
        let rows: Vec<(&str, String)> = vec![
            ("Name", doc.name.clone()),
            ("Location", doc.path.as_ref().and_then(|p| p.parent()).map(|p| p.display().to_string()).unwrap_or_else(|| "Not saved yet".into())),
            ("Format", format),
            ("Sample Rate", format!("{} Hz", doc.sample_rate)),
            ("Channels", if doc.n_ch() == 1 { "Mono".into() } else { "Stereo".into() }),
            ("Bit Depth", doc.source_bits.map(|b| format!("{b}-bit")).unwrap_or_else(|| "32-bit (float)".into())),
            ("Duration", format_time(doc.len() as f64, doc.sample_rate)),
            ("Samples", format!("{}", doc.len())),
            ("Peak Amplitude", format!("{:.2} dBFS", lin_to_db(pk).max(-200.0))),
            ("RMS (loudest ch.)", format!("{:.2} dBFS", lin_to_db(rms).max(-200.0))),
            ("Markers", format!("{}", doc.markers.len())),
            ("Unsaved Changes", if doc.dirty { "Yes".into() } else { "No".into() }),
        ];
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("props").num_columns(2).striped(true).spacing([10.0, 3.0]).show(ui, |ui| {
                for (k, v) in rows {
                    ui.label(RichText::new(k).color(TEXT_DIM).size(11.5));
                    ui.label(RichText::new(v).size(11.5));
                    ui.end_row();
                }
            });
        });
    }
}
