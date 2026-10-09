//! Effect windows and the smaller modal dialogs.

use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, RichText, Sense, Shape, Stroke, Ui};

use crate::app::{Action, App, Dialog, SAMPLE_RATES};
use crate::dsp::effects::EffectDef;
use crate::dsp::params::{Kind, Params, Value};
use crate::dsp::util::format_time;
use crate::engine::PREVIEW_TAG;
use crate::io::WavFormat;
use crate::theme::*;

const SHORTCUTS: &[(&str, &str)] = &[
    ("Play / stop", "Space"),
    ("Record (Stop or Space ends it)", "Shift+Space"),
    ("Open / Save / Save As", "Ctrl+O / Ctrl+S / Ctrl+Shift+S"),
    ("New multitrack session / new audio file", "Ctrl+N / Ctrl+Shift+N"),
    ("Waveform / Multitrack editor", "9 / 0"),
    ("Undo / Redo", "Ctrl+Z / Ctrl+Shift+Z (or Ctrl+Y)"),
    ("Cut / Copy / Paste", "Ctrl+X / Ctrl+C / Ctrl+V"),
    ("Copy to New / Paste to New", "Alt+Shift+C / Ctrl+Alt+V"),
    ("Mix Paste", "Ctrl+Shift+V"),
    ("Repeat previous command", "Shift+R"),
    ("Delete selection", "Delete or Backspace"),
    ("Crop to selection", "Ctrl+T"),
    ("Select all / deselect", "Ctrl+A / Esc"),
    ("Extend selection", "Shift+click or Shift+drag"),
    ("Select visible range", "Double-click"),
    ("Add marker", "M"),
    ("Previous / next marker", "← / →"),
    ("Start / end of file", "Home / End"),
    ("Zoom in / out / full", "= / - / \\"),
    ("Zoom time at the pointer", "Mouse wheel"),
    ("Scroll", "Shift+wheel, middle-drag or Hand tool"),
    ("Zoom amplitude", "Alt+wheel"),
    ("Spectral display", "Shift+D"),
    ("Capture noise print", "Shift+P"),
    ("Toggle effect preview", "Space (with an effect open)"),
    ("Multitrack: add audio track", "Alt+A"),
    ("Multitrack: split clip at playhead", "Ctrl+K"),
    ("Multitrack: move / trim / fade a clip", "Drag the clip / its edges / its yellow handles"),
    ("Shape a fade (both editors)", "Drag the fade handle up/down; hold Ctrl for cosine"),
    ("Multitrack: snap off while dragging", "Hold Alt"),
    ("Multitrack: show automation", "A on the track header"),
    ("Multitrack: add / move / delete automation point", "Double-click or drag the line / drag a point / double-click a point"),
    ("Multitrack: crossfade two clips", "Overlap them on the same track"),
    ("Multitrack: scroll tracks", "Wheel over the track headers, or Alt+wheel"),
    ("Multitrack: open a clip's file", "Double-click the clip"),
];

fn centered(w: egui::Window<'_>) -> egui::Window<'_> {
    w.collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, -40.0))
}

