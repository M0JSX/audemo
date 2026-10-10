//! The Preferences window, laid out like Audition's: a list of categories
//! on the left, the chosen page on the right, Cancel and OK at the bottom.
//! Changes are made to a copy and applied together on OK.

use eframe::egui::{self, vec2, Align, Color32, Layout, RichText, Stroke, Ui};

use crate::app::{App, Dialog};
use crate::prefs::*;
use crate::theme::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Appearance,
    ChannelMapping,
    AudioHardware,
    AutoSave,
    ControlSurface,
    Data,
    Effects,
    MediaCache,
    Memory,
    Markers,
    Multitrack,
    MultitrackClips,
    Playback,
    Spectral,
    TimeDisplay,
    Video,
}

impl Page {
    pub const ALL: [Page; 17] = [
        Page::General,
        Page::Appearance,
        Page::ChannelMapping,
        Page::AudioHardware,
        Page::AutoSave,
        Page::ControlSurface,
        Page::Data,
        Page::Effects,
        Page::MediaCache,
        Page::Memory,
        Page::Markers,
        Page::Multitrack,
        Page::MultitrackClips,
        Page::Playback,
        Page::Spectral,
        Page::TimeDisplay,
        Page::Video,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::ChannelMapping => "Audio Channel Mapping",
            Page::AudioHardware => "Audio Hardware",
            Page::AutoSave => "Auto Save",
            Page::ControlSurface => "Control Surface",
            Page::Data => "Data",
            Page::Effects => "Effects",
            Page::MediaCache => "Media & Disk Cache",
            Page::Memory => "Memory",
            Page::Markers => "Markers & Metadata",
            Page::Multitrack => "Multitrack",
            Page::MultitrackClips => "Multitrack Clips",
            Page::Playback => "Playback and Recording",
            Page::Spectral => "Spectral Displays",
            Page::TimeDisplay => "Time Display",
            Page::Video => "Video",
        }
    }
}

pub struct PrefsDialog {
    pub page: Page,
    pub edit: Prefs,
    /// The settings when the window opened.
    opened_with: Prefs,
    hosts: Vec<String>,
    /// Device lists and capabilities for `listed`: (host, output) they describe.
    listed: (Option<String>, Option<String>, Option<String>),
    inputs: Vec<String>,
    outputs: Vec<String>,
    out_channels: usize,
    rates: Vec<u32>,
    buffer_range: Option<(u32, u32)>,
    in_channels: usize,
    color_elem: usize,
    total_ram: Option<u64>,
}

impl PrefsDialog {
    pub fn new(prefs: &Prefs, page: Page) -> Self {
        let mut d = PrefsDialog {
            page,
            edit: prefs.clone(),
            opened_with: prefs.clone(),
            hosts: crate::engine::hosts(),
            listed: (Some("\u{0}".into()), None, None),
            inputs: Vec::new(),
            outputs: Vec::new(),
            out_channels: 2,
            rates: Vec::new(),
            buffer_range: None,
            in_channels: 2,
            color_elem: 0,
            total_ram: total_ram(),
        };
        d.refresh_devices();
        d
    }

    /// Re-read devices when the device class or device changed.
    fn refresh_devices(&mut self) {
        let key = (self.edit.host.clone(), self.edit.output_device.clone(), self.edit.input_device.clone());
        if key == self.listed {
            return;
        }
        if key.0 != self.listed.0 {
            let (i, o) = crate::engine::list_devices(self.edit.host.as_deref());
            self.inputs = i;
            self.outputs = o;
        }
        let (ch, rates, buf) = crate::engine::output_caps(self.edit.host.as_deref(), self.edit.output_device.as_deref());
        self.out_channels = ch.max(1);
        self.rates = rates;
        self.buffer_range = buf;
        self.in_channels = crate::engine::input_channels(self.edit.host.as_deref(), self.edit.input_device.as_deref()).max(1);
        self.listed = key;
    }
}

