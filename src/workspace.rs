//! Favorites, Media Browser, Effects Rack and Properties panels.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};

use crate::app::{Action, App, Dialog, EffectDialog, RackSlot, FAVORITES, RACK_SLOTS};
use crate::dsp::effects::Category;
use crate::dsp::util::{format_time, lin_to_db, peak_ch, rms_ch};
use crate::engine::BROWSER_TAG;
use crate::io::OPEN_EXTENSIONS;
use crate::theme::*;

/// Built-in Effects Rack presets: (name, [(effect id, effect preset)]).
const RACK_PRESETS: &[(&str, &[(&str, &str)])] = &[
    ("Podcast Voice", &[("parametric_eq", "Remove Rumble"), ("dynamics", "Gentle Vocal"), ("hard_limiter", "Limit to -1 dB")]),
    ("Broadcast Voice", &[("parametric_eq", "Vocal Presence"), ("dynamics", "Broadcast"), ("hard_limiter", "Limit to -1 dB")]),
    ("Clean Up Recording", &[("dehummer", "50 Hz, 4 harmonics"), ("adaptive_nr", ""), ("declicker", "")]),
    ("SSB Radio Voice", &[("parametric_eq", "SSB Voice (2.4 kHz)"), ("dynamics", "Broadcast"), ("hard_limiter", "Limit to -1 dB")]),
    ("Telephone", &[("parametric_eq", "Telephone"), ("distortion", "Radio Overdrive")]),
];

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
        // In the Multitrack editor the panel shows the selected track's rack.
        if self.mode == crate::app::Mode::Multitrack && self.session().is_some() {
            self.track_rack(ui);
            return;
        }
        let effects = self.effects.clone();
        let mut changed = false;
        let mut edit: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mut add: Option<(usize, usize)> = None; // (slot, effect)

        // Presets row.
        ui.horizontal(|ui| {
            ui.label(RichText::new("Presets:").color(TEXT_DIM));
            egui::ComboBox::from_id_source("rack_presets").width(ui.available_width() - 4.0).selected_text("(Default)").show_ui(ui, |ui| {
                if ui.selectable_label(false, "(Default)").clicked() {
                    self.rack.clear();
                    changed = true;
                }
                for (name, chain) in RACK_PRESETS {
                    if ui.selectable_label(false, *name).clicked() {
                        self.rack = chain
                            .iter()
                            .filter_map(|(id, preset)| {
                                let i = effects.iter().position(|e| e.id == *id)?;
                                let params = if preset.is_empty() { effects[i].default_params() } else { effects[i].preset_params(preset).unwrap_or_else(|| effects[i].default_params()) };
                                Some(RackSlot { idx: i, params, on: true })
                            })
                            .collect();
                        changed = true;
                    }
                }
            });
        });

        // Sixteen slots, always shown.
        let list_h = (ui.available_height() - 112.0).max(80.0);
        egui::Frame::none().fill(BG_LIST).inner_margin(egui::Margin::same(2.0)).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                let n = self.rack.len();
                for i in 0..RACK_SLOTS {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if let Some(slot) = self.rack.get_mut(i) {
                            if icon_button_sized(ui, Icon::Power, slot.on, if slot.on { "Turn effect off" } else { "Turn effect on" }, egui::vec2(18.0, 18.0), true).clicked() {
                                slot.on = !slot.on;
                                changed = true;
                            }
                            ui.label(RichText::new(format!("{:>2}", i + 1)).monospace().color(TEXT_DIM));
                            let def = &effects[slot.idx];
                            let name = RichText::new(def.name).color(if slot.on { TEXT } else { TEXT_DIM });
                            if ui.selectable_label(false, name).on_hover_text("Click to edit settings").clicked() && !def.params.is_empty() {
                                edit = Some(i);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.menu_button(RichText::new("▸").color(TEXT_DIM), |ui| {
                                    if !def.params.is_empty() && ui.button("Edit Effect…").clicked() {
                                        edit = Some(i);
                                        ui.close_menu();
                                    }
                                    if i > 0 && ui.button("Move Up").clicked() {
                                        swap = Some((i, i - 1));
                                        ui.close_menu();
                                    }
                                    if i + 1 < n && ui.button("Move Down").clicked() {
                                        swap = Some((i, i + 1));
                                        ui.close_menu();
                                    }
                                    if ui.button("Remove Effect").clicked() {
                                        remove = Some(i);
                                        ui.close_menu();
                                    }
                                });
                            });
                        } else {
                            icon_button_sized(ui, Icon::Power, false, "Empty slot", egui::vec2(18.0, 18.0), false);
                            ui.label(RichText::new(format!("{:>2}", i + 1)).monospace().color(Color32::from_rgb(0x66, 0x66, 0x66)));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if i == n {
                                    ui.menu_button(RichText::new("▸").color(TEXT), |ui| {
                                        for cat in std::iter::once(Category::Basic).chain(Category::MENU_ORDER) {
                                            if cat == Category::Generate {
                                                continue;
                                            }
                                            ui.menu_button(cat.name(), |ui| {
                                                for (ei, e) in effects.iter().enumerate().filter(|(_, e)| e.category == cat && !e.changes_length) {
                                                    if ui.button(e.name).clicked() {
                                                        add = Some((i, ei));
                                                        ui.close_menu();
                                                    }
                                                }
                                            });
                                        }
                                    })
                                    .response
                                    .on_hover_text("Add an effect");
                                }
                            });
                        }
                    });
                }
            });
        });
        if let Some((_, ei)) = add {
            self.rack.push(RackSlot { idx: ei, params: effects[ei].default_params(), on: true });
            changed = true;
            if !effects[ei].params.is_empty() {
                edit = Some(self.rack.len() - 1);
            }
        }
        if let Some(i) = remove {
            self.rack.remove(i);
            changed = true;
        }
        if let Some((a, b)) = swap {
            self.rack.swap(a, b);
            changed = true;
        }

        // Gain, mix, process and apply.
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Input").color(TEXT_DIM));
            changed |= hot_drag(ui, egui::DragValue::new(&mut self.rack_in_db).speed(0.1).range(-24.0..=24.0).fixed_decimals(1).suffix(" dB")).changed();
            ui.add_space(8.0);
            ui.label(RichText::new("Output").color(TEXT_DIM));
            changed |= hot_drag(ui, egui::DragValue::new(&mut self.rack_out_db).speed(0.1).range(-24.0..=24.0).fixed_decimals(1).suffix(" dB")).changed();
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Mix:").color(TEXT_DIM));
            ui.label(RichText::new("Dry").color(TEXT_DIM).size(11.0));
            ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(60.0);
            changed |= ui.add(egui::Slider::new(&mut self.rack_mix, 0.0..=100.0).show_value(false)).changed();
            ui.label(RichText::new("Wet").color(TEXT_DIM).size(11.0));
            changed |= hot_drag(ui, egui::DragValue::new(&mut self.rack_mix).speed(0.5).range(0.0..=100.0).fixed_decimals(0).suffix(" %")).changed();
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Process:").color(TEXT_DIM));
            let before = self.rack_entire;
            egui::ComboBox::from_id_source("rack_process").width(110.0).selected_text(if self.rack_entire { "Entire File" } else { "Selection Only" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.rack_entire, false, "Selection Only");
                ui.selectable_value(&mut self.rack_entire, true, "Entire File");
            });
            changed |= before != self.rack_entire;
            let has_doc = self.doc().is_some();
            let any_on = self.rack.iter().any(|s| s.on);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(has_doc && any_on, egui::Button::new("Apply")).on_hover_text("Process the audio with every enabled effect").clicked() {
                    self.actions.push(Action::ApplyRack);
                }
                ui.add_space(4.0);
                let tip = if self.rack_on { "Master power: on — playback is heard through the rack" } else { "Master power: off — turn on to hear the rack while playing" };
                if icon_button_sized(ui, Icon::Power, self.rack_on, tip, egui::vec2(22.0, 22.0), has_doc).clicked() {
                    self.rack_on = !self.rack_on;
                }
            });
        });
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
                track_fx: None,
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
        let format = match doc.export {
            Some(e) if doc.path.is_some() => e.summary(),
            _ => doc.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_else(|| "Unsaved".into()),
        };
        let path = doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Not saved yet".into());
        let basic: Vec<(&str, String)> = vec![
            ("Duration", format_time(doc.len() as f64, doc.sample_rate)),
            ("Sample Rate", format!("{} Hz", doc.sample_rate)),
            ("Channels", if doc.n_ch() == 1 { "Mono".into() } else { "Stereo".into() }),
            ("Bit Depth", doc.source_bits.map(|b| format!("{b}")).unwrap_or_else(|| "32 (float)".into())),
            ("Format", format),
            ("File Path", path),
        ];
        let advanced: Vec<(&str, String)> = vec![
            ("Samples", format!("{}", doc.len())),
            ("Peak Amplitude", format!("{:.2} dBFS", lin_to_db(pk).max(-200.0))),
            ("RMS (loudest channel)", format!("{:.2} dBFS", lin_to_db(rms).max(-200.0))),
            ("Markers", format!("{}", doc.markers.len())),
            ("Unsaved Changes", if doc.dirty { "Yes".into() } else { "No".into() }),
        ];
        let name = doc.display_name();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new(name).font(bold(13.0)).color(Color32::WHITE));
            ui.label(RichText::new("Audio File").color(TEXT_DIM).size(11.5));
            ui.add_space(4.0);
            let grid = |ui: &mut Ui, id: &str, rows: Vec<(&str, String)>| {
                egui::Grid::new(id).num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
                    for (k, v) in rows {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(format!("{k}:")).color(TEXT_DIM).size(11.5));
                        });
                        ui.label(RichText::new(v).size(11.5));
                        ui.end_row();
                    }
                });
            };
            grid(ui, "props_basic", basic);
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new("Advanced").size(12.0)).default_open(false).show(ui, |ui| {
                grid(ui, "props_adv", advanced);
            });
        });
    }
}
