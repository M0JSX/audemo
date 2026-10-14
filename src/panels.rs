//! Menu bar, toolbar and the docked panel layout of the default workspace:
//! Files/Favorites, Media Browser/Effects Rack/Markers/Properties and History
//! down the left, the Editor in the centre with its transport, Levels and
//! Selection/View along the bottom, then the status bar.

use std::time::Duration;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, RichText, Rounding, Sense, Stroke, Ui};

use crate::app::{Action, App, Dialog, Mode, Panel, Tool, FAVORITES};
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
        egui::TextEdit::singleline(&mut text).id(id).desired_width(92.0).clip_text(false).font(egui::TextStyle::Monospace).text_color(HOT()).frame(has_focus),
    );
    ui.data_mut(|d| d.insert_temp(id, text.clone()));
    if resp.lost_focus() {
        return parse_time(&text, sr);
    }
    None
}

impl App {
    pub fn menu_bar(&mut self, ctx: &egui::Context) {
        // macOS gets the native menu bar at the top of the screen instead.
        #[cfg(any(target_os = "macos", audemo_check_menu))]
        if self.native_menu_active() {
            return;
        }
        let in_mt = self.mode == Mode::Multitrack && self.session().is_some();
        let has_doc = self.doc().is_some() || in_mt;
        let has_sel = if in_mt { self.session().map(|s| !s.selected_clips.is_empty()).unwrap_or(false) } else { self.doc().and_then(|d| d.sel_range()).is_some() };
        let can_undo = if in_mt { self.session().map(|s| !s.undo.is_empty()).unwrap_or(false) } else { self.doc().map(|d| !d.undo.is_empty()).unwrap_or(false) };
        let can_redo = if in_mt { self.session().map(|s| !s.redo.is_empty()).unwrap_or(false) } else { self.doc().map(|d| !d.redo.is_empty()).unwrap_or(false) };
        let has_clip = if in_mt { self.mt.clipboard.is_some() } else { self.clipboard.is_some() };
        let effects = self.effects.clone();
        let recent = self.prefs.recent.clone();
        let last = self.last_effect.as_ref().map(|(i, _)| effects[*i].name);
        let km = self.keymap.clone();
        let has_markers = if in_mt { self.session().map(|s| !s.markers.is_empty()).unwrap_or(false) } else { self.doc().map(|d| !d.markers.is_empty()).unwrap_or(false) };
        let in_wave = self.mode == Mode::Waveform && self.doc().is_some();
        let markers_io = in_wave || (in_mt && self.session().is_some());
        egui::TopBottomPanel::top("menu")
            .frame(egui::Frame::none().fill(BG_DEEP()).inner_margin(egui::Margin::symmetric(6.0, 3.0)))
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        ui.menu_button("New", |ui| {
                            if item(ui, "Multitrack Session…", &km.label("new_session"), true) { self.actions.push(Action::NewSession); }
                            if item(ui, "Audio File…", &km.label("new"), true) { self.actions.push(Action::New); }
                        });
                        if item(ui, "Open…", &km.label("open"), true) { self.actions.push(Action::Open); }
                        ui.menu_button("Open Append", |ui| {
                            if item(ui, "To Current…", "", has_doc) { self.actions.push(Action::OpenAppend); }
                        });
                        ui.menu_button("Open Recent", |ui| {
                            if recent.is_empty() {
                                ui.label(RichText::new("No recent files").color(TEXT_DIM()));
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
                        if item(ui, "Close", &km.label("close"), has_doc) { self.actions.push(Action::Close(usize::MAX)); }
                        if item(ui, "Close All", "", has_doc) { self.actions.push(Action::CloseAll); }
                        ui.separator();
                        if item(ui, "Save", &km.label("save"), has_doc) { self.actions.push(Action::Save); }
                        if item(ui, "Save As…", &km.label("save_as"), has_doc) { self.actions.push(Action::SaveAs); }
                        if item(ui, "Save Selection As…", "", has_sel) { self.actions.push(Action::SaveSelectionAs); }
                        if item(ui, "Save All", "", has_doc) { self.actions.push(Action::SaveAll); }
                        ui.separator();
                        ui.menu_button("Import", |ui| {
                            if item(ui, "Markers…", "", markers_io) { self.actions.push(Action::MarkerCmd(crate::markers_ui::MarkerCmd::ImportCsv)); }
                        });
                        ui.menu_button("Export", |ui| {
                            if item(ui, "Markers…", "", markers_io && has_markers) { self.actions.push(Action::MarkerCmd(crate::markers_ui::MarkerCmd::ExportCsv)); }
                        });
                        ui.separator();
                        if item(ui, "Exit", &km.label("quit"), true) { self.actions.push(Action::Quit); }
                    });
                    ui.menu_button("Edit", |ui| {
                        if item(ui, "Undo", &km.label("undo"), can_undo) { self.actions.push(Action::Undo); }
                        if item(ui, "Redo", &km.label("redo"), can_redo) { self.actions.push(Action::Redo); }
                        let repeat = match last {
                            Some(n) => format!("Repeat Previous Command ({n})"),
                            None => "Repeat Previous Command".to_string(),
                        };
                        if item(ui, &repeat, &km.label("repeat"), last.is_some() && has_doc) { self.actions.push(Action::RepeatLast); }
                        ui.separator();
                        if item(ui, "Cut", &km.label("cut"), has_sel) { self.actions.push(Action::Cut); }
                        if item(ui, "Copy", &km.label("copy"), has_sel) { self.actions.push(Action::Copy); }
                        if item(ui, "Copy to New", &km.label("copy_new"), has_doc) { self.actions.push(Action::CopyToNew); }
                        if item(ui, "Paste", &km.label("paste"), has_clip) { self.actions.push(Action::Paste); }
                        if item(ui, "Paste to New", &km.label("paste_new"), has_clip) { self.actions.push(Action::PasteNew); }
                        if item(ui, "Mix Paste…", &km.label("mix_paste"), has_clip && has_doc) {
                            self.dialog = Some(Dialog::MixPaste { mode: 1, clip_db: 0.0, orig_db: 0.0 });
                        }
                        ui.separator();
                        if item(ui, "Delete", &km.label("delete"), has_sel) { self.actions.push(Action::Delete); }
                        if item(ui, "Crop", &km.label("crop"), has_sel) { self.actions.push(Action::Crop); }
                        let has_spec = self.doc().map(|d| d.spec_sel.is_some()).unwrap_or(false);
                        if item(ui, "Auto Heal Selection", &km.label("heal"), has_spec) { self.actions.push(Action::SpectralHeal(None)); }
                        ui.separator();
                        ui.menu_button("Select", |ui| {
                            if item(ui, "Select All", &km.label("select_all"), has_doc) { self.actions.push(Action::SelectAll); }
                            if item(ui, "Deselect All", &km.label("deselect"), has_sel) { self.actions.push(Action::Deselect); }
                        });
                        ui.menu_button("Insert", |ui| {
                            for (label, id) in [("Silence…", "gen_silence"), ("Tones…", "gen_tone"), ("Noise…", "gen_noise")] {
                                if item(ui, label, "", has_doc) {
                                    if let Some(i) = self.find_effect(id) { self.actions.push(Action::OpenEffect(i)); }
                                }
                            }
                        });
                        ui.menu_button("Marker", |ui| {
                            if item(ui, "Add Cue Marker", &km.label("marker"), has_doc) { self.actions.push(Action::AddMarker); }
                            if item(ui, "Add CD Track Marker", &km.label("marker_cd"), has_doc) { self.actions.push(Action::AddMarkerKind(crate::app::MarkerKind::CdTrack)); }
                            if item(ui, "Add Cartridge Timer Marker", &km.label("marker_cart"), has_doc) { self.actions.push(Action::AddMarkerKind(crate::app::MarkerKind::CartTimer)); }
                            ui.separator();
                            if item(ui, "Move Playhead to Previous Marker", &km.label("prev_marker"), has_markers) { self.actions.push(Action::PrevMarker); }
                            if item(ui, "Move Playhead to Next Marker", &km.label("next_marker"), has_markers) { self.actions.push(Action::NextMarker); }
                            ui.separator();
                            if item(ui, "Show Markers Panel", "", true) { self.show_panel(Panel::Markers); }
                            if item(ui, "Show Playlist Panel", "", true) { self.show_panel(Panel::Playlist); }
                        });
                        ui.menu_button("Zero Crossings", |ui| {
                            for (z, id) in crate::markers_ui::ZeroMode::ALL.into_iter().zip(["zc_in", "zc_out", "zc_ll", "zc_lr", "zc_rl", "zc_rr"]) {
                                if item(ui, z.label(), &km.label(id), in_wave) { self.actions.push(Action::ZeroCross(z)); }
                            }
                        });
                        ui.menu_button("Snapping", |ui| {
                            if ui.checkbox(&mut self.prefs.snap_markers, "Snap to Markers").changed() { self.prefs.save(); }
                            if ui.checkbox(&mut self.prefs.snap_zero, "Snap to Zero Crossings").changed() { self.prefs.save(); }
                        });
                        if item(ui, "Convert Sample Type…", "", has_doc) {
                            if let Some((rate, channels)) = self.doc().map(|d| (d.sample_rate, d.n_ch())) {
                                self.dialog = Some(Dialog::Convert { rate, channels });
                            }
                        }
                        ui.separator();
                        if item(ui, "Keyboard Shortcuts…", &km.label("shortcuts"), true) { self.dialog = Some(Dialog::Shortcuts(Box::default())); }
                        ui.menu_button("Preferences", |ui| {
                            for page in crate::prefs_ui::Page::ALL {
                                if item(ui, &format!("{}…", page.name()), "", true) { self.actions.push(Action::PreferencesPage(page)); }
                            }
                        });
                    });
                    ui.menu_button("Multitrack", |ui| {
                        let has_session = self.session().is_some();
                        let in_mt = has_session && self.mode == Mode::Multitrack;
                        let has_time_sel = self.session().and_then(|s| s.sel_range()).is_some();
                        if item(ui, "Add Audio Track", &km.label("mt_add_track"), has_session) { self.actions.push(Action::MtAddTrack); }
                        if item(ui, "Add Bus Track", &km.label("mt_add_bus"), has_session) { self.actions.push(Action::MtAddBus); }
                        if item(ui, "Delete Selected Track", "", has_session) { self.actions.push(Action::MtDeleteTrack); }
                        ui.separator();
                        if item(ui, "Insert Files…", "", has_session) { self.actions.push(Action::MtInsertFiles); }
                        if let Some(id) = self.doc().map(|d| d.id) {
                            let name = self.doc().map(|d| d.name.clone()).unwrap_or_default();
                            if item(ui, &format!("Insert \"{name}\" at Cursor"), "", has_session) { self.actions.push(Action::MtInsertDoc(id)); }
                        }
                        if item(ui, "Split Clip at Playhead", &km.label("mt_split"), in_mt) { self.actions.push(Action::MtSplit); }
                        ui.menu_button("Clip", |ui| {
                            use crate::mt_ui::ClipCmd as C;
                            for (label, id, cmd) in [
                                ("Group Clips", "mt_group", C::Group),
                                ("Ungroup Clips", "mt_ungroup", C::Ungroup),
                                ("Loop", "mt_loop", C::ToggleLoop),
                                ("Lock in Time", "mt_lock", C::ToggleLock),
                                ("Clip Effects…", "mt_clip_fx", C::ShowFx),
                                ("Stretch…", "mt_stretch", C::StretchDialog),
                                ("Match Clip Volume…", "mt_match", C::MatchDialog),
                            ] {
                                if item(ui, label, &km.label(id), in_mt) { self.actions.push(Action::MtClip(cmd)); }
                            }
                        });
                        if item(ui, "Tempo and Metronome…", "", has_session) { self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::TempoDialog)); }
                        let (grid, metro) = self.session().map(|s| (s.show_grid, s.metronome_on)).unwrap_or((false, false));
                        if item(ui, if grid { "✓ Show Beat Grid" } else { "Show Beat Grid" }, &km.label("mt_grid"), has_session) { self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::ToggleGrid)); }
                        if item(ui, if metro { "✓ Metronome" } else { "Metronome" }, &km.label("mt_metronome"), has_session) { self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::ToggleMetronome)); }
                        let fmt = self.session().map(|s| s.master_format);
                        ui.add_enabled_ui(has_session, |ui| {
                            ui.menu_button("Master Channels", |ui| {
                                for f in crate::session::MasterFormat::ALL {
                                    let on = fmt == Some(f);
                                    if item(ui, &format!("{}{}", if on { "✓ " } else { "" }, f.label()), "", true) {
                                        self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::SetMasterFormat(f)));
                                    }
                                }
                            });
                        });
                        let cenv = self.prefs.mt_clip_env;
                        if item(ui, if cenv { "✓ Show Clip Envelopes" } else { "Show Clip Envelopes" }, &km.label("mt_clip_env"), has_session) { self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::ToggleClipEnv)); }
                        ui.menu_button("Bounce to New Track", |ui| {
                            use crate::mt_ui::{BounceKind as B, ClipCmd as C};
                            if item(ui, "Selected Track", &km.label("mt_bounce"), in_mt) { self.actions.push(Action::MtClip(C::Bounce(B::Track))); }
                            if item(ui, "Time Selection in Selected Track", &km.label("mt_bounce_sel"), in_mt && has_time_sel) { self.actions.push(Action::MtClip(C::Bounce(B::TimeSelection))); }
                            if item(ui, "Selected Clips", &km.label("mt_bounce_clips"), in_mt) { self.actions.push(Action::MtClip(C::Bounce(B::Clips))); }
                        });
                        ui.separator();
                        if item(ui, "Export to Premiere Pro (FCP XML)…", "", has_session) { self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::ExportXml)); }
                        ui.menu_button("Mixdown Session to New File", |ui| {
                            if item(ui, "Entire Session", "", has_session) { self.actions.push(Action::MtMixdown(false)); }
                            if item(ui, "Time Selection", "", has_time_sel) { self.actions.push(Action::MtMixdown(true)); }
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
                                    if item(ui, "Capture Noise Print", &km.label("capture_noise"), has_sel) { self.actions.push(Action::CaptureNoise); }
                                    if item(ui, "Learn Sound Model", "", has_sel) { self.actions.push(Action::LearnSoundModel); }
                                    ui.separator();
                                }
                                for (i, e) in effects.iter().enumerate().filter(|(_, e)| e.category == cat) {
                                    let label = if e.params.is_empty() { e.name.to_string() } else { format!("{}…", e.name) };
                                    if item(ui, &label, "", has_doc) { self.actions.push(Action::OpenEffect(i)); }
                                }
                            });
                        }
                        ui.separator();
                        if let Some(i) = crate::plugin_ui::plugin_menu(ui, &effects, has_doc, |_| true) { self.actions.push(Action::OpenEffect(i)); }
                        if item(ui, "Audio Plug-In Manager…", "", true) { self.actions.push(Action::PluginManager); }
                    });
                    ui.menu_button("Favorites", |ui| {
                        for (label, id, preset) in FAVORITES {
                            if item(ui, label, "", has_doc) { self.actions.push(Action::ApplyFavorite(*id, *preset)); }
                        }
                    });
                    ui.menu_button("View", |ui| {
                        if item(ui, "CD Layout…", "", true) { self.cd.show = true; }
                        if item(ui, "Waveform Editor", &km.label("mode_waveform"), true) { self.actions.push(Action::SetMode(Mode::Waveform)); }
                        if item(ui, "Multitrack Editor", &km.label("mode_multitrack"), true) { self.actions.push(Action::SetMode(Mode::Multitrack)); }
                        ui.separator();
                        if item(ui, "Zoom In (Time)", &km.label("zoom_in"), has_doc) { self.actions.push(Action::ZoomIn); }
                        if item(ui, "Zoom Out (Time)", &km.label("zoom_out"), has_doc) { self.actions.push(Action::ZoomOut); }
                        if item(ui, "Zoom Out Full (All Axes)", &km.label("zoom_full"), has_doc) { self.actions.push(Action::ZoomFull); }
                        if item(ui, "Zoom to Selection", "", has_sel) { self.actions.push(Action::ZoomSel); }
                        if item(ui, "Zoom In (Amplitude)", "", has_doc) { self.actions.push(Action::AmpIn); }
                        if item(ui, "Zoom Out (Amplitude)", "", has_doc) { self.actions.push(Action::AmpOut); }
                        ui.separator();
                        ui.checkbox(&mut self.show_spectral, format!("Show Spectral Frequency Display   {}", km.label("spectral")));
                        if ui.checkbox(&mut self.show_pitch, format!("Show Pitch Display   {}", km.label("pitch_display"))).changed() && self.show_pitch {
                            self.show_spectral = true;
                        }
                        ui.checkbox(&mut self.follow, "Follow Playhead");
                        if item(ui, "Play with Pre-roll and Post-roll", &km.label("play_roll"), has_doc) { self.actions.push(Action::PlayWithRoll); }
                        if item(ui, "Punch In (Record over Selection)", &km.label("punch"), has_doc) { self.actions.push(Action::PunchRecord); }
                    });
                    ui.menu_button("Window", |ui| {
                        ui.menu_button("Workspace", |ui| {
                            if item(ui, "Reset to Default", "", true) { self.reset_workspace(); }
                        });
                        ui.separator();
                        if item(ui, "Amplitude Statistics…", "", has_doc) { self.actions.push(Action::AmplitudeStatistics); }
                        ui.separator();
                        for panel in Panel::ALL {
                            if item(ui, panel.name(), "", true) { self.show_panel(panel); }
                        }
                        ui.checkbox(&mut self.show_bottom, "Selection/View");
                        ui.separator();
                        ui.checkbox(&mut self.show_left, "Show Left Panels");
                    });
                    ui.menu_button("Help", |ui| {
                        if item(ui, "Keyboard Shortcuts…", "", true) { self.dialog = Some(Dialog::Shortcuts(Box::default())); }
                        ui.separator();
                        if item(ui, "Check for Updates…", "", self.update_check.is_none()) { self.check_for_updates(true); }
                        if item(ui, "Audemo on GitHub", "", true) { let _ = crate::update::open_url(crate::update::RELEASES_PAGE); }
                        ui.separator();
                        if item(ui, "About Audemo", "", true) { self.dialog = Some(Dialog::About); }
                    });
                });
            });
    }

    pub fn show_panel(&mut self, panel: Panel) {
        if Panel::METERS.contains(&panel) {
            self.show_meters = true;
            self.tab_meter = panel;
            return;
        }
        self.show_left = true;
        if Panel::TOP.contains(&panel) {
            self.tab_top = panel;
        } else if Panel::MIDDLE.contains(&panel) {
            self.tab_mid = panel;
        } else if Panel::BOTTOM.contains(&panel) {
            self.tab_bot = panel;
        }
    }

    pub fn reset_workspace(&mut self) {
        self.show_left = true;
        self.show_bottom = true;
        self.show_meters = true;
        self.tab_top = Panel::Files;
        self.tab_mid = Panel::EffectsRack;
        self.tab_bot = Panel::History;
        self.tab_meter = Panel::Levels;
    }

    pub fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::none().fill(BG_PANEL()).inner_margin(egui::Margin::symmetric(6.0, 3.0)).stroke(Stroke::new(1.0_f32, BORDER())))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let mt = self.mode == Mode::Multitrack;
                    if mode_button(ui, Icon::Waveform, "Waveform", !mt, true, "Waveform Editor (9)").clicked() {
                        self.actions.push(Action::SetMode(Mode::Waveform));
                    }
                    if mode_button(ui, Icon::Multitrack, "Multitrack", mt, true, "Multitrack Editor (0)").clicked() {
                        self.actions.push(Action::SetMode(Mode::Multitrack));
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    if icon_button(ui, Icon::Spectral, self.show_spectral, "Show Spectral Frequency Display (Shift+D)").clicked() {
                        self.show_spectral = !self.show_spectral;
                    }
                    ui.separator();
                    let mt_tip = "In the Multitrack editor: drag a clip to move it, its edges to trim, its yellow handles to fade";
                    icon_button_sized(ui, Icon::Move, mt, mt_tip, vec2(24.0, 22.0), false);
                    if icon_button_sized(ui, Icon::Razor, false, "Split clips at the playhead (Ctrl+K)", vec2(24.0, 22.0), mt).clicked() {
                        self.actions.push(Action::MtSplit);
                    }
                    icon_button_sized(ui, Icon::Slip, false, "Slip editing arrives in a later release", vec2(24.0, 22.0), false);
                    for (icon, t, tip) in [
                        (Icon::Selection, Tool::Selection, "Time Selection Tool (T)"),
                        (Icon::Hand, Tool::Hand, "Hand Tool (H; middle-drag works with any tool)"),
                        (Icon::Marquee, Tool::Marquee, "Marquee Selection Tool (E): drag a rectangle on the spectral display"),
                        (Icon::Lasso, Tool::Lasso, "Lasso Selection Tool (D): draw around an area of the spectral display"),
                        (Icon::Brush, Tool::Brush, "Paintbrush Selection Tool (P): paint an area of the spectral display"),
                        (Icon::Healing, Tool::SpotHeal, "Spot Healing Brush Tool (B): paint over a click, cough or noise and it's repaired when you let go"),
                    ] {
                        if icon_button(ui, icon, self.tool == t, tip).clicked() {
                            self.set_tool(t);
                        }
                    }
                    if matches!(self.tool, Tool::Brush | Tool::SpotHeal) {
                        ui.add_space(4.0);
                        ui.label(RichText::new("Size").color(TEXT_DIM()).size(11.0));
                        hot_drag(ui, egui::DragValue::new(&mut self.brush_px).speed(0.5).range(4.0..=200.0).fixed_decimals(0).suffix(" px"));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.search_box(ui);
                        ui.add_space(10.0);
                        egui::ComboBox::from_id_source("workspace").width(120.0).selected_text("Default").show_ui(ui, |ui| {
                            if ui.selectable_label(true, "Default").clicked() {}
                            ui.separator();
                            if ui.button("Reset \"Default\" to Saved Layout").clicked() {
                                self.reset_workspace();
                                ui.close_menu();
                            }
                        });
                        ui.label(RichText::new("Workspace:").color(TEXT_DIM()));
                        ui.add_space(12.0);
                        if let Some(np) = &self.noise_print {
                            let secs = np.samples.first().map(|c| c.len()).unwrap_or(0) as f64 / np.sample_rate as f64;
                            ui.label(RichText::new(format!("Noise print {secs:.1} s")).color(HOT()).size(11.5))
                                .on_hover_text("Captured noise print for Noise Reduction (process)");
                        }
                    });
                });
            });
    }

    /// "Search Help" box: finds effects and opens them.
    fn search_box(&mut self, ui: &mut Ui) {
        let id = egui::Id::new("search_help");
        let mut q: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
        let resp = ui.add(egui::TextEdit::singleline(&mut q).id(id).hint_text("Search Help").desired_width(170.0));
        let ql = q.trim().to_lowercase();
        let mut open: Option<usize> = None;
        if !ql.is_empty() && (resp.has_focus() || resp.lost_focus()) {
            let hits: Vec<(usize, &'static str, &'static str)> = self
                .effects
                .iter()
                .enumerate()
                .filter(|(_, e)| e.name.to_lowercase().contains(&ql) || e.description.to_lowercase().contains(&ql))
                .take(10)
                .map(|(i, e)| (i, e.name, e.category.name()))
                .collect();
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                open = hits.first().map(|h| h.0);
            } else if !hits.is_empty() {
                egui::Area::new(id.with("popup")).order(egui::Order::Foreground).fixed_pos(resp.rect.left_bottom() + vec2(-120.0, 2.0)).show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(290.0);
                        for (i, name, cat) in &hits {
                            let r = ui.selectable_label(false, format!("{name}   ·   {cat}"));
                            if r.clicked() || r.is_pointer_button_down_on() {
                                open = Some(*i);
                            }
                        }
                    });
                });
            }
        }
        if let Some(i) = open {
            q.clear();
            self.actions.push(Action::OpenEffect(i));
        }
        ui.data_mut(|d| d.insert_temp(id, q));
    }

    pub fn left_column(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("left")
            .resizable(true)
            .default_width(270.0)
            .width_range(200.0..=460.0)
            .frame(egui::Frame::none().fill(BG_DEEP()).inner_margin(egui::Margin::same(2.0)))
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
                        let mut tab = self.tab_bot;
                        tab_strip(ui, &Panel::BOTTOM, &mut tab);
                        self.tab_bot = tab;
                        match tab {
                            Panel::MatchLoudness => self.match_loudness_panel(ui),
                            _ => self.history(ui),
                        }
                    });
                egui::CentralPanel::default().frame(panel_frame()).show_inside(ui, |ui| {
                    let mut tab = self.tab_mid;
                    tab_strip(ui, &Panel::MIDDLE, &mut tab);
                    self.tab_mid = tab;
                    match tab {
                        Panel::MediaBrowser => self.media_browser(ui),
                        Panel::Markers => self.markers_tab(ui),
                        Panel::Playlist => self.playlist_panel(ui),
                        Panel::EssentialSound => self.essential_panel(ui),
                        Panel::Properties => self.properties_panel(ui),
                        Panel::Diagnostics => self.diagnostics_panel(ui),
                        Panel::BatchProcess => self.batch_panel(ui),
                        Panel::Metadata => self.metadata_panel(ui),
                        _ => self.effects_rack(ui),
                    }
                });
            });
    }

    fn files_tab(&mut self, ui: &mut Ui) {
        let has_doc = self.doc().is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            if icon_button_sized(ui, Icon::FolderOpen, false, "Open File (Ctrl+O)", vec2(22.0, 20.0), true).clicked() {
                self.actions.push(Action::Open);
            }
            if icon_button_sized(ui, Icon::NewFile, false, "New Audio File (Ctrl+Shift+N)", vec2(22.0, 20.0), true).clicked() {
                self.actions.push(Action::New);
            }
            if icon_button_sized(ui, Icon::Multitrack, false, "New Multitrack Session (Ctrl+N)", vec2(22.0, 20.0), true).clicked() {
                self.actions.push(Action::NewSession);
            }
            if icon_button_sized(ui, Icon::CloseFile, false, "Close File (Ctrl+W)", vec2(22.0, 20.0), has_doc).clicked() {
                self.actions.push(Action::Close(usize::MAX));
            }
        });
        let mut switch = None;
        let mut switch_session = None;
        let mut select = None;
        let mut insert: Option<u64> = None;
        let mut close_doc: Option<usize> = None;
        let mut new_action: Option<Action> = None;
        let mut close_keys: Option<Vec<crate::app::FileKey>> = None;
        let mut picked_empty = false;
        // Row clicks: (row key, Cmd/Ctrl held, Shift held).
        let mut picked: Option<(crate::app::FileKey, bool, bool)> = None;
        self.files_drawn = true;
        {
            let live = self.file_keys();
            self.files_sel.retain(|k| live.contains(k));
        }
        let sel_set = self.files_sel.clone();
        let focused = self.files_focus;
        let n_sel = sel_set.len();

        // One row per session and file; every row is a single strip that
        // clicks (select), double-clicks (open) and drags (into a track).
        struct Row {
            key: crate::app::FileKey,
            session: Option<usize>,
            doc: Option<(usize, u64)>,
            icon: Icon,
            icon_col: Color32,
            name: String,
            active: bool,
            cols: [String; 6],
        }
        let mut rows: Vec<Row> = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            let active = Some(i) == self.active_session && self.mode == Mode::Multitrack;
            rows.push(Row {
                key: crate::app::FileKey::Session(s.id),
                session: Some(i),
                doc: None,
                icon: Icon::Multitrack,
                icon_col: if active { HOT() } else { Color32::from_rgb(0x4d, 0xa3, 0xff) },
                name: s.name.clone(),
                active,
                cols: [
                    if s.dirty { "*".into() } else { String::new() },
                    format_time(s.end() as f64, s.sample_rate),
                    format!("{} Hz", s.sample_rate),
                    format!("{} tracks", s.tracks.len()),
                    String::new(),
                    "Session".into(),
                ],
            });
        }
        for (i, d) in self.docs.iter().enumerate() {
            let active = Some(i) == self.active;
            rows.push(Row {
                key: crate::app::FileKey::Doc(d.id),
                session: None,
                doc: Some((i, d.id)),
                icon: Icon::Waveform,
                icon_col: if active { HOT() } else { WAVE() },
                name: d.name.clone(),
                active,
                cols: [
                    if d.dirty { "*".into() } else { String::new() },
                    format_time(d.len() as f64, d.sample_rate),
                    format!("{} Hz", d.sample_rate),
                    match d.n_ch() {
                        1 => "Mono".to_string(),
                        2 => "Stereo".to_string(),
                        6 => "5.1".to_string(),
                        8 => "7.1".to_string(),
                        n => format!("{n} ch"),
                    },
                    d.source_bits.map(|b| format!("{b}")).unwrap_or_else(|| "32 (float)".into()),
                    d.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(),
                ],
            });
        }
        let font = FontId::proportional(13.0);
        let head_font = FontId::proportional(11.5);
        const HEADS: [&str; 7] = ["Name", "Status", "Duration", "Sample Rate", "Channels", "Bit Depth", "Source Format"];
        const COL_W: [f32; 6] = [52.0, 78.0, 92.0, 74.0, 78.0, 96.0];
        const GAP: f32 = 12.0;
        const ROW_H: f32 = 24.0;
        const HEAD_H: f32 = 22.0;
        let name_w = rows
            .iter()
            .map(|r| ui.fonts(|f| f.layout_no_wrap(r.name.clone(), font.clone(), TEXT()).size().x))
            .fold(60.0_f32, f32::max)
            + 30.0;
        let mut col_x = vec![8.0_f32];
        let mut x = 8.0 + name_w + GAP;
        for w in COL_W {
            col_x.push(x);
            x += w + GAP;
        }
        let total_w = x;
        let panel = egui::Frame::none().fill(BG_LIST()).show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            let view_h = ui.available_height();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let full_w = ui.available_width().max(total_w);
                // Header.
                let (hr, _) = ui.allocate_exact_size(vec2(full_w, HEAD_H), Sense::hover());
                for (k, h) in HEADS.iter().enumerate() {
                    ui.painter().text(pos2(hr.left() + col_x[k], hr.center().y), Align2::LEFT_CENTER, *h, head_font.clone(), TEXT_DIM());
                }
                for (k, row) in rows.iter().enumerate() {
                    let sense = if row.doc.is_some() { Sense::click_and_drag() } else { Sense::click() };
                    let (r, resp) = ui.allocate_exact_size(vec2(full_w, ROW_H), sense);
                    let painter = ui.painter();
                    let selected = sel_set.contains(&row.key);
                    if selected {
                        // Selected rows: accent-tinted, stronger while the panel has focus.
                        let a = if focused { 70 } else { 38 };
                        painter.rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(0x2d, 0x7f, 0xd9, a));
                    } else if row.active {
                        painter.rect_filled(r, 2.0, BG_RAISED());
                    } else if resp.hovered() {
                        painter.rect_filled(r, 2.0, Color32::from_white_alpha(14));
                    } else if k % 2 == 1 {
                        painter.rect_filled(r, 0.0, Color32::from_white_alpha(5));
                    }
                    let icon = Rect::from_center_size(pos2(r.left() + col_x[0] + 7.0, r.center().y), vec2(13.0, 13.0));
                    draw_icon(painter, icon, row.icon, row.icon_col);
                    let name_col = if row.active { HOT() } else { TEXT() };
                    painter.text(pos2(r.left() + col_x[0] + 22.0, r.center().y), Align2::LEFT_CENTER, &row.name, font.clone(), name_col);
                    for (c, text) in row.cols.iter().enumerate() {
                        painter.text(pos2(r.left() + col_x[c + 1], r.center().y), Align2::LEFT_CENTER, text, font.clone(), if row.active { TEXT() } else { TEXT_DIM() });
                    }
                    let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
                    if resp.clicked() {
                        picked = Some((row.key, cmd, shift));
                    }
                    if resp.secondary_clicked() && !selected {
                        picked = Some((row.key, false, false));
                    }
                    // Closing several: the right-clicked row is in a multi-row selection.
                    let close_many = selected && n_sel > 1;
                    if let Some(i) = row.session {
                        if (resp.clicked() && !cmd && !shift) || resp.double_clicked() {
                            switch_session = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Open in Multitrack Editor").clicked() {
                                switch_session = Some(i);
                                ui.close_menu();
                            }
                            ui.separator();
                            if close_many {
                                if ui.button(format!("Close {n_sel} Selected")).clicked() {
                                    close_keys = Some(Vec::new());
                                    ui.close_menu();
                                }
                            } else if ui.button("Close Session").clicked() {
                                close_keys = Some(vec![row.key]);
                                ui.close_menu();
                            }
                        });
                    }
                    if let Some((i, id)) = row.doc {
                        resp.dnd_set_drag_payload(crate::mt_ui::DocDrag(id));
                        if resp.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        }
                        if resp.clicked() && !cmd && !shift {
                            select = Some(i);
                        }
                        if resp.double_clicked() {
                            switch = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Open in Waveform Editor").clicked() {
                                switch = Some(i);
                                ui.close_menu();
                            }
                            if ui.button("Insert into Multitrack at Cursor").clicked() {
                                insert = Some(id);
                                ui.close_menu();
                            }
                            ui.separator();
                            if close_many {
                                if ui.button(format!("Close {n_sel} Selected")).clicked() {
                                    close_keys = Some(Vec::new());
                                    ui.close_menu();
                                }
                            } else if ui.button("Close File").clicked() {
                                close_doc = Some(i);
                                ui.close_menu();
                            }
                        });
                    }
                }
                // The empty space below: right-click for new files.
                let used = HEAD_H + rows.len() as f32 * ROW_H;
                let (_, empty) = ui.allocate_exact_size(vec2(full_w, (view_h - used - 4.0).max(32.0)), Sense::click());
                if empty.clicked() {
                    picked_empty = true;
                }
                empty.context_menu(|ui| {
                    if ui.button("New Audio File…").clicked() {
                        new_action = Some(Action::New);
                        ui.close_menu();
                    }
                    if ui.button("New Multitrack Session…").clicked() {
                        new_action = Some(Action::NewSession);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Open File…").clicked() {
                        new_action = Some(Action::Open);
                        ui.close_menu();
                    }
                });
            });
        });
        // Keyboard focus: the panel has it after a click inside it, until a click elsewhere.
        let press = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None });
        if let Some(p) = press {
            self.files_focus = panel.response.rect.contains(p);
        }
        if picked_empty {
            self.files_sel.clear();
        }
        if let Some((key, cmd, shift)) = picked {
            let order = self.file_keys();
            if shift {
                let a = self.files_anchor.and_then(|k| order.iter().position(|x| *x == k)).unwrap_or(0);
                let b = order.iter().position(|x| *x == key).unwrap_or(a);
                let (lo, hi) = (a.min(b), a.max(b));
                if !cmd {
                    self.files_sel.clear();
                }
                self.files_sel.extend(order[lo..=hi].iter().copied());
            } else if cmd {
                if !self.files_sel.remove(&key) {
                    self.files_sel.insert(key);
                }
                self.files_anchor = Some(key);
            } else {
                self.files_sel.clear();
                self.files_sel.insert(key);
                self.files_anchor = Some(key);
            }
        }
        if let Some(keys) = close_keys {
            let keys = if keys.is_empty() { self.file_keys().into_iter().filter(|k| self.files_sel.contains(k)).collect() } else { keys };
            self.close_files(keys);
        }
        if let Some(a) = new_action {
            self.actions.push(a);
        }
        if let Some(i) = close_doc {
            self.actions.push(Action::Close(i));
        }
        if let Some(i) = select {
            // A single click selects; it only changes the view in the Waveform editor.
            self.active = Some(i);
        }
        if let Some(i) = switch {
            self.active = Some(i);
            self.mode = Mode::Waveform;
        }
        if let Some(i) = switch_session {
            self.active_session = Some(i);
            self.mode = Mode::Multitrack;
        }
        if let Some(id) = insert {
            self.actions.push(Action::MtInsertDoc(id));
        }
    }

    /// Bottom strip: Levels / Frequency Analysis / Phase Meter and the Selection/View panel.
    pub fn bottom_row(&mut self, ctx: &egui::Context) {
        if !self.show_meters && !self.show_bottom {
            return;
        }
        egui::TopBottomPanel::bottom("bottom_row")
            .resizable(true)
            .default_height(150.0)
            .height_range(64.0..=420.0)
            .frame(egui::Frame::none().fill(BG_DEEP()).inner_margin(egui::Margin::same(2.0)))
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
                        let names = [if self.engine.is_recording() { "Levels (Input)" } else { "Levels" }, "Frequency Analysis", "Phase Meter"];
                        let mut tab = Panel::METERS.iter().position(|p| *p == self.tab_meter).unwrap_or(0);
                        tab_strip_named(ui, &names, &mut tab);
                        self.tab_meter = Panel::METERS[tab];
                        match self.tab_meter {
                            Panel::FrequencyAnalysis => self.frequency_analysis(ui),
                            Panel::PhaseMeter => self.phase_meter(ui),
                            _ => self.levels(ui),
                        }
                    });
                }
            });
    }

    fn selection_view(&mut self, ui: &mut Ui) {
        let Some(di) = self.active else {
            ui.label(RichText::new("No file").color(TEXT_DIM()));
            return;
        };
        let doc = &mut self.docs[di];
        let sr = doc.sample_rate;
        let (sa, sb) = doc.sel_range().unwrap_or((doc.cursor, doc.cursor));
        let mut new_sel: Option<(f64, f64)> = None;
        let mut new_view: Option<(f64, f64)> = None;
        egui::Grid::new("selview").num_columns(4).min_col_width(60.0).spacing([10.0, 4.0]).show(ui, |ui| {
            ui.label("");
            for h in ["Start", "End", "Duration"] {
                ui.label(RichText::new(h).color(TEXT_DIM()).size(11.0));
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
                ui.label(RichText::new(k).color(TEXT_DIM()).size(11.0));
                ui.label(RichText::new(v).size(11.0));
                ui.end_row();
            }
        });
    }

    fn history(&mut self, ui: &mut Ui) {
        // The Multitrack editor shows the session's history.
        let (origin, undo, redo): (String, Vec<String>, Vec<String>) = match (self.mode, self.session(), self.doc()) {
            (Mode::Multitrack, Some(s), _) => ("New Session".into(), s.undo.iter().map(|u| u.label.clone()).collect(), s.redo.iter().map(|u| u.label.clone()).collect()),
            (_, _, Some(d)) => (d.origin.clone(), d.undo.iter().map(|u| u.label.clone()).collect(), d.redo.iter().map(|u| u.label.clone()).collect()),
            _ => {
                ui.label(RichText::new("No history").color(TEXT_DIM()));
                return;
            }
        };
        let current = undo.len();
        let undos = undo.len();
        let mut entries: Vec<(usize, String, bool)> = vec![(0, origin, false)];
        for (i, l) in undo.into_iter().enumerate() {
            entries.push((i + 1, l, false));
        }
        for (k, l) in redo.into_iter().rev().enumerate() {
            entries.push((current + 1 + k, l, true));
        }
        let mut jump = None;
        let mut clear = false;
        let list_h = (ui.available_height() - 24.0).max(30.0);
        egui::Frame::none().fill(BG_LIST()).show(ui, |ui| {
            ui.set_height(list_h);
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                for (idx, label, future) in entries {
                    let text = if future { RichText::new(&label).color(TEXT_DIM()).italics() } else { RichText::new(&label) };
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                        if idx == current {
                            ui.painter().add(egui::Shape::convex_polygon(
                                vec![pos2(r.left() + 2.0, r.top() + 3.0), pos2(r.right() - 2.0, r.center().y), pos2(r.left() + 2.0, r.bottom() - 3.0)],
                                HOT(),
                                Stroke::NONE,
                            ));
                        }
                        if ui.selectable_label(idx == current, text).clicked() && idx != current {
                            jump = Some(idx);
                        }
                    });
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{undos} Undo{}", if undos == 1 { "" } else { "s" })).color(TEXT_DIM()).size(11.5));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button_sized(ui, Icon::Trash, false, "Clear History", vec2(20.0, 18.0), undos > 0 || current > 0).clicked() {
                    clear = true;
                }
            });
        });
        if let Some(j) = jump {
            self.actions.push(Action::JumpHistory(j));
        }
        if clear {
            self.actions.push(Action::ClearHistory);
        }
    }

    /// Transport and zoom controls along the bottom of the Editor panel.
    pub fn transport_bar(&mut self, ui: &mut Ui) {
        egui::TopBottomPanel::bottom("transport")
            .frame(egui::Frame::none().fill(BG_PANEL()).inner_margin(egui::Margin::symmetric(8.0, 4.0)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 1.0;
                    let mt_info = self.transport_info();
                    let sr = mt_info.map(|i| i.1).or(self.doc().map(|d| d.sample_rate)).unwrap_or(48000);
                    let recording = self.engine.is_recording();
                    let pos = match (&self.rec_target, self.doc(), mt_info) {
                        (_, _, Some(i)) => i.0,
                        (Some(t), Some(d), _) if recording && t.doc_id == d.id => (t.range.0 + d.pending) as f64,
                        _ => self.display_pos(),
                    };
                    let (r, _) = ui.allocate_exact_size(vec2(150.0, 30.0), Sense::hover());
                    let col = if recording { RECORD() } else { TIME() };
                    ui.painter().text(r.left_center() + vec2(2.0, 0.0), Align2::LEFT_CENTER, format_time(pos, sr), FontId::monospace(24.0), col);

                    let st = self.engine.status();
                    let playing = match mt_info {
                        Some(i) => i.2,
                        None => st.playing && self.doc().map(|d| d.id == st.tag).unwrap_or(false),
                    };
                    let zoom_w = 6.0 * 25.0;
                    let group_w = 9.0 * 25.0;
                    ui.add_space(((ui.available_width() - group_w - zoom_w) / 2.0).max(10.0));
                    let dt = ui.input(|i| i.stable_dt).min(0.1) as f64;
                    if icon_button(ui, Icon::Stop, false, "Stop").clicked() {
                        self.actions.push(Action::Stop);
                    }
                    if icon_button(ui, Icon::Play, playing, "Play (Space)").clicked() {
                        self.actions.push(if playing { Action::Stop } else { Action::PlayToggle });
                    }
                    if icon_button(ui, Icon::Pause, self.paused.is_some(), "Pause").clicked() {
                        self.actions.push(Action::Pause);
                    }
                    if icon_button(ui, Icon::ToStart, false, "Move Playhead to Previous (←)").clicked() {
                        self.actions.push(Action::PrevMarker);
                    }
                    let rw = icon_button(ui, Icon::Rewind, false, "Rewind (hold)");
                    if rw.is_pointer_button_down_on() {
                        self.actions.push(Action::Nudge(-4.0 * dt));
                        ui.ctx().request_repaint();
                    }
                    let ff = icon_button(ui, Icon::FastForward, false, "Fast Forward (hold)");
                    if ff.is_pointer_button_down_on() {
                        self.actions.push(Action::Nudge(4.0 * dt));
                        ui.ctx().request_repaint();
                    }
                    if icon_button(ui, Icon::ToEnd, false, "Move Playhead to Next (→)").clicked() {
                        self.actions.push(Action::NextMarker);
                    }
                    if icon_button(ui, Icon::Record, recording, "Record (Shift+Space)").clicked() {
                        self.actions.push(Action::Record);
                    }
                    if icon_button(ui, Icon::Loop, self.looping, "Loop Playback").clicked() {
                        self.looping = !self.looping;
                        self.engine.set_looping(self.looping);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        for (icon, action, tip) in [
                            (Icon::ZoomSel, Action::ZoomSel, "Zoom to Selection"),
                            (Icon::ZoomFull, Action::ZoomFull, "Zoom Out Full (\\)"),
                            (Icon::ZoomOut, Action::ZoomOut, "Zoom Out (Time) (-)"),
                            (Icon::ZoomIn, Action::ZoomIn, "Zoom In (Time) (=)"),
                            (Icon::AmpOut, Action::AmpOut, "Zoom Out (Amplitude)"),
                            (Icon::AmpIn, Action::AmpIn, "Zoom In (Amplitude) (Alt+wheel)"),
                        ] {
                            if icon_button(ui, icon, false, tip).clicked() {
                                self.actions.push(action);
                            }
                        }
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
            p.text(pos2(rect.left() + 2.0, r.center().y), Align2::LEFT_CENTER, if c == 0 { "L" } else { "R" }, FontId::monospace(9.5), TEXT_DIM());
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
                p.line_segment([pos2(hx, r.top()), pos2(hx, r.bottom())], Stroke::new(2.0_f32, if hold > -0.1 { RECORD() } else { Color32::WHITE }));
            }
            let clip = self.meter_hold[c].0 > -0.1;
            let cr = Rect::from_min_size(pos2(rect.right() - 10.0, r.top()), vec2(8.0, bar_h));
            p.rect_filled(cr, 1.0, if clip { RECORD() } else { Color32::from_rgb(0x30, 0x18, 0x18) });
        }
        let step = if bars.width() > 700.0 { 3.0 } else { 6.0 };
        let mut db = floor;
        while db <= 0.0 {
            let x = x_of(db);
            p.line_segment([pos2(x, bars.bottom() + 1.0), pos2(x, bars.bottom() + 4.0)], Stroke::new(1.0_f32, TEXT_DIM()));
            p.text(pos2(x, bars.bottom() + 5.0), Align2::CENTER_TOP, format!("{}", db as i32), FontId::monospace(9.0), TEXT_DIM());
            db += step;
        }
    }

    pub fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(24.0)
            .frame(egui::Frame::none().fill(Color32::from_rgb(0x2c, 0x2c, 0x2c)).inner_margin(egui::Margin::symmetric(8.0, 3.0)).stroke(Stroke::new(1.0_f32, BORDER())))
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
                    ui.label(RichText::new(state).color(TEXT()).size(11.5));
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
                    ui.label(RichText::new(msg).color(TEXT_DIM()).size(11.5));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(d) = self.doc() {
                            let mb = d.len() as f64 * d.n_ch() as f64 * 4.0 / 1_048_576.0;
                            let col = TEXT();
                            if let Some(free) = self.free_space() {
                                ui.label(RichText::new(format!("{:.2} GB free", free as f64 / 1_073_741_824.0)).color(col).size(11.5));
                                ui.add_space(14.0);
                            }
                            ui.label(RichText::new(format_time(d.len() as f64, d.sample_rate)).color(col).size(11.5));
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
        if let Some(sj) = &self.saving {
            let t = sj.started.elapsed().as_secs_f32();
            if t > 0.2 {
                let name = sj.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let (frac, summary, progress) = (sj.progress.get(), sj.settings.summary(), sj.progress.clone());
                egui::Area::new(egui::Id::new("saving"))
                    .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                            ui.set_width(340.0);
                            ui.label(RichText::new(format!("Saving {name}")).size(14.0));
                            ui.label(RichText::new(summary).color(TEXT_DIM()).size(11.0));
                            ui.add_space(4.0);
                            ui.add(egui::ProgressBar::new(frac).show_percentage());
                            ui.add_space(4.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Cancel").clicked() {
                                    progress.cancel();
                                }
                            });
                        });
                    });
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
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
    egui::Frame::none().fill(BG_PANEL()).inner_margin(egui::Margin::same(PANEL_MARGIN)).rounding(3.0).outer_margin(egui::Margin::same(2.0))
}

