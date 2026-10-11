//! Adobe Audition multitrack sessions (.sesx, the CC format).
//!
//! Writing: the standard Audition XML (tracks, buses and routing, volume,
//! pan, mute, solo, arm, clips with trim, gain, fades, loop and lock, track
//! volume and pan automation, tempo), so Audition opens it, plus Audemo's
//! own complete session in an XML comment, which Audition ignores. Audemo
//! reads that comment back, so nothing Audition lacks (clip effects, sends,
//! Essential Sound, automation lanes, groups, effect racks) is lost.
//!
//! Reading a .sesx saved by Audition (no comment): the same elements are
//! read back. Audition's effects are not, since their settings are Adobe's.
//!
//! Scales, from Audition's own files: positions are samples; a fader's
//! static `parameterValue` is linear gain, its keyframes a normalised fader
//! position (0 = silence, 0.65 = 0 dB, 1 = +15 dB); pan's static value is
//! -100..100 and its keyframes 0..1 (0.5 = centre).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::dsp::effects::EffectDef;
use crate::export::{Marker, MarkerKind};
use crate::session::{self, Clip, Envelope, ParsedSession, Session, Track, TrackKind};

pub const EXT: &str = "sesx";
const EMBED_TAG: &str = "audemo-session";
const MASTER_ID: u64 = 10000;

// Component GUIDs Audition uses for the fader, mute and panner.
const GUID_TRACK_FADER: &str = "f822a180-4604-47ca-850f-e9f58477dfff";
const GUID_TRACK_MUTE: &str = "d51c1020-8741-4fcc-872a-7adb3074880d";
const GUID_TRACK_PAN: &str = "b5d53c02-08c3-41e7-b9e3-2015248cf80d";
const GUID_CLIP_FADER: &str = "673cd4d7-8c21-4528-ad40-e541ff6dcd70";
const GUID_CLIP_MUTE: &str = "29e428ef-8d16-4ee2-a01a-ada26356009f";
const GUID_CLIP_PAN: &str = "80da3284-8a9e-4165-a773-f372a4d619bc";
const GUID_MASTER_FADER: &str = "ab6aebff-3601-4db4-9933-781624df75f0";
const GUID_MASTER_MUTE: &str = "44393786-9106-434d-abf4-d9760c2096e2";
const GUID_MASTER_PAN: &str = "6fa88450-7b01-4b88-9ddd-3cb7930b5d51";

pub fn is_sesx(p: &Path) -> bool {
    p.extension().map(|e| e.eq_ignore_ascii_case(EXT)).unwrap_or(false)
}

// ------------------------------------------------------------------ scales

fn db_to_lin(db: f32) -> f64 {
    if db <= -144.0 {
        0.0
    } else {
        10f64.powf(db as f64 / 20.0)
    }
}

fn lin_to_db(g: f64) -> f32 {
    if g <= 1e-7 {
        -144.0
    } else {
        (20.0 * g.log10()) as f32
    }
}

/// Linear gain → Audition's normalised fader keyframe value.
fn gain_to_fader(g: f64) -> f64 {
    if g <= 0.0 {
        0.0
    } else if g <= 1.0 {
        0.65 * g.powf(1.0 / 5f64.sqrt())
    } else {
        (0.65 + 0.35 * (20.0 * g.log10() / 15.0)).min(1.0)
    }
}

fn fader_to_gain(v: f64) -> f64 {
    if v <= 0.0 {
        0.0
    } else if v <= 0.65 {
        (v / 0.65).powf(5f64.sqrt())
    } else {
        10f64.powf(((v - 0.65) / 0.35 * 15.0) / 20.0)
    }
}

fn fmt(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.9}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn hue_of(c: [u8; 3]) -> i32 {
    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d < 1e-4 {
        return -1;
    }
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h.rem_euclid(360.0)).round() as i32
}

fn colour_of_hue(h: i32) -> [u8; 3] {
    if h < 0 {
        return [0x3f, 0xdc, 0x9b];
    }
    let (s, v) = (0.62f32, 0.86f32);
    let c = v * s;
    let hp = (h as f32 % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
}

// ------------------------------------------------------------------ base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut o = String::with_capacity(data.len() * 4 / 3 + data.len() / 57 + 4);
    for (i, ch) in data.chunks(3).enumerate() {
        if i > 0 && i % 25 == 0 {
            o.push('\n');
        }
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        o.push(B64[(n >> 18) as usize & 63] as char);
        o.push(B64[(n >> 12) as usize & 63] as char);
        o.push(if ch.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if ch.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    o
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        buf = buf << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

// ------------------------------------------------------------------ XML reader

#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
    text: String,
}

impl Element {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.0 == k).map(|a| a.1.as_str())
    }
    fn num(&self, k: &str) -> Option<f64> {
        self.attr(k).and_then(|v| v.trim().parse().ok())
    }
    fn flag(&self, k: &str) -> bool {
        matches!(self.attr(k), Some("true") | Some("1"))
    }
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }
    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        o.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let ent = &rest[1..end];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) => {
                o.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                o.push('&');
                rest = &rest[1..];
            }
        }
    }
    o.push_str(rest);
    o
}

/// The <sesx> element and the bodies of comments.
fn parse_xml(src: &str) -> Result<(Element, Vec<String>), String> {
    let (mut doc, comments) = parse_doc(src)?;
    let root = doc.children.drain(..).find(|c| c.name == "sesx").ok_or("This isn't an Audition session (no <sesx> element).")?;
    Ok((root, comments))
}

