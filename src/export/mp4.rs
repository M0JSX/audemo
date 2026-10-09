//! Minimal MPEG-4 audio (M4A) writer for one AAC track, with the index
//! ("moov") at the front so files stream, an edit list that hides the
//! encoder delay (gapless playback) and iTunes-style metadata.

use std::io::Write;

pub struct AacTrack {
    /// AudioSpecificConfig.
    pub asc: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    /// Raw AAC access units.
    pub frames: Vec<Vec<u8>>,
    /// PCM frames per access unit (1024 for AAC-LC).
    pub frame_len: u32,
    /// Encoder delay in PCM frames, trimmed by the edit list.
    pub delay: u32,
    /// Real length in PCM frames.
    pub samples: u64,
}

struct Mp4 {
    buf: Vec<u8>,
    stack: Vec<usize>,
}

impl Mp4 {
    fn open(&mut self, name: &[u8; 4]) {
        self.stack.push(self.buf.len());
        self.buf.extend_from_slice(&[0, 0, 0, 0]);
        self.buf.extend_from_slice(name);
    }
    fn full(&mut self, name: &[u8; 4], version: u8, flags: u32) {
        self.open(name);
        self.u32(((version as u32) << 24) | (flags & 0xFF_FFFF));
    }
    fn close(&mut self) {
        let at = self.stack.pop().expect("box stack");
        let len = (self.buf.len() - at) as u32;
        self.buf[at..at + 4].copy_from_slice(&len.to_be_bytes());
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    fn matrix(&mut self) {
        for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            self.u32(v);
        }
    }
}

/// MPEG-4 descriptor: tag + 4-byte length form.
fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len() as u32;
    let mut v = vec![tag, 0x80 | (n >> 21 & 0x7F) as u8, 0x80 | (n >> 14 & 0x7F) as u8, 0x80 | (n >> 7 & 0x7F) as u8, (n & 0x7F) as u8];
    v.extend_from_slice(body);
    v
}

fn esds(t: &AacTrack, max_bitrate: u32, avg_bitrate: u32, buffer: u32) -> Vec<u8> {
    let dsi = descriptor(0x05, &t.asc);
    let mut dcd = vec![0x40, 0x15];
    dcd.extend_from_slice(&buffer.to_be_bytes()[1..]);
    dcd.extend_from_slice(&max_bitrate.to_be_bytes());
    dcd.extend_from_slice(&avg_bitrate.to_be_bytes());
    dcd.extend_from_slice(&dsi);
    let dcd = descriptor(0x04, &dcd);
    let sl = descriptor(0x06, &[0x02]);
    let mut es = vec![0, 1, 0]; // ES_ID 1, no flags
    es.extend_from_slice(&dcd);
    es.extend_from_slice(&sl);
    descriptor(0x03, &es)
}

