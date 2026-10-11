//! File dialogs.
//!
//! On the desktop these are the system's own dialogs (rfd), answered at
//! once. On Android, which has no dialog that returns a file path, Audemo
//! draws its own browser over the window and the answer arrives on a later
//! frame. (Set AUDEMO_INAPP_DIALOGS=1 to use the in-app browser on the
//! desktop too.)
//!
//! Callers name each request with a key:
//!
//! * In a dispatched action, `ask(key, spec, retry)` returns the answer at
//!   once on the desktop. On Android it returns None and opens the browser;
//!   once a file is chosen `retry` is dispatched again, and this time `ask`
//!   returns the answer.
//! * Behind a button, `request(key, spec)` on click and `answer(key)` every
//!   frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Key, Order, Rect, RichText, Sense, Stroke};

use crate::app::Action;
use crate::theme::{ACCENT, BG_LIST, BG_PANEL, BG_RAISED, TEXT, TEXT_DIM, WARN};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Open,
    OpenMany,
    Save,
    Folder,
}

#[derive(Clone)]
pub struct Spec {
    kind: Kind,
    title: String,
    filters: Vec<(String, Vec<String>)>,
    name: String,
    dir: Option<PathBuf>,
}

impl Spec {
    fn new(kind: Kind) -> Spec {
        Spec { kind, title: String::new(), filters: Vec::new(), name: String::new(), dir: None }
    }
    pub fn open() -> Spec {
        Spec::new(Kind::Open)
    }
    pub fn open_many() -> Spec {
        Spec::new(Kind::OpenMany)
    }
    pub fn save(name: impl Into<String>) -> Spec {
        Spec { name: name.into(), ..Spec::new(Kind::Save) }
    }
    pub fn folder() -> Spec {
        Spec::new(Kind::Folder)
    }
    pub fn filter(mut self, label: &str, exts: &[&str]) -> Spec {
        self.filters.push((label.to_string(), exts.iter().map(|e| e.to_ascii_lowercase()).collect()));
        self
    }
    pub fn dir(mut self, dir: Option<&Path>) -> Spec {
        self.dir = dir.filter(|d| d.is_dir()).map(Path::to_path_buf);
        self
    }
    pub fn title(mut self, title: &str) -> Spec {
        self.title = title.to_string();
        self
    }
}

#[derive(Default)]
struct State {
    answers: HashMap<String, Vec<PathBuf>>,
    browser: Option<Box<Browser>>,
    last_dir: Option<PathBuf>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn in_app() -> bool {
    cfg!(target_os = "android") || std::env::var_os("AUDEMO_INAPP_DIALOGS").is_some_and(|v| v != "0" && !v.is_empty())
}

/// Show a dialog. The desktop's answers at once; Audemo's own browser
/// answers when the user confirms, then dispatches `retry` if given.
pub fn request_then(key: &str, spec: Spec, retry: Option<Action>) {
    STATE.with(|s| {
        s.borrow_mut().answers.remove(key);
    });
    if in_app() {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let start = spec.dir.clone().or_else(|| s.last_dir.clone().filter(|d| d.is_dir())).unwrap_or_else(default_dir);
            s.browser = Some(Box::new(Browser::new(key, spec, retry, start)));
        });
        return;
    }
    #[cfg(not(target_os = "android"))]
    if let Some(r) = native(&spec) {
        STATE.with(|s| {
            s.borrow_mut().answers.insert(key.to_string(), r);
        });
    }
}

/// Show a dialog for a button; collect the result with `answer`.
pub fn request(key: &str, spec: Spec) {
    request_then(key, spec, None);
}

/// The chosen paths for `key`, once (None until chosen, or if cancelled).
pub fn answer(key: &str) -> Option<Vec<PathBuf>> {
    STATE.with(|s| s.borrow_mut().answers.remove(key))
}

pub fn answer_one(key: &str) -> Option<PathBuf> {
    answer(key).and_then(|v| v.into_iter().next())
}

/// For dispatched actions: the answer if one is waiting, otherwise show
/// the dialog (and dispatch `retry` once it's answered, where that's later).
pub fn ask(key: &str, spec: Spec, retry: Action) -> Option<Vec<PathBuf>> {
    if let Some(a) = answer(key) {
        return Some(a);
    }
    request_then(key, spec, Some(retry));
    answer(key)
}

