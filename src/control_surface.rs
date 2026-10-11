//! Control surfaces speaking the Mackie Control protocol over MIDI (Mackie
//! MCU and the many controllers with an MCU mode): eight faders and channel
//! strips for a bank of multitrack tracks, the master fader, transport
//! buttons, the jog wheel and the track-name display.

use std::sync::mpsc::{channel, Receiver};

use crate::app::{Action, App, Mode};

/// MIDI ports available: (inputs, outputs).
pub fn ports() -> (Vec<String>, Vec<String>) {
    let ins = midir::MidiInput::new("Audemo")
        .map(|m| m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect())
        .unwrap_or_default();
    let outs = midir::MidiOutput::new("Audemo")
        .map(|m| m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect())
        .unwrap_or_default();
    (ins, outs)
}

/// What was last sent to the surface, so only changes are sent.
#[derive(Default, PartialEq, Clone)]
struct Shown {
    faders: [Option<u16>; 9],
    leds: Vec<(u8, bool)>,
    names: String,
}

pub struct Surface {
    _input: midir::MidiInputConnection<()>,
    output: Option<midir::MidiOutputConnection>,
    rx: Receiver<Vec<u8>>,
    /// First track shown on the eight strips.
    pub bank: usize,
    touched: [bool; 9],
    shown: Shown,
    pub description: String,
}

// Mackie Control note numbers.
const REC: u8 = 0x00;
const SOLO: u8 = 0x08;
const MUTE: u8 = 0x10;
const SELECT: u8 = 0x18;
const BANK_LEFT: u8 = 0x2E;
const BANK_RIGHT: u8 = 0x2F;
const CHAN_LEFT: u8 = 0x30;
const CHAN_RIGHT: u8 = 0x31;
const MARKER: u8 = 0x54;
const CYCLE: u8 = 0x56;
const REWIND: u8 = 0x5B;
const FORWARD: u8 = 0x5C;
const STOP: u8 = 0x5D;
const PLAY: u8 = 0x5E;
const RECORD: u8 = 0x5F;
const TOUCH: u8 = 0x68;

/// Fader position (0–16383) for a volume in dB: unity at about three
/// quarters of the travel, +6 dB at the top.
pub fn fader_from_db(db: f32) -> u16 {
    if db <= -95.0 {
        return 0;
    }
    let f = 10f32.powf((db - 6.0) / 40.0).clamp(0.0, 1.0);
    (f * 16383.0).round() as u16
}

pub fn db_from_fader(v: u16) -> f32 {
    let f = v as f32 / 16383.0;
    if f < 0.003 {
        -96.0
    } else {
        (6.0 + 40.0 * f.log10()).max(-96.0)
    }
}

impl Surface {
    /// Open the named ports (input required, output optional for feedback).
    pub fn open(input: &str, output: Option<&str>) -> Result<Surface, String> {
        let mi = midir::MidiInput::new("Audemo").map_err(|e| e.to_string())?;
        let port = mi.ports().into_iter().find(|p| mi.port_name(p).map(|n| n == input).unwrap_or(false)).ok_or_else(|| format!("MIDI input \"{input}\" isn't connected."))?;
        let (tx, rx) = channel();
        let conn = mi
            .connect(&port, "Audemo Control Surface", move |_, msg, _| {
                let _ = tx.send(msg.to_vec());
            }, ())
            .map_err(|e| e.to_string())?;
        let out = match output {
            Some(name) => {
                let mo = midir::MidiOutput::new("Audemo").map_err(|e| e.to_string())?;
                match mo.ports().into_iter().find(|p| mo.port_name(p).map(|n| n == name).unwrap_or(false)) {
                    Some(p) => mo.connect(&p, "Audemo Control Surface").ok(),
                    None => None,
                }
            }
            None => None,
        };
        let description = match (&out, output) {
            (Some(_), Some(o)) => format!("Mackie Control on {input} / {o}"),
            _ => format!("Mackie Control on {input} (no feedback)"),
        };
        Ok(Surface { _input: conn, output: out, rx, bank: 0, touched: [false; 9], shown: Shown::default(), description })
    }