/// A small XML parser: elements, attributes, text and CDATA. Also returns
/// the bodies of comments (for Audemo's embedded session).
fn parse_doc(src: &str) -> Result<(Element, Vec<String>), String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Element> = vec![Element { name: "#document".into(), ..Default::default() }];
    let mut comments = Vec::new();
    let err = |m: &str, at: usize| format!("Not a readable .sesx file ({m} at byte {at}).");
    while i < b.len() {
        if b[i] != b'<' {
            let end = src[i..].find('<').map(|k| i + k).unwrap_or(b.len());
            let t = &src[i..end];
            if !t.trim().is_empty() {
                stack.last_mut().unwrap().text.push_str(&unescape(t));
            }
            i = end;
            continue;
        }
        if src[i..].starts_with("<!--") {
            let end = src[i + 4..].find("-->").ok_or_else(|| err("unclosed comment", i))? + i + 4;
            comments.push(src[i + 4..end].to_string());
            i = end + 3;
        } else if src[i..].starts_with("<![CDATA[") {
            let end = src[i + 9..].find("]]>").ok_or_else(|| err("unclosed CDATA", i))? + i + 9;
            stack.last_mut().unwrap().text.push_str(&src[i + 9..end]);
            i = end + 3;
        } else if src[i..].starts_with("<?") {
            i = src[i..].find("?>").ok_or_else(|| err("unclosed declaration", i))? + i + 2;
        } else if src[i..].starts_with("<!") {
            i = src[i..].find('>').ok_or_else(|| err("unclosed declaration", i))? + i + 1;
        } else if src[i..].starts_with("</") {
            let end = src[i..].find('>').ok_or_else(|| err("unclosed tag", i))? + i;
            let name = src[i + 2..end].trim();
            let el = stack.pop().ok_or_else(|| err("unbalanced tags", i))?;
            if el.name != name || stack.is_empty() {
                return Err(err(&format!("</{name}> doesn't match <{}>", el.name), i));
            }
            stack.last_mut().unwrap().children.push(el);
            i = end + 1;
        } else {
            // Start tag: name, attributes, optional self-close.
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let mut el = Element { name: src[i + 1..j].to_string(), ..Default::default() };
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    return Err(err("unclosed tag", i));
                }
                if b[j] == b'>' {
                    stack.push(el);
                    j += 1;
                    break;
                }
                if b[j] == b'/' && b.get(j + 1) == Some(&b'>') {
                    stack.last_mut().unwrap().children.push(el);
                    j += 2;
                    break;
                }
                let k0 = j;
                while j < b.len() && b[j] != b'=' && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                    j += 1;
                }
                let key = src[k0..j].to_string();
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if b.get(j) != Some(&b'=') {
                    el.attrs.push((key, String::new()));
                    continue;
                }
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let q = *b.get(j).ok_or_else(|| err("unclosed tag", i))?;
                if q != b'"' && q != b'\'' {
                    return Err(err("unquoted attribute", j));
                }
                let v0 = j + 1;
                let v1 = src[v0..].find(q as char).ok_or_else(|| err("unclosed attribute", j))? + v0;
                el.attrs.push((key, unescape(&src[v0..v1])));
                j = v1 + 1;
            }
            i = j;
        }
    }
    let doc = stack.pop().ok_or_else(|| err("no document", 0))?;
    if !stack.is_empty() {
        return Err(err("unclosed elements", b.len()));
    }
    Ok((doc, comments))
}

// ------------------------------------------------------------------ markers (XMP)

fn guid(seed: u64) -> String {
    // Deterministic, unique per marker: a 128-bit value from two mixes.
    let mix = |mut x: u64| {
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
        x ^ (x >> 33)
    };
    let (a, b) = (mix(seed ^ 0x6175_6465_6d6f), mix(seed.wrapping_add(0x9e37_79b9_7f4a_7c15)));
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xffff, b >> 48, b & 0xffff_ffff_ffff)
}

/// Session markers as Audition's XMP block: cue markers (cart timers among
/// them), CD track markers and subclip markers, ranges with a duration.
fn xmp_markers(markers: &[Marker], rate: u32) -> String {
    if markers.is_empty() {
        return String::new();
    }
    let fr = format!("f{rate}");
    let track = |title: &str, ty: &str, want: &dyn Fn(MarkerKind) -> bool| {
        let mut items = String::new();
        for (i, m) in markers.iter().enumerate().filter(|(_, m)| want(m.kind)) {
            let g = guid(i as u64 * 1_000_003 + m.pos as u64);
            items += "                        <rdf:li rdf:parseType=\"Resource\">\n";
            items += &format!("                           <xmpDM:startTime>{}</xmpDM:startTime>\n", m.pos);
            if m.is_range() {
                items += &format!("                           <xmpDM:duration>{}</xmpDM:duration>\n", m.len);
            }
            items += &format!("                           <xmpDM:name>{}</xmpDM:name>\n", esc(&m.name));
            if !m.description.is_empty() {
                items += &format!("                           <xmpDM:comment>{}</xmpDM:comment>\n", esc(&m.description));
            }
            items += &format!("                           <xmpDM:cuePointParams>\n                              <rdf:Seq>\n                                 <rdf:li rdf:parseType=\"Resource\">\n                                    <xmpDM:key>marker_guid</xmpDM:key>\n                                    <xmpDM:value>xmp:id:{g}</xmpDM:value>\n                                 </rdf:li>\n                              </rdf:Seq>\n                           </xmpDM:cuePointParams>\n                           <xmpDM:guid>xmp:id:{g}</xmpDM:guid>\n");
            items += "                        </rdf:li>\n";
        }
        let body = if items.is_empty() { String::new() } else { format!("                  <xmpDM:markers>\n                     <rdf:Seq>\n{items}                     </rdf:Seq>\n                  </xmpDM:markers>\n") };
        format!("               <rdf:li rdf:parseType=\"Resource\">\n                  <xmpDM:trackName>{title}</xmpDM:trackName>\n                  <xmpDM:trackType>{ty}</xmpDM:trackType>\n                  <xmpDM:frameRate>{fr}</xmpDM:frameRate>\n{body}               </rdf:li>\n")
    };
    let tracks = track("CuePoint Markers", "Cue", &|k| matches!(k, MarkerKind::Cue | MarkerKind::CartTimer))
        + &track("CD Track Markers", "Track", &|k| k == MarkerKind::CdTrack)
        + &track("Subclip Markers", "InOut", &|k| k == MarkerKind::Subclip);
    format!(
        "    <xmpMetadata><![CDATA[<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Audemo\">\n   <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n      <rdf:Description rdf:about=\"\"\n            xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n            xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n            xmlns:xmpDM=\"http://ns.adobe.com/xmp/1.0/DynamicMedia/\">\n         <xmp:CreatorTool>Audemo</xmp:CreatorTool>\n         <dc:format>application/xml</dc:format>\n         <xmpDM:Tracks>\n            <rdf:Bag>\n{tracks}            </rdf:Bag>\n         </xmpDM:Tracks>\n      </rdf:Description>\n   </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>]]></xmpMetadata>\n"
    )
}

