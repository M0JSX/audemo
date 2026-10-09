//! Native macOS menu bar (muda). On Windows and Linux the menus stay inside
//! the window, as Audition's do.

use std::path::PathBuf;

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use crate::app::{Action, App, Dialog, Panel, FAVORITES};
use crate::dsp::effects::Category;

pub struct NativeMenu {
    _menu: Menu,
    recent: Vec<PathBuf>,
}

fn acc(shift: bool, code: Code) -> Option<Accelerator> {
    let m = if shift { Modifiers::SUPER | Modifiers::SHIFT } else { Modifiers::SUPER };
    Some(Accelerator::new(Some(m), code))
}

fn acc_mods(m: Modifiers, code: Code) -> Option<Accelerator> {
    Some(Accelerator::new(Some(m), code))
}

fn item(id: &str, text: &str, accel: Option<Accelerator>) -> MenuItem {
    MenuItem::with_id(id, text, true, accel)
}

fn sep() -> PredefinedMenuItem {
    PredefinedMenuItem::separator()
}

impl NativeMenu {
    pub fn build(app: &App) -> Option<Self> {
        let menu = Menu::new();

        let app_menu = Submenu::new("Audemo", true);
        let _ = app_menu.append(&item("about", "About Audemo", None));
        let _ = app_menu.append(&sep());
        let _ = app_menu.append(&item("prefs", "Preferences…", acc(false, Code::Comma)));
        let _ = app_menu.append(&sep());
        let _ = app_menu.append(&PredefinedMenuItem::hide(None));
        let _ = app_menu.append(&PredefinedMenuItem::hide_others(None));
        let _ = app_menu.append(&PredefinedMenuItem::show_all(None));
        let _ = app_menu.append(&sep());
        let _ = app_menu.append(&item("quit", "Quit Audemo", acc(false, Code::KeyQ)));

        let file = Submenu::new("File", true);
        let new = Submenu::new("New", true);
        let _ = new.append(&item("new", "Audio File…", acc(false, Code::KeyN)));
        let _ = file.append(&new);
        let _ = file.append(&item("open", "Open…", acc(false, Code::KeyO)));
        let append = Submenu::new("Open Append", true);
        let _ = append.append(&item("append", "To Current…", None));
        let _ = file.append(&append);
        let recent = Submenu::new("Open Recent", true);
        for (i, r) in app.prefs.recent.iter().enumerate() {
            let name = r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| r.display().to_string());
            let _ = recent.append(&item(&format!("recent:{i}"), &name, None));
        }
        if !app.prefs.recent.is_empty() {
            let _ = recent.append(&sep());
        }
        let _ = recent.append(&item("clear_recent", "Clear Recent", None));
        let _ = file.append(&recent);
        let _ = file.append(&sep());
        let _ = file.append(&item("close", "Close", acc(false, Code::KeyW)));
        let _ = file.append(&item("close_all", "Close All", None));
        let _ = file.append(&sep());
        let _ = file.append(&item("save", "Save", acc(false, Code::KeyS)));
        let _ = file.append(&item("save_as", "Save As…", acc(true, Code::KeyS)));
        let _ = file.append(&item("save_sel", "Save Selection As…", None));
        let _ = file.append(&item("save_all", "Save All", None));

        // Clipboard commands go to the focused text box when there is one
        // (see `text_command`), otherwise to the audio.
        let edit = Submenu::new("Edit", true);
        let _ = edit.append(&item("undo", "Undo", acc(false, Code::KeyZ)));
        let _ = edit.append(&item("redo", "Redo", acc(true, Code::KeyZ)));
        let _ = edit.append(&item("repeat", "Repeat Previous Command", None));
        let _ = edit.append(&sep());
        let _ = edit.append(&item("cut", "Cut", acc(false, Code::KeyX)));
        let _ = edit.append(&item("copy", "Copy", acc(false, Code::KeyC)));
        let _ = edit.append(&item("copy_new", "Copy to New", acc_mods(Modifiers::ALT | Modifiers::SHIFT, Code::KeyC)));
        let _ = edit.append(&item("paste", "Paste", acc(false, Code::KeyV)));
        let _ = edit.append(&item("paste_new", "Paste to New", acc_mods(Modifiers::SUPER | Modifiers::ALT, Code::KeyV)));
        let _ = edit.append(&item("mix_paste", "Mix Paste…", acc(true, Code::KeyV)));
        let _ = edit.append(&sep());
        let _ = edit.append(&item("delete", "Delete", None));
        let _ = edit.append(&item("crop", "Crop", acc(false, Code::KeyT)));
        let _ = edit.append(&sep());
        let select = Submenu::new("Select", true);
        let _ = select.append(&item("select_all", "Select All", acc(false, Code::KeyA)));
        let _ = select.append(&item("deselect", "Deselect All", None));
        let _ = edit.append(&select);
        let insert = Submenu::new("Insert", true);
        for (label, id) in [("Silence…", "gen_silence"), ("Tones…", "gen_tone"), ("Noise…", "gen_noise")] {
            if let Some(i) = app.find_effect(id) {
                let _ = insert.append(&item(&format!("fx:{i}"), label, None));
            }
        }
        let _ = edit.append(&insert);
        let marker = Submenu::new("Marker", true);
        let _ = marker.append(&item("marker", "Add Marker", None));
        let _ = edit.append(&marker);
        let _ = edit.append(&item("convert", "Convert Sample Type…", None));

