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

/// Frames held by a [`StreamBuf`]'s ring (about 43 s at 48 kHz).
pub const STREAM_RING: usize = 1 << 21;

/// Audio rendered on the fly (the real-time Effects Rack). A renderer thread
/// writes whole blocks and flags them ready; the audio callback reads them
/// without locking and holds its position if it reaches an unready block.
/// The frames live in a ring covering the playhead and what's just ahead of
/// it, so a long file doesn't need a buffer of its whole length.
pub struct StreamBuf {
    pub len: usize,
    pub n_ch: usize,
    /// Ring size in frames (a whole number of blocks).
    cap: usize,
    data: Vec<Vec<AtomicU32>>,
    /// Per ring block: 1 + the file block it holds (0 = nothing).
    ready: Vec<AtomicUsize>,
    play_pos: AtomicU64,
    region: [AtomicUsize; 2],
    looping: AtomicBool,
}

impl StreamBuf {
    pub fn new(len: usize, n_ch: usize, pos: usize) -> Arc<Self> {
        let n_ch = n_ch.max(1);
        let cap = len.min(STREAM_RING).div_ceil(STREAM_BLOCK).max(1) * STREAM_BLOCK;
        Arc::new(StreamBuf {
            len,
            n_ch,
            cap,
            data: (0..n_ch).map(|_| (0..cap).map(|_| AtomicU32::new(0)).collect()).collect(),
            ready: (0..cap / STREAM_BLOCK).map(|_| AtomicUsize::new(0)).collect(),
            play_pos: AtomicU64::new((pos as f64).to_bits()),
            region: [AtomicUsize::new(0), AtomicUsize::new(len)],
            looping: AtomicBool::new(false),
        })
    }

    /// Write `chans` starting at block-aligned frame `start` and mark the
    /// blocks it fully covers (or that end at the buffer's end) ready.
    pub fn write(&self, start: usize, chans: &[Vec<f32>]) {
        let n = chans.first().map(|c| c.len()).unwrap_or(0).min(self.len.saturating_sub(start));
        let nb = self.ready.len();
        // The blocks about to be overwritten stop being ready first.
        let mut b = start / STREAM_BLOCK;
        while b * STREAM_BLOCK < start + n {
            self.ready[b % nb].store(0, Ordering::Release);
            b += 1;
        }
        for (c, dst) in self.data.iter().enumerate() {
            let src = &chans[c.min(chans.len() - 1)];
            for i in 0..n {
                dst[(start + i) % self.cap].store(src[i].to_bits(), Ordering::Relaxed);
            }
        }
        let end = start + n;
        let mut b = start / STREAM_BLOCK;
        while b * STREAM_BLOCK < end {
            let block_end = ((b + 1) * STREAM_BLOCK).min(self.len);
            if b * STREAM_BLOCK >= start && block_end <= end {
                self.ready[b % nb].store(b + 1, Ordering::Release);
            }
            b += 1;
        }
    }

    pub fn is_ready(&self, frame: usize) -> bool {
        let b = frame / STREAM_BLOCK;
        frame < self.len && self.ready[b % self.ready.len()].load(Ordering::Acquire) == b + 1
    }

