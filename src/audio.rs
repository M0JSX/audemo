//! Audio storage for open files: chunked, shared and disk-backed.
//!
//! Each channel is a list of chunks of up to [`CHUNK`] frames. Chunks are
//! immutable and reference-counted, so an edit builds a new chunk list that
//! shares every chunk it didn't touch: undo steps cost only the audio that
//! actually changed. Each chunk keeps a min/max summary per [`SUM`] frames
//! (plus its overall peak and sum of squares), so drawing and level readouts
//! don't need the samples.
//!
//! Chunk samples live in memory until the total passes the memory budget
//! (Preferences > Memory); then the least recently used are written to a
//! working file in the temp folder and read back when needed. The audio
//! callback never waits: it reads chunks with `try_lock`, and a prefetch keeps
//! the audio ahead of the playhead in memory.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Frames per full chunk (about 1.4 s at 48 kHz).
pub const CHUNK: usize = 1 << 16;
/// Frames per min/max summary entry.
pub const SUM: usize = 256;

// ------------------------------------------------------------------ chunks

pub struct Chunk {
    id: u64,
    len: usize,
    lo: f32,
    hi: f32,
    sumsq: f64,
    sum: Box<[(f32, f32)]>,
    /// The samples, while in memory.
    data: Mutex<Option<Arc<[f32]>>>,
    /// Offset in the working file, once written there.
    slot: Mutex<Option<u64>>,
}

impl Chunk {
    fn new(samples: Vec<f32>) -> Arc<Chunk> {
        let len = samples.len();
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut sumsq = 0.0f64;
        let sum: Box<[(f32, f32)]> = samples
            .chunks(SUM)
            .map(|b| {
                let mut l = f32::MAX;
                let mut h = f32::MIN;
                for &v in b {
                    l = l.min(v);
                    h = h.max(v);
                    sumsq += (v as f64) * (v as f64);
                }
                lo = lo.min(l);
                hi = hi.max(h);
                (l, h)
            })
            .collect();
        if len == 0 {
            lo = 0.0;
            hi = 0.0;
        }
        let data: Arc<[f32]> = samples.into();
        let c = Arc::new(Chunk {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            len,
            lo,
            hi,
            sumsq,
            sum,
            data: Mutex::new(Some(data)),
            slot: Mutex::new(None),
        });
        store().admit(&c);
        c
    }

    /// The samples (read back from the working file if they were moved there).
    pub fn data(self: &Arc<Self>) -> Arc<[f32]> {
        let cur = self.data.lock().unwrap().clone();
        if let Some(d) = cur {
            store().touch(self);
            return d;
        }
        let loaded: Arc<[f32]> = store().read(self).into();
        let d = {
            let mut g = self.data.lock().unwrap();
            if g.is_none() {
                *g = Some(loaded);
            }
            g.clone().unwrap()
        };
        store().admit(self);
        d
    }

    /// The samples if they're in memory and not locked this instant (for the
    /// audio callback, which must not wait).
    fn try_data(&self) -> Option<Arc<[f32]>> {
        self.data.try_lock().ok().and_then(|g| g.clone())
    }

    fn resident(&self) -> bool {
        self.data.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        let slot = self.slot.get_mut().ok().and_then(|s| s.take());
        let bytes = if self.data.get_mut().ok().map(|d| d.is_some()).unwrap_or(false) { self.len * 4 } else { 0 };
        if let Some(s) = STORE.get() {
            s.forget(self.id, slot, bytes);
        }
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

// ------------------------------------------------------------------ the store

struct Inner {
    file: Option<File>,
    path: Option<PathBuf>,
    next_off: u64,
    free: Vec<u64>,
    /// Least recently used first: tick → chunk.
    lru: BTreeMap<u64, Weak<Chunk>>,
    tick_of: std::collections::HashMap<u64, u64>,
    tick: u64,
    resident: usize,
    budget: usize,
}

pub struct Store {
    inner: Mutex<Inner>,
}

static STORE: OnceLock<Store> = OnceLock::new();

fn store() -> &'static Store {
    STORE.get_or_init(|| Store {
        inner: Mutex::new(Inner {
            file: None,
            path: None,
            next_off: 0,
            free: Vec::new(),
            lru: BTreeMap::new(),
            tick_of: Default::default(),
            tick: 0,
            resident: 0,
            budget: default_budget(),
        }),
    })
}

/// A quarter of installed memory, between 512 MB and 8 GB.
pub fn default_budget() -> usize {
    let ram = crate::prefs_ui::total_ram().unwrap_or(8 << 30) as usize;
    (ram / 4).clamp(512 << 20, 8 << 30)
}

/// Set the memory budget for audio (bytes); 0 = the default.
pub fn set_budget(bytes: usize) {
    let s = store();
    let mut g = s.inner.lock().unwrap();
    g.budget = if bytes == 0 { default_budget() } else { bytes.max(64 << 20) };
    let hold = s.evict(&mut g);
    drop(g);
    drop(hold);
}

static WORK_DIR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// Folder for the working file (None = the system's temporary folder).
/// Takes effect when the working file is first needed.
pub fn set_work_dir(dir: Option<std::path::PathBuf>) {
    *WORK_DIR.lock().unwrap() = dir;
}

/// (bytes of audio in memory, bytes in the working file, budget).
pub fn usage() -> (usize, u64, usize) {
    let g = store().inner.lock().unwrap();
    (g.resident, g.next_off - g.free.len() as u64 * slot_bytes(), g.budget)
}

fn slot_bytes() -> u64 {
    (CHUNK * 4) as u64
}

impl Store {
    fn admit(&self, c: &Arc<Chunk>) {
        let mut g = self.inner.lock().unwrap();
        if !g.tick_of.contains_key(&c.id) {
            g.resident += c.len * 4;
        }
        Self::bump(&mut g, c);
        let hold = self.evict(&mut g);
        // Chunks are released only after the lock: a last reference would
        // otherwise take it again as it drops.
        drop(g);
        drop(hold);
    }

