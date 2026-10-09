//! File decoding (symphonia). Writing lives in `crate::export`.

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey};
use symphonia::core::probe::Hint;

pub const OPEN_EXTENSIONS: &[&str] = &[
    "wav", "wave", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "mp4", "aac", "alac",
    "caf", "mkv", "webm",
];

pub struct Decoded {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
    pub bits: Option<u32>,
    pub meta: crate::export::Metadata,
    /// Markers stored in the file (WAV cue points): (frame, name).
    pub markers: Vec<(usize, String)>,
}

fn read_tags(rev: &MetadataRevision, meta: &mut crate::export::Metadata, total: &mut Option<String>) {
    use crate::export::Tag;
    for t in rev.tags() {
        let tag = match t.std_key {
            Some(StandardTagKey::TrackTitle) => Some(Tag::Title),
            Some(StandardTagKey::Artist) => Some(Tag::Artist),
            Some(StandardTagKey::Album) => Some(Tag::Album),
            Some(StandardTagKey::AlbumArtist) => Some(Tag::AlbumArtist),
            Some(StandardTagKey::Genre) => Some(Tag::Genre),
            Some(StandardTagKey::Date) => Some(Tag::Year),
            Some(StandardTagKey::TrackNumber) => Some(Tag::Track),
            Some(StandardTagKey::Composer) => Some(Tag::Composer),
            Some(StandardTagKey::Comment) => Some(Tag::Comment),
            Some(StandardTagKey::Copyright) => Some(Tag::Copyright),
            Some(StandardTagKey::TrackTotal) => {
                *total = Some(t.value.to_string());
                None
            }
            Some(_) => None,
            None => Tag::from_key(&t.key),
        };
        if let Some(tag) = tag {
            let v = t.value.to_string();
            if !v.trim().is_empty() && meta.get(tag).is_empty() {
                meta.set(tag, v.trim());
            }
        }
    }
}

pub fn load(path: &Path) -> Result<Decoded, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Couldn't open file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
        .map_err(|e| format!("Unrecognised audio format: {e}"))?;
    let mut format = probed.format;
    let mut meta = crate::export::Metadata::default();
    let mut track_total = None;
    if let Some(m) = probed.metadata.get() {
        if let Some(rev) = m.current() {
            read_tags(rev, &mut meta, &mut track_total);
        }
    }
    if let Some(rev) = format.metadata().current() {
        read_tags(rev, &mut meta, &mut track_total);
    }
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("The file has no audio track.")?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let bits = track.codec_params.bits_per_sample;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Unsupported codec: {e}"))?;

    let mut channels: Vec<Vec<f32>> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("Read error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let n_ch = spec.channels.count().max(1);
                sample_rate = spec.rate;
                if channels.is_empty() {
                    channels = vec![Vec::new(); n_ch];
                }
                let frames = decoded.capacity() as u64;
                let mut sb = SampleBuffer::<f32>::new(frames, spec);
                sb.copy_interleaved_ref(decoded);
                for frame in sb.samples().chunks(n_ch) {
                    for (c, s) in frame.iter().enumerate() {
                        if let Some(ch) = channels.get_mut(c) {
                            ch.push(*s);
                        }
                    }
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Decode error: {e}")),
        }
    }
    if channels.is_empty() || channels[0].is_empty() {
        return Err("The file contains no audio samples.".into());
    }
    // Keep at most stereo; the editor is a two-channel waveform editor.
    if channels.len() > 2 {
        channels.truncate(2);
    }
    let len = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    for c in channels.iter_mut() {
        c.truncate(len);
    }
    // MPEG-4 audio: apply the edit list (encoder delay and padding), which
    // the decoder leaves in.
    if matches!(crate::export::Container::from_path(path), Some(crate::export::Container::M4a)) {
        if let Some((skip, dur, ts)) = mp4_edit(path) {
            let scale = sample_rate as f64 / ts.max(1) as f64;
            let skip = (skip as f64 * scale).round() as usize;
            let dur = (dur as f64 * scale).round() as usize;
            let len = channels[0].len();
            if skip < len && dur > 0 && skip + dur <= len + 4096 {
                for c in channels.iter_mut() {
                    c.drain(..skip);
                    c.truncate(dur);
                }
            }
        }
    }
    let len = channels[0].len();
    if let Some(total) = track_total {
        let n = meta.get(crate::export::Tag::Track).to_string();
        if !n.is_empty() && !n.contains('/') && !total.trim().is_empty() {
            meta.set(crate::export::Tag::Track, format!("{n}/{}", total.trim()));
        }
    }
    let mut markers = Vec::new();
    if matches!(crate::export::Container::from_path(path), Some(crate::export::Container::Wav)) {
        let (m, info) = crate::export::meta::read_wav_extras(path);
        markers = m.into_iter().filter(|(p, _)| *p <= len).collect();
        for (t, v) in info.entries() {
            if meta.get(t).is_empty() {
                meta.set(t, v);
            }
        }
    }
    Ok(Decoded { sample_rate, channels, bits, meta, markers })
}