    /// Frames the ring holds.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[inline]
    fn sample(&self, c: usize, i: usize) -> f32 {
        f32::from_bits(self.data[c.min(self.n_ch - 1)][i % self.cap].load(Ordering::Relaxed))
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
    buffer: crate::audio::Audio,
    /// Bumped whenever `buffer` is replaced, so the callback refills its window.
    buffer_gen: u64,
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
    /// Per-channel peaks of what's playing (up to 7.1), and how many channels.
    ch_peaks: ([f32; 8], usize),
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

/// The UI thread's copy of the recording so far (first two channels), built
/// into chunked audio as it arrives (so a long take is held once, partly on
/// disk), and readable for drawing the live waveform.
pub struct RecView {
    take: crate::audio::Builder,
    /// Interleaved samples already copied.
    taken: usize,
}

impl Default for RecView {
    fn default() -> Self {
        RecView { take: crate::audio::Builder::new(1), taken: 0 }
    }
}

impl RecView {
    pub fn frames(&self) -> usize {
        self.take.len()
    }

    pub fn n_ch(&self) -> usize {
        self.take.n_ch()
    }

    /// Exact (min, max) of channel `c` over frames [a, b).
    pub fn min_max(&self, c: usize, a: usize, b: usize) -> (f32, f32) {
        self.take.min_max(c, a, b)
    }

    /// Channel `c`'s frames [a, b).
    pub fn range(&self, c: usize, a: usize, b: usize) -> Vec<f32> {
        self.take.range(c, a, b)
    }

    fn reset(&mut self, n: usize) {
        self.take = crate::audio::Builder::new(n);
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
    // Asking about another ASIO driver must not unload the one in use.
    let d = if is_asio(host_name) { asio_device(name, false) } else { find_output(host_name, name) };
    let Some(d) = d else { return (2, Vec::new(), None) };
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
    if is_asio(host_name) {
        // Read from the registry: enumerating through the driver would
        // load each one, which fails while another is in use.
        let names = asio_driver_names();
        return (names.clone(), names);
    }
    if is_jack(host_name) {
        return (vec![JACK_DEVICE.into()], vec![JACK_DEVICE.into()]);
    }
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

pub const ASIO: &str = "ASIO";
pub const JACK: &str = "JACK";
/// The single "device" listed for JACK: Audemo's own client on the server.
const JACK_DEVICE: &str = "JACK server";

pub fn is_asio(host_name: Option<&str>) -> bool {
    host_name == Some(ASIO)
}

pub fn is_jack(host_name: Option<&str>) -> bool {
    host_name == Some(JACK)
}

thread_local! {
    /// ASIO allows one loaded driver per process, and each cpal ASIO host
    /// would load its own, so one host is kept along with the device of the
    /// loaded driver. Output and input streams share that device (and its
    /// buffers). Engine and Preferences calls all come from the UI thread.
    static ASIO_STATE: std::cell::RefCell<(Option<cpal::Host>, Option<cpal::Device>)> = const { std::cell::RefCell::new((None, None)) };
}

/// The ASIO driver `name` (None = the loaded one, else the first that
/// works). Another driver can only be loaded when `may_switch` and nothing
/// still uses the current one.
fn asio_device(name: Option<&str>, may_switch: bool) -> Option<cpal::Device> {
    ASIO_STATE.with(|st| {
        let mut st = st.borrow_mut();
        if st.0.is_none() {
            let id = cpal::available_hosts().into_iter().find(|h| h.name() == ASIO)?;
            st.0 = Some(cpal::host_from_id(id).ok()?);
        }
        if let Some(d) = &st.1 {
            if name.map(|n| d.name().map(|x| x == n).unwrap_or(false)).unwrap_or(true) {
                return Some(d.clone());
            }
            if !may_switch {
                return None;
            }
        }
        // Unloads the previous driver (once no stream holds it).
        st.1 = None;
        let host = st.0.as_ref()?;
        let named = name.and_then(|n| host.devices().ok()?.find(|d| d.name().map(|x| x == n).unwrap_or(false)));
        let dev = named.or_else(|| host.devices().ok()?.find(|d| d.default_output_config().is_ok()));
        st.1 = dev.clone();
        dev
    })
}

/// Unload the ASIO driver (when no stream uses it), so the next open
/// starts it afresh with the chosen buffer size and rate.
fn release_asio() {
    ASIO_STATE.with(|st| st.borrow_mut().1 = None);
}

/// Installed ASIO drivers, as registered under HKLM\SOFTWARE\ASIO.
fn asio_driver_names() -> Vec<String> {
    #[cfg(windows)]
    {
        win_asio::driver_names()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Open the ASIO driver's own control panel (buffer size, clock, routing).
pub fn asio_control_panel(name: Option<&str>) -> Result<(), String> {
    asio_device(name, false).ok_or("Another ASIO driver is in use. Click OK to switch to this one, then open its settings.")?;
    #[cfg(windows)]
    {
        win_asio::control_panel()
    }
    #[cfg(not(windows))]
    {
        Err("ASIO is only available on Windows.".into())
    }
}

#[cfg(windows)]
mod win_asio {
    use std::ffi::c_void;

    type Hkey = *mut c_void;
    // HKEY_LOCAL_MACHINE is (HKEY)(LONG)0x80000002, sign-extended.
    const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002u32 as i32 as isize as Hkey;
    const KEY_READ: u32 = 0x2_0019;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(key: Hkey, sub_key: *const u16, options: u32, sam: u32, result: *mut Hkey) -> i32;
        fn RegEnumKeyExW(key: Hkey, index: u32, name: *mut u16, name_len: *mut u32, reserved: *mut u32, class: *mut u16, class_len: *mut u32, last_write: *mut c_void) -> i32;
        fn RegCloseKey(key: Hkey) -> i32;
    }

    // From the ASIO SDK's host library, which cpal's ASIO support links in
    // (C++ linkage: `ASIOError ASIOControlPanel(void)`).
    extern "C" {
        #[link_name = "?ASIOControlPanel@@YAJXZ"]
        fn ASIOControlPanel() -> i32;
    }

    pub fn driver_names() -> Vec<String> {
        let path: Vec<u16> = "SOFTWARE\\ASIO".encode_utf16().chain(Some(0)).collect();
        let mut key: Hkey = std::ptr::null_mut();
        let mut out = Vec::new();
        unsafe {
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return out;
            }
            for i in 0.. {
                let mut buf = [0u16; 256];
                let mut len = buf.len() as u32;
                let r = RegEnumKeyExW(key, i, buf.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                if r != 0 {
                    break;
                }
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            RegCloseKey(key);
        }
        out
    }

    pub fn control_panel() -> Result<(), String> {
        match unsafe { ASIOControlPanel() } {
            0 => Ok(()),
            e => Err(format!("The ASIO driver has no control panel (error {e}).")),
        }
    }
}

/// Audemo's client on the JACK server (its ports connect to the system
/// ports automatically). None when no server is running.
fn jack_device(output: bool) -> Option<cpal::Device> {
    #[cfg(target_os = "linux")]
    {
        use cpal::platform::JackDevice;
        let d = if output { JackDevice::default_output_device("Audemo", true, false) } else { JackDevice::default_input_device("Audemo", true, false) };
        d.ok().map(cpal::Device::from)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = output;
        None
    }
}

fn find_output(host_name: Option<&str>, name: Option<&str>) -> Option<cpal::Device> {
    if is_asio(host_name) {
        return asio_device(name, true);
    }
    if is_jack(host_name) {
        return jack_device(true);
    }
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
    if is_asio(host_name) {
        // Input and output must come from the driver that is loaded.
        return asio_device(name, false);
    }
    if is_jack(host_name) {
        return jack_device(false);
    }
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
            buffer: crate::audio::Audio::empty(1),
            buffer_gen: 0,
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
            ch_peaks: ([0.0; 8], 2),
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
        spawn_prefetch(Arc::downgrade(&engine.shared));
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
        self.release_driver();
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
        self.release_driver();
        let r = self.open_output();
        self.error = r.err();
        true
    }

    /// Before reopening on ASIO: let the driver restart with the new
    /// settings (it keeps its buffers while any stream uses it).
    fn release_driver(&mut self) {
        if is_asio(self.config.host.as_deref()) || self.host_name == ASIO {
            self.rec = None;
            release_asio();
        }
    }

    /// Unload an ASIO driver that Preferences loaded to read its settings
    /// when the engine isn't using ASIO.
    pub fn drop_idle_driver(&self) {
        if !is_asio(self.config.host.as_deref()) && self.host_name != ASIO {
            release_asio();
        }
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

    /// Monitor volume (linear), e.g. from a control surface's master fader.
    pub fn set_volume(&self, v: f32) {
        if let Ok(mut s) = self.shared.lock() {
            s.volume = v.clamp(0.0, 4.0);
        }
    }

    pub fn play(&self, buffer: impl Into<crate::audio::Audio>, rate: u32, start: f64, end: f64, looping: bool, tag: u64) {
        self.play_ex(buffer, None, rate, start, end, looping, tag);
    }

    /// Play `buffer`, or — when `stream` is given — the stream rendered from it.
    #[allow(clippy::too_many_arguments)]
    pub fn play_ex(&self, buffer: impl Into<crate::audio::Audio>, stream: Option<Arc<StreamBuf>>, rate: u32, start: f64, end: f64, looping: bool, tag: u64) {
        let buffer: crate::audio::Audio = buffer.into();
        // Have the start in memory before the callback needs it.
        buffer.prefetch(start.max(0.0) as usize, (start.max(0.0) as usize).saturating_add(rate as usize * 4));
        if let Ok(mut s) = self.shared.lock() {
            let len = buffer.len() as f64;
            if let Some(sb) = &stream {
                sb.set_region(start, end, looping);
                sb.set_pos(start);
            }
            s.stream = stream;
            s.mix = None;
            s.buffer = buffer;
            s.buffer_gen += 1;
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
    pub fn replace_buffer(&self, buffer: impl Into<crate::audio::Audio>, rate: u32, start: f64, end: f64) {
        let buffer: crate::audio::Audio = buffer.into();
        if let Ok(mut s) = self.shared.lock() {
            let len = buffer.len() as f64;
            s.stream = None;
            s.mix = None;
            s.buffer = buffer;
            s.buffer_gen += 1;
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

    /// Move playback to `pos` while scrubbing. Unless a selection is
    /// looping, the whole file (`len` frames) becomes playable, so you can
    /// scrub back past where playback started or on past its end.
    pub fn scrub(&self, pos: f64, len: f64) {
        if let Ok(mut s) = self.shared.lock() {
            if !s.looping {
                s.start = 0.0;
                s.end = s.end.max(len);
            }
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

    /// Per-channel output peaks since the last call, and the channel count
    /// of what's playing.
    pub fn take_channel_peaks(&self) -> ([f32; 8], usize) {
        match self.shared.lock() {
            Ok(mut s) => {
                let v = s.ch_peaks;
                s.ch_peaks.0 = [0.0; 8];
                v
            }
            Err(_) => ([0.0; 8], 2),
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
        let Ok(mut s) = r.shared.lock() else { return };
        let v = &mut r.view;
        let total = s.samples - s.samples % n_ch;
        let mut i = v.taken;
        let mut picked: Vec<f32> = Vec::new();
        while i < total {
            let chunk = &s.chunks[i / s.chunk_len];
            let off = i % s.chunk_len;
            let n = (chunk.len() - off).min(total - i);
            picked.clear();
            for frame in chunk[off..off + n].chunks_exact(n_ch) {
                for c in 0..keep {
                    picked.push(frame[r.map[c]]);
                }
            }
            v.take.push_interleaved(&picked, keep);
            i += n - n % n_ch;
            if n % n_ch != 0 {
                // A frame straddles two chunks (only if chunk_len % n_ch != 0).
                break;
            }
        }
        v.taken = i;
        // Input chunks already copied are freed (the take lives in `view`).
        let consumed = i / s.chunk_len;
        free_consumed(&mut s.chunks, consumed);
    }

    /// The recording so far, for live drawing.
    pub fn rec_view(&self) -> Option<&RecView> {
        self.rec.as_ref().map(|r| &r.view)
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
        // ASIO input and output share one clock: changing the rate for the
        // input would retune the running output.
        let want_rate = if is_asio(self.config.host.as_deref()) { None } else { want_rate };
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

    /// Stop recording and return the take (at most two channels).
    pub fn stop_recording(&mut self) -> Option<(crate::audio::Audio, u32)> {
        let rec = self.rec.as_mut()?;
        // Stop the input first, then collect whatever arrived since the last frame.
        rec._stream.pause().ok();
        self.poll_recording();
        let rec = self.rec.take()?;
        let rate = rec.rate;
        drop(rec._stream);
        Some((rec.view.take.finish(), rate))
    }
}

/// Keeps the audio just ahead of the playhead in memory, so the callback
/// (which never waits) doesn't find it on disk.
fn spawn_prefetch(shared: std::sync::Weak<Mutex<Shared>>) {
    std::thread::Builder::new()
        .name("audemo-prefetch".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let Some(sh) = shared.upgrade() else { return };
            let job = sh.lock().ok().and_then(|s| (s.playing && s.mix.is_none() && s.stream.is_none()).then(|| (s.buffer.clone(), s.pos, s.src_rate, s.start, s.looping)));
            drop(sh);
            if let Some((audio, pos, rate, start, looping)) = job {
                let a = pos.max(0.0) as usize;
                audio.prefetch(a, a.saturating_add(rate as usize * 8));
                if looping {
                    let s0 = start.max(0.0) as usize;
                    audio.prefetch(s0, s0.saturating_add(rate as usize * 2));
                }
            }
        })
        .ok();
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
    // Up to 7.1 (surround sessions).
    let mut scratch: Vec<Vec<f32>> = (0..8).map(|_| Vec::with_capacity(8192)).collect();
    let mut win = (0usize, 0usize);
    let mut win_version = u64::MAX;
    // Last frame of the previous block, for interpolating across the join.
    let mut prev_last = [0.0f32; 8];
    // Frames of the file being played, copied out ahead of time (the file's
    // audio is chunked and may be partly on disk).
    let mut window = crate::audio::RtWindow::new(8, 32768);
    let mut window_gen = u64::MAX;
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
                if st.buffer_gen != window_gen {
                    window.len = 0;
                    window_gen = st.buffer_gen;
                }
                let n_src = match (&stream, &st.mix) {
                    (Some(sb), _) => sb.n_ch,
                    (None, Some(m)) => m.master_format.channels(),
                    (None, None) => st.buffer.n_ch(),
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
                                prev_last = [0.0; 8];
                                if contiguous {
                                    for (c, p) in prev_last.iter_mut().enumerate().take(m.master_format.channels()) {
                                        *p = scratch[c][win.1 - win.0 - 1];
                                    }
                                }
                                crate::session::mix_into(m, start, start + MIX_BLOCK, &mut scratch, &mut st.track_peaks, &mut st.mix_scratch);
                                win = (start, start + MIX_BLOCK);
                            }
                        }
                        // Each source channel (up to 7.1), then Audemo's left and right
                        // (surround mixed down) onto the mapped device channels, or
                        // surround straight to a device with enough channels.
                        let n_read = n_src.clamp(1, 8);
                        let mut vals = [0.0f32; 8];
                        for (c, out) in vals.iter_mut().enumerate().take(n_read) {
                            let (a, b) = match &stream {
                                Some(sb) => {
                                    let a = sb.sample(c, i0);
                                    (a, if sb.is_ready(i0 + 1) { sb.sample(c, i0 + 1) } else { a })
                                }
                                None if st.mix.is_some() => {
                                    let ch = &scratch[c];
                                    if i0 < win.0 {
                                        (prev_last[c], ch[0])
                                    } else {
                                        (ch[i0 - win.0], ch[i0 + 1 - win.0])
                                    }
                                }
                                None => {
                                    if !window.contains(i0) || (!window.contains(i0 + 1) && i0 + 1 < st.buffer.len()) {
                                        window.fill(&st.buffer, i0);
                                    }
                                    let ch = &window.data[c.min(n_src - 1).min(window.data.len() - 1)];
                                    if window.contains(i0) {
                                        let a = ch[i0 - window.start];
                                        (a, if window.contains(i0 + 1) { ch[i0 + 1 - window.start] } else { a })
                                    } else {
                                        (0.0, 0.0)
                                    }
                                }
                            };
                            *out = (a + (b - a) * fr) * st.volume;
                        }
                        let lr = match n_read {
                            1 => [vals[0], vals[0]],
                            2 => [vals[0], vals[1]],
                            n => crate::dsp::resample::surround_downmix(&vals[..n]),
                        };
                        pk[0] = pk[0].max(lr[0].abs());
                        pk[1] = pk[1].max(lr[1].abs());
                        st.ch_peaks.1 = n_read;
                        for (p, v) in st.ch_peaks.0.iter_mut().zip(&vals[..n_read]) {
                            *p = p.max(v.abs());
                        }
                        frame.iter_mut().for_each(|s| *s = silence);
                        if n_read > 2 && channels >= n_read {
                            for (c, v) in vals.iter().enumerate().take(n_read) {
                                frame[c] = T::from_sample(v.clamp(-1.0, 1.0));
                            }
                        } else if channels == 1 {
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

/// Free the input chunks the take has copied, never the last one: the input
/// callback keeps writing into the last chunk until it's full, so freeing it
/// (when the UI had caught up exactly to its end) would make new samples land
/// in a chunk that was already copied, and the next poll would read past the
/// end of the list.
fn free_consumed(chunks: &mut [Vec<f32>], consumed: usize) {
    let n = consumed.min(chunks.len().saturating_sub(1));
    for c in chunks.iter_mut().take(n) {
        if c.capacity() > 0 {
            *c = Vec::new();
        }
    }
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
    fn freeing_recorded_chunks_keeps_the_one_being_written() {
        let len = 4;
        let mut chunks: Vec<Vec<f32>> = vec![vec![0.0; len], vec![0.0; len]];
        // The UI has copied everything: both chunks are full and consumed.
        free_consumed(&mut chunks, 2);
        assert_eq!(chunks[0].capacity(), 0);
        assert_eq!(chunks[1].len(), len, "the last chunk stays, so the input starts a new one");
        // What the input callback does next: the last chunk is full, so it adds one.
        assert!(chunks.last().map(|c| c.len() >= len).unwrap());
    }

    #[test]
    fn rec_view_min_max_matches_scan() {
        let mut v = RecView::default();
        v.reset(1);
        // Longer than a chunk, so the view spans finished chunks and the tail.
        let n = crate::audio::CHUNK + 10_000;
        let data: Vec<f32> = (0..n).map(|i| ((i as f32 * 0.37).sin() * (i as f32 / n as f32))).collect();
        for part in data.chunks(4099) {
            v.take.push(&[part.to_vec()]);
        }
        assert_eq!(v.frames(), n);
        let c = crate::audio::CHUNK;
        for &(a, b) in &[(0, 1), (3, 70), (63, 129), (100, 9_999), (5, 10_000), (640, 704), (c - 50, c + 50), (c + 9_990, n + 2_000), (0, n)] {
            let want = data[a..b.min(n)].iter().fold((f32::MAX, f32::MIN), |m, &x| (m.0.min(x), m.1.max(x)));
            assert_eq!(v.min_max(0, a, b), want, "{a}..{b}");
        }
        assert_eq!(v.range(0, c - 5, c + 5), data[c - 5..c + 5].to_vec());
    }
}
