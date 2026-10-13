//! Builds the Audition test pack: sessions exercising each part of the
//! .sesx support, with the audio they use and a checklist. Run with
//! `AUDEMO_PACK=<folder> cargo test -- --ignored make_audition_test_pack`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::export::{Marker, MarkerKind};
use crate::session::{Envelope, Session, Track, TrackKind};

const SR: u32 = 48000;

fn secs(s: f64) -> usize {
    (s * SR as f64).round() as usize
}

/// A short beep at every second (higher on whole bars of 4), so positions
/// and keyframes can be read off the waveform.
fn clicks(len_s: f64, hz: f32) -> Vec<Vec<f32>> {
    let n = secs(len_s);
    let mut v = vec![0.0f32; n];
    for k in 0..len_s.ceil() as usize {
        let f = if k % 4 == 0 { hz * 2.0 } else { hz };
        for i in 0..secs(0.05) {
            let at = secs(k as f64) + i;
            if at < n {
                let env = 1.0 - i as f32 / secs(0.05) as f32;
                v[at] = (std::f32::consts::TAU * f * i as f32 / SR as f32).sin() * 0.5 * env;
            }
        }
    }
    vec![v]
}

/// A steady tone (mono or stereo), easy to read level and pan from.
fn tone(len_s: f64, hz: f32, stereo: bool) -> Vec<Vec<f32>> {
    let c: Vec<f32> = (0..secs(len_s)).map(|i| (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin() * 0.5).collect();
    if stereo {
        vec![c.clone(), c]
    } else {
        vec![c]
    }
}

/// Pink-ish noise (stereo, uncorrelated).
fn noise(len_s: f64) -> Vec<Vec<f32>> {
    let mut seed = 0x1234_5678u32;
    let mut ch = || {
        let mut b = [0.0f32; 3];
        (0..secs(len_s))
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let w = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                b[0] = 0.997 * b[0] + 0.029 * w;
                b[1] = 0.985 * b[1] + 0.032 * w;
                b[2] = 0.950 * b[2] + 0.048 * w;
                (b[0] + b[1] + b[2] + 0.02 * w) * 2.0
            })
            .collect::<Vec<f32>>()
    };
    let l = ch();
    let r = ch();
    vec![l, r]
}

struct Pack {
    dir: PathBuf,
    files: BTreeMap<String, Vec<Vec<f32>>>,
}

impl Pack {
    fn source(&mut self, s: &mut Session, file: &str, audio: Vec<Vec<f32>>) -> (u64, PathBuf) {
        let id = s.add_source(file.trim_end_matches(".wav").into(), Arc::new(audio.clone()));
        self.files.insert(file.into(), audio);
        (id, self.dir.join("Audio").join(file))
    }

    fn write(&self, s: &Session, paths: &BTreeMap<u64, PathBuf>) {
        let xml = super::sesx::to_sesx(s, paths, &self.dir);
        std::fs::write(self.dir.join(format!("{}.sesx", s.name)), xml).unwrap();
    }
}

fn track(s: &mut Session, name: &str, colour: [u8; 3]) -> usize {
    let id = s.new_id();
    s.tracks.push(Track::new(id, name.into(), colour));
    s.tracks.len() - 1
}

fn clip(s: &mut Session, ti: usize, src: u64, at_s: f64) -> u64 {
    let id = s.insert_clip(ti, src, secs(at_s)).unwrap();
    let c = s.clip_mut(id).unwrap();
    c.fade_in = 0;
    c.fade_out = 0;
    id
}

const CHECKLIST: &str = include_str!("sesx_pack_checklist.txt");

