//! Export a multitrack session as Final Cut Pro XML (xmeml), which Adobe
//! Premiere Pro and other editors import: tracks, clip placement, trims and
//! clip gain. Each Audemo track becomes one XML track per channel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::session::Session;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// file://localhost/… URL of an absolute path.
fn path_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = if s.starts_with('/') { s } else { format!("/{s}") };
    let mut out = String::from("file://localhost");
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out += &format!("%{b:02X}"),
        }
    }
    out
}

/// The session as xmeml. `files` gives each source's file, the file's
/// sample rate, channel count and length in session samples.
pub fn session_xml(s: &Session, files: &BTreeMap<u64, (PathBuf, u32, usize, usize)>, fps: u32) -> String {
    let sr = s.sample_rate as f64;
    let fr = |samples: usize| ((samples as f64 / sr) * fps as f64).round() as i64;
    let rate = format!("<rate><timebase>{fps}</timebase><ntsc>FALSE</ntsc></rate>");
    let duration = fr(s.end());
    let mut o = String::new();
    o += "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"4\">\n";
    o += &format!("  <sequence id=\"sequence-1\">\n    <name>{}</name>\n    <duration>{duration}</duration>\n    {rate}\n", esc(&s.name));
    o += "    <media>\n      <audio>\n";
    o += &format!("        <format><samplecharacteristics><depth>16</depth><samplerate>{}</samplerate></samplecharacteristics></format>\n", s.sample_rate);
    let mut clip_n = 0;
    let mut file_written: std::collections::HashSet<u64> = Default::default();
    for t in s.tracks.iter().filter(|t| !t.is_bus()) {
        let n_ch = t.clips.iter().filter_map(|c| files.get(&c.source)).map(|f| f.2).max().unwrap_or(1).clamp(1, 2);
        for ch in 0..n_ch {
            o += "        <track>\n";
            for c in &t.clips {
                let Some((path, fsr, f_ch, flen)) = files.get(&c.source) else { continue };
                if ch >= *f_ch {
                    continue;
                }
                clip_n += 1;
                // Clip positions are session samples, as is `flen`; a looped
                // clip exports its first pass.
                let src_fr = fr;
                let len = c.len.min(flen.saturating_sub(c.offset)).max(1);
                let gain = crate::dsp::util::db_to_lin(c.gain_db).clamp(0.0, 3.98);
                o += &format!("          <clipitem id=\"clipitem-{clip_n}\">\n");
                o += &format!("            <name>{}</name>\n            <enabled>{}</enabled>\n", esc(&c.name), if c.mute { "FALSE" } else { "TRUE" });
                o += &format!("            <duration>{}</duration>\n            {rate}\n", fr(*flen));
                o += &format!("            <start>{}</start>\n            <end>{}</end>\n", fr(c.start), fr(c.start + len));
                o += &format!("            <in>{}</in>\n            <out>{}</out>\n", src_fr(c.offset), src_fr(c.offset + len));
                if file_written.insert(c.source) {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    o += &format!("            <file id=\"file-{}\">\n              <name>{}</name>\n              <pathurl>{}</pathurl>\n              {rate}\n", c.source, esc(&name), esc(&path_url(path)));
                    o += &format!("              <duration>{}</duration>\n", fr(*flen));
                    o += &format!("              <media><audio><samplecharacteristics><depth>16</depth><samplerate>{fsr}</samplerate></samplecharacteristics><channelcount>{f_ch}</channelcount></audio></media>\n            </file>\n");
                } else {
                    o += &format!("            <file id=\"file-{}\"/>\n", c.source);
                }
                o += &format!("            <sourcetrack><mediatype>audio</mediatype><trackindex>{}</trackindex></sourcetrack>\n", ch + 1);
                if (gain - 1.0).abs() > 1e-4 {
                    o += "            <filter><effect><name>Audio Levels</name><effectid>audiolevels</effectid><effectcategory>audiolevels</effectcategory><effecttype>audiolevels</effecttype><mediatype>audio</mediatype>";
                    o += &format!("<parameter><parameterid>level</parameterid><name>Level</name><valuemin>0</valuemin><valuemax>3.98109</valuemax><value>{gain:.5}</value></parameter></effect></filter>\n");
                }
                o += "          </clipitem>\n";
            }
            o += "        </track>\n";
        }
    }
    o += "      </audio>\n    </media>\n  </sequence>\n</xmeml>\n";
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn writes_tracks_and_clips() {
        let mut s = Session::new(1, "Edit & Mix".into(), 48000, 1);
        let src = s.add_source("voice".into(), Arc::new(vec![vec![0.0; 96000], vec![0.0; 96000]]));
        let c = s.insert_clip(0, src, 48000).unwrap();
        {
            let k = s.clip_mut(c).unwrap();
            k.offset = 24000;
            k.len = 48000;
            k.gain_db = -6.0;
        }
        let mut files = BTreeMap::new();
        files.insert(src, (PathBuf::from("/Media/My Voice.wav"), 48000, 2, 96000));
        let x = session_xml(&s, &files, 30);
        assert!(x.contains("<name>Edit &amp; Mix</name>"));
        assert_eq!(x.matches("<clipitem").count(), 2, "one per channel");
        assert!(x.contains("<start>30</start>") && x.contains("<end>60</end>") && x.contains("<in>15</in>") && x.contains("<out>45</out>"));
        assert!(x.contains("file://localhost/Media/My%20Voice.wav"));
        assert!(x.contains("<value>0.50119</value>"));
    }
}