        let effects = Submenu::new("Effects", true);
        let _ = effects.append(&item("show_rack", "Show Effects Rack", None));
        let _ = effects.append(&sep());
        for (i, e) in app.effects.iter().enumerate().filter(|(_, e)| e.category == Category::Basic) {
            let _ = effects.append(&item(&format!("fx:{i}"), e.name, None));
        }
        let _ = effects.append(&sep());
        for cat in Category::MENU_ORDER {
            if cat == Category::Generate {
                let _ = effects.append(&sep());
            }
            let sub = Submenu::new(cat.name(), true);
            if cat == Category::Restoration {
                let _ = sub.append(&item("noise_print", "Capture Noise Print", None));
                let _ = sub.append(&sep());
            }
            for (i, e) in app.effects.iter().enumerate().filter(|(_, e)| e.category == cat) {
                let label = if e.params.is_empty() { e.name.to_string() } else { format!("{}…", e.name) };
                let _ = sub.append(&item(&format!("fx:{i}"), &label, None));
            }
            let _ = effects.append(&sub);
        }

        let favorites = Submenu::new("Favorites", true);
        for (i, (label, _, _)) in FAVORITES.iter().enumerate() {
            let _ = favorites.append(&item(&format!("fav:{i}"), label, None));
        }

        let view = Submenu::new("View", true);
        let _ = view.append(&item("zoom_in", "Zoom In (Time)", None));
        let _ = view.append(&item("zoom_out", "Zoom Out (Time)", None));
        let _ = view.append(&item("zoom_full", "Zoom Out Full (All Axes)", None));
        let _ = view.append(&item("zoom_sel", "Zoom to Selection", None));
        let _ = view.append(&item("amp_in", "Zoom In (Amplitude)", None));
        let _ = view.append(&item("amp_out", "Zoom Out (Amplitude)", None));
        let _ = view.append(&sep());
        let _ = view.append(&item("spectral", "Show/Hide Spectral Frequency Display", None));
        let _ = view.append(&item("follow", "Follow Playhead On/Off", None));

        let window = Submenu::new("Window", true);
        let workspace = Submenu::new("Workspace", true);
        let _ = workspace.append(&item("reset_ws", "Reset \"Default\" to Saved Layout", None));
        let _ = window.append(&workspace);
        let _ = window.append(&sep());
        let _ = window.append(&item("amp_stats", "Amplitude Statistics…", None));
        let _ = window.append(&sep());
        for p in Panel::ALL {
            let _ = window.append(&item(&format!("panel:{}", p.name()), p.name(), None));
        }
        let _ = window.append(&item("selview", "Selection/View", None));
        let _ = window.append(&sep());
        let _ = window.append(&PredefinedMenuItem::minimize(None));
        let _ = window.append(&PredefinedMenuItem::fullscreen(None));

        let help = Submenu::new("Help", true);
        let _ = help.append(&item("shortcuts", "Keyboard Shortcuts", None));

        for sub in [&app_menu, &file, &edit, &effects, &favorites, &view, &window, &help] {
            menu.append(sub).ok()?;
        }
        menu.init_for_nsapp();
        Some(NativeMenu { _menu: menu, recent: app.prefs.recent.clone() })
    }
}

impl App {
    pub fn native_menu_active(&self) -> bool {
        self.native_menu.is_some()
    }

    pub fn init_native_menu(&mut self) {
        self.native_menu = NativeMenu::build(self);
    }

