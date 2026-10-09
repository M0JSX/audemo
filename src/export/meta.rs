//! File metadata (title, artist, …) and the tag formats each container uses:
//! RIFF INFO for WAV, ID3v2.3 for MP3, Vorbis comments for FLAC and iTunes
//! `ilst` atoms for M4A.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    Track,
    Composer,
    Comment,
    Copyright,
}

pub const TAGS: [Tag; 10] = [
    Tag::Title,
    Tag::Artist,
    Tag::Album,
    Tag::AlbumArtist,
    Tag::Genre,
    Tag::Year,
    Tag::Track,
    Tag::Composer,
    Tag::Comment,
    Tag::Copyright,
];

impl Tag {
    pub fn index(self) -> usize {
        TAGS.iter().position(|t| *t == self).unwrap_or(0)
    }
    pub fn label(self) -> &'static str {
        match self {
            Tag::Title => "Title",
            Tag::Artist => "Artist",
            Tag::Album => "Album",
            Tag::AlbumArtist => "Album Artist",
            Tag::Genre => "Genre",
            Tag::Year => "Year",
            Tag::Track => "Track Number",
            Tag::Composer => "Composer",
            Tag::Comment => "Comment",
            Tag::Copyright => "Copyright",
        }
    }
    pub fn vorbis(self) -> &'static str {
        match self {
            Tag::Title => "TITLE",
            Tag::Artist => "ARTIST",
            Tag::Album => "ALBUM",
            Tag::AlbumArtist => "ALBUMARTIST",
            Tag::Genre => "GENRE",
            Tag::Year => "DATE",
            Tag::Track => "TRACKNUMBER",
            Tag::Composer => "COMPOSER",
            Tag::Comment => "COMMENT",
            Tag::Copyright => "COPYRIGHT",
        }
    }
    /// ID3v2.3 frame id.
    pub fn id3(self) -> &'static [u8; 4] {
        match self {
            Tag::Title => b"TIT2",
            Tag::Artist => b"TPE1",
            Tag::Album => b"TALB",
            Tag::AlbumArtist => b"TPE2",
            Tag::Genre => b"TCON",
            Tag::Year => b"TYER",
            Tag::Track => b"TRCK",
            Tag::Composer => b"TCOM",
            Tag::Comment => b"COMM",
            Tag::Copyright => b"TCOP",
        }
    }
    /// RIFF INFO chunk id (WAV has no album-artist field).
    pub fn riff(self) -> Option<&'static [u8; 4]> {
        Some(match self {
            Tag::Title => b"INAM",
            Tag::Artist => b"IART",
            Tag::Album => b"IPRD",
            Tag::AlbumArtist => return None,
            Tag::Genre => b"IGNR",
            Tag::Year => b"ICRD",
            Tag::Track => b"ITRK",
            Tag::Composer => b"IMUS",
            Tag::Comment => b"ICMT",
            Tag::Copyright => b"ICOP",
        })
    }
    /// iTunes metadata atom.
    pub fn mp4(self) -> &'static [u8; 4] {
        match self {
            Tag::Title => b"\xa9nam",
            Tag::Artist => b"\xa9ART",
            Tag::Album => b"\xa9alb",
            Tag::AlbumArtist => b"aART",
            Tag::Genre => b"\xa9gen",
            Tag::Year => b"\xa9day",
            Tag::Track => b"trkn",
            Tag::Composer => b"\xa9wrt",
            Tag::Comment => b"\xa9cmt",
            Tag::Copyright => b"cprt",
        }
    }
    /// Match a tag name from another tagging scheme (case-insensitive).
    pub fn from_key(key: &str) -> Option<Tag> {
        let k = key.trim().to_ascii_uppercase();
        TAGS.iter().copied().find(|t| {
            t.vorbis() == k
                || t.id3() == k.as_bytes()
                || t.riff().map(|r| r == k.as_bytes()).unwrap_or(false)
                || (k == "YEAR" && *t == Tag::Year)
                || (k == "ALBUM ARTIST" && *t == Tag::AlbumArtist)
                || (k == "IPRT" && *t == Tag::Track)
                || (k == "TDRC" && *t == Tag::Year)
        })
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub values: [String; 10],
}

impl Metadata {
    pub fn get(&self, t: Tag) -> &str {
        &self.values[t.index()]
    }
    pub fn set(&mut self, t: Tag, v: impl Into<String>) {
        self.values[t.index()] = v.into();
    }
    pub fn is_empty(&self) -> bool {
        self.values.iter().all(|v| v.trim().is_empty())
    }
    /// The non-empty fields, trimmed, in display order.
    pub fn entries(&self) -> impl Iterator<Item = (Tag, &str)> {
        TAGS.iter().map(move |t| (*t, self.get(*t).trim())).filter(|(_, v)| !v.is_empty())
    }
    /// "3/12" → (3, 12); "3" → (3, 0).
    pub fn track_numbers(&self) -> Option<(u16, u16)> {
        let s = self.get(Tag::Track).trim();
        let mut it = s.splitn(2, '/');
        let n = it.next()?.trim().parse::<u16>().ok()?;
        let total = it.next().and_then(|t| t.trim().parse::<u16>().ok()).unwrap_or(0);
        Some((n, total))
    }
}