pub fn ask_one(key: &str, spec: Spec, retry: Action) -> Option<PathBuf> {
    ask(key, spec, retry).and_then(|v| v.into_iter().next())
}

/// True while Audemo's own browser is showing (keyboard shortcuts pause).
pub fn is_open() -> bool {
    STATE.with(|s| s.borrow().browser.is_some())
}

#[cfg(not(target_os = "android"))]
fn native(spec: &Spec) -> Option<Vec<PathBuf>> {
    let mut d = rfd::FileDialog::new();
    if !spec.title.is_empty() {
        d = d.set_title(spec.title.clone());
    }
    for (label, exts) in &spec.filters {
        d = d.add_filter(label.clone(), exts);
    }
    if let Some(dir) = &spec.dir {
        d = d.set_directory(dir);
    }
    if !spec.name.is_empty() {
        d = d.set_file_name(spec.name.clone());
    }
    match spec.kind {
        Kind::Open => d.pick_file().map(|p| vec![p]),
        Kind::OpenMany => d.pick_files(),
        Kind::Save => d.save_file().map(|p| vec![p]),
        Kind::Folder => d.pick_folder().map(|p| vec![p]),
    }
}

// ------------------------------------------------------------------ browser

/// Where the browser starts when nothing better is known.
fn default_dir() -> PathBuf {
    if cfg!(target_os = "android") {
        let base = PathBuf::from("/storage/emulated/0");
        for d in ["Music", "Recordings"] {
            if base.join(d).is_dir() {
                return base.join(d);
            }
        }
        return base;
    }
    crate::prefs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Shortcuts down the left of the browser.
fn places() -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    if cfg!(target_os = "android") {
        let base = PathBuf::from("/storage/emulated/0");
        v.push(("Internal storage".into(), base.clone()));
        for d in ["Music", "Recordings", "Download", "Documents", "Podcasts", "Movies", "DCIM"] {
            v.push((d.into(), base.join(d)));
        }
        // SD cards and USB drives appear as /storage/XXXX-XXXX.
        if let Ok(rd) = std::fs::read_dir("/storage") {
            let mut extra: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                n != "emulated" && n != "self" && p.is_dir()
            }).collect();
            extra.sort();
            for p in extra {
                let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                v.push((format!("Card / USB {n}"), p));
            }
        }
    } else {
        if let Some(h) = crate::prefs::home_dir() {
            v.push(("Home".into(), h.clone()));
            for d in ["Desktop", "Documents", "Music", "Downloads"] {
                v.push((d.into(), h.join(d)));
            }
        }
        if cfg!(windows) {
            for c in b'C'..=b'Z' {
                let p = PathBuf::from(format!("{}:\\", c as char));
                if p.is_dir() {
                    v.push((format!("{}:", c as char), p));
                }
            }
        } else {
            v.push(("Computer".into(), PathBuf::from("/")));
            #[cfg(target_os = "macos")]
            v.push(("Volumes".into(), PathBuf::from("/Volumes")));
        }
    }
    v.retain(|(_, p)| p.is_dir());
    v
}

struct Entry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    size: u64,
}

struct Browser {
    key: String,
    spec: Spec,
    retry: Option<Action>,
    dir: PathBuf,
    listed: Option<PathBuf>,
    entries: Vec<Entry>,
    error: Option<String>,
    filter: usize,
    selected: Vec<PathBuf>,
    name: String,
    path_text: String,
    confirm_replace: Option<PathBuf>,
    new_folder: Option<String>,
}

impl Browser {
    fn new(key: &str, spec: Spec, retry: Option<Action>, dir: PathBuf) -> Browser {
        let name = spec.name.clone();
        Browser {
            key: key.to_string(),
            spec,
            retry,
            path_text: dir.display().to_string(),
            dir,
            listed: None,
            entries: Vec::new(),
            error: None,
            filter: 0,
            selected: Vec::new(),
            name,
            confirm_replace: None,
            new_folder: None,
        }
    }

    fn exts(&self) -> Option<&[String]> {
        self.spec.filters.get(self.filter).map(|f| f.1.as_slice()).filter(|e| !e.iter().any(|x| x == "*"))
    }