/// An XMP property written either as an attribute or as a child element.
fn xmp_field<'a>(el: &'a Element, key: &str) -> Option<&'a str> {
    el.attr(key).or_else(|| el.child(key).map(|c| c.text.as_str())).map(str::trim).filter(|v| !v.is_empty())
}

fn collect<'a>(el: &'a Element, name: &str, out: &mut Vec<&'a Element>) {
    for c in &el.children {
        if c.name == name {
            out.push(c);
        }
        collect(c, name, out);
    }
}

/// Markers from a session's XMP block.
fn read_xmp_markers(xmp: &str, rate: u32) -> Vec<Marker> {
    let Ok((doc, _)) = parse_doc(xmp) else { return Vec::new() };
    let mut lis = Vec::new();
    collect(&doc, "rdf:li", &mut lis);
    let mut out = Vec::new();
    for t in lis.into_iter().filter(|l| xmp_field(l, "xmpDM:trackType").is_some()) {
        let ty = xmp_field(t, "xmpDM:trackType").unwrap_or("Cue");
        let title = xmp_field(t, "xmpDM:trackName").unwrap_or("");
        let kind = match ty {
            "Track" => MarkerKind::CdTrack,
            "InOut" => MarkerKind::Subclip,
            _ if title.to_ascii_lowercase().contains("cart") => MarkerKind::CartTimer,
            _ => MarkerKind::Cue,
        };
        // Times count frames at the track's rate ("f48000": samples).
        let fr = xmp_field(t, "xmpDM:frameRate").and_then(|f| f.trim_start_matches('f').split('s').next().and_then(|n| n.parse::<f64>().ok())).filter(|f| *f > 0.0).unwrap_or(rate as f64);
        let scale = rate as f64 / fr;
        let Some(seq) = t.child("xmpDM:markers").and_then(|m| m.child("rdf:Seq")) else { continue };
        for m in seq.all("rdf:li") {
            let Some(start) = xmp_field(m, "xmpDM:startTime").and_then(|v| v.parse::<f64>().ok()) else { continue };
            let dur = xmp_field(m, "xmpDM:duration").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            out.push(Marker {
                pos: (start * scale).round().max(0.0) as usize,
                len: (dur * scale).round().max(0.0) as usize,
                name: xmp_field(m, "xmpDM:name").unwrap_or("").to_string(),
                kind,
                description: xmp_field(m, "xmpDM:comment").unwrap_or("").to_string(),
            });
        }
    }
    out.sort_by_key(|m| m.pos);
    out
}

// ------------------------------------------------------------------ writing

fn keyframes(points: &[(usize, f64)]) -> String {
    let mut o = String::from("              <parameterKeyframes enableSplines=\"false\" readOnly=\"false\">\n");
    for (pos, v) in points {
        o += &format!("                <parameterKeyframe sampleOffset=\"{pos}\" select=\"false\" type=\"linear\" value=\"{}\"/>\n", fmt(*v));
    }
    o += "              </parameterKeyframes>\n";
    o
}

fn param(index: u32, name: &str, value: f64, keys: Option<String>) -> String {
    let name_attr = if name.is_empty() { String::new() } else { format!(" name=\"{name}\"") };
    match keys {
        Some(k) => format!("            <parameter index=\"{index}\"{name_attr} parameterValue=\"{}\">\n{k}            </parameter>\n", fmt(value)),
        None => format!("            <parameter index=\"{index}\"{name_attr} parameterValue=\"{}\"/>\n", fmt(value)),
    }
}

fn fade_attrs(len: usize, curve: f32, cos: bool) -> (&'static str, i32) {
    if len == 0 {
        ("log", 19)
    } else if cos {
        ("cosine", 0)
    } else if curve.abs() < 1e-3 {
        ("linear", 0)
    } else {
        ("log", (curve * 100.0).round() as i32)
    }
}

fn track_params(t: &Track, output: &str, input: &str, master: bool) -> String {
    let (gf, gm, gp) = if master { (GUID_MASTER_FADER, GUID_MASTER_MUTE, GUID_MASTER_PAN) } else { (GUID_TRACK_FADER, GUID_TRACK_MUTE, GUID_TRACK_PAN) };
    let vol_keys = (!t.vol_env.points.is_empty()).then(|| keyframes(&t.vol_env.points.iter().map(|(p, db)| (*p, gain_to_fader(db_to_lin(*db)))).collect::<Vec<_>>()));
    let pan_keys = (!t.pan_env.points.is_empty()).then(|| keyframes(&t.pan_env.points.iter().map(|(p, v)| (*p, ((*v as f64 / 100.0) + 1.0) / 2.0)).collect::<Vec<_>>()));
    let m = if t.mute { 1.0 } else { 0.0 };
    let mut o = format!(
        "        <trackAudioParameters audioChannelType=\"stereo\" automationMode=\"1\" monitoring=\"false\" recordArmed=\"{}\" solo=\"{}\" soloSafe=\"{}\">\n          {output}\n          {input}\n",
        t.arm, t.solo, master
    );
    o += &format!("          <component componentGuid=\"{gf}\" componentID=\"Audition.Fader\" id=\"trackFader\" name=\"volume\" powered=\"true\">\n");
    o += &param(0, "volume", db_to_lin(t.volume_db), vol_keys);
    o += &param(1, "static gain", 1.0, None);
    o += "          </component>\n";
    o += &format!("          <component componentGuid=\"{gm}\" componentID=\"Audition.Mute\" id=\"trackMute\" name=\"Mute\" powered=\"true\">\n");
    o += &param(0, "", m, None);
    o += &param(1, "mute", m, None);
    o += "          </component>\n";
    o += &format!("          <component componentGuid=\"{gp}\" componentID=\"Audition.StereoPanner\" id=\"trackPan\" name=\"StereoPanner\" powered=\"true\">\n");
    o += &param(0, "Pan", t.pan as f64, pan_keys);
    o += "          </component>\n";
    o += "        </trackAudioParameters>\n";
    o
}

