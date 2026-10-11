//! Effect windows and the smaller modal dialogs.

use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Pos2, RichText, Sense, Shape, Stroke, Ui};

use crate::app::{Action, App, Dialog, SAMPLE_RATES};
use crate::dsp::effects::EffectDef;
use crate::dsp::params::{Kind, Params, Value};
use crate::dsp::util::format_time;
use crate::engine::PREVIEW_TAG;
use crate::export::{Container, WavFormat, AAC_BITRATES, MP3_BITRATES};
use crate::theme::*;


/// A dialog that opens centred and can then be dragged by its title bar.
fn centered<'a>(ctx: &egui::Context, w: egui::Window<'a>) -> egui::Window<'a> {
    w.collapsible(false)
        .resizable(false)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.screen_rect().center() - vec2(0.0, 40.0))
        .constrain(true)
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
                centered(ctx, egui::Window::new("New Audio File").open(&mut open)).show(ctx, |ui| {
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
                        ui.label(RichText::new("Hz").color(TEXT_DIM()));
                        ui.end_row();
                        ui.label("Channels:");
                        egui::ComboBox::from_id_source("nf_ch").width(120.0).selected_text(match channels { 1 => "Mono", 6 => "5.1", 8 => "7.1", _ => "Stereo" }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut channels, 1, "Mono");
                            ui.selectable_value(&mut channels, 2, "Stereo");
                            ui.selectable_value(&mut channels, 6, "5.1");
                            ui.selectable_value(&mut channels, 8, "7.1");
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
                        ui.label(RichText::new("Recording starts as soon as you click OK. Input is converted to this format when you stop.").color(TEXT_DIM()).size(11.0));
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
            Dialog::Export { mut settings, path, selection } => {
                let mut open_meta = false;
                centered(ctx, egui::Window::new(if selection { "Save Selection As" } else { "Save As" })).show(ctx, |ui| {
                    ui.set_min_width(420.0);
                    let (frames, ch, rate) = match self.doc() {
                        Some(d) => {
                            ui.label(RichText::new(format!("{} • {}", d.name, d.format_label())).color(TEXT_DIM()));
                            let frames = if selection { let (a, b) = d.target_range(); b - a } else { d.len() };
                            (frames, d.n_ch(), d.sample_rate)
                        }
                        None => (0, 2, 48000),
                    };
                    ui.add_space(4.0);
                    egui::Grid::new("export").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Format");
                        egui::ComboBox::from_id_source("exp_container").width(220.0).selected_text(settings.container.label()).show_ui(ui, |ui| {
                            for c in Container::ALL {
                                ui.selectable_value(&mut settings.container, c, c.label());
                            }
                        });
                        ui.end_row();
                        match settings.container {
                            Container::Wav => {
                                ui.label("Sample type");
                                egui::ComboBox::from_id_source("exp_fmt").width(220.0).selected_text(settings.wav.label()).show_ui(ui, |ui| {
                                    for f in WavFormat::ALL {
                                        ui.selectable_value(&mut settings.wav, f, f.label());
                                    }
                                });
                                ui.end_row();
                            }
                            Container::Flac => {
                                ui.label("Sample type");
                                egui::ComboBox::from_id_source("exp_flac").width(220.0).selected_text(format!("{}-bit integer", settings.flac_bits)).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut settings.flac_bits, 16, "16-bit integer");
                                    ui.selectable_value(&mut settings.flac_bits, 24, "24-bit integer");
                                });
                                ui.end_row();
                            }
                            Container::Mp3 => {
                                ui.label("Mode");
                                ui.horizontal(|ui| {
                                    ui.radio_value(&mut settings.mp3_vbr, false, "Constant bitrate");
                                    ui.radio_value(&mut settings.mp3_vbr, true, "Variable bitrate");
                                });
                                ui.end_row();
                                if settings.mp3_vbr {
                                    ui.label("Quality");
                                    ui.horizontal(|ui| {
                                        let mut q = 9 - settings.mp3_vbr_quality as i32;
                                        ui.add(egui::Slider::new(&mut q, 0..=9).show_value(false));
                                        settings.mp3_vbr_quality = (9 - q).clamp(0, 9) as u8;
                                        ui.label(RichText::new(format!("V{} (≈{} kbps)", settings.mp3_vbr_quality, vbr_kbps(settings.mp3_vbr_quality))).color(TEXT_DIM()));
                                    });
                                } else {
                                    ui.label("Bitrate");
                                    egui::ComboBox::from_id_source("exp_mp3").width(120.0).selected_text(format!("{} kbps", settings.mp3_kbps)).show_ui(ui, |ui| {
                                        for b in MP3_BITRATES {
                                            ui.selectable_value(&mut settings.mp3_kbps, b, format!("{b} kbps"));
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                            Container::M4a => {
                                ui.label("Bitrate");
                                egui::ComboBox::from_id_source("exp_aac").width(120.0).selected_text(format!("{} kbps", settings.aac_kbps)).show_ui(ui, |ui| {
                                    for b in AAC_BITRATES {
                                        ui.selectable_value(&mut settings.aac_kbps, b, format!("{b} kbps"));
                                    }
                                });
                                ui.end_row();
                            }
                        }
                        if settings.integer() {
                            ui.label("Dither");
                            ui.checkbox(&mut settings.dither, "Triangular (TPDF) dither");
                            ui.end_row();
                        }
                        ui.label("Metadata");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut settings.include_meta, if settings.container == Container::Wav { "Include markers and other metadata" } else { "Include metadata" });
                            if ui.small_button("Edit…").on_hover_text("Open the Metadata panel").clicked() {
                                open_meta = true;
                            }
                        });
                        ui.end_row();
                        ui.label("Estimated size");
                        ui.label(RichText::new(estimate_size(&settings, frames, ch, rate)).color(TEXT_DIM()));
                        ui.end_row();
                    });
                    if settings.container.lossy() {
                        let note = if rate > 48000 && settings.container == Container::Mp3 {
                            format!("MP3 is lossy and tops out at 48 kHz, so this {rate} Hz audio is converted to 48 kHz. Keep a WAV or FLAC copy for editing.")
                        } else {
                            "Lossy formats discard detail on every save. Keep a WAV or FLAC copy for further editing.".to_string()
                        };
                        ui.label(RichText::new(note).color(TEXT_DIM()).size(11.0));
                    }
                    if ch > 2 && settings.container.lossy() {
                        ui.label(RichText::new("Only the first two channels are encoded.").color(WARN()).size(11.0));
                    }
                    ui.add_space(6.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("Choose location and save…").color(Color32::WHITE))).clicked() {
                            keep = false;
                            let target = path.clone().filter(|p| Container::from_path(p) == Some(settings.container)).or_else(|| self.pick_save_path(settings.container));
                            if let Some(p) = target {
                                self.prefs.export = Some(settings);
                                self.begin_save(p, settings, selection);
                            }
                        }
                    });
                });
                if open_meta {
                    self.show_panel(crate::app::Panel::Metadata);
                }
                if keep {
                    next = Some(Dialog::Export { settings, path, selection });
                }
            }
            Dialog::Preferences(d) => {
                if let Some(d) = self.preferences_window(ctx, d) {
                    next = Some(Dialog::Preferences(d));
                }
            }
            Dialog::MixPaste { mut mode, mut clip_db, mut orig_db } => {
                let mut open = true;
                centered(ctx, egui::Window::new("Mix Paste").open(&mut open)).show(ctx, |ui| {
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
                    ui.label(RichText::new("Pastes at the cursor or the start of the selection.").color(TEXT_DIM()).size(11.0));
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
                centered(ctx, egui::Window::new("Convert Sample Type")).show(ctx, |ui| {
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
                            ui.radio_value(&mut channels, 6, "5.1");
                            ui.radio_value(&mut channels, 8, "7.1");
                        });
                        ui.end_row();
                    });
                    ui.label(RichText::new("High-quality windowed-sinc resampling. Mono downmix averages the channels; surround mixes down to stereo with the centre and surrounds at -3 dB; stereo goes to the front left and right of 5.1 or 7.1.").color(TEXT_DIM()).size(11.0));
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
            Dialog::PluginManager => {
                if self.plugin_manager(ctx) {
                    next = Some(Dialog::PluginManager);
                }
            }
            Dialog::Shortcuts(mut ed) => {
                if self.shortcut_editor(ctx, &mut ed) {
                    next = Some(Dialog::Shortcuts(ed));
                }
            }
            Dialog::ClipStretch { mut percent } => {
                let mut open = true;
                let mut go = false;
                centered(ctx, egui::Window::new("Stretch Clip")).open(&mut open).show(ctx, |ui| {
                    ui.label("New length, as a percentage of the clip's original length. Pitch is kept; the stretched audio is rendered as a new source.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Stretch");
                        ui.add(egui::Slider::new(&mut percent, 25.0..=400.0).suffix(" %").fixed_decimals(1));
                    });
                    ui.horizontal(|ui| {
                        for p in [50.0f32, 80.0, 90.0, 110.0, 125.0, 150.0, 200.0] {
                            if ui.small_button(format!("{p:.0}%")).clicked() {
                                percent = p;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    if ui.button("   Stretch   ").clicked() {
                        go = true;
                    }
                });
                if go {
                    self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::Stretch(percent)));
                } else if open {
                    next = Some(Dialog::ClipStretch { percent });
                }
            }
            Dialog::MatchClipVolume { mut mode, mut target } => {
                let mut open = true;
                let mut go = false;
                centered(ctx, egui::Window::new("Match Clip Volume")).open(&mut open).show(ctx, |ui| {
                    ui.label("Set each selected clip's gain so they all measure the same.");
                    ui.add_space(6.0);
                    egui::Grid::new("mcv").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("Match to");
                        let names = ["Loudness (LUFS)", "Peak amplitude (dBFS)", "Total RMS (dB)"];
                        egui::ComboBox::from_id_source("mcv_mode").width(200.0).selected_text(names[mode.min(2)]).show_ui(ui, |ui| {
                            for (i, n) in names.iter().enumerate() {
                                if ui.selectable_value(&mut mode, i, *n).changed() {
                                    target = [-23.0, -1.0, -20.0][i];
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Target");
                        ui.add(egui::DragValue::new(&mut target).speed(0.1).range(-60.0..=0.0).fixed_decimals(1).suffix(if mode == 0 { " LUFS" } else { " dB" }));
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    if ui.button("   Match   ").clicked() {
                        go = true;
                    }
                });
                if go {
                    self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::Match { mode, target }));
                } else if open {
                    next = Some(Dialog::MatchClipVolume { mode, target });
                }
            }
            Dialog::Tempo { mut bpm, mut beats_per_bar, mut metronome_db } => {
                let mut open = true;
                let mut go = false;
                centered(ctx, egui::Window::new("Tempo and Metronome")).open(&mut open).show(ctx, |ui| {
                    egui::Grid::new("tempo").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("Tempo");
                        ui.add(egui::DragValue::new(&mut bpm).speed(0.2).range(20.0..=400.0).fixed_decimals(2).suffix(" BPM"));
                        ui.end_row();
                        ui.label("Beats per bar");
                        ui.add(egui::DragValue::new(&mut beats_per_bar).range(1..=32));
                        ui.end_row();
                        ui.label("Metronome volume");
                        ui.add(egui::Slider::new(&mut metronome_db, -40.0..=6.0).suffix(" dB"));
                        ui.end_row();
                    });
                    ui.label(RichText::new("The beat grid (Multitrack > Show Beat Grid) and snapping follow the tempo; the metronome plays during playback and recording but is never mixed down.").color(TEXT_DIM()).size(11.0));
                    if ui.button("     OK     ").clicked() {
                        go = true;
                    }
                });
                if go {
                    self.actions.push(Action::MtClip(crate::mt_ui::ClipCmd::SetTempo { bpm, beats_per_bar, metronome_db }));
                } else if open {
                    next = Some(Dialog::Tempo { bpm, beats_per_bar, metronome_db });
                }
            }
            Dialog::About => {
                let mut open = true;
                centered(ctx, egui::Window::new("About Audemo")).open(&mut open).show(ctx, |ui| {
                    ui.label(RichText::new("Audemo").size(22.0).strong().color(Color32::WHITE));
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.add_space(6.0);
                    ui.label(format!("{} effects, all processed in 32-bit float.", self.effects.len()));
                    ui.label(format!("Audio output: {} @ {} Hz", self.engine.device_name, self.engine.out_rate));
                    if let Some(e) = &self.engine.error {
                        ui.label(RichText::new(e).color(WARN()));
                    }
                });
                if open {
                    next = Some(Dialog::About);
                }
            }
            Dialog::Update(r) => {
                let mut open = true;
                centered(ctx, egui::Window::new("Update Available").open(&mut open)).show(ctx, |ui| {
                    ui.set_max_width(480.0);
                    ui.label(RichText::new(format!("Audemo {} is available", r.version)).size(17.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new(format!("You have version {}.", env!("CARGO_PKG_VERSION"))).color(TEXT_DIM()));
                    if !r.notes.trim().is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("What's new").strong());
                        egui::Frame::none().fill(BG_LIST()).inner_margin(egui::Margin::same(6.0)).show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                                ui.label(RichText::new(r.notes.trim()).size(12.0));
                            });
                        });
                    }
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.prefs.check_updates, "Check for updates at startup");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Download…").color(Color32::WHITE)).fill(ACCENT())).on_hover_text("Open the release page to download the installer").clicked() {
                            if let Err(e) = crate::update::open_url(&r.page) {
                                self.set_status(e);
                            }
                            keep = false;
                        }
                        if ui.button("Skip This Version").on_hover_text("Don't mention this version at startup again").clicked() {
                            self.prefs.skip_version = r.version.clone();
                            keep = false;
                        }
                        if ui.button("Later").clicked() {
                            keep = false;
                        }
                    });
                });
                if !open {
                    keep = false;
                }
                if !keep {
                    self.prefs.save();
                } else {
                    next = Some(Dialog::Update(r));
                }
            }
            Dialog::Message { title, text } => {
                let mut open = true;
                centered(ctx, egui::Window::new(title.clone()).open(&mut open)).show(ctx, |ui| {
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
                centered(ctx, egui::Window::new("Save changes?")).show(ctx, |ui| {
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
                centered(ctx, egui::Window::new("Quit Audemo?")).show(ctx, |ui| {
                    ui.label("These files have unsaved changes:");
                    for n in &unsaved {
                        ui.label(RichText::new(format!("•  {n}")).color(WARN()));
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
                centered(ctx, egui::Window::new("New Multitrack Session").open(&mut open)).show(ctx, |ui| {
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
                        ui.label(RichText::new("Stereo").color(TEXT_DIM()));
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
                centered(ctx, egui::Window::new("Save changes?")).show(ctx, |ui| {
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
        let show_window = self.prefs.show_plugin_window;
        let Some(Dialog::Effect(d)) = &mut self.dialog else { return };
        let def: &EffectDef = &effects[d.idx];
        let is_plugin = crate::plugin::is_plugin(def);
        // A plug-in's controls come from an editing instance of it.
        if is_plugin && d.plugin.is_none() && d.error.is_none() {
            match crate::plugin::editor::PluginEditor::open(&d.params, sr) {
                Ok(mut ed) => {
                    // Preferences > Effects: open the plug-in's own window too.
                    if show_window && ed.has_window() {
                        let _ = ed.toggle_window();
                    }
                    d.plugin = Some(ed);
                }
                Err(e) => d.error = Some(e),
            }
        }
        if let Some(ed) = d.plugin.as_mut() {
            ed.poll_window();
            ctx.request_repaint_after(std::time::Duration::from_millis(if ed.window_open() { 30 } else { 200 }));
        }
        let rack_mode = d.rack_slot.is_some() || d.track_fx.is_some();
        let track_mode = d.track_fx.is_some();
        let scope = if let Some((sid, tid, slot_id)) = d.track_fx {
            let (owner, pos) = self
                .sessions
                .iter()
                .find(|s| s.id == sid)
                .and_then(|s| s.rack(tid))
                .map(|(fx, _, name)| (name, fx.iter().position(|f| f.id == slot_id)))
                .unwrap_or_default();
            match pos {
                Some(p) => format!("{owner} effects rack, slot {}", p + 1),
                None => format!("{owner} effects rack (this effect was removed)"),
            }
        } else if rack_mode {
            format!("Effects Rack slot {}", d.rack_slot.unwrap() + 1)
        } else if def.generator {
            if scope.starts_with("Selection") { format!("Replaces the {}", scope.to_lowercase()) } else { "Inserts at the cursor".to_string() }
        } else {
            format!("Applies to: {scope}")
        };
        // A compressor or gate in a track's rack can be keyed by another track.
        let sidechain: Option<(u64, Option<u64>, u64, Option<u64>, Vec<(u64, String)>)> = d.track_fx.filter(|_| crate::dsp::effects::rt::takes_sidechain(def.id)).and_then(|(sid, owner, slot_id)| {
            let s = self.sessions.iter().find(|s| s.id == sid)?;
            let tid = owner.filter(|id| s.tracks.iter().any(|t| t.id == *id))?;
            let current = s.rack(Some(tid))?.0.iter().find(|f| f.id == slot_id)?.sidechain;
            let others = s.tracks.iter().filter(|t| t.id != tid && !t.is_bus()).map(|t| (t.id, t.name.clone())).collect();
            Some((sid, owner, slot_id, current, others))
        });
        let mut sidechain_pick: Option<Option<u64>> = None;
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
                ui.label(RichText::new(def.description).color(TEXT_DIM()));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if is_plugin {
                        if let Some(ed) = d.plugin.as_mut() {
                            if ed.has_window() {
                                let label = if ed.window_open() { "Hide Plug-In Window" } else { "Show Plug-In Window" };
                                if ui.button(label).on_hover_text("The plug-in's own controls").clicked() {
                                    if let Err(e) = ed.toggle_window() {
                                        d.error = Some(e);
                                    }
                                }
                            }
                            let lat = ed.latency();
                            if lat > 0 {
                                ui.label(RichText::new(format!("Latency {lat} samples (compensated when applied)")).color(TEXT_DIM()).size(11.0));
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Reset").on_hover_text("The plug-in's default settings").clicked() {
                                d.params = def.default_params();
                                d.preset = "(Default)".into();
                            }
                        });
                        return;
                    }
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
                if is_plugin {
                    if let Some(ed) = d.plugin.as_mut() {
                        egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                            ed.ui(ui, &mut d.params);
                        });
                    }
                } else {
                    egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                        param_grid(ui, def, &mut d.params, sr);
                    });
                }
                if let Some((_, _, _, current, others)) = &sidechain {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Sidechain input");
                        let name = current.and_then(|id| others.iter().find(|o| o.0 == id)).map(|o| o.1.clone()).unwrap_or_else(|| "None (the track itself)".into());
                        egui::ComboBox::from_id_source("sidechain").width(220.0).selected_text(name).show_ui(ui, |ui| {
                            if ui.selectable_label(current.is_none(), "None (the track itself)").clicked() {
                                sidechain_pick = Some(None);
                            }
                            for (id, n) in others {
                                if ui.selectable_label(*current == Some(*id), n).clicked() {
                                    sidechain_pick = Some(Some(*id));
                                }
                            }
                        });
                    })
                    .response
                    .on_hover_text("Detect the level from another track's clips, e.g. to duck music under a voice");
                }
                if let Some(e) = &d.error {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(WARN()));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if rack_mode {
                        let hint = if track_mode { "Changes are heard live as the session plays (Space)." } else { "Changes are heard live while the rack's master power is on." };
                        ui.label(RichText::new(hint).color(TEXT_DIM()).size(11.0));
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
                ui.label(RichText::new(scope.clone()).color(TEXT_DIM()).size(11.0));
            });
        let live_update = if rack_mode && d.params != before { d.rack_slot.map(|s| (s, d.params.clone())) } else { None };
        let live_track = if d.params != before { d.track_fx.map(|t| (t, d.params.clone())) } else { None };
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
        if let (Some(pick), Some((sid, owner, slot_id, _, _))) = (sidechain_pick, &sidechain) {
            // (The session is changed directly: the dialog is still borrowed.)
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == *sid) {
                s.push_undo("Sidechain");
                if let Some(slot) = s.rack_mut(*owner).and_then(|(fx, _)| fx.iter_mut().find(|f| f.id == *slot_id)) {
                    slot.sidechain = pick;
                }
                s.touch();
            }
        }
        if apply && d.track_fx.is_some() {
            self.close_effect_dialog();
        } else if apply && rack_mode {
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
        if let Some(((sid, tid, slot), p)) = live_track {
            self.mt_set_fx_params(sid, tid, slot, p);
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
                    ui.label(RichText::new(format!("{v:+.0}")).font(FontId::monospace(9.0)).color(HOT()));
                    let r = ui.add(egui::Slider::new(&mut v, -20.0..=20.0).vertical().show_value(false));
                    if r.changed() {
                        params.set(pd.key, Value::F(v.round()));
                    }
                    if r.double_clicked() {
                        params.set(pd.key, Value::F(0.0));
                    }
                    let short = pd.label.replace(" kHz", "k").replace(" Hz", "");
                    ui.label(RichText::new(short).font(FontId::proportional(9.0)).color(TEXT_DIM()));
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
    // Drawn curves first, full width, then the other parameters.
    let spline = def.params.iter().any(|p| p.key == "spline") && params.b("spline");
    for pd in def.params.iter().filter(|p| matches!(p.kind, Kind::Curve { .. })) {
        ui.label(RichText::new(pd.label).color(TEXT_DIM()));
        let mut pts = params.curve(pd.key);
        if curve_editor(ui, egui::Id::new(("curve", def.id, pd.key)), &pd.kind, &mut pts, spline) {
            params.set(pd.key, Value::S(crate::dsp::params::curve_text(&pts)));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Click to add a point, drag to move, double-click a point to delete it.").size(11.0).color(TEXT_DIM()));
            if ui.small_button("Reset").clicked() {
                params.set(pd.key, pd.default_value());
            }
        });
        ui.add_space(6.0);
    }
    let mut group = "";
    egui::Grid::new(("params", def.id)).num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
        for pd in def.params.iter().filter(|p| !matches!(p.kind, Kind::Curve { .. })) {
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
                Kind::Curve { .. } => {}
            }
            ui.end_row();
        }
    });
}

