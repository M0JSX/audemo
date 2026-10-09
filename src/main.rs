// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dialogs;
mod dsp;
mod editor;
mod engine;
mod io;
mod panels;
mod theme;

use std::path::PathBuf;

use eframe::egui;

impl eframe::App for app::App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
        self.menu_bar(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);
        if self.show_meters {
            self.meters(ctx);
        }
        self.transport(ctx);
        if self.show_left {
            self.left_panel(ctx);
        }
        if self.show_right {
            self.right_panel(ctx);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::BG_DEEP).inner_margin(egui::Margin::same(4.0)))
            .show(ctx, |ui| self.editor_ui(ui));
        self.dialogs(ctx);
        self.job_overlay(ctx);
        self.finish_frame(ctx);
    }
}

fn main() -> eframe::Result<()> {
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
