//! Generic parameter editor for a plug-in in an effect window: one control
//! per parameter, with the plug-in's own value text.

use eframe::egui::{self, RichText, Ui};

use super::{vst3, Plugin};
use crate::dsp::params::{Params, Value};
use crate::theme::*;

pub struct PluginEditor {
    /// The plug-in's own window. Declared first so it closes before the
    /// instance it belongs to is destroyed.
    window: Option<Box<dyn super::view::Window>>,
    pub plugin: Plugin,
    cid: String,
    sample_rate: u32,
    params: Vec<vst3::ParamInfo>,
    filter: String,
    /// The state text last written to (or read from) the effect's params.
    state: String,
}

impl PluginEditor {
    /// An editing instance for the plug-in in `p` (UI thread).
    pub fn open(p: &Params, sample_rate: u32) -> Result<PluginEditor, String> {
        let cid = p.s("plugin");
        let state = p.s("state");
        let mut plugin = Plugin::create(&cid, &state, sample_rate, 2, 1024, false)?;
        let params = plugin.get().params();
        Ok(PluginEditor { window: None, plugin, cid, sample_rate, params, filter: String::new(), state })
    }

    pub fn name(&mut self) -> String {
        self.plugin.get().name()
    }

    pub fn latency(&mut self) -> usize {
        self.plugin.get().latency()
    }

    /// Whether the plug-in can show its own window here.
    pub fn has_window(&mut self) -> bool {
        self.plugin.get().has_window()
    }

    pub fn window_open(&self) -> bool {
        self.window.is_some()
    }

    /// Show or hide the plug-in's own window.
    pub fn toggle_window(&mut self) -> Result<(), String> {
        if self.window.take().is_some() {
            return Ok(());
        }
        let title = self.name();
        self.window = Some(self.plugin.get().open_window(&title)?);
        Ok(())
    }

    /// Close the window once the user has closed it. Call every frame.
    pub fn poll_window(&mut self) {
        if self.window.as_ref().map(|w| w.closed()).unwrap_or(false) {
            self.window = None;
        }
    }

    /// Pick up a state set from outside (preset, Reset).
    fn sync_from(&mut self, p: &Params) {
        let s = p.s("state");
        if s == self.state {
            return;
        }
        if s.is_empty() {
            // Back to defaults: a fresh instance is the only reliable way.
            if let Ok(fresh) = Plugin::create(&self.cid, "", self.sample_rate, 2, 1024, false) {
                let reopen = self.window.take().is_some();
                self.plugin = fresh;
                if reopen {
                    let _ = self.toggle_window();
                }
                self.params = self.plugin.get().params();
            }
        } else {
            self.plugin.get().set_state_text(&s);
        }
        self.state = s;
    }

    /// Write the instance's state to the effect's settings, with the
    /// parameter changes that led to it (for running instances).
    fn store(&mut self, p: &mut Params, edits: &[(u32, f64)]) {
        let s = self.plugin.state();
        if s != self.state {
            p.set("edits", Value::S(super::encode_edits(&self.state, edits)));
            p.set("state", Value::S(s.clone()));
            self.state = s;
        }
    }

    /// Draw the controls; edits update `p` (the effect's settings).
    pub fn ui(&mut self, ui: &mut Ui, p: &mut Params) {
        self.sync_from(p);
        // Edits made in the plug-in's own window.
        if let Some(edits) = self.plugin.get().take_touched() {
            self.store(p, &edits);
        }
        let visible: Vec<usize> = self
            .params
            .iter()
            .enumerate()
            .filter(|(_, q)| q.flags & vst3::PARAM_HIDDEN == 0 && q.flags & vst3::PARAM_BYPASS == 0)
            .filter(|(_, q)| self.filter.is_empty() || q.title.to_lowercase().contains(&self.filter.to_lowercase()))
            .map(|(i, _)| i)
            .collect();
        if self.params.len() > 12 {
            ui.horizontal(|ui| {
                ui.label("Find");
                ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(200.0));
                ui.label(RichText::new(format!("{} parameters", self.params.len())).color(TEXT_DIM()).size(11.0));
            });
        }
        if self.params.is_empty() {
            ui.label(RichText::new("This plug-in has no parameters to show.").color(TEXT_DIM()));
            return;
        }
        let mut changed: Vec<(u32, f64)> = Vec::new();
        egui::Grid::new(("plugin_params", self.cid.as_str())).num_columns(3).spacing([12.0, 5.0]).show(ui, |ui| {
            for i in visible {
                let q = self.params[i].clone();
                let inst = self.plugin.get();
                let v = inst.get_param(q.id);
                let read_only = q.flags & vst3::PARAM_READ_ONLY != 0;
                let title = if q.units.is_empty() { q.title.clone() } else { format!("{} ({})", q.title, q.units) };
                ui.label(title);
                let text = inst.param_text(q.id, v);
                ui.add_enabled_ui(!read_only, |ui| {
                    if q.steps == 1 {
                        let mut on = v >= 0.5;
                        if ui.checkbox(&mut on, "").changed() {
                            changed.push((q.id, if on { 1.0 } else { 0.0 }));
                        }
                    } else if q.steps > 1 && q.steps <= 64 {
                        let steps = q.steps;
                        let cur = (v * steps as f64).round() as i32;
                        let mut pick = cur;
                        egui::ComboBox::from_id_source(("pp", q.id)).width(220.0).selected_text(text.clone()).show_ui(ui, |ui| {
                            for k in 0..=steps {
                                let label = inst.param_text(q.id, k as f64 / steps as f64);
                                ui.selectable_value(&mut pick, k, label);
                            }
                        });
                        if pick != cur {
                            changed.push((q.id, pick as f64 / steps as f64));
                        }
                    } else {
                        let mut x = v;
                        let r = ui.add(egui::Slider::new(&mut x, 0.0..=1.0).show_value(false));
                        if r.changed() {
                            changed.push((q.id, x));
                        }
                        if r.double_clicked() {
                            changed.push((q.id, q.default));
                        }
                    }
                });
                ui.label(RichText::new(text).color(HOT()).monospace());
                ui.end_row();
            }
        });
        if !changed.is_empty() {
            let inst = self.plugin.get();
            for (id, v) in &changed {
                inst.set_param(*id, *v);
            }
            self.store(p, &changed);
        }
    }
}