fn moov(t: &AacTrack, ilst: &[u8], mdat_payload_start: u64) -> Vec<u8> {
    let ts = t.sample_rate;
    let media_dur = t.frames.len() as u64 * t.frame_len as u64;
    let play_dur = t.samples;
    let total_bytes: u64 = t.frames.iter().map(|f| f.len() as u64).sum();
    let secs = (media_dur as f64 / ts as f64).max(1e-6);
    let avg = (total_bytes as f64 * 8.0 / secs) as u32;
    let max_frame = t.frames.iter().map(|f| f.len()).max().unwrap_or(0) as u32;
    let max_rate = (max_frame as f64 * 8.0 * ts as f64 / t.frame_len as f64) as u32;
    let wide = media_dur > u32::MAX as u64 || total_bytes + (1 << 24) > u32::MAX as u64;
    let v = if wide { 1 } else { 0 };

    let mut m = Mp4 { buf: Vec::new(), stack: Vec::new() };
    m.open(b"moov");
    // Movie header.
    m.full(b"mvhd", v, 0);
    if wide {
        m.u64(0);
        m.u64(0);
        m.u32(ts);
        m.u64(play_dur);
    } else {
        m.u32(0);
        m.u32(0);
        m.u32(ts);
        m.u32(play_dur as u32);
    }
    m.u32(0x0001_0000); // rate 1.0
    m.u16(0x0100); // volume 1.0
    m.bytes(&[0; 10]);
    m.matrix();
    m.bytes(&[0; 24]);
    m.u32(2); // next track id
    m.close();

    m.open(b"trak");
    m.full(b"tkhd", v, 0x7); // enabled, in movie, in preview
    if wide {
        m.u64(0);
        m.u64(0);
        m.u32(1);
        m.u32(0);
        m.u64(play_dur);
    } else {
        m.u32(0);
        m.u32(0);
        m.u32(1);
        m.u32(0);
        m.u32(play_dur as u32);
    }
    m.bytes(&[0; 8]);
    m.u16(0); // layer
    m.u16(1); // alternate group
    m.u16(0x0100); // volume
    m.u16(0);
    m.matrix();
    m.u32(0);
    m.u32(0);
    m.close();

    // Edit list: skip the encoder delay, play exactly `samples`.
    m.open(b"edts");
    m.full(b"elst", v, 0);
    m.u32(1);
    if wide {
        m.u64(play_dur);
        m.u64(t.delay as u64);
    } else {
        m.u32(play_dur as u32);
        m.u32(t.delay);
    }
    m.u16(1);
    m.u16(0);
    m.close();
    m.close();

    m.open(b"mdia");
    m.full(b"mdhd", v, 0);
    if wide {
        m.u64(0);
        m.u64(0);
        m.u32(ts);
        m.u64(media_dur);
    } else {
        m.u32(0);
        m.u32(0);
        m.u32(ts);
        m.u32(media_dur as u32);
    }
    m.u16(0x55C4); // language "und"
    m.u16(0);
    m.close();
    m.full(b"hdlr", 0, 0);
    m.u32(0);
    m.bytes(b"soun");
    m.bytes(&[0; 12]);
    m.bytes(b"SoundHandler\0");
    m.close();
    m.open(b"minf");
    m.full(b"smhd", 0, 0);
    m.u16(0);
    m.u16(0);
    m.close();
    m.open(b"dinf");
    m.full(b"dref", 0, 0);
    m.u32(1);
    m.full(b"url ", 0, 1);
    m.close();
    m.close();
    m.close();

    m.open(b"stbl");
    m.full(b"stsd", 0, 0);
    m.u32(1);
    m.open(b"mp4a");
    m.bytes(&[0; 6]);
    m.u16(1); // data reference index
    m.bytes(&[0; 8]);
    m.u16(t.channels);
    m.u16(16);
    m.u16(0);
    m.u16(0);
    m.u32(if ts <= 0xFFFF { ts << 16 } else { 0 });
    m.full(b"esds", 0, 0);
    let e = esds(t, max_rate.max(avg), avg, max_frame);
    m.bytes(&e);
    m.close();
    m.close();
    m.close();
    // Every access unit lasts frame_len.
    m.full(b"stts", 0, 0);
    m.u32(1);
    m.u32(t.frames.len() as u32);
    m.u32(t.frame_len);
    m.close();
    // One chunk holding every sample.
    m.full(b"stsc", 0, 0);
    m.u32(1);
    m.u32(1);
    m.u32(t.frames.len() as u32);
    m.u32(1);
    m.close();
    m.full(b"stsz", 0, 0);
    m.u32(0);
    m.u32(t.frames.len() as u32);
    for f in &t.frames {
        m.u32(f.len() as u32);
    }
    m.close();
    if wide {
        m.full(b"co64", 0, 0);
        m.u32(1);
        m.u64(mdat_payload_start);
    } else {
        m.full(b"stco", 0, 0);
        m.u32(1);
        m.u32(mdat_payload_start as u32);
    }
    m.close();
    m.close(); // stbl
    m.close(); // minf
    m.close(); // mdia
    m.close(); // trak

    if !ilst.is_empty() {
        m.open(b"udta");
        m.full(b"meta", 0, 0);
        m.full(b"hdlr", 0, 0);
        m.u32(0);
        m.bytes(b"mdir");
        m.bytes(b"appl");
        m.bytes(&[0; 8]);
        m.u8(0);
        m.close();
        m.open(b"ilst");
        m.bytes(ilst);
        m.close();
        m.close();
        m.close();
    }
    m.close(); // moov
    m.buf
}

pub fn write<W: Write>(out: &mut W, t: &AacTrack, ilst: &[u8]) -> Result<(), String> {
    let err = |e: std::io::Error| format!("Write failed: {e}");
    let mut ftyp = Mp4 { buf: Vec::new(), stack: Vec::new() };
    ftyp.open(b"ftyp");
    ftyp.bytes(b"M4A ");
    ftyp.u32(0);
    ftyp.bytes(b"M4A isommp42");
    ftyp.close();
    let total: u64 = t.frames.iter().map(|f| f.len() as u64).sum();
    let big_mdat = total + 8 > u32::MAX as u64;
    let mdat_head = if big_mdat { 16 } else { 8 };
    // The moov size doesn't depend on the offset value.
    let probe = moov(t, ilst, 0);
    let start = ftyp.buf.len() as u64 + probe.len() as u64 + mdat_head;
    let moov = moov(t, ilst, start);
    debug_assert_eq!(moov.len(), probe.len());
    out.write_all(&ftyp.buf).map_err(err)?;
    out.write_all(&moov).map_err(err)?;
    if big_mdat {
        out.write_all(&1u32.to_be_bytes()).map_err(err)?;
        out.write_all(b"mdat").map_err(err)?;
        out.write_all(&(total + 16).to_be_bytes()).map_err(err)?;
    } else {
        out.write_all(&((total + 8) as u32).to_be_bytes()).map_err(err)?;
        out.write_all(b"mdat").map_err(err)?;
    }
    for f in &t.frames {
        out.write_all(f).map_err(err)?;
    }
    Ok(())
}
