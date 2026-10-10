//! Real-time playback and recording through cpal.
//!
//! The audio callback only ever `try_lock`s the shared state, so a busy UI
//! thread produces a moment of silence rather than a glitchy stall.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub type Buffer = Arc<Vec<Vec<f32>>>;

/// Tag used when the engine is playing an effect preview rather than a file.
pub const PREVIEW_TAG: u64 = u64::MAX;
/// Tag used for Media Browser auditioning.
pub const BROWSER_TAG: u64 = u64::MAX - 1;

/// Frames per live multitrack mix block (~20 ms ahead of what you hear).
pub const MIX_BLOCK: usize = 1024;

/// Frames summarised per live-recording peak block.
pub const REC_BLOCK: usize = 64;

/// Frames per readiness block of a [`StreamBuf`].
pub const STREAM_BLOCK: usize = 1024;

/// Audio rendered on the fly (the real-time Effects Rack). A renderer thread
/// writes whole blocks and flags them ready; the audio callback reads them
/// without locking and holds its position if it reaches an unready block.
pub struct StreamBuf {
    pub len: usize,
    pub n_ch: usize,
    data: Vec<Vec<AtomicU32>>,
    ready: Vec<AtomicBool>,
    play_pos: AtomicU64,
    region: [AtomicUsize; 2],
    looping: AtomicBool,
}

impl StreamBuf {
    pub fn new(len: usize, n_ch: usize, pos: usize) -> Arc<Self> {
        let n_ch = n_ch.max(1);
        Arc::new(StreamBuf {
            len,
            n_ch,
            data: (0..n_ch).map(|_| (0..len).map(|_| AtomicU32::new(0)).collect()).collect(),
            ready: (0..len.div_ceil(STREAM_BLOCK)).map(|_| AtomicBool::new(false)).collect(),
            play_pos: AtomicU64::new((pos as f64).to_bits()),
            region: [AtomicUsize::new(0), AtomicUsize::new(len)],
            looping: AtomicBool::new(false),
        })
    }

    /// Write `chans` starting at block-aligned frame `start` and mark the
    /// blocks it fully covers (or that end at the buffer's end) ready.
    pub fn write(&self, start: usize, chans: &[Vec<f32>]) {
        let n = chans.first().map(|c| c.len()).unwrap_or(0).min(self.len.saturating_sub(start));
        for (c, dst) in self.data.iter().enumerate() {
            let src = &chans[c.min(chans.len() - 1)];
            for i in 0..n {
                dst[start + i].store(src[i].to_bits(), Ordering::Relaxed);
            }
        }
        let end = start + n;
        let mut b = start / STREAM_BLOCK;
        while b * STREAM_BLOCK < end {
            let block_end = ((b + 1) * STREAM_BLOCK).min(self.len);
            if b * STREAM_BLOCK >= start && block_end <= end {
                self.ready[b].store(true, Ordering::Release);
            }
            b += 1;
        }
    }

    pub fn is_ready(&self, frame: usize) -> bool {
        frame < self.len && self.ready[frame / STREAM_BLOCK].load(Ordering::Acquire)
    }