impl App {
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        if matches!(self.dialog, Some(Dialog::Effect(_))) {
            self.effect_dialog(ctx);
            return;
        }
        let Some(dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let mut next: Option<Dialog> = None;
        match dialog {
            Dialog::NewFile { mut name, mut rate, mut channels, mut bits, mut seconds, then_record } => {
                let mut open = true;
                centered(egui::Window::new("New Audio File").open(&mut open)).show(ctx, |ui| {
                    ui.set_min_width(340.0);
                    egui::Grid::new("newfile").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("File Name:");
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(220.0));
                        ui.end_row();
                        ui.label("Sample Rate:");
                        egui::ComboBox::from_id_source("nf_rate").width(120.0).selected_text(format!("{rate}")).show_ui(ui, |ui| {
                            for r in SAMPLE_RATES {
                                ui.selectable_value(&mut rate, r, format!("{r}"));
                            }
                        });
                        ui.label(RichText::new("Hz").color(TEXT_DIM));
                        ui.end_row();
                        ui.label("Channels:");
                        egui::ComboBox::from_id_source("nf_ch").width(120.0).selected_text(if channels == 1 { "Mono" } else { "Stereo" }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut channels, 1, "Mono");
                            ui.selectable_value(&mut channels, 2, "Stereo");
                        });
                        ui.end_row();
                        ui.label("Bit Depth:");
                        let bits_label = |b: Option<u32>| match b {
                            Some(16) => "16",
                            Some(24) => "24",
                            _ => "32 (float)",
                        };
                        egui::ComboBox::from_id_source("nf_bits").width(120.0).selected_text(bits_label(bits)).show_ui(ui, |ui| {
                            ui.selectable_value(&mut bits, Some(16), "16");
                            ui.selectable_value(&mut bits, Some(24), "24");
                            ui.selectable_value(&mut bits, None, "32 (float)");
                        });
                        ui.end_row();
                        if !then_record {
                            ui.label("Initial Length:");
                            ui.add(egui::DragValue::new(&mut seconds).speed(0.1).range(0.0..=3600.0).suffix(" s"));
                            ui.end_row();
                        }
                    });
                    if then_record {
                        ui.label(RichText::new("Recording starts as soon as you click OK. Input is converted to this format when you stop.").color(TEXT_DIM).size(11.0));
                    }
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.button("  OK  ").clicked() {
                            self.create_new(&name, rate, channels, bits, if then_record { 0.0 } else { seconds });
                            if then_record {
                                self.actions.push(Action::Record);
                            }
                            keep = false;
                        }
                    });
                });
                if open && keep {
                    next = Some(Dialog::NewFile { name, rate, channels, bits, seconds, then_record });
                }
            }
            Dialog::Export { mut format, mut dither, path, selection } => {
                centered(egui::Window::new(if selection { "Save Selection As" } else { "Save As" })).show(ctx, |ui| {
                    if let Some(d) = self.doc() {
                        ui.label(RichText::new(format!("{} • {}", d.name, d.format_label())).color(TEXT_DIM));
                    }
                    ui.add_space(4.0);
                    egui::Grid::new("export").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Format");
                        ui.label("WAV (Microsoft)");
                        ui.end_row();
                        ui.label("Sample type");
                        egui::ComboBox::from_id_source("exp_fmt").selected_text(format.label()).show_ui(ui, |ui| {
                            for f in WavFormat::ALL {
                                ui.selectable_value(&mut format, f, f.label());
                            }
                        });
                        ui.end_row();
                        ui.label("Dither");
                        ui.add_enabled(format != WavFormat::Float32, egui::Checkbox::new(&mut dither, "Triangular (TPDF) dither"));
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("Choose location and save…").color(Color32::WHITE))).clicked() {
                            keep = false;
                            if let Some(p) = path.clone().or_else(|| self.pick_save_path()) {
                                self.write_wav(p, format, dither && format != WavFormat::Float32, selection);
                            }
                        }
                    });
                });
                if keep {
                    next = Some(Dialog::Export { format, dither, path, selection });
                }
            }
            Dialog::Preferences { mut input, mut output, inputs, outputs, mut latency_ms } => {
                let mut open = true;
                centered(egui::Window::new("Preferences: Audio Hardware").open(&mut open)).show(ctx, |ui| {
                    ui.set_min_width(460.0);
                    egui::Grid::new("audiohw").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                        ui.label("Default Input");
                        let shown = input.clone().unwrap_or_else(|| "System default".into());
                        egui::ComboBox::from_id_source("hw_in").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                            ui.selectable_value(&mut input, None, "System default");
                            for n in &inputs {
                                ui.selectable_value(&mut input, Some(n.clone()), n);
                            }
                        });
                        ui.end_row();
                        ui.label("Default Output");
                        let shown = output.clone().unwrap_or_else(|| "System default".into());
                        egui::ComboBox::from_id_source("hw_out").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                            ui.selectable_value(&mut output, None, "System default");
                            for n in &outputs {
                                ui.selectable_value(&mut output, Some(n.clone()), n);
                            }
                        });
                        ui.end_row();
                        ui.label("Output in use");
                        ui.label(RichText::new(format!("{} @ {} Hz, {} ch", self.engine.device_name, self.engine.out_rate, self.engine.out_channels)).color(TEXT_DIM));
                        ui.end_row();
                        ui.label("Recording latency");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut latency_ms).speed(0.5).range(0.0..=1000.0).fixed_decimals(1).suffix(" ms"));
                            ui.label(RichText::new("moves multitrack takes earlier to line up with what you heard").color(TEXT_DIM).size(11.0));
                        });
                        ui.end_row();
                    });
                    ui.label(RichText::new("Recordings are captured at the input device's native rate and converted to the file's sample rate when you stop.").color(TEXT_DIM).size(11.0));
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("  OK  ").color(Color32::WHITE))).clicked() {
                            keep = false;
                            if self.engine.is_recording() {
                                self.stop_recording();
                            }
                            self.engine.input_name = input.clone();
                            let out_changed = self.prefs.output_device != output;
                            self.prefs.input_device = input.clone();
                            self.prefs.output_device = output.clone();
                            self.prefs.rec_offset_ms = latency_ms;
                            self.prefs.save();
                            if out_changed {
                                match self.engine.set_output(output.clone()) {
                                    Ok(()) => self.set_status(format!("Output: {} @ {} Hz", self.engine.device_name, self.engine.out_rate)),
                                    Err(e) => next = Some(Dialog::Message { title: "Output device".into(), text: e }),
                                }
                            }
                        }
                    });
                });
                if open && keep {
                    next = Some(Dialog::Preferences { input, output, inputs, outputs, latency_ms });
                }
            }
            Dialog::MixPaste { mut mode, mut clip_db, mut orig_db } => {
                let mut open = true;
                centered(egui::Window::new("Mix Paste").open(&mut open)).show(ctx, |ui| {
                    egui::Grid::new("mixpaste").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Copied audio volume");
                        ui.add(egui::Slider::new(&mut clip_db, -40.0..=12.0).suffix(" dB").fixed_decimals(1));
                        ui.end_row();
                        ui.label("Existing audio volume");
                        ui.add_enabled(mode == 1 || mode == 3, egui::Slider::new(&mut orig_db, -40.0..=12.0).suffix(" dB").fixed_decimals(1));
                        ui.end_row();
                        ui.label("Mode");
                        ui.vertical(|ui| {
                            ui.radio_value(&mut mode, 0, "Insert");
                            ui.radio_value(&mut mode, 1, "Overlap (Mix)");
                            ui.radio_value(&mut mode, 2, "Overwrite");
                            ui.radio_value(&mut mode, 3, "Modulate");
                        });
                        ui.end_row();
                    });
                    ui.label(RichText::new("Pastes at the cursor or the start of the selection.").color(TEXT_DIM).size(11.0));
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("  OK  ").color(Color32::WHITE))).clicked() {
                            self.actions.push(Action::MixPaste { mode, clip_db, orig_db });
                            keep = false;
                        }
                    });
                });
                if open && keep {
                    next = Some(Dialog::MixPaste { mode, clip_db, orig_db });
                }
            }
            Dialog::Convert { mut rate, mut channels } => {
                centered(egui::Window::new("Convert Sample Type")).show(ctx, |ui| {
                    egui::Grid::new("convert").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Sample rate");
                        egui::ComboBox::from_id_source("cv_rate").selected_text(format!("{rate} Hz")).show_ui(ui, |ui| {
                            for r in SAMPLE_RATES {
                                ui.selectable_value(&mut rate, r, format!("{r} Hz"));
                            }
                        });
                        ui.end_row();
                        ui.label("Channels");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut channels, 1, "Mono");
                            ui.radio_value(&mut channels, 2, "Stereo");
                        });
                        ui.end_row();
                    });
                    ui.label(RichText::new("High-quality windowed-sinc resampling. Mono downmix averages both channels.").color(TEXT_DIM).size(11.0));
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("  OK  ").color(Color32::WHITE))).clicked() {
                            self.actions.push(Action::Convert(rate, channels));
                            keep = false;
                        }
                    });
                });
                if keep {
                    next = Some(Dialog::Convert { rate, channels });
                }
            }
            Dialog::Shortcuts => {
                let mut open = true;
                centered(egui::Window::new("Keyboard Shortcuts")).open(&mut open).show(ctx, |ui| {
                    egui::Grid::new("keys").num_columns(2).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                        for (what, keys) in SHORTCUTS {
                            ui.label(*what);
                            let keys = if cfg!(target_os = "macos") { keys.replace("Ctrl+", "⌘") } else { keys.to_string() };
                            ui.label(RichText::new(keys).monospace().color(WAVE_SEL));
                            ui.end_row();
                        }
                    });
                });
                if open {
                    next = Some(Dialog::Shortcuts);
                }
            }
            Dialog::About => {
                let mut open = true;
                centered(egui::Window::new("About Audemo")).open(&mut open).show(ctx, |ui| {
                    ui.label(RichText::new("Audemo").size(22.0).strong().color(Color32::WHITE));
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(6.0);
                    ui.label(format!("{} effects, all processed in 32-bit float.", self.effects.len()));
                    ui.label(format!("Audio output: {} @ {} Hz", self.engine.device_name, self.engine.out_rate));
                    if let Some(e) = &self.engine.error {
                        ui.label(RichText::new(e).color(WARN));
                    }
                });
                if open {
                    next = Some(Dialog::About);
                }
            }
            Dialog::Message { title, text } => {
                let mut open = true;
                centered(egui::Window::new(title.clone()).open(&mut open)).show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(&text);
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("  OK  ").clicked() {
                            keep = false;
                        }
                    });
                });
                if open && keep {
                    next = Some(Dialog::Message { title, text });
                }
            }
            Dialog::ConfirmClose { doc } => {
                let name = self.docs.get(doc).map(|d| d.name.clone()).unwrap_or_default();
                centered(egui::Window::new("Save changes?")).show(ctx, |ui| {
                    ui.label(format!("Save changes to “{name}” before closing?"));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.active = Some(doc);
                            self.actions.push(Action::Save);
                            keep = false;
                        }
                        if ui.button("Don't Save").clicked() {
                            self.actions.push(Action::ForceClose(doc));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if keep {
                    next = Some(Dialog::ConfirmClose { doc });
                }
            }
            Dialog::ConfirmQuit => {
                let unsaved: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|s| s.dirty)
                    .map(|s| format!("{} (session)", s.name))
                    .chain(self.docs.iter().filter(|d| d.dirty).map(|d| d.name.clone()))
                    .collect();
                centered(egui::Window::new("Quit Audemo?")).show(ctx, |ui| {
                    ui.label("These files have unsaved changes:");
                    for n in &unsaved {
                        ui.label(RichText::new(format!("•  {n}")).color(WARN));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save All and Quit").on_hover_text("Save everything (you'll be asked to name new files), then quit").clicked() {
                            if self.save_everything() {
                                self.allow_quit = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                keep = false;
                            } else {
                                self.set_status("Some files weren't saved, so Audemo is still open.");
                            }
                        }
                        if ui.button("Quit without saving").clicked() {
                            self.allow_quit = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if keep {
                    next = Some(Dialog::ConfirmQuit);
                }
            }
            Dialog::NewSession { mut name, mut rate, mut tracks } => {
                let mut open = true;
                let mut create = false;
                centered(egui::Window::new("New Multitrack Session").open(&mut open)).show(ctx, |ui| {
                    ui.set_min_width(340.0);
                    egui::Grid::new("newsession").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Session Name:");
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(220.0));
                        ui.end_row();
                        ui.label("Sample Rate:");
                        egui::ComboBox::from_id_source("ns_rate").width(120.0).selected_text(format!("{rate} Hz")).show_ui(ui, |ui| {
                            for r in SAMPLE_RATES {
                                ui.selectable_value(&mut rate, r, format!("{r} Hz"));
                            }
                        });
                        ui.end_row();
                        ui.label("Tracks:");
                        ui.add(egui::DragValue::new(&mut tracks).range(1..=64));
                        ui.end_row();
                        ui.label("Master:");
                        ui.label(RichText::new("Stereo").color(TEXT_DIM));
                        ui.end_row();
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("    OK    ").clicked() {
                            create = true;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if create {
                    let n = if name.trim().is_empty() { "Untitled Session".to_string() } else { name.trim().to_string() };
                    self.create_session(n, rate, tracks);
                } else if keep && open {
                    next = Some(Dialog::NewSession { name, rate, tracks });
                }
            }
            Dialog::ConfirmCloseSession { session } => {
                let name = self.sessions.get(session).map(|s| s.name.clone()).unwrap_or_default();
                centered(egui::Window::new("Save changes?")).show(ctx, |ui| {
                    ui.label(format!("Save changes to the session \"{name}\" before closing?"));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.active_session = Some(session);
                            self.save_session(false);
                            if !self.sessions.get(session).map(|s| s.dirty).unwrap_or(true) {
                                self.actions.push(Action::MtCloseSession(session));
                            }
                            keep = false;
                        }
                        if ui.button("Don't Save").clicked() {
                            if let Some(s) = self.sessions.get_mut(session) {
                                s.dirty = false;
                            }
                            self.actions.push(Action::MtCloseSession(session));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if keep {
                    next = Some(Dialog::ConfirmCloseSession { session });
                }
            }
            Dialog::AmplitudeStats { title, rx, stats } => {
                next = self.amp_stats_dialog(ctx, title, rx, stats);
            }
            Dialog::Effect(_) => unreachable!(),
        }
        if self.dialog.is_none() {
            self.dialog = next;
        }
    }

    fn effect_dialog(&mut self, ctx: &egui::Context) {
        let effects = self.effects.clone();
        let (sr, scope) = match self.doc() {
            Some(d) => {
                let scope = match d.sel_range() {
                    Some((a, b)) => format!("Selection {} – {}", format_time(a as f64, d.sample_rate), format_time(b as f64, d.sample_rate)),
                    None => "Entire file".to_string(),
                };
                (d.sample_rate, scope)
            }
            None => (48000, String::new()),
        };
        let previewing_now = self.engine.is_playing_tag(PREVIEW_TAG);
        let Some(Dialog::Effect(d)) = &mut self.dialog else { return };
        let def: &EffectDef = &effects[d.idx];
        let rack_mode = d.rack_slot.is_some();
        let scope = if rack_mode {
            format!("Effects Rack slot {}", d.rack_slot.unwrap() + 1)
        } else if def.generator {
            if scope.starts_with("Selection") { format!("Replaces the {}", scope.to_lowercase()) } else { "Inserts at the cursor".to_string() }
        } else {
            format!("Applies to: {scope}")
        };
        let mut open = true;
        let mut apply = false;
        let mut close = false;
        let mut toggle_preview = false;
        let mut bypass_changed = false;
        let before = d.params.clone();
        egui::Window::new(format!("Effect - {}", def.name))
            .id(egui::Id::new("effect_window"))
            .collapsible(false)
            .resizable(false)
            .default_pos(ctx.screen_rect().center() - vec2(300.0, 260.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(560.0);
                ui.label(RichText::new(def.description).color(TEXT_DIM));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Presets:");
                    let mut chosen: Option<String> = None;
                    egui::ComboBox::from_id_source("presets").width(240.0).selected_text(d.preset.clone()).show_ui(ui, |ui| {
                        if ui.selectable_label(d.preset == "(Default)", "(Default)").clicked() {
                            chosen = Some("(Default)".into());
                        }
                        for (name, _) in &def.presets {
                            if ui.selectable_label(d.preset == *name, *name).clicked() {
                                chosen = Some(name.to_string());
                            }
                        }
                    });
                    if let Some(c) = chosen {
                        d.params = def.preset_params(&c).unwrap_or_else(|| def.default_params());
                        d.preset = c;
                    }
                    if ui.small_button("Reset").on_hover_text("Restore default settings").clicked() {
                        d.params = def.default_params();
                        d.preset = "(Default)".into();
                    }
                });
                ui.add_space(4.0);
                if let Some(resp) = def.response {
                    response_curve(ui, def, &mut d.params, &mut d.drag_handle, sr, resp);
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                    param_grid(ui, def, &mut d.params, sr);
                });
                if let Some(e) = &d.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(WARN));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if rack_mode {
                        ui.label(RichText::new("Changes are heard live while the rack's master power is on.").color(TEXT_DIM).size(11.0));
                    } else {
                        let label = if d.previewing { "Stop Preview" } else { "Preview" };
                        if ui.add(egui::Button::new(label).min_size(vec2(118.0, 26.0))).on_hover_text("Loop the processed selection (Space)").clicked() {
                            toggle_preview = true;
                        }
                        if ui.checkbox(&mut d.bypass, "Bypass").on_hover_text("Hear the unprocessed audio while previewing").changed() {
                            bypass_changed = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                        let label = if rack_mode { "     OK     " } else { "    Apply    " };
                        if ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE)).min_size(vec2(90.0, 26.0))).clicked() {
                            apply = true;
                        }
                    });
                });
                ui.label(RichText::new(scope.clone()).color(TEXT_DIM).size(11.0));
            });
        let live_update = if rack_mode && d.params != before { d.rack_slot.map(|s| (s, d.params.clone())) } else { None };
        if d.params != before {
            d.last_change = Instant::now();
            if def.presets.iter().all(|(n, _)| def.preset_params(n).as_ref() != Some(&d.params)) && d.params != def.default_params() {
                d.preset = "(Custom)".into();
            }
        }
        if toggle_preview {
            d.previewing = !d.previewing;
            if !d.previewing {
                self.engine.stop();
            } else {
                d.rendered = None;
                d.last_change = Instant::now().checked_sub(std::time::Duration::from_secs(1)).unwrap_or_else(Instant::now);
            }
        }
        if bypass_changed && previewing_now {
            let buf = if d.bypass { d.dry.clone() } else { d.wet.clone() };
            if let Some(b) = buf {
                let len = b.first().map(|c| c.len()).unwrap_or(0) as f64;
                self.engine.replace_buffer(b, sr, 0.0, len);
            }
        }
        if apply && rack_mode {
            let (slot, p) = (d.rack_slot.unwrap(), d.params.clone());
            self.close_effect_dialog();
            if let Some(s) = self.rack.get_mut(slot) {
                s.params = p;
            }
            self.rack_touched();
        } else if apply {
            let p = d.params.clone();
            let idx = d.idx;
            self.actions.push(Action::ApplyEffect(idx, p));
        } else if !open || close {
            self.close_effect_dialog();
        }
        if let Some((slot, p)) = live_update {
            if let Some(s) = self.rack.get_mut(slot) {
                s.params = p;
            }
            self.rack_touched();
        }
    }
}