    fn send(&mut self, msg: &[u8]) {
        if let Some(o) = self.output.as_mut() {
            let _ = o.send(msg);
        }
    }
}

impl App {
    /// Connect the surface chosen in Preferences (or drop it).
    pub fn apply_control_surface(&mut self) {
        self.surface = None;
        if self.prefs.cs_kind != 1 {
            return;
        }
        let Some(input) = self.prefs.cs_input.clone() else { return };
        match Surface::open(&input, self.prefs.cs_output.as_deref()) {
            Ok(s) => {
                self.set_status(format!("Control surface: {}", s.description));
                self.surface = Some(s);
            }
            Err(e) => self.set_status(format!("Control surface: {e}")),
        }
    }

    /// Handle surface messages and update its faders, lights and display.
    /// Call once per frame.
    pub fn poll_control_surface(&mut self) {
        let Some(mut surf) = self.surface.take() else { return };
        let msgs: Vec<Vec<u8>> = surf.rx.try_iter().collect();
        let si = self.active_session.filter(|&i| i < self.sessions.len());
        for m in msgs {
            if m.len() < 3 {
                continue;
            }
            let (status, d1, d2) = (m[0] & 0xF0, m[1], m[2]);
            let ch = (m[0] & 0x0F) as usize;
            match status {
                // Faders: 14-bit pitch bend per strip; channel 8 is the master.
                0xE0 => {
                    let v = (d1 as u16) | ((d2 as u16) << 7);
                    let db = db_from_fader(v);
                    if ch == 8 {
                        match si {
                            Some(si) if self.mode == Mode::Multitrack => {
                                self.sessions[si].master_db = db.min(12.0);
                                self.sessions[si].touch();
                            }
                            _ => {
                                self.engine.set_volume(crate::dsp::util::db_to_lin(db.min(6.0)));
                            }
                        }
                    } else if let Some(si) = si {
                        let ti = surf.bank + ch;
                        let s = &mut self.sessions[si];
                        if let Some(t) = s.tracks.get_mut(ti) {
                            if t.vol_env.is_empty() {
                                t.volume_db = db.min(12.0);
                                s.touch();
                            }
                        }
                    }
                    surf.shown.faders[ch.min(8)] = Some(v);
                }
                // V-Pots (relative): pan; the jog wheel moves the playhead.
                0xB0 => {
                    let delta = if d2 & 0x40 != 0 { -((d2 & 0x3F) as i32) } else { (d2 & 0x3F) as i32 };
                    if (0x10..0x18).contains(&d1) {
                        if let Some(si) = si {
                            let ti = surf.bank + (d1 - 0x10) as usize;
                            let s = &mut self.sessions[si];
                            if let Some(t) = s.tracks.get_mut(ti) {
                                if t.pan_env.is_empty() {
                                    t.pan = (t.pan + delta as f32 * 2.0).clamp(-100.0, 100.0);
                                    s.touch();
                                }
                            }
                        }
                    } else if d1 == 0x3C {
                        self.actions.push(Action::Nudge(delta as f64 * 0.1));
                    }
                }
                // Buttons: note on with velocity 127 = pressed.
                0x90 => {
                    let down = d2 > 0;
                    if (TOUCH..TOUCH + 9).contains(&d1) {
                        surf.touched[(d1 - TOUCH) as usize] = down;
                        continue;
                    }
                    if !down {
                        continue;
                    }
                    match d1 {
                        PLAY => self.actions.push(Action::PlayToggle),
                        STOP => self.actions.push(Action::Stop),
                        RECORD => self.actions.push(Action::Record),
                        REWIND => self.actions.push(Action::Nudge(-5.0)),
                        FORWARD => self.actions.push(Action::Nudge(5.0)),
                        CYCLE => self.looping = !self.looping,
                        MARKER => self.actions.push(Action::AddMarker),
                        BANK_LEFT => surf.bank = surf.bank.saturating_sub(8),
                        BANK_RIGHT => surf.bank += 8,
                        CHAN_LEFT => surf.bank = surf.bank.saturating_sub(1),
                        CHAN_RIGHT => surf.bank += 1,
                        n if n < 0x20 => {
                            let Some(si) = si else { continue };
                            let ti = surf.bank + (n % 8) as usize;
                            let s = &mut self.sessions[si];
                            if ti >= s.tracks.len() {
                                continue;
                            }
                            match n / 8 {
                                0 => s.tracks[ti].arm = !s.tracks[ti].arm,
                                1 => {
                                    s.push_undo("Solo Track");
                                    s.tracks[ti].solo = !s.tracks[ti].solo;
                                }
                                2 => {
                                    s.push_undo("Mute Track");
                                    s.tracks[ti].mute = !s.tracks[ti].mute;
                                }
                                _ => s.selected_track = ti,
                            }
                            s.touch();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Keep the bank on real tracks.
        let n_tracks = si.map(|i| self.sessions[i].tracks.len()).unwrap_or(0);
        surf.bank = surf.bank.min(n_tracks.saturating_sub(1));
        // Feedback: faders (unless being touched), lights, names.
        let mut want = surf.shown.clone();
        if let Some(si) = si {
            let s = &self.sessions[si];
            for k in 0..8 {
                let t = s.tracks.get(surf.bank + k);
                want.faders[k] = Some(t.map(|t| fader_from_db(t.volume_at(self.engine.status().pos.max(0.0) as usize))).unwrap_or(0));
            }
            want.faders[8] = Some(fader_from_db(s.master_db));
            let mut leds = Vec::new();
            for k in 0..8 {
                let t = s.tracks.get(surf.bank + k);
                leds.push((REC + k as u8, t.map(|t| t.arm).unwrap_or(false)));
                leds.push((SOLO + k as u8, t.map(|t| t.solo).unwrap_or(false)));
                leds.push((MUTE + k as u8, t.map(|t| t.mute).unwrap_or(false)));
                leds.push((SELECT + k as u8, surf.bank + k == s.selected_track));
            }
            want.names = (0..8).map(|k| s.tracks.get(surf.bank + k).map(|t| format!("{:<7}", t.name.chars().take(6).collect::<String>())).unwrap_or_else(|| " ".repeat(7))).collect();
            want.leds = leds;
        }
        let st = self.engine.status();
        want.leds.push((PLAY, st.playing));
        want.leds.push((STOP, !st.playing));
        want.leds.push((RECORD, self.engine.is_recording()));
        want.leds.push((CYCLE, self.looping));
        if want != surf.shown {
            for k in 0..9 {
                if let Some(v) = want.faders[k] {
                    if surf.shown.faders[k] != Some(v) && !surf.touched[k] {
                        surf.send(&[0xE0 | k as u8, (v & 0x7F) as u8, (v >> 7) as u8]);
                    }
                }
            }
            for (note, on) in &want.leds {
                if !surf.shown.leds.contains(&(*note, *on)) {
                    surf.send(&[0x90, *note, if *on { 0x7F } else { 0 }]);
                }
            }
            if want.names != surf.shown.names {
                // Upper line of the LCD: 7 characters per strip.
                let mut sx = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00];
                sx.extend(want.names.bytes().map(|b| if b.is_ascii() && b >= 0x20 { b } else { b'?' }).take(56));
                sx.push(0xF7);
                surf.send(&sx);
            }
            surf.shown = want;
        }
        self.surface = Some(surf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fader_law_round_trips() {
        for db in [-60.0f32, -24.0, -6.0, 0.0, 6.0] {
            assert!((db_from_fader(fader_from_db(db)) - db).abs() < 0.1, "{db}");
        }
        assert_eq!(fader_from_db(-120.0), 0);
        assert_eq!(db_from_fader(0), -96.0);
        assert!(fader_from_db(0.0) > 11000 && fader_from_db(0.0) < 13000, "unity near three quarters");
    }
}