/// Editor for a drawn curve (Fade/Gain Envelope, FFT Filter …). The end
/// points stay at the ends of the x axis. Returns true when changed.
fn curve_editor(ui: &mut Ui, id: egui::Id, kind: &Kind, pts: &mut Vec<(f32, f32)>, spline: bool) -> bool {
    let Kind::Curve { x_min, x_max, log_x, x_unit, y_min, y_max, y_unit, .. } = *kind else { return false };
    let size = vec2(ui.available_width().max(540.0), 200.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let plot = rect.shrink2(vec2(8.0, 8.0));
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, LANE_BG());
    let to_u = |x: f32| if log_x { (x.max(x_min) / x_min).ln() / (x_max / x_min).ln() } else { (x - x_min) / (x_max - x_min) };
    let from_u = |u: f32| if log_x { x_min * (x_max / x_min).powf(u.clamp(0.0, 1.0)) } else { x_min + u.clamp(0.0, 1.0) * (x_max - x_min) };
    let sx = |x: f32| plot.left() + to_u(x) * plot.width();
    let sy = |y: f32| plot.bottom() - (y - y_min) / (y_max - y_min) * plot.height();
    let xs = |px: f32| from_u((px - plot.left()) / plot.width());
    let ys = |py: f32| (y_min + (plot.bottom() - py) / plot.height() * (y_max - y_min)).clamp(y_min, y_max);
    // Grid.
    let grid = Stroke::new(1.0_f32, Color32::from_white_alpha(18));
    let ticks: Vec<f32> = if log_x { vec![50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] } else { vec![25.0, 50.0, 75.0] };
    for t in ticks.iter().filter(|t| **t > x_min && **t < x_max) {
        let x = sx(*t);
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], grid);
        let label = if *t >= 1000.0 { format!("{}k", t / 1000.0) } else { format!("{t}") };
        p.text(pos2(x + 2.0, plot.bottom() - 2.0), Align2::LEFT_BOTTOM, label, FontId::proportional(9.0), TEXT_DIM());
    }
    for k in 0..=4 {
        let y = y_min + (y_max - y_min) * k as f32 / 4.0;
        let py = sy(y);
        p.line_segment([pos2(plot.left(), py), pos2(plot.right(), py)], grid);
        p.text(pos2(plot.left() + 2.0, py - 1.0), Align2::LEFT_BOTTOM, format!("{y:.0} {y_unit}"), FontId::proportional(9.0), TEXT_DIM());
    }
    // End points pinned to the ends of the axis.
    if pts.is_empty() {
        pts.push((x_min, 0.0f32.clamp(y_min, y_max)));
    }
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    pts[0].0 = x_min;
    if pts.len() == 1 {
        pts.push((x_max, pts[0].1));
    }
    let last = pts.len() - 1;
    pts[last].0 = x_max;
    let mut changed = false;
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    let near = |pts: &[(f32, f32)], pos: Pos2| -> Option<usize> {
        pts.iter().enumerate().map(|(i, (x, y))| (i, pos2(sx(*x), sy(*y)).distance(pos))).filter(|(_, d)| *d < 9.0).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|(i, _)| i)
    };
    let mut drag: Option<usize> = ui.data(|d| d.get_temp(id)).flatten();
    if resp.double_clicked() {
        if let Some(i) = pointer.and_then(|pos| near(pts, pos)).filter(|i| *i != 0 && *i != pts.len() - 1) {
            pts.remove(i);
            changed = true;
        }
        drag = None;
    } else if resp.drag_started() || resp.clicked() {
        if let Some(pos) = pointer {
            drag = match near(pts, pos) {
                Some(i) => Some(i),
                None => {
                    let pt = (xs(pos.x), ys(pos.y));
                    let at = pts.iter().position(|q| q.0 > pt.0).unwrap_or(pts.len() - 1).max(1);
                    pts.insert(at, pt);
                    changed = true;
                    Some(at)
                }
            };
        }
    }
    if resp.dragged() {
        if let (Some(i), Some(pos)) = (drag, pointer) {
            if i < pts.len() {
                let n = pts.len();
                let x = if i == 0 {
                    x_min
                } else if i == n - 1 {
                    x_max
                } else {
                    xs(pos.x).clamp(pts[i - 1].0, pts[i + 1].0)
                };
                pts[i] = (x, ys(pos.y));
                changed = true;
            }
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        drag = None;
    }
    ui.data_mut(|d| d.insert_temp(id, drag));
    // The curve.
    let line: Vec<Pos2> = (0..=240).map(|k| {
        let x = from_u(k as f32 / 240.0);
        pos2(sx(x), sy(crate::dsp::params::eval_curve(pts, x, spline, log_x).clamp(y_min, y_max)))
    }).collect();
    p.add(Shape::line(line, Stroke::new(2.0_f32, WAVE())));
    for (x, y) in pts.iter() {
        p.circle_filled(pos2(sx(*x), sy(*y)), 4.0, HOT());
    }
    if let Some(pos) = resp.hover_pos() {
        let (x, y) = (xs(pos.x), ys(pos.y));
        let xt = if log_x { format!("{x:.0} {x_unit}") } else { format!("{x:.1} {x_unit}") };
        p.text(pos2(plot.right(), plot.top()), Align2::RIGHT_TOP, format!("{xt}   {y:.1} {y_unit}"), FontId::monospace(10.0), TEXT());
    }
    changed
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
    p.rect_filled(rect, 3.0, LANE_BG());
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
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0_f32, LANE_GRID()));
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        p.text(pos2(x + 2.0, rect.bottom() - 2.0), Align2::LEFT_BOTTOM, label, FontId::monospace(9.5), TEXT_DIM());
    }
    for db in [-18.0, -12.0, -6.0, 0.0, 6.0, 12.0, 18.0] {
        let y = dy(db);
        p.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0_f32, if db == 0.0 { BORDER() } else { LANE_GRID() }));
        p.text(pos2(rect.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0}"), FontId::monospace(9.0), TEXT_DIM());
    }
    let n = (rect.width() as usize).max(2);
    let pts: Vec<_> = (0..n)
        .map(|i| {
            let x = rect.left() + i as f32 * rect.width() / (n - 1) as f32;
            let db = resp(params, sr as f32, xf(x)).clamp(-range_db * 1.5, range_db * 1.5);
            pos2(x, dy(db).clamp(rect.top(), rect.bottom()))
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(2.0_f32, WAVE())));

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
        p.circle_filled(c, if hot { 8.0 } else { 7.0 }, if hot { ACCENT() } else { Color32::from_rgb(0x2a, 0x6a, 0x52) });
        p.circle_stroke(c, 7.0, Stroke::new(1.0_f32, WAVE_SEL()));
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
            TEXT_DIM(),
        );
    }
}