/// Graphic EQs: a row of vertical faders, one per band, like a hardware EQ.
fn graphic_eq_faders(ui: &mut Ui, def: &EffectDef, params: &mut Params) {
    let bands: Vec<_> = def.params.iter().filter(|p| p.key.starts_with('b')).collect();
    let w = ((ui.available_width() - 8.0) / bands.len() as f32).clamp(16.0, 40.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for pd in bands {
            ui.allocate_ui(vec2(w, 190.0), |ui| {
                ui.vertical_centered(|ui| {
                    let mut v = params.f(pd.key);
                    ui.label(RichText::new(format!("{v:+.0}")).font(FontId::monospace(9.0)).color(HOT));
                    let r = ui.add(egui::Slider::new(&mut v, -20.0..=20.0).vertical().show_value(false));
                    if r.changed() {
                        params.set(pd.key, Value::F(v.round()));
                    }
                    if r.double_clicked() {
                        params.set(pd.key, Value::F(0.0));
                    }
                    let short = pd.label.replace(" kHz", "k").replace(" Hz", "");
                    ui.label(RichText::new(short).font(FontId::proportional(9.0)).color(TEXT_DIM));
                });
            });
        }
    });
    ui.add_space(6.0);
    egui::Grid::new(("geq_master", def.id)).num_columns(2).show(ui, |ui| {
        ui.label("Master gain");
        let mut v = params.f("master");
        if ui.add(egui::Slider::new(&mut v, -20.0..=20.0).suffix(" dB")).changed() {
            params.set("master", Value::F(v));
        }
        ui.end_row();
    });
}