    fn touch(&self, c: &Arc<Chunk>) {
        if let Ok(mut g) = self.inner.try_lock() {
            Self::bump(&mut g, c);
        }
    }

    fn bump(g: &mut Inner, c: &Arc<Chunk>) {
        if let Some(t) = g.tick_of.remove(&c.id) {
            g.lru.remove(&t);
        }
        g.tick += 1;
        let t = g.tick;
        g.lru.insert(t, Arc::downgrade(c));
        g.tick_of.insert(c.id, t);
    }

    fn forget(&self, id: u64, slot: Option<u64>, bytes: usize) {
        let Ok(mut g) = self.inner.lock() else { return };
        if let Some(t) = g.tick_of.remove(&id) {
            g.lru.remove(&t);
            g.resident = g.resident.saturating_sub(bytes);
        }
        if let Some(s) = slot {
            g.free.push(s);
        }
    }

    /// Move least recently used chunks to the working file until under
    /// budget. Returns the chunks it looked at, to be dropped by the caller
    /// once the lock is released.
    #[must_use]
    fn evict(&self, g: &mut Inner) -> Vec<Arc<Chunk>> {
        let mut skipped = Vec::new();
        let mut hold = Vec::new();
        while g.resident > g.budget {
            let Some((&t, _)) = g.lru.iter().next() else { break };
            let w = g.lru.remove(&t).unwrap();
            let Some(c) = w.upgrade() else { continue };
            g.tick_of.remove(&c.id);
            // Busy (being read right now): try the next one.
            let Ok(mut data) = c.data.try_lock() else {
                skipped.push(c);
                continue;
            };
            let Some(d) = data.clone() else {
                drop(data);
                hold.push(c);
                continue;
            };
            let mut slot = c.slot.lock().unwrap();
            if slot.is_none() {
                match Self::write(g, &d) {
                    Some(off) => *slot = Some(off),
                    None => {
                        // No working file: keep it in memory.
                        drop(data);
                        drop(slot);
                        skipped.push(c);
                        break;
                    }
                }
            }
            *data = None;
            g.resident = g.resident.saturating_sub(c.len * 4);
            drop(slot);
            drop(data);
            hold.push(c);
        }
        for c in skipped {
            Self::bump(g, &c);
            hold.push(c);
        }
        hold
    }

    fn file(g: &mut Inner) -> Option<&File> {
        if g.file.is_none() {
            let dir = WORK_DIR.lock().unwrap().clone().filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir);
            let path = dir.join(format!("audemo-work-{}-{}.bin", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
            let mut o = std::fs::OpenOptions::new();
            o.read(true).write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // FILE_FLAG_DELETE_ON_CLOSE: Windows removes it when Audemo exits.
                o.custom_flags(0x0400_0000);
            }
            let f = o.open(&path).ok()?;
            // Elsewhere the name can go now; the open file lives on.
            #[cfg(unix)]
            let _ = std::fs::remove_file(&path);
            g.file = Some(f);
            g.path = Some(path);
        }
        g.file.as_ref()
    }

    fn write(g: &mut Inner, d: &[f32]) -> Option<u64> {
        let off = match g.free.pop() {
            Some(o) => o,
            None => {
                let o = g.next_off;
                g.next_off += slot_bytes();
                o
            }
        };
        let f = Self::file(g)?;
        let bytes: Vec<u8> = d.iter().flat_map(|v| v.to_le_bytes()).collect();
        if write_at(f, &bytes, off).is_err() {
            g.free.push(off);
            return None;
        }
        Some(off)
    }

