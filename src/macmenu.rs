//! Native macOS menu bar (muda). On Windows and Linux the menus stay inside
//! the window, as Audition's do.

use std::path::PathBuf;

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use crate::app::{Action, App, Dialog, Mode, Panel, FAVORITES};
use crate::dsp::effects::Category;

pub struct NativeMenu {
    _menu: Menu,
    recent: Vec<PathBuf>,
    /// The effect list this menu was built from (plug-ins change it).
    effects_gen: u64,
    /// The shortcuts and snapping settings it shows.
    keymap_gen: u64,
    snap: (bool, bool),
}

/// The menu accelerator for a command's first shortcut. Keys without ⌘ or
/// ⌥ are left to the editor (a menu would take them from text boxes).
fn ka(app: &App, id: &str) -> Option<Accelerator> {
    use eframe::egui::Key as K;
    let ch = *app.keymap.get(id).iter().find(|c| c.ctrl || c.alt)?;
    let code = match ch.key {
        K::A => Code::KeyA, K::B => Code::KeyB, K::C => Code::KeyC, K::D => Code::KeyD, K::E => Code::KeyE,
        K::F => Code::KeyF, K::G => Code::KeyG, K::H => Code::KeyH, K::I => Code::KeyI, K::J => Code::KeyJ,
        K::K => Code::KeyK, K::L => Code::KeyL, K::M => Code::KeyM, K::N => Code::KeyN, K::O => Code::KeyO,
        K::P => Code::KeyP, K::Q => Code::KeyQ, K::R => Code::KeyR, K::S => Code::KeyS, K::T => Code::KeyT,
        K::U => Code::KeyU, K::V => Code::KeyV, K::W => Code::KeyW, K::X => Code::KeyX, K::Y => Code::KeyY,
        K::Z => Code::KeyZ,
        K::Num0 => Code::Digit0, K::Num1 => Code::Digit1, K::Num2 => Code::Digit2, K::Num3 => Code::Digit3, K::Num4 => Code::Digit4,
        K::Num5 => Code::Digit5, K::Num6 => Code::Digit6, K::Num7 => Code::Digit7, K::Num8 => Code::Digit8, K::Num9 => Code::Digit9,
        K::Space => Code::Space, K::Comma => Code::Comma, K::Period => Code::Period, K::Minus => Code::Minus, K::Equals => Code::Equal,
        K::Slash => Code::Slash, K::Backslash => Code::Backslash, K::Semicolon => Code::Semicolon, K::Quote => Code::Quote,
        K::OpenBracket => Code::BracketLeft, K::CloseBracket => Code::BracketRight, K::Backtick => Code::Backquote,
        K::Enter => Code::Enter, K::Tab => Code::Tab, K::Escape => Code::Escape, K::Delete => Code::Delete, K::Backspace => Code::Backspace,
        K::Home => Code::Home, K::End => Code::End, K::PageUp => Code::PageUp, K::PageDown => Code::PageDown,
        K::ArrowLeft => Code::ArrowLeft, K::ArrowRight => Code::ArrowRight, K::ArrowUp => Code::ArrowUp, K::ArrowDown => Code::ArrowDown,
        K::F1 => Code::F1, K::F2 => Code::F2, K::F3 => Code::F3, K::F4 => Code::F4, K::F5 => Code::F5, K::F6 => Code::F6,
        K::F7 => Code::F7, K::F8 => Code::F8, K::F9 => Code::F9, K::F10 => Code::F10, K::F11 => Code::F11, K::F12 => Code::F12,
        _ => return None,
    };
    let mut m = Modifiers::empty();
    if ch.ctrl {
        m |= Modifiers::SUPER;
    }
    if ch.shift {
        m |= Modifiers::SHIFT;
    }
    if ch.alt {
        m |= Modifiers::ALT;
    }
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
        let _ = app_menu.append(&item("prefs", "Settings…", ka(app, "prefs")));
        let _ = app_menu.append(&sep());
        let _ = app_menu.append(&PredefinedMenuItem::hide(None));
        let _ = app_menu.append(&PredefinedMenuItem::hide_others(None));
        let _ = app_menu.append(&PredefinedMenuItem::show_all(None));
        let _ = app_menu.append(&sep());
        let _ = app_menu.append(&item("quit", "Quit Audemo", ka(app, "quit")));

        let file = Submenu::new("File", true);
        let new = Submenu::new("New", true);
        let _ = new.append(&item("new_session", "Multitrack Session…", ka(app, "new_session")));
        let _ = new.append(&item("new", "Audio File…", ka(app, "new")));
        let _ = file.append(&new);
        let _ = file.append(&item("open", "Open…", ka(app, "open")));
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
        let _ = file.append(&item("close", "Close", ka(app, "close")));
        let _ = file.append(&item("close_all", "Close All", None));
        let _ = file.append(&sep());
        let _ = file.append(&item("save", "Save", ka(app, "save")));
        let _ = file.append(&item("save_as", "Save As…", ka(app, "save_as")));
        let _ = file.append(&item("save_sel", "Save Selection As…", None));
        let _ = file.append(&item("save_all", "Save All", None));
        let _ = file.append(&sep());
        let import = Submenu::new("Import", true);
        let _ = import.append(&item("import_markers", "Markers…", None));
        let _ = file.append(&import);
        let export = Submenu::new("Export", true);
        let _ = export.append(&item("export_markers", "Markers…", None));
        let _ = file.append(&export);

        // Clipboard commands go to the focused text box when there is one
        // (see `text_command`), otherwise to the audio.
        let edit = Submenu::new("Edit", true);
        let _ = edit.append(&item("undo", "Undo", ka(app, "undo")));
        let _ = edit.append(&item("redo", "Redo", ka(app, "redo")));
        let _ = edit.append(&item("repeat", "Repeat Previous Command", None));
        let _ = edit.append(&sep());
        let _ = edit.append(&item("cut", "Cut", ka(app, "cut")));
        let _ = edit.append(&item("copy", "Copy", ka(app, "copy")));
        let _ = edit.append(&item("copy_new", "Copy to New", ka(app, "copy_new")));
        let _ = edit.append(&item("paste", "Paste", ka(app, "paste")));
        let _ = edit.append(&item("paste_new", "Paste to New", ka(app, "paste_new")));
        let _ = edit.append(&item("mix_paste", "Mix Paste…", ka(app, "mix_paste")));
        let _ = edit.append(&sep());
        let _ = edit.append(&item("delete", "Delete", None));
        let _ = edit.append(&item("crop", "Crop", ka(app, "crop")));
        let _ = edit.append(&item("heal", "Auto Heal Selection", ka(app, "heal")));
        let _ = edit.append(&sep());
        let select = Submenu::new("Select", true);
        let _ = select.append(&item("select_all", "Select All", ka(app, "select_all")));
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
        let _ = marker.append(&item("marker", "Add Cue Marker", ka(app, "marker")));
        let _ = marker.append(&item("marker_cd", "Add CD Track Marker", ka(app, "marker_cd")));
        let _ = marker.append(&item("marker_cart", "Add Cartridge Timer Marker", ka(app, "marker_cart")));
        let _ = marker.append(&sep());
        let _ = marker.append(&item("panel:Markers", "Show Markers Panel", None));
        let _ = marker.append(&item("panel:Playlist", "Show Playlist Panel", None));
        let _ = edit.append(&marker);
        let zc = Submenu::new("Zero Crossings", true);
        for (z, id) in crate::markers_ui::ZeroMode::ALL.into_iter().zip(["zc_in", "zc_out", "zc_ll", "zc_lr", "zc_rl", "zc_rr"]) {
            let _ = zc.append(&item(id, z.label(), ka(app, id)));
        }
        let _ = edit.append(&zc);
        let snapping = Submenu::new("Snapping", true);
        let tick = |on: bool, t: &str| if on { format!("✓ {t}") } else { format!("    {t}") };
        let _ = snapping.append(&item("snap_markers", &tick(app.prefs.snap_markers, "Snap to Markers"), None));
        let _ = snapping.append(&item("snap_zero", &tick(app.prefs.snap_zero, "Snap to Zero Crossings"), None));
        let _ = edit.append(&snapping);
        let _ = edit.append(&sep());
        let _ = edit.append(&item("shortcuts", "Keyboard Shortcuts…", ka(app, "shortcuts")));
        let _ = edit.append(&item("convert", "Convert Sample Type…", None));

        let multitrack = Submenu::new("Multitrack", true);
        let _ = multitrack.append(&item("mt_add_track", "Add Audio Track", ka(app, "mt_add_track")));
        let _ = multitrack.append(&item("mt_add_bus", "Add Bus Track", ka(app, "mt_add_bus")));
        let _ = multitrack.append(&item("mt_delete_track", "Delete Selected Track", None));
        let _ = multitrack.append(&sep());
        let _ = multitrack.append(&item("mt_insert", "Insert Files…", None));
        let _ = multitrack.append(&item("mt_insert_doc", "Insert Current File at Cursor", None));
        let _ = multitrack.append(&item("mt_split", "Split Clip at Playhead", ka(app, "mt_split")));
        let _ = multitrack.append(&sep());
        let mixdown = Submenu::new("Mixdown Session to New File", true);
        let _ = mixdown.append(&item("mt_mix_all", "Entire Session", None));
        let _ = mixdown.append(&item("mt_mix_sel", "Time Selection", None));
        let _ = multitrack.append(&mixdown);

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
                let _ = sub.append(&item("learn_sound", "Learn Sound Model", None));
                let _ = sub.append(&sep());
            }
            for (i, e) in app.effects.iter().enumerate().filter(|(_, e)| e.category == cat) {
                let label = if e.params.is_empty() { e.name.to_string() } else { format!("{}…", e.name) };
                let _ = sub.append(&item(&format!("fx:{i}"), &label, None));
            }
            let _ = effects.append(&sub);
        }
        let _ = effects.append(&sep());
        for (format, title) in [(crate::plugin::Format::Au, "Audio Units"), (crate::plugin::Format::Vst3, "VST 3")] {
            let sub = Submenu::new(title, true);
            let groups = crate::plugin_ui::plugins_by_vendor(&app.effects, format, |_| true);
            if groups.is_empty() {
                let _ = sub.append(&MenuItem::with_id(format!("none:{title}"), "None found (scan in the Audio Plug-In Manager)", false, None));
            }
            for (vendor, items) in groups {
                let vs = Submenu::new(&vendor, true);
                for (i, name) in items {
                    let _ = vs.append(&item(&format!("fx:{i}"), &format!("{name}…"), None));
                }
                let _ = sub.append(&vs);
            }
            let _ = effects.append(&sub);
        }
        let _ = effects.append(&item("plugin_manager", "Audio Plug-In Manager…", None));

        let favorites = Submenu::new("Favorites", true);
        for (i, (label, _, _)) in FAVORITES.iter().enumerate() {
            let _ = favorites.append(&item(&format!("fav:{i}"), label, None));
        }

        let view = Submenu::new("View", true);
        let _ = view.append(&item("mode_wave", "Waveform Editor", None));
        let _ = view.append(&item("mode_mt", "Multitrack Editor", None));
        let _ = view.append(&sep());
        let _ = view.append(&item("zoom_in", "Zoom In (Time)", None));
        let _ = view.append(&item("zoom_out", "Zoom Out (Time)", None));
        let _ = view.append(&item("zoom_full", "Zoom Out Full (All Axes)", None));
        let _ = view.append(&item("zoom_sel", "Zoom to Selection", None));
        let _ = view.append(&item("amp_in", "Zoom In (Amplitude)", None));
        let _ = view.append(&item("amp_out", "Zoom Out (Amplitude)", None));
        let _ = view.append(&sep());
        let _ = view.append(&item("spectral", "Show/Hide Spectral Frequency Display", None));
        let _ = view.append(&item("pitch_display", "Show/Hide Pitch Display", ka(app, "pitch_display")));
        let _ = view.append(&item("follow", "Follow Playhead On/Off", None));
        let _ = view.append(&item("play_roll", "Play with Pre-roll and Post-roll", ka(app, "play_roll")));
        let _ = view.append(&item("punch", "Punch In (Record over Selection)", ka(app, "punch")));

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

        for sub in [&app_menu, &file, &edit, &multitrack, &effects, &favorites, &view, &window, &help] {
            menu.append(sub).ok()?;
        }
        menu.init_for_nsapp();
        Some(NativeMenu { _menu: menu, recent: app.prefs.recent.clone(), effects_gen: app.effects_gen, keymap_gen: app.keymap_gen, snap: (app.prefs.snap_markers, app.prefs.snap_zero) })
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
        let stale = self
            .native_menu
            .as_ref()
            .map(|m| m.recent != self.prefs.recent || m.effects_gen != self.effects_gen || m.keymap_gen != self.keymap_gen || m.snap != (self.prefs.snap_markers, self.prefs.snap_zero))
            .unwrap_or(false);
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
                self.dialog = Some(Dialog::Shortcuts(Box::default()));
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
            "pitch_display" => {
                self.show_pitch = !self.show_pitch;
                self.show_spectral |= self.show_pitch;
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
            "plugin_manager" => Action::PluginManager,
            "play_roll" => Action::PlayWithRoll,
            "selview" => {
                self.show_bottom = !self.show_bottom;
                return;
            }
            "prefs" => Action::Preferences,
            "quit" => Action::Quit,
            "new" => Action::New,
            "new_session" => Action::NewSession,
            "mt_add_track" => Action::MtAddTrack,
            "mt_delete_track" => Action::MtDeleteTrack,
            "mt_add_bus" => Action::MtAddBus,
            "mt_insert" => Action::MtInsertFiles,
            "mt_insert_doc" => match self.doc() {
                Some(d) => Action::MtInsertDoc(d.id),
                None => return,
            },
            "mt_split" => Action::MtSplit,
            "mt_mix_all" => Action::MtMixdown(false),
            "mt_mix_sel" => Action::MtMixdown(true),
            "mode_wave" => Action::SetMode(Mode::Waveform),
            "mode_mt" => Action::SetMode(Mode::Multitrack),
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
            "heal" => Action::SpectralHeal(None),
            "select_all" => Action::SelectAll,
            "deselect" => Action::Deselect,
            "marker" => Action::AddMarker,
            "noise_print" => Action::CaptureNoise,
            "learn_sound" => Action::LearnSoundModel,
            "zoom_in" => Action::ZoomIn,
            "zoom_out" => Action::ZoomOut,
            "zoom_full" => Action::ZoomFull,
            "zoom_sel" => Action::ZoomSel,
            "amp_in" => Action::AmpIn,
            "amp_out" => Action::AmpOut,
            "import_markers" => Action::MarkerCmd(crate::markers_ui::MarkerCmd::ImportCsv),
            "export_markers" => Action::MarkerCmd(crate::markers_ui::MarkerCmd::ExportCsv),
            "snap_markers" => {
                self.prefs.snap_markers = !self.prefs.snap_markers;
                self.prefs.save();
                return;
            }
            "snap_zero" => {
                self.prefs.snap_zero = !self.prefs.snap_zero;
                self.prefs.save();
                return;
            }
            other => match crate::shortcuts::action(other) {
                Some(a) => a,
                None => return,
            },
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