fn param_grid(ui: &mut Ui, def: &EffectDef, params: &mut Params, _sr: u32) {
    if def.id.starts_with("graphic_eq") {
        return graphic_eq_faders(ui, def, params);
    }
    let mut group = "";
    egui::Grid::new(("params", def.id)).num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
        for pd in &def.params {
            if pd.group != group {
                group = pd.group;
                if !group.is_empty() {
                    ui.label(RichText::new(group).font(bold(12.5)).color(Color32::WHITE));
                    ui.end_row();
                }
            }
            ui.label(pd.label);
            match &pd.kind {
                Kind::Float { min, max, unit, log, decimals, .. } => {
                    let mut v = params.f(pd.key);
                    let mut s = egui::Slider::new(&mut v, *min..=*max).logarithmic(*log).fixed_decimals(*decimals);
                    if !unit.is_empty() {
                        s = s.suffix(format!(" {unit}"));
                    }
                    if *log && *min > 0.0 {
                        s = s.smallest_positive(*min as f64);
                    }
                    if ui.add(s).changed() {
                        params.set(pd.key, Value::F(v));
                    }
                }
                Kind::Choice { options, .. } => {
                    let mut c = params.c(pd.key).min(options.len().saturating_sub(1));
                    let before = c;
                    egui::ComboBox::from_id_source(("choice", def.id, pd.key)).width(220.0).selected_text(options[c]).show_ui(ui, |ui| {
                        for (i, o) in options.iter().enumerate() {
                            ui.selectable_value(&mut c, i, *o);
                        }
                    });
                    if c != before {
                        params.set(pd.key, Value::C(c));
                    }
                }
                Kind::Toggle { .. } => {
                    let mut b = params.b(pd.key);
                    if ui.checkbox(&mut b, "").changed() {
                        params.set(pd.key, Value::B(b));
                    }
                }
                Kind::Text { .. } => {
                    let mut t = params.s(pd.key);
                    if ui.add(egui::TextEdit::singleline(&mut t).desired_width(220.0)).changed() {
                        params.set(pd.key, Value::S(t));
                    }
                }
            }
            ui.end_row();
        }
    });
}