    fn read(&self, c: &Chunk) -> Vec<f32> {
        let off = c.slot.lock().unwrap().unwrap_or(0);
        let mut bytes = vec![0u8; c.len * 4];
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(f) = Self::file(&mut g) {
                let _ = read_at(f, &mut bytes, off);
            }
        }
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.write_all_at(buf, off)
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, off)
}

#[cfg(windows)]
fn write_at(f: &File, mut buf: &[u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, off)?;
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(f: &File, mut buf: &mut [u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_read(buf, off)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[n..];
        off += n as u64;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_at(_: &File, _: &[u8], _: u64) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(not(any(unix, windows)))]
fn read_at(_: &File, _: &mut [u8], _: u64) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

// ------------------------------------------------------------------ channels

#[derive(Clone, Default)]
struct Channel {
    chunks: Vec<Arc<Chunk>>,
    /// First frame of each chunk.
    starts: Vec<usize>,
    len: usize,
}

impl Channel {
    fn from_chunks(chunks: Vec<Arc<Chunk>>) -> Channel {
        let mut starts = Vec::with_capacity(chunks.len());
        let mut len = 0;
        let chunks: Vec<Arc<Chunk>> = chunks.into_iter().filter(|c| c.len > 0).collect();
        for c in &chunks {
            starts.push(len);
            len += c.len;
        }
        Channel { chunks, starts, len }
    }

    fn from_samples(s: &[f32]) -> Channel {
        Channel::from_chunks(s.chunks(CHUNK).map(|b| Chunk::new(b.to_vec())).collect())
    }

    /// (chunk index, offset in it) for frame `x` (`x == len` gives one past
    /// the last chunk).
    fn locate(&self, x: usize) -> (usize, usize) {
        if x >= self.len {
            return (self.chunks.len(), 0);
        }
        let i = self.starts.partition_point(|&s| s <= x) - 1;
        (i, x - self.starts[i])
    }

    fn read(&self, start: usize, out: &mut [f32]) {
        let end = (start + out.len()).min(self.len);
        out.iter_mut().for_each(|v| *v = 0.0);
        if start >= end {
            return;
        }
        let (mut i, mut off) = self.locate(start);
        let mut w = 0;
        let n = end - start;
        while w < n && i < self.chunks.len() {
            let d = self.chunks[i].data();
            let take = (d.len() - off).min(n - w);
            out[w..w + take].copy_from_slice(&d[off..off + take]);
            w += take;
            i += 1;
            off = 0;
        }
    }

    fn read_rt(&self, start: usize, out: &mut [f32]) -> bool {
        let end = (start + out.len()).min(self.len);
        out.iter_mut().for_each(|v| *v = 0.0);
        if start >= end {
            return true;
        }
        let (mut i, mut off) = self.locate(start);
        let mut w = 0;
        let n = end - start;
        let mut ok = true;
        while w < n && i < self.chunks.len() {
            let take = (self.chunks[i].len - off).min(n - w);
            match self.chunks[i].try_data() {
                Some(d) => out[w..w + take].copy_from_slice(&d[off..off + take]),
                None => ok = false,
            }
            w += take;
            i += 1;
            off = 0;
        }
        ok
    }

    fn min_max(&self, a: usize, b: usize) -> (f32, f32) {
        let b = b.min(self.len);
        if a >= b {
            return (0.0, 0.0);
        }
        let (mut i, mut off) = self.locate(a);
        let mut pos = a;
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        while pos < b && i < self.chunks.len() {
            let c = &self.chunks[i];
            let take = (c.len - off).min(b - pos);
            if off == 0 && take == c.len {
                lo = lo.min(c.lo);
                hi = hi.max(c.hi);
            } else {
                // Whole summary blocks from the summary; the ragged ends
                // from the samples when they're short, else by the block.
                let first = off.div_ceil(SUM);
                let last = (off + take) / SUM;
                let exact = |s: usize, e: usize, lo: &mut f32, hi: &mut f32| {
                    if e <= s {
                        return;
                    }
                    if e - s <= SUM {
                        if let Some(d) = c.try_data().or_else(|| if c.resident() { None } else { Some(c.data()) }) {
                            for &v in &d[s..e] {
                                *lo = lo.min(v);
                                *hi = hi.max(v);
                            }
                            return;
                        }
                    }
                    for k in s / SUM..e.div_ceil(SUM) {
                        let (l, h) = c.sum[k];
                        *lo = lo.min(l);
                        *hi = hi.max(h);
                    }
                };
                if last > first {
                    exact(off, first * SUM, &mut lo, &mut hi);
                    for k in first..last {
                        let (l, h) = c.sum[k];
                        lo = lo.min(l);
                        hi = hi.max(h);
                    }
                    exact(last * SUM, off + take, &mut lo, &mut hi);
                } else {
                    exact(off, off + take, &mut lo, &mut hi);
                }
            }
            pos += take;
            i += 1;
            off = 0;
        }
        if lo > hi {
            (0.0, 0.0)
        } else {
            (lo, hi)
        }
    }

    /// The channel with frames [a, b) replaced by `new`.
    fn splice(&self, a: usize, b: usize, new: &[f32]) -> Channel {
        let a = a.min(self.len);
        let b = b.min(self.len).max(a);
        let (ia, off_a) = self.locate(a);
        let (ib, off_b) = self.locate(b);
        let mut out: Vec<Arc<Chunk>> = self.chunks[..ia].to_vec();
        let mut mid: Vec<f32> = Vec::with_capacity(off_a + new.len() + CHUNK);
        if off_a > 0 {
            mid.extend_from_slice(&self.chunks[ia].data()[..off_a]);
        }
        mid.extend_from_slice(new);
        let mut rest = ib;
        if ib < self.chunks.len() && off_b > 0 {
            mid.extend_from_slice(&self.chunks[ib].data()[off_b..]);
            rest = ib + 1;
        }
        // A short piece joins the chunk after it rather than staying a sliver.
        if !mid.is_empty() && mid.len() % CHUNK < CHUNK / 8 && rest < self.chunks.len() && self.chunks[rest].len < CHUNK / 2 {
            mid.extend_from_slice(&self.chunks[rest].data());
            rest += 1;
        }
        out.extend(mid.chunks(CHUNK).map(|p| Chunk::new(p.to_vec())));
        out.extend(self.chunks[rest..].iter().cloned());
        Channel::from_chunks(out)
    }

    fn range(&self, a: usize, b: usize) -> Channel {
        // Cropping shares the whole chunks inside the range.
        let a = a.min(self.len);
        let b = b.min(self.len).max(a);
        let (ia, off_a) = self.locate(a);
        let (ib, off_b) = self.locate(b);
        if ia == ib {
            if a == b {
                return Channel::default();
            }
            let d = self.chunks[ia].data();
            return Channel::from_chunks(vec![Chunk::new(d[off_a..off_b].to_vec())]);
        }
        let mut out = Vec::new();
        if off_a > 0 {
            out.push(Chunk::new(self.chunks[ia].data()[off_a..].to_vec()));
        } else {
            out.push(self.chunks[ia].clone());
        }
        out.extend(self.chunks[ia + 1..ib.min(self.chunks.len())].iter().cloned());
        if ib < self.chunks.len() && off_b > 0 {
            out.push(Chunk::new(self.chunks[ib].data()[..off_b].to_vec()));
        }
        Channel::from_chunks(out)
    }
}

// ------------------------------------------------------------------ audio

/// One or more channels of the same length. Cloning is cheap (shared).
#[derive(Clone)]
pub struct Audio {
    chans: Arc<Vec<Channel>>,
}

impl Default for Audio {
    fn default() -> Self {
        Audio::empty(1)
    }
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Audio({} ch × {})", self.n_ch(), self.len())
    }
}

impl From<Vec<Vec<f32>>> for Audio {
    fn from(v: Vec<Vec<f32>>) -> Self {
        Audio::from_vec(&v)
    }
}

impl From<&[Vec<f32>]> for Audio {
    fn from(v: &[Vec<f32>]) -> Self {
        Audio::from_vec(v)
    }
}

impl From<Arc<Vec<Vec<f32>>>> for Audio {
    fn from(v: Arc<Vec<Vec<f32>>>) -> Self {
        Audio::from_vec(&v)
    }
}

impl Audio {
    pub fn empty(n_ch: usize) -> Audio {
        Audio { chans: Arc::new(vec![Channel::default(); n_ch.max(1)]) }
    }

    pub fn from_vec(v: &[Vec<f32>]) -> Audio {
        if v.is_empty() {
            return Audio::empty(1);
        }
        let len = v.iter().map(|c| c.len()).min().unwrap_or(0);
        Audio { chans: Arc::new(v.iter().map(|c| Channel::from_samples(&c[..len])).collect()) }
    }

    pub fn len(&self) -> usize {
        self.chans.iter().map(|c| c.len).min().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn n_ch(&self) -> usize {
        self.chans.len()
    }

    /// Same storage (an unchanged copy).
    pub fn ptr_eq(&self, other: &Audio) -> bool {
        Arc::ptr_eq(&self.chans, &other.chans)
    }

    /// Frames [start, start + out.len()) of channel `c` (zeros past the end).
    pub fn read(&self, c: usize, start: usize, out: &mut [f32]) {
        match self.chans.get(c) {
            Some(ch) => ch.read(start, out),
            None => out.iter_mut().for_each(|v| *v = 0.0),
        }
    }

    /// As `read`, without ever waiting (for the audio callback). False if
    /// some of it wasn't in memory (that part reads as silence).
    pub fn read_rt(&self, c: usize, start: usize, out: &mut [f32]) -> bool {
        match self.chans.get(c) {
            Some(ch) => ch.read_rt(start, out),
            None => {
                out.iter_mut().for_each(|v| *v = 0.0);
                true
            }
        }
    }

    pub fn sample(&self, c: usize, i: usize) -> f32 {
        let mut v = [0.0f32];
        self.read(c, i, &mut v);
        v[0]
    }

    /// Channel `c`'s frames [a, b).
    pub fn channel_range(&self, c: usize, a: usize, b: usize) -> Vec<f32> {
        let b = b.min(self.len());
        let mut v = vec![0.0; b.saturating_sub(a)];
        self.read(c, a, &mut v);
        v
    }

    /// Frames [a, b) of every channel, as plain vectors (for effects).
    pub fn slice(&self, a: usize, b: usize) -> Vec<Vec<f32>> {
        (0..self.n_ch()).map(|c| self.channel_range(c, a, b)).collect()
    }

    /// Everything, as plain vectors.
    pub fn to_vec(&self) -> Vec<Vec<f32>> {
        self.slice(0, self.len())
    }

    /// Frames [a, b) as audio sharing this one's chunks.
    pub fn crop(&self, a: usize, b: usize) -> Audio {
        Audio { chans: Arc::new(self.chans.iter().map(|c| c.range(a, b)).collect()) }
    }

    /// (min, max) of channel `c` over [a, b).
    pub fn min_max(&self, c: usize, a: usize, b: usize) -> (f32, f32) {
        self.chans.get(c).map(|ch| ch.min_max(a, b)).unwrap_or((0.0, 0.0))
    }

    /// Peak level of channel `c` (from the chunk summaries).
    pub fn peak(&self, c: usize) -> f32 {
        self.chans.get(c).map(|ch| ch.chunks.iter().map(|k| k.lo.abs().max(k.hi.abs())).fold(0.0, f32::max)).unwrap_or(0.0)
    }

    /// RMS of channel `c` (from the chunk summaries).
    pub fn rms(&self, c: usize) -> f32 {
        let Some(ch) = self.chans.get(c) else { return 0.0 };
        if ch.len == 0 {
            return 0.0;
        }
        (ch.chunks.iter().map(|k| k.sumsq).sum::<f64>() / ch.len as f64).sqrt() as f32
    }

    /// Frames [a, b) replaced by `new`. Channels where `keep[c]` is true
    /// are left alone (only when the length doesn't change); a result with
    /// fewer channels than this audio is used for the extra ones too, except
    /// that a same-length stereo result leaves surround channels alone.
    pub fn replace(&self, a: usize, b: usize, new: &[Vec<f32>], keep: &[bool]) -> Audio {
        let len = self.len();
        let (a, b) = (a.min(len), b.min(len).max(a.min(len)));
        let new_len = new.first().map(|c| c.len()).unwrap_or(0);
        let same_len = new_len == b - a;
        let chans = self
            .chans
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                if same_len && (keep.get(c).copied().unwrap_or(false) || (c >= new.len() && new.len() > 1)) {
                    return ch.clone();
                }
                let src = new.get(c.min(new.len().saturating_sub(1))).map(|v| &v[..new_len.min(v.len())]).unwrap_or(&[]);
                ch.splice(a, b, src)
            })
            .collect();
        Audio { chans: Arc::new(chans) }
    }

    /// Frames [a, b) replaced by `other`, sharing its chunks (channel counts
    /// matched by repetition).
    pub fn replace_with(&self, a: usize, b: usize, other: &Audio) -> Audio {
        let len = self.len();
        let (a, b) = (a.min(len), b.min(len).max(a.min(len)));
        let chans = self
            .chans
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                let head = ch.range(0, a);
                let mid = other.chans[c.min(other.chans.len() - 1)].range(0, other.len());
                let tail = ch.range(b, len);
                Channel::from_chunks(head.chunks.into_iter().chain(mid.chunks).chain(tail.chunks).collect())
            })
            .collect();
        Audio { chans: Arc::new(chans) }
    }

    /// `other` added at the end, sharing both files' chunks (channel counts
    /// matched by repetition).
    pub fn append(&self, other: &Audio) -> Audio {
        let len = self.len();
        let chans = self
            .chans
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                let head = ch.range(0, len);
                let tail = &other.chans[c.min(other.chans.len() - 1)];
                let tail = tail.range(0, other.len());
                Channel::from_chunks(head.chunks.into_iter().chain(tail.chunks).collect())
            })
            .collect();
        Audio { chans: Arc::new(chans) }
    }

    /// Frames [a, b) taken out.
    pub fn delete(&self, a: usize, b: usize) -> Audio {
        self.replace(a, b, &vec![Vec::new(); self.n_ch()], &[])
    }

    /// `other` inserted at `at` (channel counts matched by repetition).
    pub fn insert(&self, at: usize, other: &[Vec<f32>]) -> Audio {
        self.replace(at, at, other, &[])
    }

    /// Keep these chunks in memory (reading back any that were moved out), so
    /// playback of [a, b) won't find gaps.
    pub fn prefetch(&self, a: usize, b: usize) {
        for ch in self.chans.iter() {
            let (i0, _) = ch.locate(a);
            let (i1, _) = ch.locate(b.saturating_sub(1).max(a));
            for c in &ch.chunks[i0.min(ch.chunks.len())..(i1 + 1).min(ch.chunks.len())] {
                let _ = c.data();
            }
        }
    }

    /// Number of chunks this audio shares with `other` (for tests).
    #[cfg(test)]
    fn shared_chunks(&self, other: &Audio) -> usize {
        let ids: std::collections::HashSet<u64> = other.chans.iter().flat_map(|c| c.chunks.iter().map(|k| k.id)).collect();
        self.chans.iter().flat_map(|c| c.chunks.iter()).filter(|k| ids.contains(&k.id)).count()
    }
}