/// Audition-style tab header for a panel group.
pub fn tab_strip(ui: &mut Ui, panels: &[Panel], current: &mut Panel) {
    let names: Vec<&str> = panels.iter().map(|p| p.name()).collect();
    let mut idx = panels.iter().position(|p| p == current).unwrap_or(0);
    tab_strip_named(ui, &names, &mut idx);
    *current = panels[idx];
}

/// Dark strip across the top of a panel frame with text tabs; the active
/// tab takes the panel colour, the others sit on the strip.
pub fn tab_strip_named(ui: &mut Ui, names: &[&str], current: &mut usize) {
    let full = ui.available_rect_before_wrap();
    let m = PANEL_MARGIN;
    let h = 22.0;
    let header = Rect::from_min_size(pos2(full.left() - m, full.top() - m), vec2(full.width() + 2.0 * m, h));
    let p = ui.painter().clone();
    p.rect_filled(header, Rounding { nw: 3.0, ne: 3.0, sw: 0.0, se: 0.0 }, BG_HEADER());
    // Which tabs fit; the active one always shows (in place of the last that fits).
    let font_of = |active: bool| if active { bold(12.0) } else { FontId::proportional(12.0) };
    let widths: Vec<f32> = names.iter().enumerate().map(|(i, n)| ui.fonts(|f| f.layout_no_wrap(n.to_string(), font_of(i == *current), TEXT()).size().x) + 18.0).collect();
    let avail = header.width() - 26.0;
    let mut shown: Vec<usize> = Vec::new();
    let mut used = 0.0;
    for (i, w) in widths.iter().enumerate() {
        if used + w > avail && !shown.is_empty() {
            break;
        }
        used += w + 1.0;
        shown.push(i);
    }
    if !shown.contains(current) && *current < names.len() {
        while shown.len() > 1 && shown.iter().map(|i| widths[*i] + 1.0).sum::<f32>() + widths[*current] > avail {
            shown.pop();
        }
        shown.push(*current);
    }
    let hidden: Vec<usize> = (0..names.len()).filter(|i| !shown.contains(i)).collect();
    let mut x = header.left() + 2.0;
    for &i in &shown {
        let n = names[i];
        let active = i == *current;
        let w = widths[i];
        let r = Rect::from_min_max(pos2(x, header.top() + 2.0), pos2(x + w, header.bottom()));
        let resp = ui.interact(r, ui.id().with(("tab", n)), Sense::click());
        if active {
            p.rect_filled(r, Rounding { nw: 3.0, ne: 3.0, sw: 0.0, se: 0.0 }, BG_PANEL());
        } else if resp.hovered() {
            p.rect_filled(r, Rounding { nw: 3.0, ne: 3.0, sw: 0.0, se: 0.0 }, Color32::from_rgb(0x33, 0x33, 0x33));
        }
        p.text(r.center(), Align2::CENTER_CENTER, n, font_of(active), if active { Color32::WHITE } else { TEXT_DIM() });
        if resp.clicked() {
            *current = i;
        }
        x += w + 1.0;
    }
    // Panel menu: lists the tabs that don't fit.
    let menu = Rect::from_center_size(pos2(header.right() - 11.0, header.center().y + 1.0), vec2(10.0, 8.0));
    let menu_hit = menu.expand2(vec2(6.0, 6.0));
    let mresp = ui.interact(menu_hit, ui.id().with("tab_menu"), Sense::click());
    draw_icon(&p, menu, Icon::Menu, if !hidden.is_empty() { HOT() } else if mresp.hovered() { TEXT() } else { TEXT_DIM() });
    let open_id = ui.id().with("tab_menu_open");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    if mresp.clicked() {
        open = !open;
    }
    if !hidden.is_empty() {
        let mresp = mresp.on_hover_text(format!("More panels: {}", hidden.iter().map(|i| names[*i]).collect::<Vec<_>>().join(", ")));
        if open {
            let area = egui::Area::new(open_id.with("area")).order(egui::Order::Foreground).fixed_pos(pos2(menu_hit.right() - 160.0, menu_hit.bottom() + 2.0));
            let inner = area.show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(150.0);
                    for &i in &hidden {
                        if ui.selectable_label(false, names[i]).clicked() {
                            *current = i;
                            open = false;
                        }
                    }
                });
            });
            if ui.input(|i| i.pointer.any_click()) && !inner.response.hovered() && !mresp.clicked() && !mresp.hovered() {
                open = false;
            }
        }
    } else {
        open = false;
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    ui.add_space(h - m + 4.0);
}

pub const PANEL_MARGIN: f32 = 6.0;

/// Waveform / Multitrack view buttons at the left of the toolbar.
fn mode_button(ui: &mut Ui, icon: Icon, label: &str, active: bool, enabled: bool, tip: &str) -> egui::Response {
    let font = FontId::proportional(12.5);
    let tw = ui.fonts(|f| f.layout_no_wrap(label.to_string(), font.clone(), TEXT()).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + 34.0, 22.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let bg = if active { Color32::from_rgb(0x26, 0x26, 0x26) } else if enabled && resp.hovered() { Color32::from_rgb(0x55, 0x55, 0x55) } else { BG_RAISED() };
    p.rect_filled(rect, 2.0, bg);
    p.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, BORDER()));
    let fg = if !enabled { Color32::from_rgb(0x70, 0x70, 0x70) } else if active { Color32::WHITE } else { TEXT() };
    let ir = Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(14.0, 12.0));
    draw_icon(p, ir, icon, if active { WAVE() } else { fg });
    p.text(pos2(rect.left() + 24.0, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    resp.on_hover_text(tip)
}