/// Physical memory in bytes.
pub fn total_ram() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let t = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = t.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let name = b"hw.memsize\0";
        let r = unsafe { libc::sysctlbyname(name.as_ptr() as *const libc::c_char, &mut v as *mut u64 as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0) };
        (r == 0).then_some(v)
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        struct MemStatus {
            len: u32,
            load: u32,
            total_phys: u64,
            avail_phys: u64,
            total_page: u64,
            avail_page: u64,
            total_virt: u64,
            avail_virt: u64,
            avail_ext: u64,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalMemoryStatusEx(m: *mut MemStatus) -> i32;
        }
        let mut m: MemStatus = unsafe { std::mem::zeroed() };
        m.len = std::mem::size_of::<MemStatus>() as u32;
        (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then_some(m.total_phys)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

// ------------------------------------------------------------------ widgets

/// A labelled row: label right-aligned in a fixed column, like Audition.
fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(vec2(150.0, 20.0), Layout::right_to_left(Align::Center), |ui| {
            if !label.is_empty() {
                ui.label(format!("{label}:"));
            }
        });
        add(ui);
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).color(TEXT()).strong());
    ui.add_space(2.0);
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(TEXT_DIM()).size(11.0));
}

fn group(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .stroke(Stroke::new(1.0_f32, BORDER()))
        .rounding(3.0)
        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn combo<T: Copy + PartialEq>(ui: &mut Ui, id: &str, width: f32, value: &mut T, options: &[T], label: impl Fn(T) -> String) {
    egui::ComboBox::from_id_source(id).width(width).selected_text(label(*value)).show_ui(ui, |ui| {
        for o in options {
            ui.selectable_value(value, *o, label(*o));
        }
    });
}

fn device_combo(ui: &mut Ui, id: &str, value: &mut Option<String>, devices: &[String], default_label: &str) {
    let shown = value.clone().map(|v| if devices.contains(&v) { v } else { format!("{v} (not connected)") }).unwrap_or_else(|| default_label.to_string());
    egui::ComboBox::from_id_source(id).width(300.0).selected_text(shown).show_ui(ui, |ui| {
        ui.selectable_value(value, None, default_label);
        for n in devices {
            ui.selectable_value(value, Some(n.clone()), n);
        }
    });
}

/// A device-channel choice. A saved channel the current device doesn't
/// have is shown as such and kept unless another is picked.
fn channel_combo(ui: &mut Ui, id: (&str, &str), v: &mut u32, names: &[String]) {
    let k = *v as usize;
    let shown = names.get(k).cloned().unwrap_or_else(|| format!("{} (not on this device; channel {} is used)", k + 1, names.len().max(1)));
    egui::ComboBox::from_id_source(id).width(300.0).selected_text(shown).show_ui(ui, |ui| {
        for (i, n) in names.iter().enumerate() {
            if ui.selectable_label(i == k, n).clicked() {
                *v = i as u32;
            }
        }
    });
}

fn hex_color(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

// ------------------------------------------------------------------ window

impl App {
    /// Draw the Preferences window. Returns the dialog to keep showing, or
    /// None once it's closed (OK applies the changes).
    pub fn preferences_window(&mut self, ctx: &egui::Context, mut d: Box<PrefsDialog>) -> Option<Box<PrefsDialog>> {
        let mut open = true;
        let mut ok = false;
        let mut cancel = false;
        d.refresh_devices();
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(false)
            .fixed_size([820.0, 560.0])
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -10.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // Category list.
                    egui::Frame::none().fill(BG_LIST()).stroke(Stroke::new(1.0_f32, ACCENT())).rounding(2.0).inner_margin(egui::Margin::same(4.0)).show(ui, |ui| {
                        ui.set_width(170.0);
                        ui.set_min_height(510.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for p in Page::ALL {
                                let sel = d.page == p;
                                let text = RichText::new(p.name()).color(if sel { Color32::WHITE } else { TEXT() });
                                if ui.add(egui::SelectableLabel::new(sel, text)).clicked() {
                                    d.page = p;
                                }
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.set_width(620.0);
                        ui.set_min_height(510.0);
                        ui.label(RichText::new(d.page.name()).color(TEXT_DIM()));
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_source(("prefs_page", d.page.name())).max_height(500.0).auto_shrink([false, false]).show(ui, |ui| {
                            self.prefs_page(ui, &mut d);
                        });
                    });
                });
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new("     OK     ").rounding(10.0)).clicked() {
                        ok = true;
                    }
                    if ui.add(egui::Button::new("   Cancel   ").rounding(10.0)).clicked() {
                        cancel = true;
                    }
                });
            });
        if ok {
            let mut new = d.edit.clone();
            new.sanitize();
            self.apply_prefs(ctx, new, &d.opened_with);
            return None;
        }
        if cancel || !open {
            return None;
        }
        Some(d)
    }

    fn prefs_page(&mut self, ui: &mut Ui, d: &mut PrefsDialog) {
        let p = &mut d.edit;
        match d.page {
            Page::General => {
                group(ui, |ui| {
                    row(ui, "Startup", |ui| combo(ui, "startup", 240.0, &mut p.startup, Startup::ALL, |v| v.label().into()));
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.show_tooltips, "Show Tool Tips");
                    });
                });
                section(ui, "Time Selection and Playhead");
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.return_to_start, "Return CTI to start position on stop");
                    });
                    row(ui, "Zoom Factor", |ui| {
                        ui.add(egui::Slider::new(&mut p.zoom_factor, 10.0..=90.0).suffix(" %").fixed_decimals(0));
                    });
                    row(ui, "Mouse Wheel Zoom Factor", |ui| {
                        ui.add(egui::Slider::new(&mut p.wheel_zoom, 5.0..=100.0).suffix(" %").fixed_decimals(0));
                    });
                    row(ui, "Auto-scroll Navigation", |ui| combo(ui, "autoscroll", 140.0, &mut p.autoscroll, AutoScroll::ALL, |v| v.label().into()));
                });
                section(ui, "Media Browser");
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.browser_autoplay, "Auto-Play files when selected");
                    });
                });
            }
            Page::Appearance => {
                group(ui, |ui| {
                    row(ui, "Preset", |ui| {
                        let before = p.appearance;
                        combo(ui, "appearance", 180.0, &mut p.appearance, AppearancePreset::ALL, |v| v.label().into());
                        if p.appearance != before {
                            match p.appearance {
                                AppearancePreset::Default => {
                                    p.brightness = 30.0;
                                    p.colors = DEFAULT_COLORS;
                                    p.gradients = false;
                                }
                                AppearancePreset::Darkest => {
                                    p.brightness = 0.0;
                                    p.colors = DEFAULT_COLORS;
                                }
                                AppearancePreset::Light => {
                                    p.brightness = 85.0;
                                    p.colors = DEFAULT_COLORS;
                                }
                                AppearancePreset::Custom => {}
                            }
                        }
                    });
                });
                section(ui, "Color");
                group(ui, |ui| {
                    let before = (p.colors, p.brightness, p.gradients);
                    row(ui, "Element", |ui| {
                        let mut e = d.color_elem;
                        egui::ComboBox::from_id_source("color_elem").width(200.0).selected_text(COLOR_NAMES[e]).show_ui(ui, |ui| {
                            for (i, n) in COLOR_NAMES.iter().enumerate() {
                                ui.selectable_value(&mut e, i, *n);
                            }
                        });
                        d.color_elem = e;
                        let mut rgb = hex_color(p.colors[e]);
                        if ui.color_edit_button_srgba(&mut rgb).changed() {
                            p.colors[e] = (rgb.r() as u32) << 16 | (rgb.g() as u32) << 8 | rgb.b() as u32;
                        }
                        if ui.small_button("Default").clicked() {
                            p.colors[e] = DEFAULT_COLORS[e];
                        }
                    });
                    // Hue and saturation of the chosen element.
                    let c = hex_color(p.colors[d.color_elem]);
                    let mut hsv = egui::ecolor::Hsva::from(c);
                    let (h0, s0) = (hsv.h, hsv.s);
                    row(ui, "Hue", |ui| {
                        ui.add(egui::Slider::new(&mut hsv.h, 0.0..=1.0).show_value(false));
                    });
                    row(ui, "Saturation", |ui| {
                        ui.add(egui::Slider::new(&mut hsv.s, 0.0..=1.0).show_value(false));
                    });
                    if hsv.h != h0 || hsv.s != s0 {
                        let c: Color32 = hsv.into();
                        p.colors[d.color_elem] = (c.r() as u32) << 16 | (c.g() as u32) << 8 | c.b() as u32;
                    }
                    row(ui, "Brightness", |ui| {
                        ui.label(RichText::new("Darker").color(TEXT_DIM()).size(11.0));
                        ui.add(egui::Slider::new(&mut p.brightness, 0.0..=100.0).show_value(false));
                        ui.label(RichText::new("Lighter").color(TEXT_DIM()).size(11.0));
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.gradients, "Use Gradients");
                    });
                    if (p.colors, p.brightness, p.gradients) != before {
                        p.appearance = AppearancePreset::Custom;
                    }
                });
                section(ui, "Interface");
                group(ui, |ui| {
                    row(ui, "Interface Scale", |ui| {
                        ui.add(egui::Slider::new(&mut p.ui_scale, 0.75..=2.0).fixed_decimals(2).suffix("×"));
                    });
                });
                note(ui, "Changes are previewed when you click OK.");
            }
            Page::ChannelMapping => {
                let out_names: Vec<String> = (0..d.out_channels).map(|i| format!("{}: {} {}", i + 1, self.engine.device_name, i + 1)).collect();
                section(ui, "Default Stereo Output");
                group(ui, |ui| {
                    egui::Grid::new("outmap").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        for (label, v) in [("Audemo 1 (L)", &mut p.out_map_l), ("Audemo 2 (R)", &mut p.out_map_r)] {
                            ui.label(label);
                            channel_combo(ui, ("om", label), v, &out_names);
                            ui.end_row();
                        }
                    });
                });
                let in_name = p.input_device.clone().unwrap_or_else(|| "Input".into());
                let in_names: Vec<String> = (0..d.in_channels).map(|i| format!("{}: {} {}", i + 1, in_name, i + 1)).collect();
                section(ui, "Default Stereo Input");
                group(ui, |ui| {
                    egui::Grid::new("inmap").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        for (label, v) in [("Audemo 1 (L)", &mut p.in_map_l), ("Audemo 2 (R)", &mut p.in_map_r)] {
                            ui.label(label);
                            channel_combo(ui, ("im", label), v, &in_names);
                            ui.end_row();
                        }
                    });
                });
                note(ui, "Mapping both sides to the same channel plays (or records) a mono mix of left and right on it.");
            }
            Page::AudioHardware => {
                group(ui, |ui| {
                    row(ui, "Device Class", |ui| {
                        let default = format!("{} (default)", crate::engine::default_host_name());
                        let shown = p.host.clone().unwrap_or_else(|| default.clone());
                        egui::ComboBox::from_id_source("host").width(200.0).selected_text(shown).show_ui(ui, |ui| {
                            ui.selectable_value(&mut p.host, None, default);
                            for h in &d.hosts {
                                ui.selectable_value(&mut p.host, Some(h.clone()), h);
                            }
                        });
                    });
                });
                ui.add_space(6.0);
                group(ui, |ui| {
                    row(ui, "Default Input", |ui| device_combo(ui, "hw_in", &mut p.input_device, &d.inputs, "System Default"));
                    row(ui, "Default Output", |ui| device_combo(ui, "hw_out", &mut p.output_device, &d.outputs, "System Default"));
                    ui.add_space(4.0);
                    row(ui, "Master Clock", |ui| {
                        ui.label(format!("Out: {}", p.output_device.clone().unwrap_or_else(|| "System Default".into())));
                    });
                    row(ui, "Clock Source", |ui| {
                        ui.label("Internal");
                    });
                    ui.add_space(4.0);
                    row(ui, "I/O Buffer Size", |ui| {
                        let sizes: Vec<u32> = [32u32, 64, 128, 256, 512, 1024, 2048, 4096]
                            .into_iter()
                            .filter(|n| d.buffer_range.map(|(lo, hi)| *n >= lo && *n <= hi).unwrap_or(true))
                            .collect();
                        let label = |n: u32| if n == 0 { "Default".to_string() } else { n.to_string() };
                        egui::ComboBox::from_id_source("bufsize").width(120.0).selected_text(label(p.buffer_size)).show_ui(ui, |ui| {
                            ui.selectable_value(&mut p.buffer_size, 0, "Default");
                            for n in sizes {
                                ui.selectable_value(&mut p.buffer_size, n, n.to_string());
                            }
                        });
                        ui.label("Samples");
                        let ms = |n: u32, sr: u32| n as f32 * 1000.0 / sr.max(1) as f32;
                        let sr = if p.sample_rate > 0 { p.sample_rate } else { self.engine.out_rate };
                        if p.buffer_size > 0 {
                            ui.label(RichText::new(format!("≈ {:.1} ms", ms(p.buffer_size, sr))).color(TEXT_DIM()).size(11.0));
                        }
                    });
                    row(ui, "Sample Rate", |ui| {
                        let label = |n: u32| if n == 0 { "Device default".to_string() } else { n.to_string() };
                        egui::ComboBox::from_id_source("hwrate").width(120.0).selected_text(label(p.sample_rate)).show_ui(ui, |ui| {
                            ui.selectable_value(&mut p.sample_rate, 0, "Device default");
                            let rates = if d.rates.is_empty() { crate::app::SAMPLE_RATES.to_vec() } else { d.rates.clone() };
                            for r in rates {
                                ui.selectable_value(&mut p.sample_rate, r, r.to_string());
                            }
                        });
                        ui.label("Hz");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.force_doc_rate, "Attempt to force hardware to document sample rate");
                    });
                    ui.add_space(4.0);
                    row(ui, "", |ui| {
                        if ui.button("  Settings...  ").on_hover_text("Open the system's audio settings").clicked() {
                            open_system_audio_settings();
                        }
                    });
                });
                ui.add_space(6.0);
                note(
                    ui,
                    &format!(
                        "In use: {} via {} at {} Hz, {} output channels, buffer {}.",
                        self.engine.device_name,
                        self.engine.host_name,
                        self.engine.out_rate,
                        self.engine.out_channels,
                        self.engine.out_buffer.map(|n| format!("{n} samples")).unwrap_or_else(|| "set by the device".into())
                    ),
                );
            }
            Page::AutoSave => {
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.autosave, "Automatically save backups of multitrack sessions every");
                        ui.add_enabled(p.autosave, egui::DragValue::new(&mut p.autosave_minutes).range(1..=120));
                        ui.label("minutes");
                    });
                    row(ui, "", |ui| {
                        ui.add_enabled(p.autosave, egui::Checkbox::new(&mut p.backup_files, "Also back up open audio files with unsaved changes"));
                    });
                    row(ui, "Maximum number of backups", |ui| {
                        ui.add_enabled(p.autosave, egui::DragValue::new(&mut p.autosave_max).range(1..=100));
                        ui.label(RichText::new("per session or file").color(TEXT_DIM()).size(11.0));
                    });
                });
                section(ui, "Backup Location");
                group(ui, |ui| {
                    for loc in BackupLocation::ALL {
                        ui.radio_value(&mut p.backup_location, *loc, loc.label());
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        let shown = p.backup_dir.clone().or_else(crate::autosave::default_dir).map(|d| d.display().to_string()).unwrap_or_default();
                        ui.add_enabled(p.backup_location == BackupLocation::Folder, egui::Label::new(RichText::new(shown).monospace().size(11.0)));
                        if ui.add_enabled(p.backup_location == BackupLocation::Folder, egui::Button::new("Browse...")).clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                p.backup_dir = Some(dir);
                            }
                        }
                    });
                });
                note(ui, "Backups never replace your files. Unsaved recordings and edits in a session are written beside its backup.");
            }
            Page::ControlSurface => {
                group(ui, |ui| {
                    row(ui, "Device Class", |ui| {
                        egui::ComboBox::from_id_source("cs").width(200.0).selected_text("None").show_ui(ui, |ui| {
                            let _ = ui.selectable_label(true, "None");
                        });
                    });
                    row(ui, "", |ui| {
                        ui.add_enabled(false, egui::Button::new("Configure..."));
                    });
                });
                note(ui, "Audemo doesn't support hardware control surfaces (Mackie, EUCON, Avid) yet.");
            }
            Page::Data => {
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.add_enabled(false, egui::Checkbox::new(&mut true.clone(), "Automatically convert all files to 32-bit upon opening"));
                    });
                    note(ui, "                                       Audemo always edits in 32-bit floating point.");
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.dither, "Dither transform results (increases dynamic range)");
                    });
                    note(ui, "                                       Used as the default when saving to 16- or 24-bit formats.");
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.smooth_delete, "Smooth delete and cut boundaries over");
                        ui.add_enabled(p.smooth_delete, egui::DragValue::new(&mut p.smooth_delete_ms).range(0.1..=50.0).speed(0.1).fixed_decimals(1));
                        ui.label("ms");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.smooth_edits, "Smooth paste boundaries by crossfading");
                        ui.add_enabled(p.smooth_edits, egui::DragValue::new(&mut p.smooth_edits_ms).range(0.1..=50.0).speed(0.1).fixed_decimals(1));
                        ui.label("ms");
                    });
                });
                section(ui, "Sample Rate Conversion");
                group(ui, |ui| {
                    row(ui, "Quality", |ui| combo(ui, "srcq", 160.0, &mut p.src_quality, SrcQuality::ALL, |v| v.label().into()));
                });
            }
            Page::Effects => {
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.show_plugin_window, "Show a plug-in's own window when opening its effect");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.scan_at_startup, "Scan for new and updated plug-ins at startup");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.rack_entire, "Effects Rack processes the entire file by default");
                    });
                    ui.add_space(6.0);
                    row(ui, "", |ui| {
                        if ui.button("Audio Plug-In Manager...").clicked() {
                            self.actions.push(crate::app::Action::PluginManager);
                        }
                    });
                });
                note(ui, "Effect previews loop the selection; press Space in an effect window to start and stop them.");
            }
            Page::MediaCache => {
                section(ui, "Temporary Folders");
                group(ui, |ui| {
                    let tmp = std::env::temp_dir().display().to_string();
                    row(ui, "Primary", |ui| {
                        ui.add_enabled(false, egui::Label::new(RichText::new(&tmp).monospace().size(11.0)));
                    });
                });
                section(ui, "Media Cache");
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.add_enabled(false, egui::Checkbox::new(&mut true.clone(), "Save Peak Files"));
                    });
                });
                note(ui, "Audemo keeps open audio in memory and builds waveform and peak data as files open, so it uses no temporary or cache files on disk.");
            }
            Page::Memory => {
                group(ui, |ui| {
                    row(ui, "Installed RAM", |ui| {
                        ui.label(d.total_ram.map(gb).unwrap_or_else(|| "Unknown".into()));
                    });
                    let used: usize = self.docs.iter().map(|doc| doc.n_ch() * doc.len() * 4 * (1 + doc.undo.len().min(1))).sum();
                    row(ui, "Open audio", |ui| {
                        ui.label(format!("{} in {} file(s), plus undo history", gb(used as u64), self.docs.len()));
                    });
                });
                section(ui, "Undo");
                group(ui, |ui| {
                    row(ui, "Undo levels", |ui| {
                        ui.add(egui::DragValue::new(&mut p.undo_levels).range(5..=500));
                        ui.label(RichText::new("per audio file (sessions keep 100)").color(TEXT_DIM()).size(11.0));
                    });
                });
                note(ui, "Each undo step of a destructive edit keeps a copy of the audio it replaced. Fewer levels use less memory.");
            }
            Page::Markers => {
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.copy_markers, "Include markers and metadata in selections copied to new files");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.save_meta, "Include markers and other metadata when saving (default)");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.write_encoder, "Write \"Audemo\" as the encoder in file metadata");
                    });
                    row(ui, "New marker name", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut p.marker_name).desired_width(160.0));
                        ui.label(RichText::new(format!("e.g. \"{} 01\"", p.marker_name.trim())).color(TEXT_DIM()).size(11.0));
                    });
                });
            }
            Page::Multitrack => {
                group(ui, |ui| {
                    row(ui, "Pan Mode", |ui| combo(ui, "panlaw", 220.0, &mut p.pan_law, PanLaw::ALL, |v| v.label().into()));
                });
                note(ui, "L/R Cut keeps the centre at full level and turns the opposite side down. Equal Power keeps loudness constant across the pan range (−3 dB at centre).");
                section(ui, "Recording");
                group(ui, |ui| {
                    row(ui, "Recording latency", |ui| {
                        ui.add(egui::DragValue::new(&mut p.rec_offset_ms).speed(0.5).range(0.0..=1000.0).fixed_decimals(1).suffix(" ms"));
                    });
                    note(ui, "                                       Moves multitrack takes earlier to line up with what you heard.");
                });
            }
            Page::MultitrackClips => {
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.auto_crossfade, "Create crossfades automatically when clips overlap");
                    });
                    row(ui, "Crossfade curve", |ui| combo(ui, "xfade", 160.0, &mut p.crossfade_curve, FadeCurve::ALL, |v| v.label().into()));
                    row(ui, "Default clip fades", |ui| {
                        ui.add(egui::DragValue::new(&mut p.clip_fade_ms).range(0.0..=10_000.0).speed(1.0).suffix(" ms"));
                        ui.label(RichText::new("fade in and out on newly placed clips").color(TEXT_DIM()).size(11.0));
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.show_clip_names, "Show clip names");
                    });
                });
            }
            Page::Playback => {
                group(ui, |ui| {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.return_to_start, "Return CTI to start position on stop");
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.follow, "Auto-scroll to follow the playhead");
                    });
                });
                section(ui, "Pre-roll and Post-roll");
                group(ui, |ui| {
                    row(ui, "Pre-roll", |ui| {
                        ui.add(egui::DragValue::new(&mut p.preroll_s).range(0.0..=30.0).speed(0.1).fixed_decimals(1).suffix(" s"));
                    });
                    row(ui, "Post-roll", |ui| {
                        ui.add(egui::DragValue::new(&mut p.postroll_s).range(0.0..=30.0).speed(0.1).fixed_decimals(1).suffix(" s"));
                    });
                    note(ui, "                                       Used by View > Play with Pre-roll and Post-roll (Alt+Space).");
                });
                section(ui, "Recording");
                group(ui, |ui| {
                    row(ui, "Recording latency", |ui| {
                        ui.add(egui::DragValue::new(&mut p.rec_offset_ms).speed(0.5).range(0.0..=1000.0).fixed_decimals(1).suffix(" ms"));
                    });
                });
                note(ui, "Recordings are captured at the input device's native rate and converted to the file's sample rate when you stop.");
            }
            Page::Spectral => {
                group(ui, |ui| {
                    row(ui, "Windowing Function", |ui| combo(ui, "specwin", 180.0, &mut p.spec_window, WindowFn::ALL, |v| v.label().into()));
                    row(ui, "Resolution", |ui| {
                        combo(ui, "specres", 120.0, &mut p.spec_size, &[256, 512, 1024, 2048, 4096, 8192], |v| v.to_string());
                        ui.label("bands");
                    });
                    row(ui, "Decibel Range", |ui| {
                        ui.add(egui::Slider::new(&mut p.spec_range_db, 48.0..=180.0).fixed_decimals(0).suffix(" dB"));
                    });
                    row(ui, "", |ui| {
                        ui.checkbox(&mut p.spec_log, "Logarithmic frequency scale");
                    });
                });
                note(ui, "Higher resolution shows finer frequency detail and blurs fast changes in time.");
            }
            Page::TimeDisplay => {
                group(ui, |ui| {
                    row(ui, "Time Display Format", |ui| combo(ui, "timefmt", 240.0, &mut p.time_format, TimeFormat::ALL, |v| v.label().into()));
                    if p.time_format == TimeFormat::Custom {
                        row(ui, "Frames per second", |ui| {
                            ui.add(egui::DragValue::new(&mut p.custom_fps).range(1.0..=1000.0).speed(0.1).fixed_decimals(2));
                        });
                    }
                });
                section(ui, "Bars and Beats");
                group(ui, |ui| {
                    row(ui, "Tempo", |ui| {
                        ui.add(egui::DragValue::new(&mut p.tempo).range(20.0..=400.0).speed(0.5).fixed_decimals(1));
                        ui.label("beats/minute");
                    });
                    row(ui, "Time Signature", |ui| {
                        ui.add(egui::DragValue::new(&mut p.beats_per_bar).range(1..=32));
                        ui.label("/");
                        combo(ui, "beatunit", 50.0, &mut p.beat_unit, &[1, 2, 4, 8, 16], |v| v.to_string());
                    });
                });
                let example = crate::dsp::util::format_time_as(crate::app::time_style_for(p), 48000.0 * 75.5, 48000);
                note(ui, &format!("1 minute 15.5 seconds shows as {example}"));
            }
            Page::Video => {
                group(ui, |ui| {
                    row(ui, "Video Display", |ui| {
                        ui.add_enabled(false, egui::Label::new("No video support"));
                    });
                });
                note(ui, "Audemo is an audio editor; it doesn't open video files or show a Video panel.");
            }
        }
    }
}

/// Edit > Preferences > Audio Hardware > Settings...: the system's own
/// audio settings.
fn open_system_audio_settings() {
    use std::process::Command;
    let r = if cfg!(target_os = "macos") {
        Command::new("open").args(["-a", "Audio MIDI Setup"]).spawn()
    } else if cfg!(windows) {
        Command::new("control").arg("mmsys.cpl").spawn()
    } else {
        Command::new("pavucontrol").spawn()
    };
    let _ = r;
}

/// Keep the dialog type reachable from `Dialog`.
pub fn dialog(prefs: &Prefs, page: Page) -> Dialog {
    Dialog::Preferences(Box::new(PrefsDialog::new(prefs, page)))
}
