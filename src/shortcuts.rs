//! Keyboard shortcuts: the command table with Audition's default keys, the
//! user's changes (saved in Preferences), key handling, and the Keyboard
//! Shortcuts editor (Edit > Keyboard Shortcuts).

use eframe::egui::{self, Key, Modifiers, RichText};

use crate::app::{Action, App, MarkerKind, Mode, Tool};
use crate::markers_ui::ZeroMode;
use crate::theme::*;

/// A key with modifiers. `ctrl` is Ctrl on Windows and Linux, ⌘ on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Chord {
    /// "Ctrl+Shift+S", "Alt+Space", "M", "=" …
    pub fn parse(s: &str) -> Option<Chord> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        let mut rest = s;
        loop {
            let lower = rest.to_ascii_lowercase();
            let strip = |p: &str| rest.get(p.len()..).unwrap_or("");
            if lower.starts_with("ctrl+") || lower.starts_with("cmd+") {
                ctrl = true;
                rest = strip(if lower.starts_with("ctrl+") { "ctrl+" } else { "cmd+" });
            } else if lower.starts_with("shift+") {
                shift = true;
                rest = strip("shift+");
            } else if lower.starts_with("alt+") || lower.starts_with("opt+") {
                alt = true;
                rest = strip("alt+");
            } else {
                break;
            }
        }
        let key = Key::from_name(rest).or_else(|| Key::from_name(&rest.to_ascii_uppercase()))?;
        Some(Chord { ctrl, shift, alt, key })
    }

    /// Text form, as `parse` reads it.
    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s += "Ctrl+";
        }
        if self.alt {
            s += "Alt+";
        }
        if self.shift {
            s += "Shift+";
        }
        s += self.key.name();
        s
    }

    /// For menus: ⌘⇧S on macOS, Ctrl+Shift+S elsewhere.
    pub fn label(&self) -> String {
        if cfg!(target_os = "macos") {
            let mut s = String::new();
            if self.alt {
                s += "⌥";
            }
            if self.shift {
                s += "⇧";
            }
            if self.ctrl {
                s += "⌘";
            }
            s += self.key.symbol_or_name();
            s
        } else {
            let mut s = String::new();
            if self.ctrl {
                s += "Ctrl+";
            }
            if self.alt {
                s += "Alt+";
            }
            if self.shift {
                s += "Shift+";
            }
            s += self.key.symbol_or_name();
            s
        }
    }

    fn modifiers(&self) -> Modifiers {
        let mut m = Modifiers::NONE;
        if self.ctrl {
            m = m | Modifiers::COMMAND;
        }
        if self.shift {
            m = m | Modifiers::SHIFT;
        }
        if self.alt {
            m = m | Modifiers::ALT;
        }
        m
    }

    fn weight(&self) -> u8 {
        self.ctrl as u8 + self.shift as u8 + self.alt as u8
    }

    /// Pressed this frame? Shift and Alt must match exactly (so Shift+D
    /// isn't also D), except for keys that need Shift to type (+).
    fn consume(&self, i: &mut egui::InputState) -> bool {
        let m = i.modifiers;
        if (m.shift && !self.shift && self.key != Key::Plus) || (m.alt && !self.alt) {
            return false;
        }
        i.consume_key(self.modifiers(), self.key)
    }

    fn from_event(key: Key, m: Modifiers) -> Chord {
        Chord { ctrl: m.command || m.ctrl, shift: m.shift, alt: m.alt, key }
    }
}

/// A command that can have keyboard shortcuts.
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub keys: &'static [&'static str],
    /// Also works while typing in a text box (Save, Open…).
    pub global: bool,
}