    /// Handle native menu clicks; rebuild when the recent list changes.
    pub fn poll_native_menu(&mut self, ctx: &eframe::egui::Context) {
        let stale = self.native_menu.as_ref().map(|m| m.recent != self.prefs.recent).unwrap_or(false);
        if stale {
            self.native_menu = NativeMenu::build(self);
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id.0.clone();
            if ctx.wants_keyboard_input() && text_command(ctx, &id) {
                continue;
            }
            self.native_menu_command(&id);
        }
    }

    fn native_menu_command(&mut self, id: &str) {
        if let Some(i) = id.strip_prefix("fx:").and_then(|n| n.parse::<usize>().ok()) {
            self.actions.push(Action::OpenEffect(i));
            return;
        }
        if let Some(i) = id.strip_prefix("fav:").and_then(|n| n.parse::<usize>().ok()) {
            if let Some((_, eid, preset)) = FAVORITES.get(i) {
                self.actions.push(Action::ApplyFavorite(*eid, *preset));
            }
            return;
        }
        if let Some(i) = id.strip_prefix("recent:").and_then(|n| n.parse::<usize>().ok()) {
            if let Some(p) = self.prefs.recent.get(i).cloned() {
                self.actions.push(Action::OpenRecent(p));
            }
            return;
        }
        if let Some(name) = id.strip_prefix("panel:") {
            for p in Panel::ALL {
                if p.name() == name {
                    self.show_panel(p);
                }
            }
            return;
        }
        let action = match id {
            "about" => {
                self.dialog = Some(Dialog::About);
                return;
            }
            "shortcuts" => {
                self.dialog = Some(Dialog::Shortcuts);
                return;
            }
            "mix_paste" => {
                self.dialog = Some(Dialog::MixPaste { mode: 1, clip_db: 0.0, orig_db: 0.0 });
                return;
            }
            "convert" => {
                if let Some((rate, channels)) = self.doc().map(|d| (d.sample_rate, d.n_ch())) {
                    self.dialog = Some(Dialog::Convert { rate, channels });
                }
                return;
            }
            "clear_recent" => {
                self.prefs.recent.clear();
                self.prefs.save();
                return;
            }
            "show_rack" => {
                self.show_panel(Panel::EffectsRack);
                return;
            }
            "spectral" => {
                self.show_spectral = !self.show_spectral;
                return;
            }
            "follow" => {
                self.follow = !self.follow;
                return;
            }
            "reset_ws" => {
                self.reset_workspace();
                return;
            }
            "amp_stats" => Action::AmplitudeStatistics,
            "selview" => {
                self.show_bottom = !self.show_bottom;
                return;
            }
            "prefs" => Action::Preferences,
            "quit" => Action::Quit,
            "new" => Action::New,
            "open" => Action::Open,
            "append" => Action::OpenAppend,
            "close" => Action::Close(usize::MAX),
            "close_all" => Action::CloseAll,
            "save" => Action::Save,
            "save_as" => Action::SaveAs,
            "save_sel" => Action::SaveSelectionAs,
            "save_all" => Action::SaveAll,
            "undo" => Action::Undo,
            "redo" => Action::Redo,
            "repeat" => Action::RepeatLast,
            "cut" => Action::Cut,
            "copy" => Action::Copy,
            "copy_new" => Action::CopyToNew,
            "paste" => Action::Paste,
            "paste_new" => Action::PasteNew,
            "delete" => Action::Delete,
            "crop" => Action::Crop,
            "select_all" => Action::SelectAll,
            "deselect" => Action::Deselect,
            "marker" => Action::AddMarker,
            "noise_print" => Action::CaptureNoise,
            "zoom_in" => Action::ZoomIn,
            "zoom_out" => Action::ZoomOut,
            "zoom_full" => Action::ZoomFull,
            "zoom_sel" => Action::ZoomSel,
            "amp_in" => Action::AmpIn,
            "amp_out" => Action::AmpOut,
            _ => return,
        };
        self.actions.push(action);
    }
}

/// Send a clipboard/undo menu command to the focused text box as the
/// equivalent egui input. Returns false for commands that aren't text edits.
fn text_command(ctx: &eframe::egui::Context, id: &str) -> bool {
    use eframe::egui::{Event, Key, Modifiers as M};
    let key = |k: Key, m: M| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
    let ev = match id {
        "cut" => Event::Cut,
        "copy" => Event::Copy,
        "paste" => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => Event::Paste(t),
            Err(_) => return true,
        },
        "select_all" => key(Key::A, M::COMMAND),
        "undo" => key(Key::Z, M::COMMAND),
        "redo" => key(Key::Z, M::COMMAND | M::SHIFT),
        _ => return false,
    };
    ctx.input_mut(|i| i.events.push(ev));
    true
}
