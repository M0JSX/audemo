//! Real-time playback and recording through cpal.
//!
//! The audio callback only ever `try_lock`s the shared state, so a busy UI
//! thread produces a moment of silence rather than a glitchy stall.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub type Buffer = Arc<Vec<Vec<f32>>>;

/// Tag used when the engine is playing an effect preview rather than a file.
pub const PREVIEW_TAG: u64 = u64::MAX;
/// Tag used for Media Browser auditioning.
pub const BROWSER_TAG: u64 = u64::MAX - 1;

/// Frames summarised per live-recording peak block.
pub const REC_BLOCK: usize = 256;

pub struct Shared {
    buffer: Buffer,
    src_rate: f64,
    pos: f64,
    start: f64,
    end: f64,
    looping: bool,
    playing: bool,
    tag: u64,
    volume: f32,
    peaks: [f32; 2],
}

/// Live state of a recording in progress, shared with the input callback.
pub struct RecShared {
    /// Interleaved samples in fixed-size chunks, so growing the recording
    /// never reallocates (and copies) inside the audio callback.
    chunks: Vec<Vec<f32>>,
    chunk_len: usize,
    samples: usize,
    /// (min, max) per channel for every REC_BLOCK frames, for live drawing.
    pub blocks: Vec<[(f32, f32); 2]>,
    acc: [(f32, f32); 2],
    acc_frames: usize,
    pub peaks: [f32; 2],
}

struct Recording {
    _stream: cpal::Stream,
    shared: Arc<Mutex<RecShared>>,
    channels: usize,
    rate: u32,
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    _stream: Option<cpal::Stream>,
    pub out_rate: u32,
    pub out_channels: usize,
    pub device_name: String,
    pub input_name: Option<String>,
    pub output_choice: Option<String>,
    pub error: Option<String>,
    rec: Option<Recording>,
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub playing: bool,
    pub pos: f64,
    pub tag: u64,
}

/// Names of the available (input, output) devices.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
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

fn find_output(name: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(n) = name {
        if let Ok(mut it) = host.output_devices() {
            if let Some(d) = it.find(|d| d.name().map(|x| x == n).unwrap_or(false)) {
                return Some(d);
            }
        }
    }
    host.default_output_device()
}

