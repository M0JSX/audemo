//! Menu bar, toolbar and the docked panel layout of the default workspace:
//! Files/Favorites, Media Browser/Effects Rack/Markers/Properties and History
//! down the left, the Editor in the centre with its transport, Levels and
//! Selection/View along the bottom, then the status bar.

use std::time::Duration;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, Ui};

use crate::app::{Action, App, Dialog, Panel, Tool, FAVORITES};
use crate::dsp::effects::Category;
use crate::dsp::util::{format_time, parse_time};
use crate::engine::PREVIEW_TAG;
use crate::theme::*;

fn sc(s: &str) -> String {
    if cfg!(target_os = "macos") {
        s.replace("Ctrl+Alt+", "⌥⌘").replace("Alt+Shift+", "⌥⇧").replace("Ctrl+Shift+", "⇧⌘").replace("Ctrl+", "⌘").replace("Shift+", "⇧").replace("Alt+", "⌥")
    } else {
        s.to_string()
    }
}

fn item(ui: &mut Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    let b = egui::Button::new(label).shortcut_text(sc(shortcut));
    let clicked = ui.add_enabled(enabled, b).clicked();
    if clicked {
        ui.close_menu();
    }
    clicked
}

/// An editable time field; returns a new value in samples when committed.
fn time_field(ui: &mut Ui, id: egui::Id, samples: f64, sr: u32, enabled: bool) -> Option<f64> {
    let formatted = format_time(samples, sr);
    let mut text: String = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| formatted.clone());
    let has_focus = ui.memory(|m| m.has_focus(id));
    if !has_focus {
        text = formatted;
    }
    let resp = ui.add_enabled(
        enabled,
        egui::TextEdit::singleline(&mut text).id(id).desired_width(78.0).font(egui::TextStyle::Monospace),
    );
    ui.data_mut(|d| d.insert_temp(id, text.clone()));
    if resp.lost_focus() {
        return parse_time(&text, sr);
    }
    None
}