/// The first audio track's edit: (media frames to skip, frames to play,
/// media timescale), when it has a single plain edit.
fn mp4_edit(path: &Path) -> Option<(u64, u64, u32)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    // Find moov among the top-level boxes.
    let mut pos = 0u64;
    let moov = loop {
        if pos + 8 > file_len {
            return None;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let mut head = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().ok()?);
            head = 16;
        } else if size == 0 {
            size = file_len - pos;
        }
        if size < head {
            return None;
        }
        if &h[4..8] == b"moov" {
            if size > 64 << 20 {
                return None;
            }
            let mut b = vec![0u8; (size - head) as usize];
            f.read_exact(&mut b).ok()?;
            break b;
        }
        pos += size;
    };
    fn boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = Vec::new();
        let mut o = 0;
        while o + 8 <= b.len() {
            let size = u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
            let (size, head) = if size == 1 && o + 16 <= b.len() {
                (u64::from_be_bytes(b[o + 8..o + 16].try_into().unwrap()) as usize, 16)
            } else if size == 0 {
                (b.len() - o, 8)
            } else {
                (size, 8)
            };
            if size < head || o + size > b.len() {
                break;
            }
            out.push(([b[o + 4], b[o + 5], b[o + 6], b[o + 7]], &b[o + head..o + size]));
            o += size;
        }
        out
    }
    let be32 = |b: &[u8], o: usize| b.get(o..o + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap()));
    let be64 = |b: &[u8], o: usize| b.get(o..o + 8).map(|s| u64::from_be_bytes(s.try_into().unwrap()));
    let top = boxes(&moov);
    let mvhd = top.iter().find(|b| &b.0 == b"mvhd")?.1;
    let movie_ts = if mvhd.first()? == &1 { be32(mvhd, 20)? } else { be32(mvhd, 12)? };
    for (_, trak) in top.iter().filter(|b| &b.0 == b"trak") {
        let inner = boxes(trak);
        let Some(mdia) = inner.iter().find(|b| &b.0 == b"mdia").map(|b| boxes(b.1)) else { continue };
        let is_audio = mdia.iter().any(|b| &b.0 == b"hdlr" && b.1.get(8..12) == Some(b"soun"));
        if !is_audio {
            continue;
        }
        let mdhd = mdia.iter().find(|b| &b.0 == b"mdhd")?.1;
        let media_ts = if mdhd.first()? == &1 { be32(mdhd, 20)? } else { be32(mdhd, 12)? };
        let edts = inner.iter().find(|b| &b.0 == b"edts").map(|b| boxes(b.1))?;
        let elst = edts.iter().find(|b| &b.0 == b"elst")?.1;
        let v1 = elst.first()? == &1;
        let n = be32(elst, 4)? as usize;
        let mut edits = Vec::new();
        for i in 0..n {
            let (dur, time) = if v1 {
                let o = 8 + i * 20;
                (be64(elst, o)?, be64(elst, o + 8)? as i64)
            } else {
                let o = 8 + i * 12;
                (be32(elst, o)? as u64, be32(elst, o + 4)? as i32 as i64)
            };
            edits.push((dur, time));
        }
        let real: Vec<_> = edits.iter().filter(|e| e.1 >= 0).collect();
        if real.len() != 1 || edits.len() != 1 || movie_ts == 0 {
            return None;
        }
        let (dur, time) = *real[0];
        let dur_media = (dur as f64 * media_ts as f64 / movie_ts as f64).round() as u64;
        return Some((time as u64, dur_media, media_ts));
    }
    None
}