#[test]
#[ignore]
fn make_audition_test_pack() {
    let Ok(dir) = std::env::var("AUDEMO_PACK") else {
        eprintln!("set AUDEMO_PACK to the folder to write");
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(dir.join("Audio")).unwrap();
    let mut pack = Pack { dir: dir.clone(), files: BTreeMap::new() };

    // 1. Basics: static track volume and pan, clip gain, mute, solo.
    {
        let mut s = Session::new(1, "01 Basics".into(), SR, 0);
        let mut paths = BTreeMap::new();
        let (a, pa) = pack.source(&mut s, "tone 440 Hz mono.wav", tone(8.0, 440.0, false));
        let (b, pb) = pack.source(&mut s, "tone 660 Hz stereo.wav", tone(8.0, 660.0, true));
        let (c, pc) = pack.source(&mut s, "clicks.wav", clicks(16.0, 1000.0));
        paths.extend([(a, pa), (b, pb), (c, pc)]);
        let t1 = track(&mut s, "Vol -6 Pan L50", [200, 80, 40]);
        s.tracks[t1].volume_db = -6.0;
        s.tracks[t1].pan = -50.0;
        clip(&mut s, t1, a, 0.0);
        let t2 = track(&mut s, "Vol +3 Pan R100", [60, 160, 220]);
        s.tracks[t2].volume_db = 3.0;
        s.tracks[t2].pan = 100.0;
        let k = clip(&mut s, t2, b, 4.0);
        s.clip_mut(k).unwrap().gain_db = -6.0;
        let t3 = track(&mut s, "Muted", [120, 200, 90]);
        s.tracks[t3].mute = true;
        let k = clip(&mut s, t3, c, 0.0);
        s.clip_mut(k).unwrap().name = "Clip gain +4.5".into();
        s.clip_mut(k).unwrap().gain_db = 4.5;
        let t4 = track(&mut s, "Solo", [220, 200, 60]);
        s.tracks[t4].solo = true;
        let k = clip(&mut s, t4, c, 2.0);
        s.clip_mut(k).unwrap().mute = true;
        s.clip_mut(k).unwrap().name = "Muted clip".into();
        pack.write(&s, &paths);
    }

    // 2. Fades of every shape.
    {
        let mut s = Session::new(2, "02 Fades".into(), SR, 0);
        let mut paths = BTreeMap::new();
        let (a, pa) = pack.source(&mut s, "noise stereo.wav", noise(6.0));
        paths.insert(a, pa);
        let shapes: [(&str, f32, bool); 5] = [("Linear", 0.0, false), ("Log +50", 0.5, false), ("Log -50", -0.5, false), ("Log +100", 1.0, false), ("Cosine", 0.0, true)];
        for (name, curve, cos) in shapes {
            let ti = track(&mut s, &format!("Fades: {name}"), [180, 120, 220]);
            let id = clip(&mut s, ti, a, 1.0);
            let c = s.clip_mut(id).unwrap();
            c.name = format!("2 s in, 1.5 s out, {name}");
            c.fade_in = secs(2.0);
            c.fade_out = secs(1.5);
            c.fade_in_curve = curve;
            c.fade_out_curve = curve;
            c.fade_in_cos = cos;
            c.fade_out_cos = cos;
        }
        pack.write(&s, &paths);
    }

    // 3. Track automation.
    {
        let mut s = Session::new(3, "03 Track automation".into(), SR, 0);
        let mut paths = BTreeMap::new();
        let (a, pa) = pack.source(&mut s, "tone 440 Hz mono.wav", tone(8.0, 440.0, false));
        let (b, pb) = pack.source(&mut s, "noise stereo.wav", noise(6.0));
        paths.extend([(a, pa), (b, pb)]);
        let t1 = track(&mut s, "Volume 0 > -20 > +6 dB", [200, 80, 40]);
        clip(&mut s, t1, a, 0.0);
        s.tracks[t1].vol_env = Envelope { points: vec![(secs(1.0), 0.0), (secs(3.0), -20.0), (secs(5.0), -20.0), (secs(7.0), 6.0)] };
        s.tracks[t1].show_env = true;
        let t2 = track(&mut s, "Pan L100 > R100", [60, 160, 220]);
        clip(&mut s, t2, b, 1.0);
        s.tracks[t2].pan_env = Envelope { points: vec![(secs(1.0), -100.0), (secs(4.0), 100.0), (secs(5.0), 0.0)] };
        s.tracks[t2].show_env = true;
        pack.write(&s, &paths);
    }

    // 4. Clip envelopes, on clips that start inside their source.
    {
        let mut s = Session::new(4, "04 Clip envelopes".into(), SR, 0);
        let mut paths = BTreeMap::new();
        let (a, pa) = pack.source(&mut s, "tone 440 Hz mono.wav", tone(8.0, 440.0, false));
        let (b, pb) = pack.source(&mut s, "noise stereo.wav", noise(6.0));
        let (c, pc) = pack.source(&mut s, "clicks.wav", clicks(16.0, 1000.0));
        paths.extend([(a, pa), (b, pb), (c, pc)]);
        // Untrimmed clip at 1 s: volume dips to -18 dB from 2 s to 4 s of the clip.
        let t1 = track(&mut s, "Clip volume (untrimmed)", [200, 80, 40]);
        let k = clip(&mut s, t1, a, 1.0);
        {
            let k = s.clip_mut(k).unwrap();
            k.name = "dip -18 dB, clip 2-4 s".into();
            let o = k.offset;
            k.vol_env = Envelope { points: vec![(o + secs(1.5), 0.0), (o + secs(2.0), -18.0), (o + secs(4.0), -18.0), (o + secs(4.5), 0.0)] };
        }
        // Trimmed: the clip shows source 2 s..6 s, placed at 3 s. Its volume
        // ramps from silence at its start to unity 1 s in (session 3 s → 4 s).
        let t2 = track(&mut s, "Clip volume (trimmed start)", [220, 140, 40]);
        let k = clip(&mut s, t2, c, 1.0);
        s.trim_start(k, secs(3.0));
        s.trim_end(k, secs(7.0));
        {
            let k = s.clip_mut(k).unwrap();
            k.name = "silence > 0 dB over first 1 s".into();
            let o = k.offset;
            k.vol_env = Envelope { points: vec![(o, crate::session::VOL_ENV_MIN), (o + secs(1.0), 0.0)] };
        }
        // Clip pan: left to right across the clip.
        let t3 = track(&mut s, "Clip pan L > R", [60, 160, 220]);
        let k = clip(&mut s, t3, b, 2.0);
        {
            let k = s.clip_mut(k).unwrap();
            k.name = "pan L100 at 0 s, R100 at 5 s".into();
            let o = k.offset;
            k.pan_env = Envelope { points: vec![(o, -100.0), (o + secs(5.0), 100.0)] };
        }
        pack.write(&s, &paths);
    }

    // 5. Markers and tempo.
    {
        let mut s = Session::new(5, "05 Markers and tempo".into(), SR, 0);
        let mut paths = BTreeMap::new();
        let (c, pc) = pack.source(&mut s, "clicks.wav", clicks(16.0, 1000.0));
        paths.insert(c, pc);
        let t = track(&mut s, "Clicks", [120, 200, 90]);
        clip(&mut s, t, c, 0.0);
        s.tempo = 100.0;
        s.beats_per_bar = 3;
        s.show_grid = true;
        s.markers = vec![
            Marker { pos: secs(1.0), len: 0, name: "Cue at 1 s".into(), kind: MarkerKind::Cue, description: "A plain cue marker".into() },
            Marker { pos: secs(3.0), len: secs(2.0), name: "Range 3-5 s".into(), kind: MarkerKind::Cue, description: String::new() },
            Marker { pos: secs(6.0), len: 0, name: "CD track at 6 s".into(), kind: MarkerKind::CdTrack, description: String::new() },
            Marker { pos: secs(8.0), len: secs(4.0), name: "Subclip 8-12 s".into(), kind: MarkerKind::Subclip, description: String::new() },
        ];
        pack.write(&s, &paths);
    }

    // 6. Buses, looped and locked clips.
    {
        let mut s = Session::new(6, "06 Buses and loops".into(), SR, 0);
        let mut paths = BTreeMap::new();
        let (a, pa) = pack.source(&mut s, "tone 440 Hz mono.wav", tone(8.0, 440.0, false));
        let (c, pc) = pack.source(&mut s, "clicks.wav", clicks(16.0, 1000.0));
        paths.extend([(a, pa), (c, pc)]);
        let bus = track(&mut s, "Bus -6 dB", [154, 154, 168]);
        s.tracks[bus].kind = TrackKind::Bus;
        s.tracks[bus].volume_db = -6.0;
        let bus_id = s.tracks[bus].id;
        let t1 = track(&mut s, "Into bus (looped clip)", [200, 80, 40]);
        s.tracks[t1].output = Some(bus_id);
        let k = clip(&mut s, t1, c, 0.0);
        s.trim_end(k, secs(4.0));
        s.clip_mut(k).unwrap().looped = true;
        s.trim_end(k, secs(20.0));
        s.clip_mut(k).unwrap().name = "4 s of clicks, looped to 20 s".into();
        let t2 = track(&mut s, "Into bus (locked clip)", [60, 160, 220]);
        s.tracks[t2].output = Some(bus_id);
        let k = clip(&mut s, t2, a, 2.0);
        s.clip_mut(k).unwrap().locked = true;
        s.clip_mut(k).unwrap().name = "Locked in time".into();
        pack.write(&s, &paths);
    }

    for (name, audio) in &pack.files {
        crate::export::save(&dir.join("Audio").join(name), audio.as_slice(), SR, &crate::export::ExportSettings::default(), &Default::default(), &[], &Default::default()).unwrap();
    }
    std::fs::write(dir.join("CHECKLIST.txt"), CHECKLIST.replace('\n', "\r\n")).unwrap();
}