const fn c(id: &'static str, label: &'static str, group: &'static str, keys: &'static [&'static str]) -> Command {
    Command { id, label, group, keys, global: false }
}

const fn g(id: &'static str, label: &'static str, group: &'static str, keys: &'static [&'static str]) -> Command {
    Command { id, label, group, keys, global: true }
}

/// Every command with a shortcut, with Audition's default keys.
pub const COMMANDS: &[Command] = &[
    g("new_session", "New Multitrack Session", "File", &["Ctrl+N"]),
    g("new", "New Audio File", "File", &["Ctrl+Shift+N"]),
    g("open", "Open", "File", &["Ctrl+O"]),
    g("save", "Save", "File", &["Ctrl+S"]),
    g("save_as", "Save As", "File", &["Ctrl+Shift+S"]),
    c("save_sel", "Save Selection As", "File", &[]),
    c("save_all", "Save All", "File", &[]),
    c("close", "Close", "File", &["Ctrl+W"]),
    c("close_all", "Close All", "File", &[]),
    g("quit", "Quit", "File", &["Ctrl+Q"]),
    g("prefs", "Preferences", "Edit", &["Ctrl+,"]),
    c("shortcuts", "Keyboard Shortcuts", "Edit", &[]),
    c("undo", "Undo", "Edit", &["Ctrl+Z"]),
    c("redo", "Redo", "Edit", &["Ctrl+Shift+Z", "Ctrl+Y"]),
    c("repeat", "Repeat Previous Command", "Edit", &["Shift+R"]),
    c("cut", "Cut", "Edit", &["Ctrl+X"]),
    c("copy", "Copy", "Edit", &["Ctrl+C"]),
    c("copy_new", "Copy to New", "Edit", &["Alt+Shift+C"]),
    c("paste", "Paste", "Edit", &["Ctrl+V"]),
    c("paste_new", "Paste to New", "Edit", &["Ctrl+Alt+V"]),
    c("mix_paste", "Mix Paste", "Edit", &["Ctrl+Shift+V"]),
    c("delete", "Delete", "Edit", &["Delete", "Backspace"]),
    c("crop", "Crop", "Edit", &["Ctrl+T"]),
    c("select_all", "Select All", "Edit", &["Ctrl+A"]),
    c("deselect", "Deselect All", "Edit", &["Escape"]),
    c("heal", "Auto Heal Selection", "Edit", &["Ctrl+U"]),
    c("capture_noise", "Capture Noise Print", "Effects", &["Shift+P"]),
    c("marker", "Add Cue Marker", "Markers", &["M"]),
    c("marker_cd", "Add CD Track Marker", "Markers", &["Shift+M"]),
    c("marker_cart", "Add Cartridge Timer Marker", "Markers", &[]),
    c("prev_marker", "Move Playhead to Previous Marker", "Markers", &["Left"]),
    c("next_marker", "Move Playhead to Next Marker", "Markers", &["Right"]),
    c("zc_in", "Zero Crossings: Adjust Selection Inward", "Edit", &["Shift+I"]),
    c("zc_out", "Zero Crossings: Adjust Selection Outward", "Edit", &["Shift+O"]),
    c("zc_ll", "Zero Crossings: Adjust Left Side to Left", "Edit", &["Shift+H"]),
    c("zc_lr", "Zero Crossings: Adjust Left Side to Right", "Edit", &["Shift+J"]),
    c("zc_rl", "Zero Crossings: Adjust Right Side to Left", "Edit", &["Shift+K"]),
    c("zc_rr", "Zero Crossings: Adjust Right Side to Right", "Edit", &["Shift+L"]),
    c("play", "Play/Stop", "Transport", &["Space"]),
    c("record", "Record", "Transport", &["Shift+Space"]),
    c("punch", "Punch In (Record over Selection)", "Transport", &["Ctrl+Shift+Space"]),
    c("play_roll", "Play with Pre-roll and Post-roll", "Transport", &["Alt+Space"]),
    c("to_start", "Move Playhead to Start", "Transport", &["Home"]),
    c("to_end", "Move Playhead to End", "Transport", &["End"]),
    c("zoom_in", "Zoom In (Time)", "View", &["=", "+"]),
    c("zoom_out", "Zoom Out (Time)", "View", &["-"]),
    c("zoom_full", "Zoom Out Full", "View", &["\\"]),
    c("spectral", "Show Spectral Frequency Display", "View", &["Shift+D"]),
    c("mode_waveform", "Waveform Editor", "View", &["9"]),
    c("mode_multitrack", "Multitrack Editor", "View", &["0"]),
    c("tool_select", "Time Selection Tool", "Tools", &["T"]),
    c("tool_hand", "Hand Tool", "Tools", &["H"]),
    c("tool_marquee", "Marquee Selection Tool", "Tools", &["E"]),
    c("tool_lasso", "Lasso Selection Tool", "Tools", &["D"]),
    c("tool_brush", "Paintbrush Selection Tool", "Tools", &["P"]),
    c("tool_heal", "Spot Healing Brush Tool", "Tools", &["B"]),
    c("mt_add_track", "Add Audio Track", "Multitrack", &["Alt+A"]),
    c("mt_add_bus", "Add Bus Track", "Multitrack", &["Alt+B"]),
    c("mt_split", "Split Clip at Playhead", "Multitrack", &["Ctrl+K"]),
];

/// The action a command runs.
pub fn action(id: &str) -> Option<Action> {
    let tool = |t: Tool| Action::Status(format!("__tool:{}", t as u8));
    Some(match id {
        "new_session" => Action::NewSession,
        "new" => Action::New,
        "open" => Action::Open,
        "save" => Action::Save,
        "save_as" => Action::SaveAs,
        "save_sel" => Action::SaveSelectionAs,
        "save_all" => Action::SaveAll,
        "close" => Action::Close(usize::MAX),
        "close_all" => Action::CloseAll,
        "quit" => Action::Quit,
        "prefs" => Action::Preferences,
        "shortcuts" => Action::Status("__shortcuts".into()),
        "undo" => Action::Undo,
        "redo" => Action::Redo,
        "repeat" => Action::RepeatLast,
        "cut" => Action::Cut,
        "copy" => Action::Copy,
        "copy_new" => Action::CopyToNew,
        "paste" => Action::Paste,
        "paste_new" => Action::PasteNew,
        "mix_paste" => Action::Status("__mix_paste".into()),
        "delete" => Action::Delete,
        "crop" => Action::Crop,
        "select_all" => Action::SelectAll,
        "deselect" => Action::Deselect,
        "heal" => Action::SpectralHeal(None),
        "capture_noise" => Action::CaptureNoise,
        "marker" => Action::AddMarker,
        "marker_cd" => Action::AddMarkerKind(MarkerKind::CdTrack),
        "marker_cart" => Action::AddMarkerKind(MarkerKind::CartTimer),
        "prev_marker" => Action::PrevMarker,
        "next_marker" => Action::NextMarker,
        "zc_in" => Action::ZeroCross(ZeroMode::Inward),
        "zc_out" => Action::ZeroCross(ZeroMode::Outward),
        "zc_ll" => Action::ZeroCross(ZeroMode::LeftToLeft),
        "zc_lr" => Action::ZeroCross(ZeroMode::LeftToRight),
        "zc_rl" => Action::ZeroCross(ZeroMode::RightToLeft),
        "zc_rr" => Action::ZeroCross(ZeroMode::RightToRight),
        "play" => Action::PlayToggle,
        "record" => Action::Record,
        "punch" => Action::PunchRecord,
        "play_roll" => Action::PlayWithRoll,
        "to_start" => Action::ToStart,
        "to_end" => Action::ToEnd,
        "zoom_in" => Action::ZoomIn,
        "zoom_out" => Action::ZoomOut,
        "zoom_full" => Action::ZoomFull,
        "spectral" => Action::Status("__toggle_spectral".into()),
        "mode_waveform" => Action::SetMode(Mode::Waveform),
        "mode_multitrack" => Action::SetMode(Mode::Multitrack),
        "tool_select" => tool(Tool::Selection),
        "tool_hand" => tool(Tool::Hand),
        "tool_marquee" => tool(Tool::Marquee),
        "tool_lasso" => tool(Tool::Lasso),
        "tool_brush" => tool(Tool::Brush),
        "tool_heal" => tool(Tool::SpotHeal),
        "mt_add_track" => Action::MtAddTrack,
        "mt_add_bus" => Action::MtAddBus,
        "mt_split" => Action::MtSplit,
        _ => return None,
    })
}

pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// The keys in use: defaults with the user's changes applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    pub keys: Vec<(&'static str, Vec<Chord>)>,
}

impl Keymap {
    /// `custom`: (command id, keys as text separated by ", "; empty = none).
    pub fn new(custom: &[(String, String)]) -> Keymap {
        let keys = COMMANDS
            .iter()
            .map(|c| {
                let text: Vec<String> = match custom.iter().find(|(id, _)| id == c.id) {
                    Some((_, v)) => v.split(", ").filter(|s| !s.trim().is_empty()).map(|s| s.to_string()).collect(),
                    None => c.keys.iter().map(|s| s.to_string()).collect(),
                };
                (c.id, text.iter().filter_map(|t| Chord::parse(t)).collect())
            })
            .collect();
        Keymap { keys }
    }

    pub fn get(&self, id: &str) -> &[Chord] {
        self.keys.iter().find(|(i, _)| *i == id).map(|(_, k)| k.as_slice()).unwrap_or(&[])
    }

    /// The first key of a command, for its menu item ("" if none).
    pub fn label(&self, id: &str) -> String {
        self.get(id).first().map(|c| c.label()).unwrap_or_default()
    }

    /// The changes from the defaults, as saved in Preferences.
    pub fn custom(&self) -> Vec<(String, String)> {
        self.keys
            .iter()
            .filter_map(|(id, chords)| {
                let text: Vec<String> = chords.iter().map(|c| c.text()).collect();
                let default: Vec<String> = command(id)?.keys.iter().filter_map(|k| Chord::parse(k)).map(|c| c.text()).collect();
                (text != default).then(|| (id.to_string(), text.join(", ")))
            })
            .collect()
    }

    /// Commands (other than `except`) that use `chord`.
    pub fn users(&self, chord: &Chord, except: &str) -> Vec<&'static str> {
        self.keys.iter().filter(|(id, k)| *id != except && k.contains(chord)).map(|(id, _)| *id).collect()
    }

    /// Commands whose keys were pressed this frame. `typing`: a text box
    /// has the keyboard, so only global commands count.
    pub fn pressed(&self, i: &mut egui::InputState, typing: bool) -> Vec<&'static str> {
        let mut all: Vec<(&Chord, &'static str)> = self
            .keys
            .iter()
            .filter(|(id, _)| !typing || command(id).map(|c| c.global).unwrap_or(false))
            .flat_map(|(id, ks)| ks.iter().map(move |k| (k, *id)))
            .collect();
        // Most specific first: Ctrl+Shift+S before Ctrl+S.
        all.sort_by_key(|(k, _)| std::cmp::Reverse(k.weight()));
        let mut out = Vec::new();
        for (k, id) in all {
            if k.consume(i) && !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}

/// Edit > Keyboard Shortcuts window state.
#[derive(Default)]
pub struct ShortcutEditor {
    pub filter: String,
    /// Command waiting for a key press.
    pub capturing: Option<&'static str>,
    pub message: String,
}

impl App {
    /// Apply the user's shortcut changes (from Preferences).
    pub fn rebuild_keymap(&mut self) {
        self.keymap = Keymap::new(&self.prefs.shortcuts);
        self.keymap_gen += 1;
    }

    fn save_keymap(&mut self) {
        self.prefs.shortcuts = self.keymap.custom();
        self.prefs.save();
        self.keymap_gen += 1;
    }

    /// The Keyboard Shortcuts window. Returns false when closed.
    pub fn shortcut_editor(&mut self, ctx: &egui::Context, ed: &mut ShortcutEditor) -> bool {
        let mut open = true;
        // A key press for the command being assigned.
        if let Some(id) = ed.capturing {
            let pressed = ctx.input_mut(|i| {
                let mut found = None;
                i.events.retain(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } if found.is_none() => {
                        found = Some(Chord::from_event(*key, *modifiers));
                        false
                    }
                    _ => true,
                });
                found
            });
            if let Some(ch) = pressed {
                ed.capturing = None;
                if ch.key == Key::Escape && !ch.ctrl && !ch.shift && !ch.alt {
                    ed.message = "Cancelled.".into();
                } else {
                    let users = self.keymap.users(&ch, id);
                    for u in &users {
                        if let Some((_, ks)) = self.keymap.keys.iter_mut().find(|(i, _)| i == u) {
                            ks.retain(|k| *k != ch);
                        }
                    }
                    if let Some((_, ks)) = self.keymap.keys.iter_mut().find(|(i, _)| *i == id) {
                        if !ks.contains(&ch) {
                            ks.push(ch);
                        }
                    }
                    ed.message = if users.is_empty() {
                        format!("{} → {}", command(id).map(|c| c.label).unwrap_or(id), ch.label())
                    } else {
                        let names: Vec<&str> = users.iter().filter_map(|u| command(u).map(|c| c.label)).collect();
                        format!("{} → {} (removed from {})", command(id).map(|c| c.label).unwrap_or(id), ch.label(), names.join(", "))
                    };
                    self.save_keymap();
                }
            }
        }
        egui::Window::new("Keyboard Shortcuts").open(&mut open).collapsible(false).default_size([620.0, 520.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Search");
                ui.add(egui::TextEdit::singleline(&mut ed.filter).desired_width(220.0));
                if ui.button("Reset All to Defaults").clicked() {
                    self.keymap = Keymap::new(&[]);
                    self.save_keymap();
                    ed.message = "All shortcuts restored to their defaults.".into();
                }
            });
            if ed.capturing.is_some() {
                ui.label(RichText::new("Press the new shortcut (Esc cancels)…").color(ACCENT()));
            } else if !ed.message.is_empty() {
                ui.label(RichText::new(&ed.message).color(TEXT_DIM()));
            }
            ui.separator();
            let f = ed.filter.to_lowercase();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("shortcuts").num_columns(3).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    let mut group = "";
                    for cmd in COMMANDS {
                        let keys = self.keymap.get(cmd.id).to_vec();
                        let key_text: Vec<String> = keys.iter().map(|k| k.label()).collect();
                        if !f.is_empty() && !cmd.label.to_lowercase().contains(&f) && !cmd.group.to_lowercase().contains(&f) && !key_text.join(" ").to_lowercase().contains(&f) {
                            continue;
                        }
                        if cmd.group != group {
                            group = cmd.group;
                            ui.label(RichText::new(group).strong().color(TEXT_DIM()));
                            ui.end_row();
                        }
                        ui.label(cmd.label);
                        ui.horizontal(|ui| {
                            for (i, k) in keys.iter().enumerate() {
                                if ui.button(k.label()).on_hover_text("Click to remove").clicked() {
                                    if let Some((_, ks)) = self.keymap.keys.iter_mut().find(|(id, _)| *id == cmd.id) {
                                        ks.remove(i);
                                    }
                                    self.save_keymap();
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.small_button("Add…").clicked() {
                                ed.capturing = Some(cmd.id);
                                ed.message.clear();
                            }
                            if ui.small_button("Default").clicked() {
                                if let Some((_, ks)) = self.keymap.keys.iter_mut().find(|(id, _)| *id == cmd.id) {
                                    *ks = cmd.keys.iter().filter_map(|k| Chord::parse(k)).collect();
                                }
                                self.save_keymap();
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        });
        open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // Needs egui's key names (the type-check stubs don't have them).
    #[cfg_attr(audemo_stub, ignore)]
    fn defaults_parse_and_custom_round_trips() {
        for c in COMMANDS {
            for k in c.keys {
                assert!(Chord::parse(k).is_some(), "{} key {k}", c.id);
            }
            assert!(action(c.id).is_some(), "{} has no action", c.id);
        }
        let mut km = Keymap::new(&[]);
        assert!(km.custom().is_empty());
        let ch = Chord::parse("Ctrl+Alt+M").unwrap();
        km.keys.iter_mut().find(|(i, _)| *i == "marker").unwrap().1 = vec![ch];
        let custom = km.custom();
        assert_eq!(custom, vec![("marker".to_string(), "Ctrl+Alt+M".to_string())]);
        assert_eq!(Keymap::new(&custom), km);
        // A command with every key removed stays unbound.
        let none = Keymap::new(&[("play".into(), String::new())]);
        assert!(none.get("play").is_empty());
    }
}