/// Typical LAME VBR bitrate for quality `q` (V0–V9).
fn vbr_kbps(q: u8) -> u32 {
    [245, 225, 190, 175, 165, 130, 115, 100, 85, 65][q.min(9) as usize]
}

fn estimate_size(s: &crate::export::ExportSettings, frames: usize, ch: usize, rate: u32) -> String {
    let secs = frames as f64 / rate.max(1) as f64;
    let pcm = |bytes: usize| frames as f64 * ch as f64 * bytes as f64;
    let (bytes, approx) = match s.container {
        Container::Wav => (pcm(match s.wav { WavFormat::Pcm16 => 2, WavFormat::Pcm24 => 3, _ => 4 }), false),
        Container::Flac => (pcm(s.flac_bits as usize / 8) * 0.6, true),
        Container::Mp3 => ((if s.mp3_vbr { vbr_kbps(s.mp3_vbr_quality) } else { s.mp3_kbps }) as f64 * 125.0 * secs, s.mp3_vbr),
        Container::M4a => (s.aac_kbps as f64 * 125.0 * secs, false),
    };
    let mb = bytes / 1_000_000.0;
    let text = if mb >= 1000.0 { format!("{:.2} GB", mb / 1000.0) } else if mb >= 1.0 { format!("{mb:.1} MB") } else { format!("{:.0} KB", bytes / 1000.0) };
    if approx { format!("about {text}") } else { text }
}