fn find_input(name: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
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
    pub fn new(output: Option<String>, input: Option<String>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            buffer: Arc::new(Vec::new()),
            src_rate: 48000.0,
            pos: 0.0,
            start: 0.0,
            end: 0.0,
            looping: false,
            playing: false,
            tag: 0,
            volume: 1.0,
            peaks: [0.0; 2],
        }));
        let mut engine = Engine {
            shared,
            _stream: None,
            out_rate: 48000,
            out_channels: 2,
            device_name: "No output device".into(),
            input_name: input,
            output_choice: output,
            error: None,
            rec: None,
        };
        if let Err(e) = engine.open_output() {
            engine.error = Some(e);
        }
        engine
    }

    /// Switch output device (None = system default) and reopen the stream.
    pub fn set_output(&mut self, name: Option<String>) -> Result<(), String> {
        self.stop();
        self._stream = None;
        self.output_choice = name;
        let r = self.open_output();
        self.error = r.clone().err();
        r
    }

    fn open_output(&mut self) -> Result<(), String> {
        let device = find_output(self.output_choice.as_deref()).ok_or("No audio output device found.")?;
        self.device_name = device.name().unwrap_or_else(|_| "Default output".into());
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let fmt = supported.sample_format();
        let cfg: cpal::StreamConfig = supported.into();
        self.out_rate = cfg.sample_rate.0;
        self.out_channels = cfg.channels as usize;
        let shared = self.shared.clone();
        let stream = match fmt {
            cpal::SampleFormat::F32 => build_output::<f32>(&device, &cfg, shared),
            cpal::SampleFormat::I16 => build_output::<i16>(&device, &cfg, shared),
            cpal::SampleFormat::U16 => build_output::<u16>(&device, &cfg, shared),
            cpal::SampleFormat::I32 => build_output::<i32>(&device, &cfg, shared),
            cpal::SampleFormat::F64 => build_output::<f64>(&device, &cfg, shared),
            other => Err(format!("Unsupported output sample format {other:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        self._stream = Some(stream);
        Ok(())
    }

    pub fn play(&self, buffer: Buffer, rate: u32, start: f64, end: f64, looping: bool, tag: u64) {
        if let Ok(mut s) = self.shared.lock() {
            let len = buffer.first().map(|c| c.len()).unwrap_or(0) as f64;
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

    /// Swap in new audio (e.g. a re-rendered preview) without restarting.
    pub fn replace_buffer(&self, buffer: Buffer, rate: u32, start: f64, end: f64) {
        if let Ok(mut s) = self.shared.lock() {
            let len = buffer.first().map(|c| c.len()).unwrap_or(0) as f64;
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
        }
    }

    pub fn set_volume(&self, v: f32) {
        if let Ok(mut s) = self.shared.lock() {
            s.volume = v;
        }
    }

    pub fn seek(&self, pos: f64) {
        if let Ok(mut s) = self.shared.lock() {
            s.pos = pos.clamp(s.start, s.end.max(s.start));
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
        match &self.rec {
            Some(r) => r.shared.lock().map(|s| s.samples / r.channels.max(1)).unwrap_or(0),
            None => 0,
        }
    }

    /// Run `f` over the live peak blocks of the current recording.
    pub fn with_rec_blocks<R>(&self, f: impl FnOnce(&[[(f32, f32); 2]]) -> R) -> Option<R> {
        let r = self.rec.as_ref()?;
        let s = r.shared.lock().ok()?;
        Some(f(&s.blocks))
    }

    /// Start recording from the chosen input; returns (rate, channels).
    pub fn start_recording(&mut self) -> Result<(u32, usize), String> {
        if let Some(r) = &self.rec {
            return Ok((r.rate, r.channels));
        }
        let device = find_input(self.input_name.as_deref()).ok_or("No audio input device found.")?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let fmt = supported.sample_format();
        let cfg: cpal::StreamConfig = supported.into();
        // One-second chunks; the first is allocated up front.
        let chunk_len = (cfg.sample_rate.0 as usize * cfg.channels.max(1) as usize).max(4096);
        let shared = Arc::new(Mutex::new(RecShared {
            chunks: vec![Vec::with_capacity(chunk_len)],
            chunk_len,
            samples: 0,
            blocks: Vec::new(),
            acc: [(f32::MAX, f32::MIN); 2],
            acc_frames: 0,
            peaks: [0.0; 2],
        }));
        let s = shared.clone();
        let stream = match fmt {
            cpal::SampleFormat::F32 => build_input::<f32>(&device, &cfg, s),
            cpal::SampleFormat::I16 => build_input::<i16>(&device, &cfg, s),
            cpal::SampleFormat::U16 => build_input::<u16>(&device, &cfg, s),
            cpal::SampleFormat::I32 => build_input::<i32>(&device, &cfg, s),
            other => Err(format!("Unsupported input sample format {other:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        let (rate, channels) = (cfg.sample_rate.0, cfg.channels as usize);
        self.rec = Some(Recording { _stream: stream, shared, channels, rate });
        Ok((rate, channels))
    }

    /// Stop recording and return de-interleaved audio (max two channels).
    pub fn stop_recording(&mut self) -> Option<(Vec<Vec<f32>>, u32)> {
        let rec = self.rec.take()?;
        let rate = rec.rate;
        let n_ch = rec.channels.max(1);
        drop(rec._stream);
        let chunks = std::mem::take(&mut rec.shared.lock().ok()?.chunks);
        let data: Vec<f32> = chunks.concat();
        let keep = n_ch.min(2);
        let mut out = vec![Vec::with_capacity(data.len() / n_ch); keep];
        for frame in data.chunks(n_ch) {
            for (c, o) in out.iter_mut().enumerate() {
                o.push(frame.get(c).copied().unwrap_or(0.0));
            }
        }
        Some((out, rate))
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
                let n_src = st.buffer.len();
                for frame in data.chunks_mut(channels) {
                    if st.playing && n_src > 0 {
                        let i0 = st.pos.floor() as usize;
                        let fr = (st.pos - i0 as f64) as f32;
                        for (c, out) in frame.iter_mut().enumerate() {
                            let ch = &st.buffer[c.min(n_src - 1)];
                            let a = ch.get(i0).copied().unwrap_or(0.0);
                            let b = ch.get(i0 + 1).copied().unwrap_or(a);
                            let v = (a + (b - a) * fr) * st.volume;
                            if c < 2 {
                                pk[c] = pk[c].max(v.abs());
                            }
                            *out = T::from_sample(v.clamp(-1.0, 1.0));
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
                        let v: f32 = cpal::Sample::to_sample::<f32>(frame[c.min(frame.len() - 1)]);
                        let a = &mut s.acc[c];
                        a.0 = a.0.min(v);
                        a.1 = a.1.max(v);
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
                    s.acc_frames += 1;
                    if s.acc_frames == REC_BLOCK {
                        let blk = s.acc;
                        s.blocks.push(blk);
                        s.acc = [(f32::MAX, f32::MIN); 2];
                        s.acc_frames = 0;
                    }
                }
            },
            |e| eprintln!("audio input error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}
