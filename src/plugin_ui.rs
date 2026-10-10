//! Audio Plug-In Manager window and the plug-in menus.

use eframe::egui::{self, Align, Align2, Layout, RichText, Ui};

use crate::app::App;
use crate::dsp::effects::{Category, EffectDef};
use crate::plugin;
use crate::theme::*;

/// Plug-in effects of one format grouped by vendor: (vendor, [(effect index, name)]).
pub fn plugins_by_vendor(effects: &[EffectDef], format: plugin::Format, filter: impl Fn(&EffectDef) -> bool) -> Vec<(String, Vec<(usize, &'static str)>)> {
    let mut groups: Vec<(String, Vec<(usize, &'static str)>)> = Vec::new();
    for (i, e) in effects.iter().enumerate().filter(|(_, e)| e.category == Category::Plugin && plugin::format_of(e) == Some(format) && filter(e)) {
        let v = plugin::vendor_of(e);
        let v = if v.trim().is_empty() { "Other".to_string() } else { v };
        match groups.iter_mut().find(|g| g.0 == v) {
            Some(g) => g.1.push((i, e.name)),
            None => groups.push((v, vec![(i, e.name)])),
        }
    }
    groups.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    for g in groups.iter_mut() {
        g.1.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    }
    groups
}

/// "VST 3" (and on macOS "Audio Units") submenus with vendor submenus of
/// plug-ins. Returns the picked effect.
pub fn plugin_menu(ui: &mut Ui, effects: &[EffectDef], enabled: bool, filter: impl Fn(&EffectDef) -> bool) -> Option<usize> {
    let mut pick = None;
    let formats: &[plugin::Format] = if cfg!(target_os = "macos") { &[plugin::Format::Au, plugin::Format::Vst3] } else { &[plugin::Format::Vst3] };
    for &format in formats {
        let groups = plugins_by_vendor(effects, format, &filter);
        let title = match format {
            plugin::Format::Vst3 => "VST 3",
            plugin::Format::Au => "Audio Units",
        };
        ui.menu_button(title, |ui| {
            if groups.is_empty() {
                ui.label(RichText::new("None found. Use Effects > Audio Plug-In Manager to scan.").color(TEXT_DIM()));
            }
            for (vendor, items) in groups {
                ui.menu_button(vendor, |ui| {
                    for (i, name) in items {
                        if ui.add_enabled(enabled, egui::Button::new(format!("{name}…"))).clicked() {
                            pick = Some(i);
                            ui.close_menu();
                        }
                    }
                });
            }
        });
    }
    pick
}

impl App {
    /// The Audio Plug-In Manager window. Returns false when closed.
    pub fn plugin_manager(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let reg = plugin::snapshot();
        let scanning = self.plugin_scan.is_some();
        let mut toggle: Option<(String, bool)> = None;
        let mut all: Option<bool> = None;
        let mut scan: Option<bool> = None;
        let mut add_folder = false;
        let mut remove_folder: Option<std::path::PathBuf> = None;
        egui::Window::new("Audio Plug-In Manager")
            .collapsible(false)
            .resizable(true)
            .default_size([720.0, 520.0])
            .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, -20.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if cfg!(target_os = "macos") {
                    ui.label(RichText::new("Audio Units installed on this Mac are listed automatically when you scan.").color(TEXT_DIM()).size(11.0));
                }
                ui.label(RichText::new("Folders searched for VST3 plug-ins").strong());
                for d in plugin::default_folders() {
                    ui.label(RichText::new(d.display().to_string()).color(TEXT_DIM()).monospace().size(11.0));
                }
                for d in &reg.folders {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(d.display().to_string()).monospace().size(11.0));
                        if ui.small_button("×").on_hover_text("Stop searching this folder").clicked() {
                            remove_folder = Some(d.clone());
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!scanning, egui::Button::new("Add Folder…")).clicked() {
                        add_folder = true;
                    }
                    if ui.add_enabled(!scanning, egui::Button::new("Scan for Plug-Ins")).on_hover_text("Find new and updated plug-ins").clicked() {
                        scan = Some(false);
                    }
                    if ui.add_enabled(!scanning, egui::Button::new("Rescan All")).on_hover_text("Check every plug-in again, including ones that failed").clicked() {
                        scan = Some(true);
                    }
                });
                if let Some(p) = &self.plugin_scan {
                    if let Ok(p) = p.lock() {
                        let frac = if p.total == 0 { 0.0 } else { p.done as f32 / p.total as f32 };
                        ui.add(egui::ProgressBar::new(frac).text(format!("Scanning {} ({}/{})", p.current, p.done, p.total)));
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} plug-ins", reg.plugins.len())).strong());
                    ui.add(egui::TextEdit::singleline(&mut self.plugin_filter).hint_text("Filter").desired_width(180.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Disable All").clicked() {
                            all = Some(false);
                        }
                        if ui.small_button("Enable All").clicked() {
                            all = Some(true);
                        }
                    });
                });
                let f = self.plugin_filter.to_lowercase();
                let list_h = (ui.available_height() - if reg.failed.is_empty() { 40.0 } else { 140.0 }).max(120.0);
                egui::Frame::none().fill(BG_LIST()).inner_margin(egui::Margin::same(4.0)).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_source("plugins").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                        if reg.plugins.is_empty() {
                            ui.label(RichText::new(if scanning { "Scanning…" } else { "No plug-ins found. Install some (or add the folder they're in), then Scan." }).color(TEXT_DIM()));
                        }
                        egui::Grid::new("plugin_list").num_columns(5).striped(true).spacing([10.0, 3.0]).show(ui, |ui| {
                            for p in reg.plugins.iter().filter(|p| f.is_empty() || p.name.to_lowercase().contains(&f) || p.vendor.to_lowercase().contains(&f)) {
                                let mut on = p.enabled;
                                if ui.checkbox(&mut on, "").on_hover_text("Show in the Effects menu and racks").changed() {
                                    toggle = Some((p.cid.clone(), on));
                                }
                                ui.label(RichText::new(&p.name).color(if p.enabled { TEXT() } else { TEXT_DIM() })).on_hover_text(p.path.display().to_string());
                                ui.label(RichText::new(&p.vendor).color(TEXT_DIM()));
                                ui.label(RichText::new(p.format.label()).color(TEXT_DIM()).size(11.0));
                                ui.label(RichText::new(&p.version).color(TEXT_DIM()).size(11.0));
                                ui.end_row();
                            }
                        });
                    });
                });
                if !reg.failed.is_empty() {
                    egui::CollapsingHeader::new(RichText::new(format!("{} couldn't be loaded", reg.failed.len())).color(WARN())).show(ui, |ui| {
                        egui::ScrollArea::vertical().id_source("failed").max_height(90.0).show(ui, |ui| {
                            for (p, why) in &reg.failed {
                                ui.horizontal(|ui| {
                                    ui.label(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).on_hover_text(p.display().to_string());
                                    ui.label(RichText::new(why).color(TEXT_DIM()).size(11.0));
                                });
                            }
                        });
                    });
                }
                ui.label(
                    RichText::new("Plug-ins are checked in a separate process, so one that crashes is listed above instead of closing Audemo.")
                        .color(TEXT_DIM())
                        .size(10.5),
                );
            });
        let mut changed = false;
        if let Some((cid, on)) = toggle {
            plugin::update(|r| {
                if let Some(p) = r.plugins.iter_mut().find(|p| p.cid == cid) {
                    p.enabled = on;
                }
            });
            changed = true;
        }
        if let Some(on) = all {
            plugin::update(|r| r.plugins.iter_mut().for_each(|p| p.enabled = on));
            changed = true;
        }
        if let Some(d) = remove_folder {
            plugin::update(|r| r.folders.retain(|x| *x != d));
            scan = Some(false);
        }
        if add_folder {
            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                plugin::update(|r| {
                    if !r.folders.contains(&d) {
                        r.folders.push(d);
                    }
                });
                scan = Some(false);
            }
        }
        if changed {
            self.reload_plugins();
        }
        if let Some(full) = scan {
            if self.plugin_scan.is_none() {
                self.plugin_scan = Some(plugin::start_scan(full));
            }
        }
        if self.plugin_scan.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        open
    }
}