    #[inline]
    fn sample(&self, c: usize, i: usize) -> f32 {
        f32::from_bits(self.data[c.min(self.n_ch - 1)][i].load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub fn sample_for_test(&self, c: usize, i: usize) -> f32 {
        self.sample(c, i)
    }

    /// Where playback currently is (or was last asked to be).
    pub fn pos(&self) -> f64 {
        f64::from_bits(self.play_pos.load(Ordering::Relaxed))
    }

    pub fn set_pos(&self, pos: f64) {
        self.play_pos.store(pos.to_bits(), Ordering::Relaxed);
    }

    /// (start, end, looping) of the range being played.
    pub fn region(&self) -> (usize, usize, bool) {
        (self.region[0].load(Ordering::Relaxed), self.region[1].load(Ordering::Relaxed), self.looping.load(Ordering::Relaxed))
    }

    fn set_region(&self, start: f64, end: f64, looping: bool) {
        self.region[0].store(start.max(0.0) as usize, Ordering::Relaxed);
        self.region[1].store((end.max(0.0) as usize).min(self.len), Ordering::Relaxed);
        self.looping.store(looping, Ordering::Relaxed);
    }
}

pub struct Shared {
    buffer: Buffer,
    stream: Option<Arc<StreamBuf>>,
    /// A multitrack session, mixed live in the callback.
    mix: Option<Arc<crate::session::MixState>>,
    /// Bumped whenever `mix` is replaced, so the callback refreshes its window.
    mix_gen: u64,
    /// Per-track (L, R) peaks of the session mix since the UI last read them.
    track_peaks: Vec<[f32; 2]>,
    /// Per-track mix buffers, sized on the UI thread so the callback never allocates.
    mix_scratch: crate::session::MixScratch,
    src_rate: f64,
    pos: f64,
    start: f64,
    end: f64,
    looping: bool,
    playing: bool,
    tag: u64,
    volume: f32,
    peaks: [f32; 2],
    /// Device output channels that Audemo's left and right play on.
    out_map: [usize; 2],
}

/// Live state of a recording in progress, shared with the input callback.
pub struct RecShared {
    /// Interleaved samples in fixed-size chunks, so growing the recording
    /// never reallocates (and copies) inside the audio callback.
    chunks: Vec<Vec<f32>>,
    chunk_len: usize,
    samples: usize,
    pub peaks: [f32; 2],
}

/// The UI thread's de-interleaved copy of the recording so far (first two
/// channels), with a min/max summary per REC_BLOCK frames. It is topped up
/// once per frame, so the live waveform is drawn from the real samples.
#[derive(Default)]
pub struct RecView {
    pub chans: Vec<Vec<f32>>,
    blocks: Vec<Vec<(f32, f32)>>,
    /// Interleaved samples already copied.
    taken: usize,
}

impl RecView {
    pub fn frames(&self) -> usize {
        self.chans.first().map(|c| c.len()).unwrap_or(0)
    }

    /// Exact (min, max) of channel `c` over frames [a, b).
    pub fn min_max(&self, c: usize, a: usize, b: usize) -> (f32, f32) {
        let c = c.min(self.chans.len().saturating_sub(1));
        let (ch, bl) = (&self.chans[c], &self.blocks[c]);
        let b = b.min(ch.len());
        let fold = |acc: (f32, f32), v: f32| (acc.0.min(v), acc.1.max(v));
        let mut m = (f32::MAX, f32::MIN);
        if a >= b {
            return (0.0, 0.0);
        }
        let first_full = a.div_ceil(REC_BLOCK);
        let last_full = b / REC_BLOCK;
        if last_full > first_full + 1 {
            m = ch[a..first_full * REC_BLOCK].iter().copied().fold(m, fold);
            m = bl[first_full..last_full.min(bl.len())].iter().fold(m, |m, &(lo, hi)| (m.0.min(lo), m.1.max(hi)));
            m = ch[(last_full.min(bl.len()) * REC_BLOCK)..b].iter().copied().fold(m, fold);
        } else {
            m = ch[a..b].iter().copied().fold(m, fold);
        }
        m
    }

    fn reset(&mut self, n: usize) {
        self.chans = vec![Vec::new(); n];
        self.blocks = vec![Vec::new(); n];
        self.taken = 0;
    }
}

struct Recording {
    _stream: cpal::Stream,
    shared: Arc<Mutex<RecShared>>,
    /// Device input channels recorded as left and right.
    map: [usize; 2],
    channels: usize,
    rate: u32,
    view: RecView,
}

/// How the audio hardware is set up (from Preferences > Audio Hardware and
/// Audio Channel Mapping).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioConfig {
    /// Audio API name ("CoreAudio", "WASAPI", "ALSA" …); None = default.
    pub host: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    /// Frames per buffer (0 = the device's default).
    pub buffer: u32,
    /// Output sample rate (0 = the device's default).
    pub rate: u32,
    pub out_map: [usize; 2],
    pub in_map: [usize; 2],
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    _stream: Option<cpal::Stream>,
    pub out_rate: u32,
    pub out_channels: usize,
    /// Frames per buffer actually in use (None = the device decides).
    pub out_buffer: Option<u32>,
    pub device_name: String,
    pub host_name: String,
    pub config: AudioConfig,
    /// A sample rate requested for the file being played ("force hardware
    /// to document sample rate").
    rate_override: Option<u32>,
    pub error: Option<String>,
    rec: Option<Recording>,
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub playing: bool,
    pub pos: f64,
    pub tag: u64,
}

/// Names of the audio APIs this system offers ("Device Class").
pub fn hosts() -> Vec<String> {
    cpal::available_hosts().into_iter().map(|h| h.name().to_string()).collect()
}

/// The audio API named `name`, or the system default.
fn host(name: Option<&str>) -> cpal::Host {
    if let Some(n) = name {
        if let Some(id) = cpal::available_hosts().into_iter().find(|h| h.name() == n) {
            if let Ok(h) = cpal::host_from_id(id) {
                return h;
            }
        }
    }
    cpal::default_host()
}

pub fn default_host_name() -> String {
    cpal::default_host().id().name().to_string()
}

/// What an output device supports: (channels, sample rates, buffer-size range).
pub fn output_caps(host_name: Option<&str>, name: Option<&str>) -> (usize, Vec<u32>, Option<(u32, u32)>) {
    let Some(d) = find_output(host_name, name) else { return (2, Vec::new(), None) };
    let ch = d.default_output_config().map(|c| c.channels() as usize).unwrap_or(2);
    let mut rates = Vec::new();
    let mut buf = None;
    if let Ok(ranges) = d.supported_output_configs() {
        for r in ranges {
            for rate in [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000] {
                if r.min_sample_rate().0 <= rate && rate <= r.max_sample_rate().0 && !rates.contains(&rate) {
                    rates.push(rate);
                }
            }
            if let cpal::SupportedBufferSize::Range { min, max } = r.buffer_size() {
                buf = Some((*min, *max));
            }
        }
    }
    rates.sort_unstable();
    (ch, rates, buf)
}

/// Input channels of an input device.
pub fn input_channels(host_name: Option<&str>, name: Option<&str>) -> usize {
    find_input(host_name, name).and_then(|d| d.default_input_config().ok()).map(|c| c.channels() as usize).unwrap_or(2)
}

/// Names of the available (input, output) devices of `host_name`.
pub fn list_devices(host_name: Option<&str>) -> (Vec<String>, Vec<String>) {
    let host = host(host_name);
    let ins = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let outs = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    (ins, outs)
}

fn find_output(host_name: Option<&str>, name: Option<&str>) -> Option<cpal::Device> {
    let host = host(host_name);
    if let Some(n) = name {
        if let Ok(mut it) = host.output_devices() {
            if let Some(d) = it.find(|d| d.name().map(|x| x == n).unwrap_or(false)) {
                return Some(d);
            }
        }
    }
    host.default_output_device()
}

fn find_input(host_name: Option<&str>, name: Option<&str>) -> Option<cpal::Device> {
    let host = host(host_name);
    if let Some(n) = name {
        if let Ok(mut it) = host.input_devices() {
            if let Some(d) = it.find(|d| d.name().map(|x| x == n).unwrap_or(false)) {
                return Some(d);
            }
        }
    }
    host.default_input_device()
}

impl Engine {
    pub fn new(config: AudioConfig) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            buffer: Arc::new(Vec::new()),
            stream: None,
            mix: None,
            mix_gen: 0,
            track_peaks: Vec::new(),
            mix_scratch: Default::default(),
            src_rate: 48000.0,
            pos: 0.0,
            start: 0.0,
            end: 0.0,
            looping: false,
            playing: false,
            tag: 0,
            volume: 1.0,
            peaks: [0.0; 2],
            out_map: config.out_map,
        }));
        let mut engine = Engine {
            shared,
            _stream: None,
            out_rate: 48000,
            out_channels: 2,
            out_buffer: None,
            device_name: "No output device".into(),
            host_name: String::new(),
            config,
            rate_override: None,
            error: None,
            rec: None,
        };
        if let Err(e) = engine.open_output() {
            engine.error = Some(e);
        }
        engine
    }

    /// Apply a new hardware setup, reopening the output if it changed.
    pub fn set_config(&mut self, config: AudioConfig) -> Result<(), String> {
        let reopen = config.host != self.config.host || config.output != self.config.output || config.buffer != self.config.buffer || config.rate != self.config.rate;
        if let Ok(mut s) = self.shared.lock() {
            s.out_map = config.out_map;
        }
        self.config = config;
        if !reopen && self._stream.is_some() {
            return Ok(());
        }
        self.stop();
        self._stream = None;
        self.rate_override = None;
        let r = self.open_output();
        self.error = r.clone().err();
        r
    }

    /// Run the output at `rate` if the device can (None = the configured
    /// rate). Returns true if the output was reopened.
    pub fn set_rate_override(&mut self, rate: Option<u32>) -> bool {
        let want = rate.or(if self.config.rate > 0 { Some(self.config.rate) } else { None });
        if rate == self.rate_override || want == Some(self.out_rate) && rate.is_some() {
            self.rate_override = rate;
            return false;
        }
        self.rate_override = rate;
        self.stop();
        self._stream = None;
        let r = self.open_output();
        self.error = r.err();
        true
    }

    fn open_output(&mut self) -> Result<(), String> {
        let host_name = self.config.host.clone();
        self.host_name = host(host_name.as_deref()).id().name().to_string();
        let device = find_output(host_name.as_deref(), self.config.output.as_deref()).ok_or("No audio output device found.")?;
        self.device_name = device.name().unwrap_or_else(|_| "Default output".into());
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let mut fmt = supported.sample_format();
        let mut buf_range = supported.buffer_size().clone();
        let mut cfg: cpal::StreamConfig = supported.into();
        // A chosen sample rate, if a configuration of the device offers it.
        let want_rate = self.rate_override.or(if self.config.rate > 0 { Some(self.config.rate) } else { None });
        if let Some(rate) = want_rate.filter(|r| *r != cfg.sample_rate.0) {
            if let Ok(ranges) = device.supported_output_configs() {
                let ranges: Vec<_> = ranges.collect();
                let fits = |r: &cpal::SupportedStreamConfigRange| r.min_sample_rate().0 <= rate && rate <= r.max_sample_rate().0;
                let pick = ranges
                    .iter()
                    .find(|r| fits(r) && r.channels() == cfg.channels && r.sample_format() == fmt)
                    .or_else(|| ranges.iter().find(|r| fits(r) && r.channels() == cfg.channels))
                    .or_else(|| ranges.iter().find(|r| fits(r)));
                if let Some(r) = pick {
                    let sc = r.clone().with_sample_rate(cpal::SampleRate(rate));
                    fmt = sc.sample_format();
                    buf_range = sc.buffer_size().clone();
                    cfg = sc.into();
                }
            }
        }
        // A chosen buffer size, clamped to what the device accepts.
        self.out_buffer = None;
        if self.config.buffer > 0 {
            let n = match buf_range {
                cpal::SupportedBufferSize::Range { min, max } => self.config.buffer.clamp(min, max.max(min)),
                cpal::SupportedBufferSize::Unknown => self.config.buffer,
            };
            cfg.buffer_size = cpal::BufferSize::Fixed(n);
            self.out_buffer = Some(n);
        }
        self.out_rate = cfg.sample_rate.0;
        self.out_channels = cfg.channels as usize;
        let build = |cfg: &cpal::StreamConfig| {
            let shared = self.shared.clone();
            match fmt {
                cpal::SampleFormat::F32 => build_output::<f32>(&device, cfg, shared),
                cpal::SampleFormat::I16 => build_output::<i16>(&device, cfg, shared),
                cpal::SampleFormat::U16 => build_output::<u16>(&device, cfg, shared),
                cpal::SampleFormat::I32 => build_output::<i32>(&device, cfg, shared),
                cpal::SampleFormat::F64 => build_output::<f64>(&device, cfg, shared),
                other => Err(format!("Unsupported output sample format {other:?}")),
            }
        };
        let stream = match build(&cfg) {
            Ok(s) => s,
            Err(_) if self.out_buffer.is_some() => {
                // Some drivers refuse fixed sizes: fall back to the default.
                cfg.buffer_size = cpal::BufferSize::Default;
                self.out_buffer = None;
                build(&cfg)?
            }
            Err(e) => return Err(e),
        };
        stream.play().map_err(|e| e.to_string())?;
        self._stream = Some(stream);
        Ok(())
    }

    pub fn play(&self, buffer: Buffer, rate: u32, start: f64, end: f64, looping: bool, tag: u64) {
        self.play_ex(buffer, None, rate, start, end, looping, tag);
    }

    /// Play `buffer`, or — when `stream` is given — the stream rendered from it.
    #[allow(clippy::too_many_arguments)]
    pub fn play_ex(&self, buffer: Buffer, stream: Option<Arc<StreamBuf>>, rate: u32, start: f64, end: f64, looping: bool, tag: u64) {
        if let Ok(mut s) = self.shared.lock() {
            let len = buffer.first().map(|c| c.len()).unwrap_or(0) as f64;
            if let Some(sb) = &stream {
                sb.set_region(start, end, looping);
                sb.set_pos(start);
            }
            s.stream = stream;
            s.mix = None;
            s.buffer = buffer;
            s.src_rate = rate as f64;
            s.start = start.clamp(0.0, len);
            s.end = end.clamp(s.start, len);
            s.pos = s.start;
            s.looping = looping;
            s.tag = tag;
            s.playing = s.end > s.start;
        }
    }

    /// Play a multitrack session, mixed live from `state`.
    pub fn play_mix(&self, state: Arc<crate::session::MixState>, start: f64, end: f64, looping: bool, tag: u64) {
        let old = if let Ok(mut s) = self.shared.lock() {
            s.src_rate = state.sample_rate as f64;
            s.start = start.max(0.0);
            s.end = end.max(s.start);
            s.pos = s.start;
            s.looping = looping;
            s.tag = tag;
            s.playing = s.end > s.start;
            s.stream = None;
            s.mix_gen += 1;
            s.track_peaks = vec![[0.0; 2]; state.tracks.len()];
            s.mix_scratch.reserve(state.tracks.len(), MIX_BLOCK);
            s.mix.replace(state)
        } else {
            None
        };
        // Release the old state here, not in the audio callback.
        drop(old);
    }

    /// Swap in an edited session while it plays.
    pub fn set_mix(&self, state: Arc<crate::session::MixState>) {
        let old = match self.shared.lock() {
            Ok(mut s) if s.mix.is_some() => {
                // No window reset: the block already rendered plays out (≈20 ms)
                // and effects never process the same audio twice.
                s.end = s.end.max(s.pos);
                if s.track_peaks.len() != state.tracks.len() {
                    s.track_peaks = vec![[0.0; 2]; state.tracks.len()];
                }
                s.mix_scratch.reserve(state.tracks.len(), MIX_BLOCK);
                s.mix.replace(state)
            }
            _ => None,
        };
        drop(old);
    }

    /// Per-track peaks of the playing session since the last call.
    pub fn take_track_peaks(&self) -> Vec<[f32; 2]> {
        match self.shared.lock() {
            Ok(mut s) => {
                let v = s.track_peaks.clone();
                s.track_peaks.iter_mut().for_each(|p| *p = [0.0; 2]);
                v
            }
            Err(_) => Vec::new(),
        }
    }

    /// Change the end of the range being played (a session grew).
    pub fn set_end(&self, end: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.end = end.max(s.start);
        }
    }

    /// Switch the playing source between the plain buffer (None) and a
    /// rendered stream, keeping the position.
    pub fn set_stream(&self, stream: Option<Arc<StreamBuf>>) {
        if let Ok(mut s) = self.shared.lock() {
            if let Some(sb) = &stream {
                sb.set_region(s.start, s.end, s.looping);
                sb.set_pos(s.pos);
            }
            s.stream = stream;
        }
    }

    /// Whether the engine is currently playing through a stream.
    pub fn has_stream(&self) -> bool {
        self.shared.lock().map(|s| s.stream.is_some()).unwrap_or(false)
    }

    /// Swap in new audio (e.g. a re-rendered preview) without restarting.
    pub fn replace_buffer(&self, buffer: Buffer, rate: u32, start: f64, end: f64) {
        if let Ok(mut s) = self.shared.lock() {
            let len = buffer.first().map(|c| c.len()).unwrap_or(0) as f64;
            s.stream = None;
            s.mix = None;
            s.buffer = buffer;
            s.src_rate = rate as f64;
            s.start = start.clamp(0.0, len);
            s.end = end.clamp(s.start, len);
            if s.pos >= s.end || s.pos < s.start {
                s.pos = s.start;
            }
        }
    }

    pub fn stop(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.playing = false;
        }
    }

    pub fn set_looping(&self, looping: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.looping = looping;
            if let Some(sb) = &s.stream {
                sb.set_region(s.start, s.end, looping);
            }
        }
    }

    pub fn seek(&self, pos: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.pos = pos.clamp(s.start, s.end.max(s.start));
            if let Some(sb) = &s.stream {
                sb.set_pos(s.pos);
            }
        }
    }

    pub fn status(&self) -> Status {
        match self.shared.lock() {
            Ok(s) => Status { playing: s.playing, pos: s.pos, tag: s.tag },
            Err(_) => Status { playing: false, pos: 0.0, tag: 0 },
        }
    }

    pub fn is_playing_tag(&self, tag: u64) -> bool {
        let st = self.status();
        st.playing && st.tag == tag
    }

    /// Peak levels since the last call (output, or input while recording).
    pub fn take_peaks(&self) -> [f32; 2] {
        if let Some(r) = &self.rec {
            if let Ok(mut s) = r.shared.lock() {
                let v = s.peaks;
                s.peaks = [0.0; 2];
                return v;
            }
        }
        match self.shared.lock() {
            Ok(mut s) => {
                let v = s.peaks;
                s.peaks = [0.0; 2];
                v
            }
            Err(_) => [0.0; 2],
        }
    }

    pub fn is_recording(&self) -> bool {
        self.rec.is_some()
    }

    /// (input rate, input channels) of the recording in progress.
    pub fn recording_format(&self) -> Option<(u32, usize)> {
        self.rec.as_ref().map(|r| (r.rate, r.channels))
    }

    pub fn recorded_frames(&self) -> usize {
        self.rec.as_ref().map(|r| r.view.frames()).unwrap_or(0)
    }

    /// Copy newly recorded audio into the UI-side view. Call once per frame.
    pub fn poll_recording(&mut self) {
        let Some(r) = &mut self.rec else { return };
        let n_ch = r.channels.max(1);
        let keep = n_ch.min(2);
        let Ok(s) = r.shared.lock() else { return };
        let v = &mut r.view;
        let total = s.samples - s.samples % n_ch;
        let mut i = v.taken;
        while i < total {
            let chunk = &s.chunks[i / s.chunk_len];
            let off = i % s.chunk_len;
            let n = (chunk.len() - off).min(total - i);
            for frame in chunk[off..off + n].chunks_exact(n_ch) {
                for c in 0..keep {
                    v.chans[c].push(frame[r.map[c]]);
                }
            }
            i += n - n % n_ch;
            if n % n_ch != 0 {
                // A frame straddles two chunks (only if chunk_len % n_ch != 0).
                break;
            }
        }
        v.taken = i;
        drop(s);
        let frames = v.frames();
        for c in 0..keep {
            let done = v.blocks[c].len();
            for bi in done..frames / REC_BLOCK {
                let seg = &v.chans[c][bi * REC_BLOCK..(bi + 1) * REC_BLOCK];
                let m = seg.iter().fold((f32::MAX, f32::MIN), |m, &x| (m.0.min(x), m.1.max(x)));
                v.blocks[c].push(m);
            }
        }
    }

    /// The recording so far, for live drawing.
    pub fn rec_view(&self) -> Option<&RecView> {
        self.rec.as_ref().map(|r| &r.view).filter(|v| !v.chans.is_empty())
    }

    /// Start recording from the chosen input; returns (rate, channels).
    /// `want_rate` (the file's or session's rate) is used when the input
    /// supports it, so the take needs no sample-rate conversion afterwards.
    pub fn start_recording(&mut self, want_rate: Option<u32>) -> Result<(u32, usize), String> {
        if let Some(r) = &self.rec {
            return Ok((r.rate, r.channels));
        }
        let device = find_input(self.config.host.as_deref(), self.config.input.as_deref()).ok_or("No audio input device found.")?;
        let default = device.default_input_config().map_err(|e| e.to_string())?;
        let at_rate = want_rate.filter(|&r| r != default.sample_rate().0).and_then(|r| {
            let ranges = device.supported_input_configs().ok()?;
            ranges
                .filter(|c| c.channels() == default.channels() && c.sample_format() == default.sample_format())
                .find(|c| c.min_sample_rate().0 <= r && r <= c.max_sample_rate().0)
                .map(|c| c.with_sample_rate(cpal::SampleRate(r)))
        });
        // Try the file's rate first; if the device refuses it (another app
        // may be holding it at its current rate), record at its default.
        let mut tries = at_rate.into_iter().chain(std::iter::once(default)).peekable();
        let (stream, shared, map, cfg) = loop {
            let supported = tries.next().expect("default config is always tried");
            let last = tries.peek().is_none();
            let fmt = supported.sample_format();
            let cfg: cpal::StreamConfig = supported.into();
            // One-second chunks (a whole number of frames); the first is allocated up front.
            let chunk_len = cfg.sample_rate.0.max(4096) as usize * cfg.channels.max(1) as usize;
            let shared = Arc::new(Mutex::new(RecShared {
                chunks: vec![Vec::with_capacity(chunk_len)],
                chunk_len,
                samples: 0,
                peaks: [0.0; 2],
            }));
            let s = shared.clone();
            let n_in = cfg.channels.max(1) as usize;
            let map = [self.config.in_map[0].min(n_in - 1), self.config.in_map[1].min(n_in - 1)];
            let built = match fmt {
                cpal::SampleFormat::F32 => build_input::<f32>(&device, &cfg, s, map),
                cpal::SampleFormat::I16 => build_input::<i16>(&device, &cfg, s, map),
                cpal::SampleFormat::U16 => build_input::<u16>(&device, &cfg, s, map),
                cpal::SampleFormat::I32 => build_input::<i32>(&device, &cfg, s, map),
                other => Err(format!("Unsupported input sample format {other:?}")),
            }
            .and_then(|st| st.play().map(|_| st).map_err(|e| e.to_string()));
            match built {
                Ok(st) => break (st, shared, map, cfg),
                Err(e) if last => return Err(e),
                Err(_) => continue,
            }
        };
        let (rate, channels) = (cfg.sample_rate.0, cfg.channels as usize);
        let mut view = RecView::default();
        view.reset(channels.clamp(1, 2));
        self.rec = Some(Recording { _stream: stream, shared, map, channels, rate, view });
        Ok((rate, channels))
    }

    /// Stop recording and return de-interleaved audio (max two channels).
    pub fn stop_recording(&mut self) -> Option<(Vec<Vec<f32>>, u32)> {
        let rec = self.rec.as_mut()?;
        // Stop the input first, then collect whatever arrived since the last frame.
        rec._stream.pause().ok();
        self.poll_recording();
        let rec = self.rec.take()?;
        let rate = rec.rate;
        drop(rec._stream);
        Some((rec.view.chans, rate))
    }
}