    fn go(&mut self, dir: PathBuf) {
        self.path_text = dir.display().to_string();
        self.dir = dir;
        self.listed = None;
        self.selected.clear();
        self.confirm_replace = None;
    }

    fn list(&mut self) {
        if self.listed.as_ref() == Some(&self.dir) {
            return;
        }
        self.listed = Some(self.dir.clone());
        self.entries.clear();
        self.error = None;
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(if e.kind() == std::io::ErrorKind::PermissionDenied && cfg!(target_os = "android") {
                    "Audemo isn't allowed to read this folder. Allow All files access for Audemo in Settings.".into()
                } else {
                    format!("Couldn't read this folder: {e}")
                });
                return;
            }
        };
        let exts: Option<Vec<String>> = self.exts().map(|e| e.to_vec());
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let is_dir = path.is_dir();
            if !is_dir {
                if self.spec.kind == Kind::Folder {
                    continue;
                }
                if let Some(exts) = &exts {
                    let ext = path.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                    if !exts.contains(&ext) {
                        continue;
                    }
                }
            }
            let size = if is_dir { 0 } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
            self.entries.push(Entry { path, name, is_dir, size });
        }
        self.entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    }

    /// The path a Save would write, with the filter's extension added when
    /// the name has none.
    fn save_target(&self) -> Option<PathBuf> {
        let name = self.name.trim();
        if name.is_empty() || name.contains('/') {
            return None;
        }
        let mut p = self.dir.join(name);
        if p.extension().is_none() {
            if let Some(e) = self.exts().and_then(|e| e.first()) {
                p = p.with_extension(e);
            }
        }
        Some(p)
    }

    fn can_confirm(&self) -> bool {
        match self.spec.kind {
            Kind::Open | Kind::OpenMany => !self.selected.is_empty(),
            Kind::Save => self.save_target().is_some(),
            Kind::Folder => self.dir.is_dir(),
        }
    }

    /// The answer, or None while a Replace? question is pending.
    fn confirm(&mut self) -> Option<Vec<PathBuf>> {
        match self.spec.kind {
            Kind::Open | Kind::OpenMany => Some(self.selected.clone()),
            Kind::Folder => Some(vec![self.dir.clone()]),
            Kind::Save => {
                let p = self.save_target()?;
                if p.exists() && self.confirm_replace.as_ref() != Some(&p) {
                    self.confirm_replace = Some(p);
                    return None;
                }
                Some(vec![p])
            }
        }
    }
}