impl App {
    pub fn menu_bar(&mut self, ctx: &egui::Context) {
        let has_doc = self.doc().is_some();
        let has_sel = self.doc().and_then(|d| d.sel_range()).is_some();
        let can_undo = self.doc().map(|d| !d.undo.is_empty()).unwrap_or(false);
        let can_redo = self.doc().map(|d| !d.redo.is_empty()).unwrap_or(false);
        let has_clip = self.clipboard.is_some();
        let effects = self.effects.clone();
        let recent = self.prefs.recent.clone();
        let last = self.last_effect.as_ref().map(|(i, _)| effects[*i].name);
        egui::TopBottomPanel::top("menu")
            .frame(egui::Frame::none().fill(BG_DEEP).inner_margin(egui::Margin::symmetric(6.0, 3.0)))
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        ui.menu_button("New", |ui| {
                            if item(ui, "Audio File…", "Ctrl+N", true) { self.actions.push(Action::New); }
                        });
                        if item(ui, "Open…", "Ctrl+O", true) { self.actions.push(Action::Open); }
                        ui.menu_button("Open Append", |ui| {
                            if item(ui, "To Current…", "", has_doc) { self.actions.push(Action::OpenAppend); }
                        });
                        ui.menu_button("Open Recent", |ui| {
                            if recent.is_empty() {
                                ui.label(RichText::new("No recent files").color(TEXT_DIM));
                            }
                            for r in &recent {
                                let name = r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                if ui.button(name).on_hover_text(r.display().to_string()).clicked() {
                                    ui.close_menu();
                                    self.actions.push(Action::OpenRecent(r.clone()));
                                }
                            }
                            if !recent.is_empty() {
                                ui.separator();
                                if item(ui, "Clear Recent", "", true) {
                                    self.prefs.recent.clear();
                                    self.prefs.save();
                                }
                            }
                        });
                        ui.separator();
                        if item(ui, "Close", "Ctrl+W", has_doc) { self.actions.push(Action::Close(usize::MAX)); }
                        if item(ui, "Close All", "", has_doc) { self.actions.push(Action::CloseAll); }
                        ui.separator();
                        if item(ui, "Save", "Ctrl+S", has_doc) { self.actions.push(Action::Save); }
                        if item(ui, "Save As…", "Ctrl+Shift+S", has_doc) { self.actions.push(Action::SaveAs); }
                        if item(ui, "Save Selection As…", "", has_sel) { self.actions.push(Action::SaveSelectionAs); }
                        if item(ui, "Save All", "", has_doc) { self.actions.push(Action::SaveAll); }
                        ui.separator();
                        if item(ui, "Exit", "Ctrl+Q", true) { self.actions.push(Action::Quit); }
                    });
                    ui.menu_button("Edit", |ui| {
                        if item(ui, "Undo", "Ctrl+Z", can_undo) { self.actions.push(Action::Undo); }
                        if item(ui, "Redo", "Ctrl+Shift+Z", can_redo) { self.actions.push(Action::Redo); }
                        let repeat = match last {
                            Some(n) => format!("Repeat Previous Command ({n})"),
                            None => "Repeat Previous Command".to_string(),
                        };
                        if item(ui, &repeat, "Shift+R", last.is_some() && has_doc) { self.actions.push(Action::RepeatLast); }
                        ui.separator();
                        if item(ui, "Cut", "Ctrl+X", has_sel) { self.actions.push(Action::Cut); }
                        if item(ui, "Copy", "Ctrl+C", has_sel) { self.actions.push(Action::Copy); }
                        if item(ui, "Copy to New", "Alt+Shift+C", has_doc) { self.actions.push(Action::CopyToNew); }
                        if item(ui, "Paste", "Ctrl+V", has_clip) { self.actions.push(Action::Paste); }
                        if item(ui, "Paste to New", "Ctrl+Alt+V", has_clip) { self.actions.push(Action::PasteNew); }
                        if item(ui, "Mix Paste…", "Ctrl+Shift+V", has_clip && has_doc) {
                            self.dialog = Some(Dialog::MixPaste { mode: 1, clip_db: 0.0, orig_db: 0.0 });
                        }
                        ui.separator();
                        if item(ui, "Delete", "Del", has_sel) { self.actions.push(Action::Delete); }
                        if item(ui, "Crop", "Ctrl+T", has_sel) { self.actions.push(Action::Crop); }
                        ui.separator();
                        ui.menu_button("Select", |ui| {
                            if item(ui, "Select All", "Ctrl+A", has_doc) { self.actions.push(Action::SelectAll); }
                            if item(ui, "Deselect All", "Esc", has_sel) { self.actions.push(Action::Deselect); }
                        });
                        ui.menu_button("Insert", |ui| {
                            for (label, id) in [("Silence…", "gen_silence"), ("Tones…", "gen_tone"), ("Noise…", "gen_noise")] {
                                if item(ui, label, "", has_doc) {
                                    if let Some(i) = self.find_effect(id) { self.actions.push(Action::OpenEffect(i)); }
                                }
                            }
                        });
                        ui.menu_button("Marker", |ui| {
                            if item(ui, "Add Marker", "M", has_doc) { self.actions.push(Action::AddMarker); }
                        });
                        if item(ui, "Convert Sample Type…", "", has_doc) {
                            if let Some((rate, channels)) = self.doc().map(|d| (d.sample_rate, d.n_ch())) {
                                self.dialog = Some(Dialog::Convert { rate, channels });
                            }
                        }
                        ui.separator();
                        ui.menu_button("Preferences", |ui| {
                            if item(ui, "Audio Hardware…", "", true) { self.actions.push(Action::Preferences); }
                        });
                    });
                    ui.menu_button("Effects", |ui| {
                        if item(ui, "Show Effects Rack", "", true) { self.show_panel(Panel::EffectsRack); }
                        ui.separator();
                        for (i, e) in effects.iter().enumerate().filter(|(_, e)| e.category == Category::Basic) {
                            if item(ui, e.name, "", has_doc) { self.actions.push(Action::OpenEffect(i)); }
                        }
                        ui.separator();
                        for cat in Category::MENU_ORDER {
                            if cat == Category::Generate {
                                ui.separator();
                            }
                            ui.menu_button(cat.name(), |ui| {
                                if cat == Category::Restoration {
                                    if item(ui, "Capture Noise Print", "Shift+P", has_sel) { self.actions.push(Action::CaptureNoise); }
                                    ui.separator();
                                }
                                for (i, e) in effects.iter().enumerate().filter(|(_, e)| e.category == cat) {
                                    let label = if e.params.is_empty() { e.name.to_string() } else { format!("{}…", e.name) };
                                    if item(ui, &label, "", has_doc) { self.actions.push(Action::OpenEffect(i)); }
                                }
                            });
                        }
                    });
                    ui.menu_button("Favorites", |ui| {
                        for (label, id, preset) in FAVORITES {
                            if item(ui, label, "", has_doc) { self.actions.push(Action::ApplyFavorite(*id, *preset)); }
                        }
                    });
                    ui.menu_button("View", |ui| {
                        if item(ui, "Zoom In (Time)", "=", has_doc) { self.actions.push(Action::ZoomIn); }
                        if item(ui, "Zoom Out (Time)", "-", has_doc) { self.actions.push(Action::ZoomOut); }
                        if item(ui, "Zoom Out Full (All Axes)", "\\", has_doc) { self.actions.push(Action::ZoomFull); }
                        if item(ui, "Zoom to Selection", "", has_sel) { self.actions.push(Action::ZoomSel); }
                        if item(ui, "Zoom In (Amplitude)", "", has_doc) { self.actions.push(Action::AmpIn); }
                        if item(ui, "Zoom Out (Amplitude)", "", has_doc) { self.actions.push(Action::AmpOut); }
                        ui.separator();
                        ui.checkbox(&mut self.show_spectral, format!("Show Spectral Frequency Display   {}", sc("Shift+D")));
                        ui.checkbox(&mut self.follow, "Follow Playhead");
                    });
                    ui.menu_button("Window", |ui| {
                        ui.menu_button("Workspace", |ui| {
                            if item(ui, "Reset to Default", "", true) { self.reset_workspace(); }
                        });
                        ui.separator();
                        for panel in [Panel::EffectsRack, Panel::Favorites, Panel::Files, Panel::History] {
                            if item(ui, panel.name(), "", true) { self.show_panel(panel); }
                        }
                        ui.checkbox(&mut self.show_meters, "Levels");
                        for panel in [Panel::Markers, Panel::MediaBrowser, Panel::Properties] {
                            if item(ui, panel.name(), "", true) { self.show_panel(panel); }
                        }
                        ui.checkbox(&mut self.show_bottom, "Selection/View");
                        ui.separator();
                        ui.checkbox(&mut self.show_left, "Show Left Panels");
                    });
                    ui.menu_button("Help", |ui| {
                        if item(ui, "Keyboard Shortcuts", "", true) { self.dialog = Some(Dialog::Shortcuts); }
                        if item(ui, "About Audemo", "", true) { self.dialog = Some(Dialog::About); }
                    });
                });
            });
    }

    pub fn show_panel(&mut self, panel: Panel) {
        self.show_left = true;
        if Panel::TOP.contains(&panel) {
            self.tab_top = panel;
        } else if Panel::MIDDLE.contains(&panel) {
            self.tab_mid = panel;
        }
    }

    pub fn reset_workspace(&mut self) {
        self.show_left = true;
        self.show_bottom = true;
        self.show_meters = true;
        self.tab_top = Panel::Files;
        self.tab_mid = Panel::EffectsRack;
    }

    pub fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::none().fill(BG_PANEL).inner_margin(egui::Margin::symmetric(8.0, 4.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(84.0, 24.0), Sense::hover());
                    ui.painter().rect_filled(r, 3.0, Color32::from_rgb(0x25, 0x5a, 0x9a));
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, "Waveform", FontId::proportional(12.5), Color32::WHITE);
                    ui.add_space(10.0);
                    ui.separator();
                    if icon_button(ui, Icon::Selection, self.tool == Tool::Selection, "Time Selection tool").clicked() {
                        self.tool = Tool::Selection;
                    }
                    if icon_button(ui, Icon::Hand, self.tool == Tool::Hand, "Hand tool (drag to scroll; middle-drag works with any tool)").clicked() {
                        self.tool = Tool::Hand;
                    }
                    ui.separator();
                    if icon_button(ui, Icon::Spectral, self.show_spectral, "Show Spectral Frequency Display (Shift+D)").clicked() {
                        self.show_spectral = !self.show_spectral;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match &self.noise_print {
                            Some(np) => {
                                let secs = np.samples.first().map(|c| c.len()).unwrap_or(0) as f64 / np.sample_rate as f64;
                                if ui.small_button("Clear").clicked() {
                                    self.noise_print = None;
                                }
                                ui.label(RichText::new(format!("Noise print: {secs:.2} s")).color(WAVE_SEL).size(11.5));
                            }
                            None => {
                                ui.label(RichText::new("No noise print").color(TEXT_DIM).size(11.5));
                            }
                        }
                    });
                });
            });
    }

    pub fn left_column(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("left")
            .resizable(true)
            .default_width(270.0)
            .width_range(200.0..=460.0)
            .frame(egui::Frame::none().fill(BG_DEEP).inner_margin(egui::Margin::same(2.0)))
            .show(ctx, |ui| {
                egui::TopBottomPanel::top("left_top")
                    .resizable(true)
                    .default_height(200.0)
                    .height_range(90.0..=520.0)
                    .frame(panel_frame())
                    .show_inside(ui, |ui| {
                        let mut tab = self.tab_top;
                        tab_strip(ui, &Panel::TOP, &mut tab);
                        self.tab_top = tab;
                        match tab {
                            Panel::Favorites => self.favorites_panel(ui),
                            _ => self.files_tab(ui),
                        }
                    });
                egui::TopBottomPanel::bottom("left_bottom")
                    .resizable(true)
                    .default_height(170.0)
                    .height_range(80.0..=480.0)
                    .frame(panel_frame())
                    .show_inside(ui, |ui| {
                        let mut tab = Panel::History;
                        tab_strip(ui, &Panel::BOTTOM, &mut tab);
                        self.history(ui);
                    });
                egui::CentralPanel::default().frame(panel_frame()).show_inside(ui, |ui| {
                    let mut tab = self.tab_mid;
                    tab_strip(ui, &Panel::MIDDLE, &mut tab);
                    self.tab_mid = tab;
                    match tab {
                        Panel::MediaBrowser => self.media_browser(ui),
                        Panel::Markers => self.markers_tab(ui),
                        Panel::Properties => self.properties_panel(ui),
                        _ => self.effects_rack(ui),
                    }
                });
            });
    }

    fn files_tab(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open…").clicked() {
                self.actions.push(Action::Open);
            }
            if ui.button("New…").clicked() {
                self.actions.push(Action::New);
            }
        });
        ui.add_space(4.0);
        if self.docs.is_empty() {
            ui.label(RichText::new("No files open. Drop audio here or use Open.").color(TEXT_DIM));
        }
        let mut switch = None;
        let mut close = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, d) in self.docs.iter().enumerate() {
                let active = Some(i) == self.active;
                ui.horizontal(|ui| {
                    let name = RichText::new(d.display_name()).color(if active { Color32::WHITE } else { TEXT });
                    if ui.selectable_label(active, name).clicked() {
                        switch = Some(i);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Close").clicked() {
                            close = Some(i);
                        }
                    });
                });
                let ch = if d.n_ch() == 1 { "Mono" } else { "Stereo" };
                ui.label(
                    RichText::new(format!("   {:.1} kHz · {} · {}", d.sample_rate as f32 / 1000.0, ch, format_time(d.len() as f64, d.sample_rate)))
                        .color(TEXT_DIM)
                        .size(11.0),
                );
                ui.add_space(4.0);
            }
        });
        if let Some(i) = switch {
            self.active = Some(i);
        }
        if let Some(i) = close {
            self.actions.push(Action::Close(i));
        }
    }

    fn markers_tab(&mut self, ui: &mut Ui) {
        if ui.button("Add Marker  (M)").clicked() {
            self.actions.push(Action::AddMarker);
        }
        ui.add_space(4.0);
        let Some(di) = self.active else {
            ui.label(RichText::new("Open a file to add markers.").color(TEXT_DIM));
            return;
        };
        let doc = &mut self.docs[di];
        if doc.markers.is_empty() {
            ui.label(RichText::new("No markers. Press M during playback or at the cursor.").color(TEXT_DIM));
            return;
        }
        let mut remove = None;
        let sr = doc.sample_rate;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("markers").num_columns(3).striped(true).spacing([6.0, 4.0]).show(ui, |ui| {
                for h in ["Name", "Start", ""] {
                    ui.label(RichText::new(h).color(TEXT_DIM).size(11.0));
                }
                ui.end_row();
                for (i, m) in doc.markers.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut m.name).desired_width(96.0));
                    if ui.link(RichText::new(format_time(m.pos as f64, sr)).monospace()).clicked() {
                        self.actions.push(Action::SetCursor(m.pos));
                    }
                    if ui.small_button("×").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            doc.markers.remove(i);
            doc.dirty = true;
        }
    }

    /// Bottom strip: Levels meter and the Selection/View panel.
    pub fn bottom_row(&mut self, ctx: &egui::Context) {
        if !self.show_meters && !self.show_bottom {
            return;
        }
        egui::TopBottomPanel::bottom("bottom_row")
            .resizable(true)
            .default_height(112.0)
            .height_range(64.0..=260.0)
            .frame(egui::Frame::none().fill(BG_DEEP).inner_margin(egui::Margin::same(2.0)))
            .show(ctx, |ui| {
                if self.show_bottom {
                    egui::SidePanel::right("selview")
                        .resizable(true)
                        .default_width(330.0)
                        .width_range(280.0..=520.0)
                        .frame(panel_frame())
                        .show_inside(ui, |ui| {
                            let mut tab = 0usize;
                            tab_strip_named(ui, &["Selection/View"], &mut tab);
                            self.selection_view(ui);
                        });
                }
                if self.show_meters {
                    egui::CentralPanel::default().frame(panel_frame()).show_inside(ui, |ui| {
                        let mut tab = 0usize;
                        tab_strip_named(ui, &[if self.engine.is_recording() { "Levels (Input)" } else { "Levels" }], &mut tab);
                        self.levels(ui);
                    });
                }
            });
    }

    fn selection_view(&mut self, ui: &mut Ui) {
        let Some(di) = self.active else {
            ui.label(RichText::new("No file").color(TEXT_DIM));
            return;
        };
        let doc = &mut self.docs[di];
        let sr = doc.sample_rate;
        let (sa, sb) = doc.sel_range().unwrap_or((doc.cursor, doc.cursor));
        let mut new_sel: Option<(f64, f64)> = None;
        let mut new_view: Option<(f64, f64)> = None;
        egui::Grid::new("selview").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("");
            for h in ["Start", "End", "Duration"] {
                ui.label(RichText::new(h).color(TEXT_DIM).size(11.0));
            }
            ui.end_row();
            ui.label("Selection");
            let base = ui.id().with(("sv", doc.id));
            if let Some(v) = time_field(ui, base.with(0), sa as f64, sr, true) {
                new_sel = Some((v, (sb as f64).max(v)));
            }
            if let Some(v) = time_field(ui, base.with(1), sb as f64, sr, true) {
                new_sel = Some(((sa as f64).min(v), v));
            }
            if let Some(v) = time_field(ui, base.with(2), (sb - sa) as f64, sr, true) {
                new_sel = Some((sa as f64, sa as f64 + v));
            }
            ui.end_row();
            ui.label("View");
            if let Some(v) = time_field(ui, base.with(3), doc.view_start, sr, true) {
                new_view = Some((v, v + (doc.view_end - doc.view_start)));
            }
            if let Some(v) = time_field(ui, base.with(4), doc.view_end, sr, true) {
                new_view = Some((doc.view_start, v));
            }
            if let Some(v) = time_field(ui, base.with(5), doc.view_end - doc.view_start, sr, true) {
                new_view = Some((doc.view_start, doc.view_start + v));
            }
            ui.end_row();
        });
        if let Some((a, b)) = new_sel {
            let len = doc.len() as f64;
            let (a, b) = (a.clamp(0.0, len) as usize, b.clamp(0.0, len) as usize);
            doc.sel = if b > a { Some((a, b)) } else { None };
            doc.cursor = a;
        }
        if let Some((a, b)) = new_view {
            if b > a {
                doc.view_start = a;
                doc.view_end = b;
                doc.clamp_view();
            }
        }
        ui.add_space(6.0);
        egui::Grid::new("info").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
            let mb = doc.len() as f64 * doc.n_ch() as f64 * 4.0 / 1_048_576.0;
            for (k, v) in [
                ("Format", doc.format_label()),
                ("Duration", format_time(doc.len() as f64, sr)),
                ("Samples", format!("{}", doc.len())),
                ("Memory", format!("{mb:.2} MB")),
            ] {
                ui.label(RichText::new(k).color(TEXT_DIM).size(11.0));
                ui.label(RichText::new(v).size(11.0));
                ui.end_row();
            }
        });
    }

    fn history(&mut self, ui: &mut Ui) {
        let Some(doc) = self.doc() else {
            ui.label(RichText::new("Nothing yet").color(TEXT_DIM));
            return;
        };
        let current = doc.undo.len();
        let mut entries: Vec<(usize, String, bool)> = vec![(0, doc.origin.clone(), false)];
        for (i, s) in doc.undo.iter().enumerate() {
            entries.push((i + 1, s.label.clone(), false));
        }
        for (k, s) in doc.redo.iter().rev().enumerate() {
            entries.push((current + 1 + k, s.label.clone(), true));
        }
        let mut jump = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for (idx, label, future) in entries {
                let text = if future { RichText::new(&label).color(TEXT_DIM).italics() } else { RichText::new(&label) };
                if ui.selectable_label(idx == current, text).clicked() && idx != current {
                    jump = Some(idx);
                }
            }
        });
        if let Some(j) = jump {
            self.actions.push(Action::JumpHistory(j));
        }
    }

    /// Transport and zoom controls along the bottom of the Editor panel.
    pub fn transport_bar(&mut self, ui: &mut Ui) {
        egui::TopBottomPanel::bottom("transport")
            .frame(egui::Frame::none().fill(BG_PANEL).inner_margin(egui::Margin::symmetric(8.0, 5.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let sr = self.doc().map(|d| d.sample_rate).unwrap_or(48000);
                    let recording = self.engine.is_recording();
                    let pos = match (&self.rec_target, self.doc()) {
                        (Some(t), Some(d)) if recording && t.doc_id == d.id => (t.range.0 + d.pending) as f64,
                        _ => self.display_pos(),
                    };
                    let (r, _) = ui.allocate_exact_size(vec2(150.0, 30.0), Sense::hover());
                    ui.painter().rect_filled(r, 3.0, BG_DEEP);
                    let col = if recording { RECORD } else { TIME };
                    ui.painter().text(r.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, format_time(pos, sr), FontId::monospace(22.0), col);
                    ui.add_space(8.0);

                    let st = self.engine.status();
                    let playing = st.playing && self.doc().map(|d| d.id == st.tag).unwrap_or(false);
                    if icon_button(ui, Icon::Stop, false, "Stop (also ends recording)").clicked() {
                        self.actions.push(Action::Stop);
                    }
                    if icon_button(ui, if playing { Icon::Pause } else { Icon::Play }, playing, "Play / Stop (Space)").clicked() {
                        self.actions.push(Action::PlayToggle);
                    }
                    if icon_button(ui, Icon::ToStart, false, "Move playhead to start (Home)").clicked() {
                        self.actions.push(Action::ToStart);
                    }
                    if icon_button(ui, Icon::PrevMarker, false, "Previous marker / edge (←)").clicked() {
                        self.actions.push(Action::PrevMarker);
                    }
                    if icon_button(ui, Icon::NextMarker, false, "Next marker / edge (→)").clicked() {
                        self.actions.push(Action::NextMarker);
                    }
                    if icon_button(ui, Icon::ToEnd, false, "Move playhead to end (End)").clicked() {
                        self.actions.push(Action::ToEnd);
                    }
                    if icon_button(ui, Icon::Record, recording, "Record (Shift+Space)").clicked() {
                        self.actions.push(Action::Record);
                    }
                    if icon_button(ui, Icon::Loop, self.looping, "Loop playback").clicked() {
                        self.looping = !self.looping;
                        self.engine.set_looping(self.looping);
                    }
                    ui.separator();
                    for (icon, action, tip) in [
                        (Icon::AmpIn, Action::AmpIn, "Zoom in amplitude (Alt+wheel)"),
                        (Icon::AmpOut, Action::AmpOut, "Zoom out amplitude"),
                        (Icon::ZoomIn, Action::ZoomIn, "Zoom in time (=, wheel)"),
                        (Icon::ZoomOut, Action::ZoomOut, "Zoom out time (-)"),
                        (Icon::ZoomFull, Action::ZoomFull, "Zoom out full (\\)"),
                        (Icon::ZoomSel, Action::ZoomSel, "Zoom to selection"),
                    ] {
                        if icon_button(ui, icon, false, tip).clicked() {
                            self.actions.push(action);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut vol = ui.data_mut(|d| d.get_temp::<f32>(egui::Id::new("monitor_vol"))).unwrap_or(0.0);
                        let r = ui.add(egui::Slider::new(&mut vol, -40.0..=6.0).suffix(" dB").fixed_decimals(1).show_value(true));
                        if r.changed() {
                            self.engine.set_volume(crate::dsp::util::db_to_lin(vol));
                            ui.data_mut(|d| d.insert_temp(egui::Id::new("monitor_vol"), vol));
                        }
                        ui.label(RichText::new("Monitor").color(TEXT_DIM));
                    });
                });
            });
    }

    /// Horizontal stereo peak meter with hold and clip indicator.
    fn levels(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        if rect.height() < 20.0 {
            return;
        }
        let p = ui.painter_at(rect);
        let label_w = 18.0;
        let bar_h = ((rect.height() - 22.0) / 2.0).clamp(6.0, 14.0);
        let bars = Rect::from_min_max(pos2(rect.left() + label_w, rect.top() + 4.0), pos2(rect.right() - 14.0, rect.top() + 4.0 + bar_h * 2.0 + 2.0));
        let floor = -60.0f32;
        let x_of = |db: f32| bars.left() + ((db - floor) / -floor).clamp(0.0, 1.0) * bars.width();
        for c in 0..2 {
            let y0 = bars.top() + c as f32 * (bar_h + 2.0);
            let r = Rect::from_min_size(pos2(bars.left(), y0), vec2(bars.width(), bar_h));
            p.text(pos2(rect.left() + 2.0, r.center().y), Align2::LEFT_CENTER, if c == 0 { "L" } else { "R" }, FontId::monospace(9.5), TEXT_DIM);
            p.rect_filled(r, 1.0, Color32::from_rgb(0x0c, 0x0e, 0x12));
            let db = self.meter_db[c];
            for (lo, hi, col) in [
                (floor, -12.0, Color32::from_rgb(0x2f, 0xd4, 0x8c)),
                (-12.0, -3.0, Color32::from_rgb(0xe8, 0xd2, 0x3a)),
                (-3.0, 0.0, Color32::from_rgb(0xec, 0x48, 0x3c)),
            ] {
                if db > lo {
                    let x1 = x_of(db.min(hi));
                    p.rect_filled(Rect::from_min_max(pos2(x_of(lo), r.top()), pos2(x1, r.bottom())), 0.0, col);
                }
            }
            let hold = self.meter_hold[c].0;
            if hold > floor {
                let hx = x_of(hold);
                p.line_segment([pos2(hx, r.top()), pos2(hx, r.bottom())], Stroke::new(2.0_f32, if hold > -0.1 { RECORD } else { Color32::WHITE }));
            }
            let clip = self.meter_hold[c].0 > -0.1;
            let cr = Rect::from_min_size(pos2(rect.right() - 10.0, r.top()), vec2(8.0, bar_h));
            p.rect_filled(cr, 1.0, if clip { RECORD } else { Color32::from_rgb(0x30, 0x18, 0x18) });
        }
        let step = if bars.width() > 700.0 { 3.0 } else { 6.0 };
        let mut db = floor;
        while db <= 0.0 {
            let x = x_of(db);
            p.line_segment([pos2(x, bars.bottom() + 1.0), pos2(x, bars.bottom() + 4.0)], Stroke::new(1.0_f32, TEXT_DIM));
            p.text(pos2(x, bars.bottom() + 5.0), Align2::CENTER_TOP, format!("{}", db as i32), FontId::monospace(9.0), TEXT_DIM);
            db += step;
        }
    }

    pub fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(24.0)
            .frame(egui::Frame::none().fill(Color32::from_rgb(0x1a, 0x3d, 0x66)).inner_margin(egui::Margin::symmetric(10.0, 3.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let st = self.engine.status();
                    let state = if self.engine.is_recording() {
                        "Recording"
                    } else if st.playing && st.tag == PREVIEW_TAG {
                        "Previewing"
                    } else if st.playing {
                        "Playing"
                    } else {
                        "Stopped"
                    };
                    ui.label(RichText::new(state).color(Color32::WHITE).size(11.5).strong());
                    ui.add_space(12.0);
                    let msg = if let Some(j) = &self.job {
                        format!("{}… {:.1} s", j.label, j.started.elapsed().as_secs_f32())
                    } else if self.status_at.elapsed() < Duration::from_secs(12) {
                        self.status.clone()
                    } else if self.engine.is_playing_tag(PREVIEW_TAG) {
                        "Previewing effect".into()
                    } else {
                        "Ready".into()
                    };
                    ui.label(RichText::new(msg).color(Color32::from_rgb(0xdd, 0xe6, 0xf2)).size(11.5));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(d) = self.doc() {
                            let mb = d.len() as f64 * d.n_ch() as f64 * 4.0 / 1_048_576.0;
                            let col = Color32::from_rgb(0xdd, 0xe6, 0xf2);
                            ui.label(RichText::new(format_time(d.len() as f64, d.sample_rate)).monospace().color(col).size(11.5));
                            ui.add_space(14.0);
                            ui.label(RichText::new(format!("{mb:.2} MB")).color(col).size(11.5));
                            ui.add_space(14.0);
                            ui.label(RichText::new(d.format_label()).color(col).size(11.5));
                        }
                    });
                });
            });
    }

    pub fn job_overlay(&mut self, ctx: &egui::Context) {
        let Some(j) = &self.job else { return };
        if !self.busy() {
            return;
        }
        let label = j.label.clone();
        let t = j.started.elapsed().as_secs_f32();
        if t < 0.25 {
            return;
        }
        egui::Area::new(egui::Id::new("busy"))
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(18.0));
                        ui.label(RichText::new(format!("Applying {label}…  {t:.1} s")).size(14.0));
                    });
                });
            });
    }
}

fn panel_frame() -> egui::Frame {
    egui::Frame::none().fill(BG_PANEL).inner_margin(egui::Margin::same(6.0)).stroke(Stroke::new(1.0_f32, BORDER))
}

/// Audition-style tab header for a panel group.
pub fn tab_strip(ui: &mut Ui, panels: &[Panel], current: &mut Panel) {
    let names: Vec<&str> = panels.iter().map(|p| p.name()).collect();
    let mut idx = panels.iter().position(|p| p == current).unwrap_or(0);
    tab_strip_named(ui, &names, &mut idx);
    *current = panels[idx];
}

pub fn tab_strip_named(ui: &mut Ui, names: &[&str], current: &mut usize) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, n) in names.iter().enumerate() {
            let active = i == *current;
            let text = RichText::new(*n).size(12.0).color(if active { Color32::WHITE } else { TEXT_DIM });
            let text = if active { text.strong() } else { text };
            if ui.selectable_label(active, text).clicked() {
                *current = i;
            }
        }
    });
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment([pos2(r.left() - 6.0, r.top()), pos2(r.right() + 6.0, r.top())], Stroke::new(1.0_f32, BORDER));
    ui.add_space(4.0);
}