fn build_output<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    shared: Arc<Mutex<Shared>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = cfg.channels as usize;
    let out_rate = cfg.sample_rate.0 as f64;
    // Mix window for multitrack playback: frames [win.0, win.1) of the session.
    let mut scratch: Vec<Vec<f32>> = vec![Vec::with_capacity(8192), Vec::with_capacity(8192)];
    let mut win = (0usize, 0usize);
    let mut win_version = u64::MAX;
    // Last frame of the previous block, for interpolating across the join.
    let mut prev_last = [0.0f32; 2];
    device
        .build_output_stream(
            cfg,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let silence = T::from_sample(0.0f32);
                let mut guard = match shared.try_lock() {
                    Ok(g) => g,
                    Err(_) => {
                        data.iter_mut().for_each(|s| *s = silence);
                        return;
                    }
                };
                let st: &mut Shared = &mut guard;
                let step = st.src_rate / out_rate;
                let mut pk = [0.0f32; 2];
                let stream = st.stream.clone();
                if st.mix_gen != win_version {
                    win = (0, 0);
                    win_version = st.mix_gen;
                }
                let n_src = match (&stream, &st.mix) {
                    (Some(sb), _) => sb.n_ch,
                    (None, Some(_)) => 2,
                    (None, None) => st.buffer.len(),
                };
                for frame in data.chunks_mut(channels) {
                    if st.playing && n_src > 0 {
                        let i0 = st.pos.floor() as usize;
                        let fr = (st.pos - i0 as f64) as f32;
                        if let Some(sb) = &stream {
                            if !sb.is_ready(i0) {
                                // Not rendered yet: hold here rather than skip.
                                frame.iter_mut().for_each(|s| *s = silence);
                                continue;
                            }
                        }
                        if let Some(m) = &st.mix {
                            let in_win = i0 >= win.0 && i0 + 1 < win.1;
                            let at_join = win.1 > win.0 && i0 + 1 == win.0;
                            if !in_win && !at_join {
                                // Blocks follow on from each other so stateful effects
                                // never see the same audio twice; anything else (a seek,
                                // a loop, a new session) starts afresh at the playhead.
                                let contiguous = win.1 > win.0 && i0 + 1 >= win.1 && i0 <= win.1;
                                let start = if contiguous { win.1 } else { i0 };
                                prev_last = if contiguous { [scratch[0][win.1 - win.0 - 1], scratch[1][win.1 - win.0 - 1]] } else { [0.0; 2] };
                                crate::session::mix_into(m, start, start + MIX_BLOCK, &mut scratch, &mut st.track_peaks, &mut st.mix_scratch);
                                win = (start, start + MIX_BLOCK);
                            }
                        }
                        // Audemo's left and right, then onto the mapped device channels.
                        let mut lr = [0.0f32; 2];
                        for (c, out) in lr.iter_mut().enumerate() {
                            let (a, b) = match &stream {
                                Some(sb) => {
                                    let a = sb.sample(c, i0);
                                    (a, if sb.is_ready(i0 + 1) { sb.sample(c, i0 + 1) } else { a })
                                }
                                None if st.mix.is_some() => {
                                    let ch = &scratch[c.min(1)];
                                    if i0 < win.0 {
                                        (prev_last[c.min(1)], ch[0])
                                    } else {
                                        (ch[i0 - win.0], ch[i0 + 1 - win.0])
                                    }
                                }
                                None => {
                                    let ch = &st.buffer[c.min(n_src - 1)];
                                    let a = ch.get(i0).copied().unwrap_or(0.0);
                                    (a, ch.get(i0 + 1).copied().unwrap_or(a))
                                }
                            };
                            let v = (a + (b - a) * fr) * st.volume;
                            pk[c] = pk[c].max(v.abs());
                            *out = v;
                        }
                        frame.iter_mut().for_each(|s| *s = silence);
                        if channels == 1 {
                            frame[0] = T::from_sample((0.5 * (lr[0] + lr[1])).clamp(-1.0, 1.0));
                        } else {
                            let (ml, mr) = (st.out_map[0].min(channels - 1), st.out_map[1].min(channels - 1));
                            if ml == mr {
                                frame[ml] = T::from_sample((0.5 * (lr[0] + lr[1])).clamp(-1.0, 1.0));
                            } else {
                                frame[ml] = T::from_sample(lr[0].clamp(-1.0, 1.0));
                                frame[mr] = T::from_sample(lr[1].clamp(-1.0, 1.0));
                            }
                        }
                        st.pos += step;
                        if st.pos >= st.end {
                            if st.looping && st.end > st.start {
                                st.pos = st.start;
                            } else {
                                st.playing = false;
                                st.pos = st.end;
                            }
                        }
                    } else {
                        frame.iter_mut().for_each(|s| *s = silence);
                    }
                }
                if let Some(sb) = &stream {
                    sb.set_pos(st.pos);
                }
                if channels == 1 || n_src == 1 {
                    pk[1] = pk[0];
                }
                st.peaks[0] = st.peaks[0].max(pk[0]);
                st.peaks[1] = st.peaks[1].max(pk[1]);
            },
            |e| eprintln!("audio output error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

fn build_input<T>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    shared: Arc<Mutex<RecShared>>,
    map: [usize; 2],
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = cfg.channels.max(1) as usize;
    device
        .build_input_stream(
            cfg,
            move |input: &[T], _: &cpal::InputCallbackInfo| {
                // Recording must never drop audio, so this takes the lock;
                // the UI only holds it for a few microseconds per frame.
                let Ok(mut guard) = shared.lock() else { return };
                let s: &mut RecShared = &mut guard;
                for frame in input.chunks(channels) {
                    for c in 0..2 {
                        let v: f32 = cpal::Sample::to_sample::<f32>(frame[map[c].min(frame.len() - 1)]);
                        s.peaks[c] = s.peaks[c].max(v.abs());
                    }
                    for &smp in frame {
                        if s.chunks.last().map(|c| c.len() >= s.chunk_len).unwrap_or(true) {
                            let n = s.chunk_len;
                            s.chunks.push(Vec::with_capacity(n));
                        }
                        let v: f32 = cpal::Sample::to_sample::<f32>(smp);
                        s.chunks.last_mut().unwrap().push(v);
                        s.samples += 1;
                    }
                }
            },
            |e| eprintln!("audio input error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rec_view_min_max_matches_scan() {
        let mut v = RecView::default();
        v.reset(1);
        let n = 10_000;
        v.chans[0] = (0..n).map(|i| ((i as f32 * 0.37).sin() * (i as f32 / n as f32))).collect();
        for bi in 0..n / REC_BLOCK {
            let seg = &v.chans[0][bi * REC_BLOCK..(bi + 1) * REC_BLOCK];
            v.blocks[0].push(seg.iter().fold((f32::MAX, f32::MIN), |m, &x| (m.0.min(x), m.1.max(x))));
        }
        for &(a, b) in &[(0, 1), (3, 70), (63, 129), (100, 9_999), (5, 10_000), (640, 704), (9_990, 12_000)] {
            let want = v.chans[0][a..b.min(n)].iter().fold((f32::MAX, f32::MIN), |m, &x| (m.0.min(x), m.1.max(x)));
            assert_eq!(v.min_max(0, a, b), want, "{a}..{b}");
        }
    }
}