/// Frequency-response graph with draggable band handles (log frequency axis).
fn response_curve(
    ui: &mut Ui,
    def: &EffectDef,
    params: &mut Params,
    drag: &mut Option<usize>,
    sr: u32,
    resp: crate::dsp::effects::ResponseFn,
) {
    let size = vec2(ui.available_width().max(540.0), 190.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, LANE_BG);
    let fmin = 20.0f32;
    let fmax = (sr as f32 / 2.0).min(22000.0);
    let range_db = 24.0f32;
    let fx = |f: f32| rect.left() + ((f / fmin).log10() / (fmax / fmin).log10()) * rect.width();
    let xf = |x: f32| fmin * (fmax / fmin).powf(((x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    let dy = |db: f32| rect.center().y - db / range_db * (rect.height() * 0.5 - 8.0);
    let yd = |y: f32| (rect.center().y - y) / (rect.height() * 0.5 - 8.0) * range_db;
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0] {
        if f > fmax {
            continue;
        }
        let x = fx(f);
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0_f32, LANE_GRID));
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        p.text(pos2(x + 2.0, rect.bottom() - 2.0), Align2::LEFT_BOTTOM, label, FontId::monospace(9.5), TEXT_DIM);
    }
    for db in [-18.0, -12.0, -6.0, 0.0, 6.0, 12.0, 18.0] {
        let y = dy(db);
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0_f32, if db == 0.0 { BORDER } else { LANE_GRID }));
        p.text(pos2(rect.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0}"), FontId::monospace(9.0), TEXT_DIM);
    }
    let n = (rect.width() as usize).max(2);
    let pts: Vec<_> = (0..n)
        .map(|i| {
            let x = rect.left() + i as f32 * rect.width() / (n - 1) as f32;
            let db = resp(params, sr as f32, xf(x)).clamp(-range_db * 1.5, range_db * 1.5);
            pos2(x, dy(db).clamp(rect.top(), rect.bottom()))
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(2.0_f32, WAVE)));

    // Handles.
    let find_range = |key: &str| {
        def.params.iter().find(|d| d.key == key).and_then(|d| match d.kind {
            Kind::Float { min, max, .. } => Some((min, max)),
            _ => None,
        })
    };
    let mut handle_pos = Vec::new();
    for (i, (fk, gk)) in def.handles.iter().enumerate() {
        let f = params.f(fk).clamp(fmin, fmax);
        let g = params.f(gk);
        let c = pos2(fx(f), dy(g));
        handle_pos.push(c);
        let hot = *drag == Some(i);
        p.circle_filled(c, if hot { 8.0 } else { 7.0 }, if hot { ACCENT } else { Color32::from_rgb(0x2a, 0x6a, 0x52) });
        p.circle_stroke(c, 7.0, Stroke::new(1.0_f32, WAVE_SEL));
        p.text(c, Align2::CENTER_CENTER, format!("{}", i + 1), FontId::proportional(10.0), Color32::WHITE);
    }
    if response.drag_started() {
        if let Some(pt) = response.interact_pointer_pos() {
            *drag = handle_pos
                .iter()
                .enumerate()
                .map(|(i, c)| (i, c.distance(pt)))
                .filter(|(_, d)| *d < 16.0)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .map(|(i, _)| i);
        }
    }
    if response.dragged() {
        if let (Some(i), Some(pt)) = (*drag, response.interact_pointer_pos()) {
            let (fk, gk) = def.handles[i];
            let mut f = xf(pt.x);
            if let Some((lo, hi)) = find_range(fk) {
                f = f.clamp(lo, hi);
            }
            let mut g = yd(pt.y);
            if let Some((lo, hi)) = find_range(gk) {
                g = g.clamp(lo, hi);
            }
            params.set(fk, Value::F(f.round()));
            params.set(gk, Value::F((g * 10.0).round() / 10.0));
        }
    }
    if response.drag_stopped() {
        *drag = None;
    }
    if !def.handles.is_empty() {
        p.text(
            pos2(rect.right() - 6.0, rect.top() + 4.0),
            Align2::RIGHT_TOP,
            "Drag the numbered points",
            FontId::proportional(10.0),
            TEXT_DIM,
        );
    }
}
