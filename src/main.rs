// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis_ui;
mod app;
mod audio;
mod autosave;
mod cd;
mod control_surface;
mod dialogs;
mod dsp;
mod editor;
mod engine;
mod essential;
mod export;
mod interchange;
mod io;
mod liverack;
#[cfg(any(target_os = "macos", audemo_check_menu))]
mod macmenu;
mod markers_ui;
mod mt_ui;
mod panels;
mod plugin;
mod plugin_ui;
mod prefs;
mod prefs_ui;
mod session;
mod sesx;
#[cfg(any(target_os = "macos", audemo_check_tap))]
mod systap;
#[cfg(test)]
mod sesx_pack;
mod shortcuts;
mod theme;
mod tools_ui;
mod update;
mod workspace;

use std::path::PathBuf;

use eframe::egui;

impl eframe::App for app::App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
        self.menu_bar(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);
        self.bottom_row(ctx);
        if self.show_left {
            self.left_column(ctx);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG_DEEP()).inner_margin(egui::Margin::same(2.0)))
            .show(ctx, |ui| {
                self.transport_bar(ui);
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(theme::BG_DEEP()).inner_margin(egui::Margin::same(2.0)))
                    .show_inside(ui, |ui| self.editor_ui(ui));
            });
        self.dialogs(ctx);
        self.cd_window(ctx);
        self.job_overlay(ctx);
        self.finish_frame(ctx);
    }
}

/// Where crash reports go: ~/Library/Logs/Audemo on macOS, a "crashes"
/// folder beside the preferences elsewhere.
pub fn crash_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            return Some(PathBuf::from(h).join("Library/Logs/Audemo"));
        }
    }
    prefs::config_dir().map(|d| d.join("crashes"))
}

/// On a crash, write what happened (message, place, backtrace) to a report
/// file as well as the usual output, so it can be sent in without a Terminal.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        if let Some(dir) = crash_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let path = dir.join(format!("audemo-crash-{secs}.txt"));
            let text = format!(
                "Audemo {} ({} {})\n{}\n\nthread '{}'\n\n{bt}\n",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                info,
                std::thread::current().name().unwrap_or("?")
            );
            if std::fs::write(&path, text).is_ok() {
                eprintln!("Crash report saved to {}", path.display());
            }
        }
        default(info);
    }));
}

/// The newest crash report written since Audemo last started cleanly, if any.
pub fn new_crash_report() -> Option<PathBuf> {
    let dir = crash_dir()?;
    let seen = dir.join(".seen");
    let seen_t = std::fs::metadata(&seen).and_then(|m| m.modified()).ok();
    let newest = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("audemo-crash-"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|x| x.0)?;
    let _ = std::fs::write(&seen, b"");
    match seen_t {
        Some(t) if newest.0 <= t => None,
        _ => Some(newest.1),
    }
}

fn main() -> eframe::Result<()> {
    // Plug-in scanning runs in a child process: `audemo --scan-vst3 <bundle>`.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--scan-vst3" {
        std::process::exit(plugin::scan_child(std::path::Path::new(&args[2])));
    }
    install_crash_log();
    // Before any thread starts: environment changes aren't thread-safe.
    prefs::apply_temp_dir(&prefs::Prefs::load());
    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).filter(|p| p.is_file()).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Audemo")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(egui::IconData {
                rgba: include_bytes!("../assets/icons/audemo-64.rgba").to_vec(),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native("Audemo", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, files)))))
}