fn size_text(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

/// Draw Audemo's file browser, if one is open. Returns the action to
/// dispatch once a file is chosen.
pub fn show(ctx: &egui::Context) -> Option<Action> {
    let mut b = STATE.with(|s| s.borrow_mut().browser.take())?;
    b.list();
    // None: still open; Some(None): cancelled; Some(Some(paths)): chosen.
    let mut done: Option<Option<Vec<PathBuf>>> = None;
    let screen = ctx.screen_rect();
    let size = vec2((screen.width() - 32.0).clamp(320.0, 980.0), (screen.height() - 32.0).clamp(300.0, 700.0));
    let row_h = if cfg!(target_os = "android") { 34.0 } else { 24.0 };

    // A dimmed backdrop that swallows clicks meant for the window behind.
    let dim = egui::Area::new(egui::Id::new("filedlg-dim")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(140));
    });
    ctx.move_to_top(dim.response.layer_id);

    let title = if !b.spec.title.is_empty() {
        b.spec.title.clone()
    } else {
        match b.spec.kind {
            Kind::Open => "Open".into(),
            Kind::OpenMany => "Open Files".into(),
            Kind::Save => "Save As".into(),
            Kind::Folder => "Choose a Folder".into(),
        }
    };
    let win = egui::Area::new(egui::Id::new("filedlg")).order(Order::Foreground).fixed_pos(screen.center() - size / 2.0).show(ctx, |ui| {
        egui::Frame::window(ui.style()).fill(BG_PANEL()).show(ui, |ui| {
            ui.set_min_size(size - vec2(16.0, 16.0));
            ui.set_max_size(size - vec2(16.0, 16.0));
            ui.label(RichText::new(&title).strong().size(16.0).color(TEXT()));
            ui.add_space(4.0);
            // Path bar.
            ui.horizontal(|ui| {
                let up = b.dir.parent().map(Path::to_path_buf);
                if ui.add_enabled(up.is_some(), egui::Button::new("  Up  ").min_size(vec2(0.0, row_h))).clicked() {
                    if let Some(u) = up {
                        b.go(u);
                    }
                }
                let w = ui.available_width() - 90.0;
                let r = ui.add(egui::TextEdit::singleline(&mut b.path_text).desired_width(w.max(80.0)).font(FontId::monospace(13.0)));
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let p = PathBuf::from(b.path_text.trim());
                    if p.is_dir() {
                        b.go(p);
                    } else {
                        b.path_text = b.dir.display().to_string();
                    }
                }
                if ui.add(egui::Button::new("Refresh").min_size(vec2(0.0, row_h))).clicked() {
                    b.listed = None;
                }
            });
            ui.add_space(4.0);
            let footer_h = if b.spec.kind == Kind::Save { 3.0 * (row_h + 8.0) } else { 2.0 * (row_h + 8.0) };
            let body_h = (ui.available_height() - footer_h).max(80.0);
            ui.horizontal_top(|ui| {
                // Places.
                ui.allocate_ui(vec2(170.0, body_h), |ui| {
                    egui::ScrollArea::vertical().id_source("filedlg-places").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_width(160.0);
                        for (label, p) in places() {
                            let on = b.dir.starts_with(&p) && p.components().count() > 1;
                            if ui.add_sized([160.0, row_h], egui::SelectableLabel::new(on, label)).clicked() {
                                b.go(p);
                            }
                        }
                    });
                });
                // Folder contents.
                ui.vertical(|ui| {
                    let w = ui.available_width();
                    let (frame, _) = ui.allocate_exact_size(vec2(w, body_h), Sense::hover());
                    ui.painter().rect_filled(frame, 2.0, BG_LIST());
                    let mut child = ui.child_ui(frame.shrink(2.0), egui::Layout::top_down(egui::Align::Min), None);
                    if let Some(e) = &b.error {
                        child.add_space(8.0);
                        child.label(RichText::new(e).color(WARN()));
                        #[cfg(target_os = "android")]
                        if child.button("Open Settings").clicked() {
                            crate::android::open_storage_settings();
                        }
                        return;
                    }
                    if b.entries.is_empty() {
                        child.add_space(8.0);
                        child.label(RichText::new("No matching files here.").color(TEXT_DIM()));
                    }
                    let mut go_to = None;
                    let mut confirm_now = false;
                    egui::ScrollArea::vertical().id_source(("filedlg-list", b.dir.clone())).auto_shrink([false, false]).show_rows(&mut child, row_h, b.entries.len(), |ui, rows| {
                        let rw = ui.available_width();
                        for i in rows {
                            let e = &b.entries[i];
                            let (r, resp) = ui.allocate_exact_size(vec2(rw, row_h), Sense::click());
                            let sel = b.selected.contains(&e.path) || (b.spec.kind == Kind::Save && !e.is_dir && b.name == e.name);
                            if sel {
                                ui.painter().rect_filled(r, 2.0, ACCENT().gamma_multiply(0.55));
                            } else if resp.hovered() {
                                ui.painter().rect_filled(r, 2.0, BG_RAISED());
                            }
                            let icon = Rect::from_center_size(pos2(r.left() + 14.0, r.center().y), vec2(14.0, 11.0));
                            if e.is_dir {
                                ui.painter().rect_filled(icon, 1.5, Color32::from_rgb(0xc8, 0xa0, 0x48));
                            } else {
                                ui.painter().rect_stroke(icon.shrink2(vec2(2.0, 0.0)), 1.0, Stroke::new(1.0_f32, TEXT_DIM()));
                            }
                            let font = FontId::proportional(if row_h > 30.0 { 15.0 } else { 13.0 });
                            ui.painter().text(pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, &e.name, font.clone(), TEXT());
                            if !e.is_dir {
                                ui.painter().text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, size_text(e.size), FontId::proportional(12.0), TEXT_DIM());
                            }
                            if resp.clicked() {
                                if e.is_dir {
                                    go_to = Some(e.path.clone());
                                } else {
                                    match b.spec.kind {
                                        Kind::Open => b.selected = vec![e.path.clone()],
                                        Kind::OpenMany => {
                                            if let Some(k) = b.selected.iter().position(|p| p == &e.path) {
                                                b.selected.remove(k);
                                            } else {
                                                b.selected.push(e.path.clone());
                                            }
                                        }
                                        Kind::Save => b.name = e.name.clone(),
                                        Kind::Folder => {}
                                    }
                                }
                            }
                            if resp.double_clicked() && !e.is_dir {
                                if b.spec.kind == Kind::OpenMany && !b.selected.contains(&e.path) {
                                    b.selected.push(e.path.clone());
                                }
                                confirm_now = true;
                            }
                        }
                    });
                    if let Some(p) = go_to {
                        b.go(p);
                    }
                    if confirm_now && b.can_confirm() {
                        if let Some(r) = b.confirm() {
                            done = Some(Some(r));
                        }
                    }
                });
            });
            ui.add_space(6.0);
            // Name (Save), filters, new folder.
            if b.spec.kind == Kind::Save {
                ui.horizontal(|ui| {
                    ui.label("File name:");
                    let r = ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(ui.available_width() - 8.0));
                    if r.changed() {
                        b.confirm_replace = None;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && b.can_confirm() {
                        if let Some(res) = b.confirm() {
                            done = Some(Some(res));
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if b.spec.filters.len() > 1 {
                    for i in 0..b.spec.filters.len() {
                        let label = b.spec.filters[i].0.clone();
                        if ui.add(egui::SelectableLabel::new(b.filter == i, label)).clicked() && b.filter != i {
                            b.filter = i;
                            b.listed = None;
                        }
                    }
                } else if let Some((label, exts)) = b.spec.filters.first() {
                    let shown: Vec<String> = exts.iter().take(8).map(|e| format!(".{e}")).collect();
                    ui.label(RichText::new(format!("{label} ({}{})", shown.join(" "), if exts.len() > 8 { " …" } else { "" })).color(TEXT_DIM()).size(11.5));
                }
                if matches!(b.spec.kind, Kind::Save | Kind::Folder) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(name) = b.new_folder.as_mut() {
                            let mut create = ui.button("Create").clicked();
                            let r = ui.add(egui::TextEdit::singleline(name).desired_width(180.0).hint_text("New folder name"));
                            create |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if create {
                                let n = name.trim().to_string();
                                if !n.is_empty() && !n.contains('/') {
                                    let p = b.dir.join(&n);
                                    match std::fs::create_dir(&p) {
                                        Ok(()) => {
                                            b.new_folder = None;
                                            b.go(p);
                                        }
                                        Err(e) => b.error = Some(format!("Couldn't create the folder: {e}")),
                                    }
                                }
                            }
                        } else if ui.button("New Folder").clicked() {
                            b.new_folder = Some(String::new());
                        }
                    });
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(p) = &b.confirm_replace {
                    let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    ui.label(RichText::new(format!("\"{n}\" already exists. Replace it?")).color(WARN()));
                } else if !b.selected.is_empty() && b.spec.kind == Kind::OpenMany {
                    ui.label(RichText::new(format!("{} selected", b.selected.len())).color(TEXT_DIM()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = match (b.spec.kind, b.confirm_replace.is_some()) {
                        (Kind::Save, true) => "Replace",
                        (Kind::Save, false) => "Save",
                        (Kind::Folder, _) => "Use This Folder",
                        _ => "Open",
                    };
                    let ok = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(ACCENT()).min_size(vec2(110.0, row_h));
                    if ui.add_enabled(b.can_confirm(), ok).clicked() {
                        if let Some(r) = b.confirm() {
                            done = Some(Some(r));
                        }
                    }
                    if ui.add(egui::Button::new("Cancel").min_size(vec2(90.0, row_h))).clicked() {
                        done = Some(None);
                    }
                });
            });
        });
    });
    ctx.move_to_top(win.response.layer_id);
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        done = Some(None);
    }
    match done {
        None => {
            STATE.with(|s| s.borrow_mut().browser = Some(b));
            None
        }
        Some(None) => None,
        Some(Some(paths)) => {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.last_dir = Some(b.dir.clone());
                s.answers.insert(b.key.clone(), paths);
            });
            b.retry.take()
        }
    }
}