/// Builds audio a block at a time (decoding, recording) without ever holding
/// the whole thing as one buffer. It can be read while it grows (for drawing
/// a recording as it happens).
pub struct Builder {
    pending: Vec<Vec<f32>>,
    done: Vec<Channel>,
}

impl Builder {
    pub fn new(n_ch: usize) -> Builder {
        let n = n_ch.max(1);
        Builder { pending: vec![Vec::with_capacity(CHUNK); n], done: vec![Channel::default(); n] }
    }

    pub fn n_ch(&self) -> usize {
        self.pending.len()
    }

    /// Frames so far.
    pub fn len(&self) -> usize {
        self.done[0].len + self.pending[0].len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append interleaved frames (the first `n_ch()` channels of each).
    pub fn push_interleaved(&mut self, samples: &[f32], n_ch: usize) {
        let n = self.n_ch();
        for frame in samples.chunks(n_ch.max(1)) {
            for c in 0..n {
                self.pending[c].push(frame.get(c).copied().unwrap_or(0.0));
            }
            if self.pending[0].len() == CHUNK {
                self.flush();
            }
        }
    }

    /// Append the same number of frames to each channel.
    pub fn push(&mut self, chans: &[Vec<f32>]) {
        let frames = chans.iter().map(|c| c.len()).min().unwrap_or(0);
        let mut done = 0;
        while done < frames {
            let take = (CHUNK - self.pending[0].len()).min(frames - done);
            for c in 0..self.n_ch() {
                let src = &chans[c.min(chans.len() - 1)];
                self.pending[c].extend_from_slice(&src[done..done + take]);
            }
            done += take;
            if self.pending[0].len() == CHUNK {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        for c in 0..self.n_ch() {
            let p = std::mem::replace(&mut self.pending[c], Vec::with_capacity(CHUNK));
            if !p.is_empty() {
                let ch = &mut self.done[c];
                let chunk = Chunk::new(p);
                ch.starts.push(ch.len);
                ch.len += chunk.len;
                ch.chunks.push(chunk);
            }
        }
    }

    /// (min, max) of channel `c` over [a, b) so far.
    pub fn min_max(&self, c: usize, a: usize, b: usize) -> (f32, f32) {
        let c = c.min(self.n_ch() - 1);
        let split = self.done[c].len;
        let b = b.min(self.len());
        if a >= b {
            return (0.0, 0.0);
        }
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        if a < split {
            let (l, h) = self.done[c].min_max(a, b.min(split));
            lo = l;
            hi = h;
        }
        if b > split {
            for &v in &self.pending[c][a.max(split) - split..b - split] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        if lo > hi {
            (0.0, 0.0)
        } else {
            (lo, hi)
        }
    }

    /// Channel `c`'s frames [a, b) so far.
    pub fn range(&self, c: usize, a: usize, b: usize) -> Vec<f32> {
        let c = c.min(self.n_ch() - 1);
        let split = self.done[c].len;
        let b = b.min(self.len());
        let mut out = vec![0.0; b.saturating_sub(a)];
        if a >= b {
            return out;
        }
        if a < split {
            self.done[c].read(a, &mut out[..b.min(split) - a]);
        }
        if b > split {
            let from = a.max(split);
            out[from - a..].copy_from_slice(&self.pending[c][from - split..b - split]);
        }
        out
    }

    /// Drop the first `n` frames.
    pub fn finish_skipping(self, n: usize) -> Audio {
        let a = self.finish();
        let len = a.len();
        a.crop(n.min(len), len)
    }

    pub fn finish(mut self) -> Audio {
        self.flush();
        Audio { chans: Arc::new(self.done) }
    }
}

/// A view of audio that can be read sequentially by the audio callback:
/// a window of frames copied out ahead of time.
pub struct RtWindow {
    pub start: usize,
    pub len: usize,
    pub data: Vec<Vec<f32>>,
}

impl RtWindow {
    pub fn new(n_ch: usize, cap: usize) -> RtWindow {
        RtWindow { start: 0, len: 0, data: vec![vec![0.0; cap]; n_ch.max(1)] }
    }

    /// Refill from `audio` starting at `start` (without allocating).
    pub fn fill(&mut self, audio: &Audio, start: usize) {
        let cap = self.data[0].len();
        let n = audio.n_ch().min(self.data.len());
        for c in 0..n {
            audio.read_rt(c, start, &mut self.data[c][..cap]);
        }
        self.start = start;
        self.len = cap.min(audio.len().saturating_sub(start));
    }

    #[inline]
    pub fn contains(&self, i: usize) -> bool {
        i >= self.start && i < self.start + self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize, k: f32) -> Vec<f32> {
        (0..n).map(|i| ((i as f32) * 0.001 + k).sin()).collect()
    }

    #[test]
    fn edits_match_vectors_and_share_chunks() {
        let n = CHUNK * 5 + 1234;
        let base = vec![ramp(n, 0.0), ramp(n, 1.0)];
        let a = Audio::from_vec(&base);
        assert_eq!(a.len(), n);
        assert_eq!(a.to_vec(), base);
        // Replace a short range in the middle: only the chunks it touches change.
        let new = vec![vec![0.5; 100], vec![-0.5; 100]];
        let b = a.replace(CHUNK * 2 + 10, CHUNK * 2 + 110, &new, &[]);
        let mut want = base.clone();
        for (c, w) in want.iter_mut().enumerate() {
            w.splice(CHUNK * 2 + 10..CHUNK * 2 + 110, new[c].iter().copied());
        }
        assert_eq!(b.to_vec(), want);
        let total: usize = a.chans.iter().map(|c| c.chunks.len()).sum();
        assert!(b.shared_chunks(&a) >= total - 2 * 2, "shared {} of {total}", b.shared_chunks(&a));
        // The original is untouched (it's an undo step).
        assert_eq!(a.to_vec(), base);
        // Delete and insert.
        let d = a.delete(100, CHUNK * 3);
        let mut wd = base.clone();
        wd.iter_mut().for_each(|c| {
            c.drain(100..CHUNK * 3);
        });
        assert_eq!(d.to_vec(), wd);
        let ins = d.insert(50, &[vec![9.0; 7]]);
        let mut wi = wd.clone();
        wi.iter_mut().for_each(|c| {
            c.splice(50..50, std::iter::repeat(9.0).take(7));
        });
        assert_eq!(ins.to_vec(), wi);
        // Only some channels.
        let one = a.replace(10, 20, &[vec![1.0; 10]], &[false, true]);
        assert_eq!(one.channel_range(0, 10, 20), vec![1.0; 10]);
        assert_eq!(one.channel_range(1, 0, n), base[1]);
        // Cropping shares the inside chunks.
        let c = a.crop(CHUNK / 2, CHUNK * 4 + 5);
        assert_eq!(c.channel_range(0, 0, c.len()), base[0][CHUNK / 2..CHUNK * 4 + 5].to_vec());
        assert!(c.shared_chunks(&a) >= 2 * 3);
    }

    #[test]
    fn random_edits_match_a_reference() {
        let mut seed = 12345u64;
        let mut rnd = |m: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m.max(1) as u64) as usize
        };
        let mut want = vec![ramp(CHUNK * 3 + 77, 0.3)];
        let mut a = Audio::from_vec(&want);
        for step in 0..200 {
            let len = want[0].len();
            let x = rnd(len + 1);
            let y = (x + rnd(CHUNK * 2)).min(len);
            let ins: Vec<f32> = (0..rnd(CHUNK + 50)).map(|i| (i + step) as f32).collect();
            a = a.replace(x, y, &[ins.clone()], &[]);
            want[0].splice(x..y, ins);
            if step % 20 == 0 {
                assert_eq!(a.to_vec(), want, "step {step}");
            }
        }
        assert_eq!(a.to_vec(), want);
        // Chunk count stays reasonable (no slivers piling up).
        let n = a.chans[0].chunks.len();
        assert!(n <= want[0].len() / CHUNK * 3 + 8, "{n} chunks for {}", want[0].len());
    }

    #[test]
    fn min_max_peak_and_rms() {
        let mut v = vec![0.0f32; CHUNK * 2 + 500];
        v[CHUNK + 300] = 0.9;
        v[CHUNK + 301] = -0.7;
        v[7] = 0.2;
        let a = Audio::from_vec(&[v.clone()]);
        assert_eq!(a.min_max(0, 0, v.len()), (-0.7, 0.9));
        assert_eq!(a.min_max(0, 0, 100), (0.0, 0.2));
        assert_eq!(a.min_max(0, CHUNK + 301, CHUNK + 302), (-0.7, -0.7));
        assert_eq!(a.min_max(0, CHUNK + 302, CHUNK * 2), (0.0, 0.0));
        assert!((a.peak(0) - 0.9).abs() < 1e-6);
        let want = ((0.81f64 + 0.49 + 0.04) / v.len() as f64).sqrt() as f32;
        assert!((a.rms(0) - want).abs() < 1e-6);
    }

    #[test]
    fn builder_streams_into_chunks() {
        let n = CHUNK * 2 + 999;
        let l = ramp(n, 0.0);
        let r = ramp(n, 2.0);
        let mut b = Builder::new(2);
        let inter: Vec<f32> = (0..n).flat_map(|i| [l[i], r[i]]).collect();
        for part in inter.chunks(4096 * 2 + 2) {
            b.push_interleaved(part, 2);
        }
        let a = b.finish();
        assert_eq!(a.to_vec(), vec![l.clone(), r.clone()]);
        let mut b = Builder::new(2);
        b.push(&[l[..1000].to_vec(), r[..1000].to_vec()]);
        b.push(&[l[1000..].to_vec(), r[1000..].to_vec()]);
        assert_eq!(b.finish_skipping(10).to_vec(), vec![l[10..].to_vec(), r[10..].to_vec()]);
    }

    #[test]
    fn builder_reads_while_growing_and_takes_splice_in() {
        let n = CHUNK + 5000;
        let v = ramp(n, 0.4);
        let mut b = Builder::new(1);
        b.push(&[v[..CHUNK + 1000].to_vec()]);
        assert_eq!(b.len(), CHUNK + 1000);
        assert_eq!(b.range(0, CHUNK - 10, CHUNK + 10), v[CHUNK - 10..CHUNK + 10].to_vec());
        let want = v[100..CHUNK + 900].iter().fold((f32::MAX, f32::MIN), |m, &x| (m.0.min(x), m.1.max(x)));
        assert_eq!(b.min_max(0, 100, CHUNK + 900), want);
        b.push(&[v[CHUNK + 1000..].to_vec()]);
        let take = b.finish();
        assert_eq!(take.to_vec(), vec![v.clone()]);
        // A take dropped into a file shares its chunks.
        let file = Audio::from_vec(&[vec![0.25; CHUNK * 3]]);
        let joined = file.replace_with(CHUNK, CHUNK + 10, &take);
        assert_eq!(joined.len(), CHUNK * 3 - 10 + n);
        assert_eq!(joined.channel_range(0, CHUNK, CHUNK + n), v);
        assert!(joined.shared_chunks(&take) >= 1);
        assert_eq!(joined.sample(0, CHUNK + n), 0.25);
    }

    #[test]
    fn rt_reads_and_windows() {
        let v = ramp(CHUNK + 100, 0.1);
        let a = Audio::from_vec(&[v.clone()]);
        let mut out = vec![0.0; 300];
        assert!(a.read_rt(0, CHUNK - 100, &mut out));
        assert_eq!(out, v[CHUNK - 100..CHUNK + 100].iter().copied().chain(std::iter::repeat(0.0).take(100)).collect::<Vec<_>>());
        let mut w = RtWindow::new(1, 512);
        w.fill(&a, CHUNK - 10);
        assert_eq!(w.len, 110);
        assert!(w.contains(CHUNK + 5) && !w.contains(CHUNK + 200));
        assert_eq!(w.data[0][0], v[CHUNK - 10]);
    }

    #[test]
    fn chunks_move_to_disk_and_back() {
        // A tiny budget forces everything but the newest chunks out to the
        // working file; reading brings them back intact.
        set_budget(64 << 20);
        let n = CHUNK * 300; // 300 chunks × 256 KB ≈ 75 MB > 64 MB
        let v: Vec<f32> = (0..n).map(|i| (i % 9973) as f32 * 1e-4).collect();
        let a = Audio::from_vec(&[v.clone()]);
        let (res, disk, budget) = usage();
        // (Other tests run alongside and add their own chunks.)
        assert!(res <= budget + (16 << 20), "{res} in memory, budget {budget}");
        assert!(disk > 0, "nothing was written out");
        let mut out = vec![0.0; 1000];
        a.read(0, 5, &mut out);
        assert_eq!(out, v[5..1005].to_vec());
        a.read(0, n - 1000, &mut out);
        assert_eq!(out, v[n - 1000..].to_vec());
        assert_eq!(a.min_max(0, 0, n), (0.0, 9972.0 * 1e-4));
        drop(a);
        set_budget(0);
    }
}