fn rel_path(p: &Path, base: &Path) -> String {
    match p.strip_prefix(base) {
        Ok(r) => r.to_string_lossy().to_string(),
        Err(_) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
    }
}

fn media_handler(p: &Path) -> &'static str {
    match p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() {
        Some("mp3") => "AmioMP3",
        Some("aif") | Some("aiff") | Some("aifc") => "AmioAiff",
        Some("flac") => "AmioFLAC",
        Some("ogg") | Some("oga") => "AmioOgg",
        Some("m4a") | Some("mp4") | Some("aac") => "AmioMPEG",
        _ => "AmioWav",
    }
}

/// The session as an Audition .sesx (with Audemo's own copy embedded).
/// `paths`: each used source's file; `base`: the session's folder.
pub fn to_sesx(s: &Session, paths: &BTreeMap<u64, PathBuf>, base: &Path) -> String {
    // Files: one entry per source that has a path.
    let mut file_ids: HashMap<u64, usize> = HashMap::new();
    let mut files = String::new();
    for (sid, p) in paths {
        let id = file_ids.len();
        file_ids.insert(*sid, id);
        files += &format!(
            "    <file absolutePath=\"{}\" id=\"{id}\" mediaHandler=\"{}\" relativePath=\"{}\"/>\n",
            esc(&p.to_string_lossy()),
            media_handler(p),
            esc(&rel_path(p, base))
        );
    }
    // Audition track ids: master 10000, then one per track.
    let ids: HashMap<u64, u64> = s.tracks.iter().enumerate().map(|(i, t)| (t.id, MASTER_ID + 1 + i as u64)).collect();
    let mut tracks = String::new();
    let mut duration = 0usize;
    for (i, t) in s.tracks.iter().enumerate() {
        let tid = ids[&t.id];
        let out_id = t.output.and_then(|b| ids.get(&b).copied()).unwrap_or(MASTER_ID);
        let output = format!("<trackOutput outputID=\"{out_id}\" type=\"trackID\"/>");
        let (tag, input) = match t.kind {
            TrackKind::Bus => ("busTrack", "<trackInput inputID=\"-1\"/>"),
            TrackKind::Audio => ("audioTrack", "<trackInput inputID=\"1\"/>"),
        };
        tracks += &format!(
            "      <{tag} automationLaneOpenState=\"{}\" id=\"{tid}\" index=\"{}\" select=\"false\" visible=\"true\">\n        <trackParameters trackHeight=\"{}\" trackHue=\"{}\" trackMinimized=\"false\">\n          <name>{}</name>\n        </trackParameters>\n",
            t.show_env,
            i + 1,
            t.height.round().max(40.0) as i32,
            hue_of(t.colour),
            esc(&t.name)
        );
        tracks += &track_params(t, &output, input, false);
        let mut clips: Vec<&Clip> = t.clips.iter().collect();
        clips.sort_by_key(|c| c.start);
        for (z, c) in clips.iter().enumerate() {
            let Some(fid) = file_ids.get(&c.source) else { continue };
            duration = duration.max(c.end());
            let src_ch = s.sources.get(&c.source).map(|x| x.audio.len()).filter(|n| *n > 0).unwrap_or(2);
            let (fi_t, fi_s) = fade_attrs(c.fade_in, c.fade_in_curve, c.fade_in_cos);
            let (fo_t, fo_s) = fade_attrs(c.fade_out, c.fade_out_curve, c.fade_out_cos);
            let m = if c.mute { 1.0 } else { 0.0 };
            tracks += &format!(
                "        <audioClip clipAutoCrossfade=\"true\" crossFadeHeadClipID=\"-1\" crossFadeTailClipID=\"-1\" endPoint=\"{}\" fileID=\"{fid}\" hue=\"-1\" id=\"{z}\" lockedInTime=\"{}\" looped=\"{}\" name=\"{}\" offline=\"false\" select=\"false\" sourceInPoint=\"{}\" sourceOutPoint=\"{}\" startPoint=\"{}\" zOrder=\"{z}\">\n",
                c.end(),
                c.locked,
                c.looped,
                esc(&c.name),
                c.offset,
                c.offset + c.len,
                c.start
            );
            tracks += &format!("          <component componentGuid=\"{GUID_CLIP_FADER}\" componentID=\"Audition.Fader\" id=\"clipGain\" name=\"volume\" powered=\"true\">\n");
            tracks += &param(0, "volume", db_to_lin(c.gain_db), None);
            tracks += &param(1, "static gain", 1.0, None);
            tracks += "          </component>\n";
            tracks += &format!("          <component componentGuid=\"{GUID_CLIP_MUTE}\" componentID=\"Audition.Mute\" id=\"clipMute\" name=\"Mute\" powered=\"true\">\n");
            tracks += &param(0, "", m, None);
            tracks += &param(1, "mute", m, None);
            tracks += "          </component>\n";
            tracks += &format!("          <component componentGuid=\"{GUID_CLIP_PAN}\" componentID=\"Audition.StereoPanner\" id=\"clipPan\" name=\"StereoPanner\" powered=\"true\">\n");
            tracks += &param(0, "Pan", 0.0, None);
            tracks += "          </component>\n";
            tracks += &format!("          <fadeIn crossFadeLinkType=\"linkedAsymmetric\" endPoint=\"{}\" shape=\"{fi_s}\" startPoint=\"0\" type=\"{fi_t}\"/>\n", c.fade_in.min(c.len));
            tracks += &format!("          <fadeOut crossFadeLinkType=\"linkedAsymmetric\" endPoint=\"{}\" shape=\"{fo_s}\" startPoint=\"{}\" type=\"{fo_t}\"/>\n", c.len, c.len - c.fade_out.min(c.len));
            tracks += "          <channelMap>";
            for ch in 0..2usize {
                tracks += &format!("<channel index=\"{ch}\" sourceIndex=\"{}\"/>", ch.min(src_ch - 1));
            }
            tracks += "</channelMap>\n        </audioClip>\n";
        }
        tracks += &format!("      </{tag}>\n");
    }
    // Master.
    let mut master = Track::new(0, "Mix".into(), [0, 0, 0]);
    master.volume_db = s.master_db;
    tracks += &format!(
        "      <masterTrack automationLaneOpenState=\"false\" id=\"{MASTER_ID}\" index=\"{}\" select=\"false\" visible=\"true\">\n        <trackParameters trackHeight=\"134\" trackHue=\"-1\" trackMinimized=\"false\">\n          <name>Mix</name>\n        </trackParameters>\n",
        s.tracks.len() + 1
    );
    tracks += &track_params(&master, "<trackOutput outputID=\"1\" type=\"hardwareOutput\"/>", "<trackInput inputID=\"-1\"/>", true);
    tracks += "      </masterTrack>\n";

    let linked = (s.tempo - 120.0).abs() < 1e-9 && s.beats_per_bar == 4;
    let native = session::to_text(s, paths, Some(base));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?>\n<!DOCTYPE sesx>\n<sesx version=\"1.9\">\n  <session appBuild=\"Audemo\" appVersion=\"{ver}\" audioChannelType=\"stereo\" bitDepth=\"32\" duration=\"{duration}\" sampleRate=\"{rate}\">\n    <tracks>\n{tracks}    </tracks>\n    <sessionState ctiPosition=\"{cti}\" smpteStart=\"0\">\n      <timeFormatState beatsPerBar=\"{bpb}\" beatsPerMinute=\"{bpm}\" customFrameRate=\"12\" linkToDefaultTimeSettings=\"{linked}\" noteLength=\"4\" subdivisions=\"16\" timeCodeDropFrame=\"false\" timeCodeFrameRate=\"30\" timeCodeNTSC=\"false\" timeFormat=\"timeFormatDecimal\"/>\n      <mixingOptionState defaultPanModeLogarithmic=\"true\" panPower=\"-3\" playOverlappingClips=\"false\"/>\n    </sessionState>\n{xmp}  </session>\n  <files>\n{files}  </files>\n  <!-- {EMBED_TAG} (Audemo's complete copy of this session; Audition ignores it)\n{embed}\n  -->\n</sesx>\n",
        ver = env!("CARGO_PKG_VERSION"),
        rate = s.sample_rate,
        cti = s.cursor,
        bpb = s.beats_per_bar,
        bpm = fmt(s.tempo),
        embed = b64_encode(native.as_bytes()),
        xmp = xmp_markers(&s.markers, s.sample_rate),
    )
}