/// Vorbis comment block body (FLAC METADATA_BLOCK_VORBIS_COMMENT).
pub fn vorbis_comment(meta: &Metadata, vendor: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    out.extend_from_slice(vendor.as_bytes());
    let items: Vec<String> = meta.entries().map(|(t, v)| format!("{}={}", t.vorbis(), v)).collect();
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for s in items {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    out
}

fn id3_text(s: &str) -> Vec<u8> {
    // ISO-8859-1 when it fits, otherwise UTF-16 with a byte-order mark.
    if s.chars().all(|c| (c as u32) < 256) {
        let mut v = vec![0u8];
        v.extend(s.chars().map(|c| c as u8));
        v
    } else {
        let mut v = vec![1u8, 0xFF, 0xFE];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }
}

/// A complete ID3v2.3 tag, or nothing when there is nothing to write.
pub fn id3v2(meta: &Metadata, encoder: &str) -> Vec<u8> {
    let mut frames = Vec::new();
    let mut frame = |id: &[u8; 4], body: Vec<u8>| {
        frames.extend_from_slice(id);
        frames.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frames.extend_from_slice(&[0, 0]);
        frames.extend_from_slice(&body);
    };
    for (t, v) in meta.entries() {
        if t == Tag::Comment {
            // encoding, language, empty description, text
            let text = id3_text(v);
            let enc = text[0];
            let mut body = vec![enc];
            body.extend_from_slice(b"eng");
            if enc == 1 {
                body.extend_from_slice(&[0xFF, 0xFE, 0, 0]);
            } else {
                body.push(0);
            }
            body.extend_from_slice(&text[1..]);
            frame(t.id3(), body);
        } else {
            frame(t.id3(), id3_text(v));
        }
    }
    if !encoder.is_empty() {
        frame(b"TSSE", id3_text(encoder));
    }
    if frames.is_empty() {
        return Vec::new();
    }
    let size = frames.len() as u32;
    let mut out = b"ID3\x03\x00\x00".to_vec();
    // Sync-safe size: 4 × 7 bits.
    out.extend_from_slice(&[(size >> 21 & 0x7F) as u8, (size >> 14 & 0x7F) as u8, (size >> 7 & 0x7F) as u8, (size & 0x7F) as u8]);
    out.extend_from_slice(&frames);
    out
}

/// RIFF `LIST`/`INFO` chunk (complete, including its header), or nothing.
pub fn riff_info(meta: &Metadata, software: &str) -> Vec<u8> {
    let mut body = b"INFO".to_vec();
    let mut add = |id: &[u8; 4], v: &str| {
        let mut s = v.as_bytes().to_vec();
        s.push(0);
        body.extend_from_slice(id);
        body.extend_from_slice(&(s.len() as u32).to_le_bytes());
        body.extend_from_slice(&s);
        if s.len() % 2 == 1 {
            body.push(0);
        }
    };
    for (t, v) in meta.entries() {
        if let Some(id) = t.riff() {
            add(id, v);
        }
    }
    if !software.is_empty() {
        add(b"ISFT", software);
    }
    if body.len() == 4 {
        return Vec::new();
    }
    let mut out = b"LIST".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// iTunes `ilst` items for an M4A `udta/meta` box (box contents only).
pub fn mp4_items(meta: &Metadata, encoder: &str) -> Vec<u8> {
    fn item(out: &mut Vec<u8>, name: &[u8; 4], kind: u32, payload: &[u8]) {
        let data_len = 16 + payload.len();
        out.extend_from_slice(&((8 + data_len) as u32).to_be_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&(data_len as u32).to_be_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(payload);
    }
    let mut out = Vec::new();
    for (t, v) in meta.entries() {
        if t == Tag::Track {
            if let Some((n, total)) = meta.track_numbers() {
                let mut p = vec![0, 0];
                p.extend_from_slice(&n.to_be_bytes());
                p.extend_from_slice(&total.to_be_bytes());
                p.extend_from_slice(&[0, 0]);
                item(&mut out, t.mp4(), 0, &p);
            }
        } else {
            item(&mut out, t.mp4(), 1, v.as_bytes());
        }
    }
    if !encoder.is_empty() {
        item(&mut out, b"\xa9too", 1, encoder.as_bytes());
    }
    out
}

/// Markers stored in a WAV file: `cue ` points named by `LIST`/`adtl`/`labl`.
pub fn wav_markers(cues: &[(usize, String)]) -> Vec<u8> {
    if cues.is_empty() {
        return Vec::new();
    }
    let mut cue = Vec::new();
    cue.extend_from_slice(&(cues.len() as u32).to_le_bytes());
    for (i, (pos, _)) in cues.iter().enumerate() {
        let id = i as u32 + 1;
        let pos = (*pos).min(u32::MAX as usize) as u32;
        cue.extend_from_slice(&id.to_le_bytes());
        cue.extend_from_slice(&pos.to_le_bytes());
        cue.extend_from_slice(b"data");
        cue.extend_from_slice(&0u32.to_le_bytes());
        cue.extend_from_slice(&0u32.to_le_bytes());
        cue.extend_from_slice(&pos.to_le_bytes());
    }
    let mut adtl = b"adtl".to_vec();
    for (i, (_, name)) in cues.iter().enumerate() {
        let mut s = name.as_bytes().to_vec();
        s.push(0);
        adtl.extend_from_slice(b"labl");
        adtl.extend_from_slice(&((4 + s.len()) as u32).to_le_bytes());
        adtl.extend_from_slice(&(i as u32 + 1).to_le_bytes());
        adtl.extend_from_slice(&s);
        if s.len() % 2 == 1 {
            adtl.push(0);
        }
    }
    let mut out = b"cue ".to_vec();
    out.extend_from_slice(&(cue.len() as u32).to_le_bytes());
    out.extend_from_slice(&cue);
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&(adtl.len() as u32).to_le_bytes());
    out.extend_from_slice(&adtl);
    out
}

/// Read markers and RIFF INFO fields from a WAV file's chunks. Reads only
/// the chunk headers and the small chunks, never the audio.
pub fn read_wav_extras(path: &std::path::Path) -> (Vec<(usize, String)>, Metadata) {
    use std::io::{Read, Seek, SeekFrom};
    let mut markers = Vec::new();
    let mut meta = Metadata::default();
    let Ok(mut f) = std::fs::File::open(path) else { return (markers, meta) };
    let mut head = [0u8; 12];
    if f.read_exact(&mut head).is_err() || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return (markers, meta);
    }
    let file_len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut cues: Vec<(u32, u32)> = Vec::new();
    let mut labels: Vec<(u32, String)> = Vec::new();
    let mut pos = 12u64;
    while pos + 8 <= file_len {
        let mut h = [0u8; 8];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut h).is_err() {
            break;
        }
        let id = [h[0], h[1], h[2], h[3]];
        let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
        let small = size <= 4 << 20;
        if small && (&id == b"cue " || &id == b"LIST") {
            let mut body = vec![0u8; size as usize];
            if f.read_exact(&mut body).is_err() {
                break;
            }
            if &id == b"cue " && body.len() >= 4 {
                let n = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
                for i in 0..n {
                    let o = 4 + i * 24;
                    if o + 24 > body.len() {
                        break;
                    }
                    let rd = |k: usize| u32::from_le_bytes([body[o + k], body[o + k + 1], body[o + k + 2], body[o + k + 3]]);
                    cues.push((rd(0), rd(20)));
                }
            } else if body.len() >= 4 {
                let kind = &body[0..4];
                let mut o = 4;
                while o + 8 <= body.len() {
                    let sid = [body[o], body[o + 1], body[o + 2], body[o + 3]];
                    let ssz = u32::from_le_bytes([body[o + 4], body[o + 5], body[o + 6], body[o + 7]]) as usize;
                    let start = o + 8;
                    let end = (start + ssz).min(body.len());
                    let data = &body[start..end];
                    let text = |d: &[u8]| {
                        let d = d.split(|b| *b == 0).next().unwrap_or(&[]);
                        String::from_utf8(d.to_vec()).unwrap_or_else(|_| d.iter().map(|b| *b as char).collect())
                    };
                    if kind == b"adtl" && (&sid == b"labl" || &sid == b"note") && data.len() >= 4 {
                        let cid = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                        if &sid == b"labl" || !labels.iter().any(|(c, _)| *c == cid) {
                            labels.retain(|(c, _)| *c != cid);
                            labels.push((cid, text(&data[4..])));
                        }
                    } else if kind == b"INFO" {
                        if let Some(t) = Tag::from_key(&String::from_utf8_lossy(&sid)) {
                            let v = text(data);
                            if !v.trim().is_empty() {
                                meta.set(t, v.trim());
                            }
                        }
                    }
                    o = start + ssz + (ssz & 1);
                }
            }
        }
        pos += 8 + size + (size & 1);
    }
    for (id, at) in cues {
        let name = labels.iter().find(|(c, _)| *c == id).map(|(_, n)| n.clone()).unwrap_or_default();
        markers.push((at as usize, name));
    }
    markers.sort_by_key(|m| m.0);
    (markers, meta)
}