// ------------------------------------------------------------------ reading

fn component<'a>(params: &'a Element, id: &str) -> Option<&'a Element> {
    params.all("component").find(|c| c.attr("id") == Some(id))
}

/// A component's parameter `index`: (static value, keyframes).
fn read_param(comp: &Element, index: u32) -> Option<(f64, Vec<(usize, f64)>)> {
    let p = comp.all("parameter").find(|p| p.num("index") == Some(index as f64))?;
    let v = p.num("parameterValue")?;
    let keys = p
        .child("parameterKeyframes")
        .map(|k| k.all("parameterKeyframe").filter_map(|f| Some((f.num("sampleOffset")?.max(0.0) as usize, f.num("value")?))).collect())
        .unwrap_or_default();
    Some((v, keys))
}

fn fade_curve(el: Option<&Element>) -> (usize, f32, bool) {
    let Some(f) = el else { return (0, 0.0, false) };
    let len = (f.num("endPoint").unwrap_or(0.0) - f.num("startPoint").unwrap_or(0.0)).max(0.0) as usize;
    match f.attr("type").unwrap_or("linear") {
        "cosine" => (len, 0.0, true),
        "linear" => (len, 0.0, false),
        _ => {
            let shape = f.num("shape").unwrap_or(19.0) as f32;
            (len, (shape / 100.0).clamp(-1.0, 1.0), false)
        }
    }
}

/// Where a file referenced by a .sesx is now: beside the session first
/// (relativePath), then where it was (absolutePath), then by name.
fn locate(file: &Element, base: Option<&Path>) -> Option<PathBuf> {
    let norm = |s: &str| PathBuf::from(if cfg!(windows) { s.replace('/', "\\") } else { s.replace('\\', "/") });
    let rel = file.attr("relativePath").filter(|r| !r.is_empty()).map(norm);
    let abs = file.attr("absolutePath").filter(|a| !a.is_empty()).map(norm);
    let mut cands = Vec::new();
    if let (Some(b), Some(r)) = (base, &rel) {
        cands.push(b.join(r));
    }
    if let Some(a) = &abs {
        cands.push(a.clone());
    }
    if let Some(b) = base {
        for p in [&rel, &abs].into_iter().flatten() {
            if let Some(n) = p.file_name() {
                cands.push(b.join(n));
            }
        }
    }
    cands.iter().find(|p| p.is_file()).cloned().or(abs).or_else(|| match (base, rel) {
        (Some(b), Some(r)) => Some(b.join(r)),
        _ => None,
    })
}

/// Open a .sesx. Audemo's own sessions come back exactly from the embedded
/// copy; Audition's are read from the XML.
pub fn from_sesx(id: u64, text: &str, base: Option<&Path>, effects: &[EffectDef]) -> Result<ParsedSession, String> {
    let (root, comments) = parse_xml(text)?;
    for c in &comments {
        let c = c.trim_start();
        if let Some(rest) = c.strip_prefix(EMBED_TAG) {
            let body = rest.split_once('\n').map(|x| x.1).unwrap_or("");
            if let Some(native) = b64_decode(body).and_then(|b| String::from_utf8(b).ok()) {
                if let Ok(p) = session::from_text(id, &native, base, effects) {
                    return Ok(p);
                }
            }
        }
    }
    let ses = root.child("session").ok_or("The .sesx file has no <session>.")?;
    let rate = ses.num("sampleRate").unwrap_or(48000.0).clamp(8000.0, 768000.0) as u32;
    let mut s = Session::new(id, "Untitled Session".into(), rate, 0);
    if let Some(tf) = ses.child("sessionState").and_then(|st| st.child("timeFormatState")) {
        s.tempo = tf.num("beatsPerMinute").unwrap_or(120.0).clamp(20.0, 400.0);
        s.beats_per_bar = tf.num("beatsPerBar").unwrap_or(4.0).clamp(1.0, 32.0) as u32;
    }
    // Files → sources.
    let mut sources = Vec::new();
    let mut file_src: HashMap<String, u64> = HashMap::new();
    if let Some(files) = root.child("files") {
        for f in files.all("file") {
            let Some(fid) = f.attr("id") else { continue };
            let Some(p) = locate(f, base) else { continue };
            let sid = s.new_id();
            let name = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            file_src.insert(fid.to_string(), sid);
            sources.push((sid, p, name));
        }
    }
    let tracks_el = ses.child("tracks").ok_or("The .sesx file has no tracks.")?;
    // Audition track id → Audemo track id (for routing).
    let mut tid_map: HashMap<String, u64> = HashMap::new();
    let mut outputs: Vec<(usize, String)> = Vec::new();
    for el in &tracks_el.children {
        let kind = match el.name.as_str() {
            "audioTrack" => TrackKind::Audio,
            "busTrack" => TrackKind::Bus,
            "masterTrack" => {
                if let Some(fader) = el.child("trackAudioParameters").and_then(|p| component(p, "trackFader")) {
                    if let Some((v, _)) = read_param(fader, 0) {
                        let g = v * read_param(fader, 1).map(|x| x.0).unwrap_or(1.0);
                        s.master_db = lin_to_db(g).clamp(-96.0, 12.0);
                    }
                }
                continue;
            }
            _ => continue, // video tracks and anything newer
        };
        let tp = el.child("trackParameters");
        let name = tp.and_then(|p| p.child("name")).map(|n| n.text.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Track {}", s.tracks.len() + 1));
        let hue = tp.and_then(|p| p.num("trackHue")).unwrap_or(-1.0) as i32;
        let new_id = s.new_id();
        let mut t = Track::new(new_id, name, colour_of_hue(hue));
        t.kind = kind;
        if let Some(h) = tp.and_then(|p| p.num("trackHeight")) {
            t.height = (h as f32).clamp(40.0, 400.0);
        }
        t.show_env = el.flag("automationLaneOpenState");
        if let Some(ap) = el.child("trackAudioParameters") {
            t.solo = ap.flag("solo");
            t.arm = ap.flag("recordArmed");
            if let Some(o) = ap.child("trackOutput") {
                if o.attr("type") == Some("trackID") {
                    if let Some(oid) = o.attr("outputID") {
                        outputs.push((s.tracks.len(), oid.to_string()));
                    }
                }
            }
            if let Some(fader) = component(ap, "trackFader") {
                if let Some((v, keys)) = read_param(fader, 0) {
                    let g = v * read_param(fader, 1).map(|x| x.0).unwrap_or(1.0);
                    t.volume_db = lin_to_db(g).clamp(-96.0, 12.0);
                    t.vol_env = Envelope { points: keys.iter().map(|(p, v)| (*p, lin_to_db(fader_to_gain(*v)).clamp(session::VOL_ENV_MIN, session::VOL_ENV_MAX))).collect() };
                }
            }
            if let Some(mute) = component(ap, "trackMute") {
                t.mute = read_param(mute, 1).or_else(|| read_param(mute, 0)).map(|x| x.0 >= 0.5).unwrap_or(false);
            }
            if let Some(pan) = component(ap, "trackPan") {
                if let Some((v, keys)) = read_param(pan, 0) {
                    t.pan = (v as f32).clamp(-100.0, 100.0);
                    t.pan_env = Envelope { points: keys.iter().map(|(p, v)| (*p, ((*v as f32) * 2.0 - 1.0).clamp(-1.0, 1.0) * 100.0)).collect() };
                }
            }
        }
        t.vol_env.points.sort_by_key(|p| p.0);
        t.pan_env.points.sort_by_key(|p| p.0);
        if let Some(aid) = el.attr("id") {
            tid_map.insert(aid.to_string(), new_id);
        }
        for c in el.all("audioClip") {
            let Some(&sid) = c.attr("fileID").and_then(|f| file_src.get(f)) else { continue };
            let start = c.num("startPoint").unwrap_or(0.0).max(0.0) as usize;
            let end = c.num("endPoint").unwrap_or(0.0).max(0.0) as usize;
            if end <= start {
                continue;
            }
            let len = end - start;
            let (fade_in, fade_in_curve, fade_in_cos) = fade_curve(c.child("fadeIn"));
            let (fade_out, fade_out_curve, fade_out_cos) = fade_curve(c.child("fadeOut"));
            let mut gain_db = 0.0;
            if let Some(g) = component(c, "clipGain") {
                if let Some((v, _)) = read_param(g, 0) {
                    gain_db = lin_to_db(v * read_param(g, 1).map(|x| x.0).unwrap_or(1.0)).clamp(-96.0, 24.0);
                }
            }
            let mute = component(c, "clipMute").and_then(|m| read_param(m, 1).or_else(|| read_param(m, 0))).map(|x| x.0 >= 0.5).unwrap_or(false);
            let cid = s.new_id();
            t.clips.push(Clip {
                id: cid,
                source: sid,
                name: c.attr("name").unwrap_or("Clip").to_string(),
                start,
                offset: c.num("sourceInPoint").unwrap_or(0.0).max(0.0) as usize,
                len,
                gain_db,
                fade_in: fade_in.min(len),
                fade_out: fade_out.min(len),
                fade_in_curve,
                fade_out_curve,
                fade_in_cos,
                fade_out_cos,
                mute,
                group: 0,
                looped: c.flag("looped"),
                locked: c.flag("lockedInTime"),
                fx: Vec::new(),
                fx_on: true,
                stretch: 100.0,
                es: None,
            });
        }
        t.clips.sort_by_key(|c| c.start);
        s.tracks.push(t);
    }
    for (ti, oid) in outputs {
        if let Some(&target) = tid_map.get(&oid) {
            if target != s.tracks[ti].id && s.tracks.iter().any(|t| t.id == target && t.kind == TrackKind::Bus) {
                s.tracks[ti].output = Some(target);
            }
        }
    }
    if let Some(x) = ses.child("xmpMetadata") {
        s.markers = read_xmp_markers(&x.text, rate);
    }
    if s.tracks.is_empty() {
        s.add_track_silent();
    }
    Ok(ParsedSession { session: s, sources })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_round_trip() {
        for db in [-30.3f32, -12.0, -3.0, 0.0, 6.0, 15.0] {
            let v = gain_to_fader(db_to_lin(db));
            assert!((lin_to_db(fader_to_gain(v)) - db).abs() < 0.01, "{db}");
        }
        assert!((gain_to_fader(1.0) - 0.65).abs() < 1e-9);
        // Calibration point from an Audition export: -30.3 dB -> 0.13656.
        assert!((gain_to_fader(db_to_lin(-30.3)) - 0.13656).abs() < 0.001);
        assert_eq!(b64_decode(&b64_encode(b"Audemo \x00\xff session")).unwrap(), b"Audemo \x00\xff session");
        assert_eq!(hue_of(colour_of_hue(200)), 200);
    }

    #[test]
    fn reads_audition_session() {
        let xml = r###"<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<!DOCTYPE sesx>
<sesx version="1.9">
  <session appBuild="25.2" appVersion="25.2" audioChannelType="stereo" bitDepth="32" duration="96000" sampleRate="48000">
    <tracks>
      <audioTrack automationLaneOpenState="false" id="10001" index="1" select="false" visible="true">
        <trackParameters trackHeight="134" trackHue="40" trackMinimized="false"><name>Voice &amp; Music</name></trackParameters>
        <trackAudioParameters audioChannelType="stereo" automationMode="1" monitoring="false" recordArmed="false" solo="true" soloSafe="false">
          <trackOutput outputID="10003" type="trackID"/>
          <trackInput inputID="1"/>
          <component componentGuid="x" componentID="Audition.Fader" id="trackFader" name="volume" powered="true">
            <parameter index="0" name="volume" parameterValue="0.5">
              <parameterKeyframes enableSplines="false" readOnly="false">
                <parameterKeyframe sampleOffset="0" select="false" type="linear" value="0.65"/>
                <parameterKeyframe sampleOffset="48000" select="false" type="linear" value="0.13656"/>
              </parameterKeyframes>
            </parameter>
            <parameter index="1" name="static gain" parameterValue="1"/>
          </component>
          <component componentGuid="x" componentID="Audition.Mute" id="trackMute" name="Mute" powered="true">
            <parameter index="0" parameterValue="1"/><parameter index="1" name="mute" parameterValue="1"/>
          </component>
          <component componentGuid="x" componentID="Audition.StereoPanner" id="trackPan" name="StereoPanner" powered="true">
            <parameter index="0" name="Pan" parameterValue="-34.5"/>
          </component>
        </trackAudioParameters>
        <audioClip clipAutoCrossfade="true" crossFadeHeadClipID="-1" crossFadeTailClipID="-1" endPoint="72000" fileID="0" hue="-1" id="0" lockedInTime="true" looped="false" name="Take 1" offline="false" select="false" sourceInPoint="1000" sourceOutPoint="49000" startPoint="24000" zOrder="0">
          <component componentGuid="x" componentID="Audition.Fader" id="clipGain" name="volume" powered="true">
            <parameter index="0" name="volume" parameterValue="0.5"/><parameter index="1" name="static gain" parameterValue="1"/>
          </component>
          <fadeIn crossFadeLinkType="linkedAsymmetric" endPoint="4800" shape="0" startPoint="0" type="cosine"/>
          <fadeOut crossFadeLinkType="linkedAsymmetric" endPoint="48000" shape="0" startPoint="38400" type="linear"/>
        </audioClip>
      </audioTrack>
      <busTrack automationLaneOpenState="false" id="10003" index="2" select="false" visible="true">
        <trackParameters trackHeight="134" trackHue="-1" trackMinimized="false"><name>Bus A</name></trackParameters>
        <trackAudioParameters audioChannelType="stereo" automationMode="1"><trackOutput outputID="10000" type="trackID"/></trackAudioParameters>
      </busTrack>
      <masterTrack id="10000" index="3"><trackAudioParameters>
        <component componentID="Audition.Fader" id="trackFader"><parameter index="0" name="volume" parameterValue="2"/></component>
      </trackAudioParameters></masterTrack>
    </tracks>
    <sessionState ctiPosition="0" smpteStart="0">
      <timeFormatState beatsPerBar="3" beatsPerMinute="96.5" linkToDefaultTimeSettings="false" noteLength="4" subdivisions="16"/>
    </sessionState>
    <xmpMetadata><![CDATA[<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
 <rdf:Description rdf:about="" xmlns:xmpDM="http://ns.adobe.com/xmp/1.0/DynamicMedia/">
  <xmpDM:Tracks><rdf:Bag>
   <rdf:li rdf:parseType="Resource">
    <xmpDM:trackName>CuePoint Markers</xmpDM:trackName><xmpDM:trackType>Cue</xmpDM:trackType><xmpDM:frameRate>f48000</xmpDM:frameRate>
    <xmpDM:markers><rdf:Seq>
     <rdf:li xmpDM:startTime="48000" xmpDM:name="Chorus"/>
     <rdf:li rdf:parseType="Resource"><xmpDM:startTime>96000</xmpDM:startTime><xmpDM:duration>24000</xmpDM:duration><xmpDM:name>Outro</xmpDM:name></rdf:li>
    </rdf:Seq></xmpDM:markers>
   </rdf:li>
   <rdf:li rdf:parseType="Resource">
    <xmpDM:trackName>CD Track Markers</xmpDM:trackName><xmpDM:trackType>Track</xmpDM:trackType><xmpDM:frameRate>f24000</xmpDM:frameRate>
    <xmpDM:markers><rdf:Seq><rdf:li xmpDM:startTime="12000" xmpDM:name="Track 1"/></rdf:Seq></xmpDM:markers>
   </rdf:li>
  </rdf:Bag></xmpDM:Tracks>
 </rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>]]></xmpMetadata>
  </session>
  <files>
    <file absolutePath="C:\Audio\take1.wav" id="0" mediaHandler="AmioWav" relativePath="take1.wav"/>
  </files>
</sesx>"###;
        let p = from_sesx(1, xml, Some(Path::new("/tmp/nowhere")), &[]).unwrap();
        let s = &p.session;
        assert_eq!(s.sample_rate, 48000);
        assert_eq!(s.beats_per_bar, 3);
        assert!((s.tempo - 96.5).abs() < 1e-9);
        assert!((s.master_db - 6.02).abs() < 0.01);
        assert_eq!(s.tracks.len(), 2);
        let t = &s.tracks[0];
        assert_eq!(t.name, "Voice & Music");
        assert!(t.solo && t.mute);
        assert!((t.volume_db + 6.02).abs() < 0.01);
        assert!((t.pan + 34.5).abs() < 1e-4);
        assert_eq!(t.vol_env.points.len(), 2);
        assert!(t.vol_env.points[0].1.abs() < 0.01);
        assert!((t.vol_env.points[1].1 + 30.3).abs() < 0.1);
        assert_eq!(t.output, Some(s.tracks[1].id));
        assert_eq!(s.tracks[1].kind, TrackKind::Bus);
        let c = &t.clips[0];
        assert_eq!((c.start, c.len, c.offset), (24000, 48000, 1000));
        assert!(c.locked && !c.looped);
        assert!((c.gain_db + 6.02).abs() < 0.01);
        assert_eq!((c.fade_in, c.fade_in_cos), (4800, true));
        assert_eq!((c.fade_out, c.fade_out_curve), (9600, 0.0));
        let m: Vec<(usize, usize, MarkerKind, &str)> = s.markers.iter().map(|m| (m.pos, m.len, m.kind, m.name.as_str())).collect();
        assert_eq!(m, vec![(24000, 0, MarkerKind::CdTrack, "Track 1"), (48000, 0, MarkerKind::Cue, "Chorus"), (96000, 24000, MarkerKind::Cue, "Outro")]);
                assert_eq!(p.sources.len(), 1);
        assert_eq!(p.sources[0].1.file_name().unwrap(), "take1.wav");
    }

    #[test]
    fn audemo_sesx_round_trips() {
        let mut s = Session::new(1, "Test".into(), 44100, 0);
        let tid = s.new_id();
        let mut t = Track::new(tid, "Lead <vocal>".into(), [200, 80, 40]);
        t.volume_db = -3.0;
        t.pan = 25.0;
        t.vol_env = Envelope { points: vec![(0, 0.0), (44100, -12.0)] };
        let sid = s.new_id();
        let cid = s.new_id();
        t.clips.push(Clip {
            id: cid, source: sid, name: "Take & 2".into(), start: 1000, offset: 500, len: 20000, gain_db: 2.0,
            fade_in: 1000, fade_out: 2000, fade_in_curve: 0.4, fade_out_curve: 0.0, fade_in_cos: false, fade_out_cos: true,
            mute: false, group: 7, looped: true, locked: false, fx: Vec::new(), fx_on: true, stretch: 100.0, es: None,
        });
        s.tracks.push(t);
        s.tempo = 140.0;
        s.markers = vec![
            Marker { pos: 4410, len: 0, name: "Intro & go".into(), kind: MarkerKind::Cue, description: "Count in\t<here>\nline two".into() },
            Marker { pos: 22050, len: 11025, name: "Verse".into(), kind: MarkerKind::CdTrack, description: String::new() },
            Marker { pos: 30000, len: 500, name: "Sting".into(), kind: MarkerKind::Subclip, description: String::new() },
        ];
        let base = Path::new("/sessions");
        let mut paths = BTreeMap::new();
        paths.insert(sid, PathBuf::from("/sessions/Test Files/take.wav"));
        let xml = to_sesx(&s, &paths, base);
        assert!(xml.contains("relativePath=\"Test Files/take.wav\"") || xml.contains("relativePath=\"Test Files\\take.wav\""));
        assert!(xml.contains("<name>Lead &lt;vocal&gt;</name>"));
        // Audemo's embedded copy restores everything (the clip group too).
        let back = from_sesx(9, &xml, Some(base), &[]).unwrap();
        let c = &back.session.tracks[0].clips[0];
        assert_eq!((c.start, c.offset, c.len, c.group), (1000, 500, 20000, 7));
        assert_eq!(back.session.markers, s.markers);
        // Marker edits undo with the rest of the session.
        let mut s3 = back.session;
        s3.push_undo("Delete Markers");
        s3.markers.clear();
        s3.undo();
        assert_eq!(s3.markers, s.markers);
        // Without the embedded copy (as Audition would save it), the XML
        // alone carries the essentials.
        let start = xml.find("<!-- audemo-session").unwrap();
        let end = xml[start..].find("-->").unwrap() + start + 3;
        let plain = format!("{}{}", &xml[..start], &xml[end..]);
        let back = from_sesx(9, &plain, Some(base), &[]).unwrap();
        let s2 = &back.session;
        assert!((s2.tempo - 140.0).abs() < 1e-9);
        let t2 = &s2.tracks[0];
        assert_eq!(t2.name, "Lead <vocal>");
        assert!((t2.volume_db + 3.0).abs() < 0.01 && (t2.pan - 25.0).abs() < 1e-4);
        assert!((t2.vol_env.points[1].1 + 12.0).abs() < 0.05);
        assert_eq!(s2.markers.len(), 3);
        assert_eq!((s2.markers[0].pos, s2.markers[0].len, s2.markers[0].kind), (4410, 0, MarkerKind::Cue));
        assert_eq!(s2.markers[0].name, "Intro & go");
        assert_eq!(s2.markers[0].description, "Count in\t<here>\nline two");
        assert_eq!((s2.markers[1].pos, s2.markers[1].len, s2.markers[1].kind), (22050, 11025, MarkerKind::CdTrack));
        assert_eq!((s2.markers[2].kind, s2.markers[2].len), (MarkerKind::Subclip, 500));
        let c2 = &t2.clips[0];
        assert_eq!((c2.start, c2.offset, c2.len), (1000, 500, 20000));
        assert!(c2.looped && (c2.gain_db - 2.0).abs() < 0.01);
        assert_eq!((c2.fade_in, c2.fade_out, c2.fade_out_cos), (1000, 2000, true));
        assert!((c2.fade_in_curve - 0.4).abs() < 0.01);
        assert_eq!(c2.name, "Take & 2");
    }
}
