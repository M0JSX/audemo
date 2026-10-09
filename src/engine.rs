//! Real-time playback and recording through cpal.
//!
//! The audio callback only ever `try_lock`s the shared state, so a busy UI
//! thread produces a moment of silence rather than a glitchy stall.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub type Buffer = Arc<Vec<Vec<f32>>>;

/// Tag used when the engine is playing an effect preview rather than a file.
pub const PREVIEW_TAG: u64 = u64::MAX;

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

struct Recording {
    _stream: cpal::Stream,
    data: Arc<Mutex<Vec<f32>>>,
    peaks: Arc<Mutex<[f32; 2]>>,
    channels: usize,
    rate: u32,
}

pub struct Engine {
    shared: Arc<Mutex<Shared>>,
    _stream: Option<cpal::Stream>,
    pub out_rate: u32,
    pub out_channels: usize,
    pub device_name: String,
    pub error: Option<String>,
    rec: Option<Recording>,
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub playing: bool,
    pub pos: f64,
    pub tag: u64,
}

impl Engine {
    pub fn new() -> Self {
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
            error: None,
            rec: None,
        };
        if let Err(e) = engine.open_output() {
            engine.error = Some(e);
        }
        engine
    }

    fn open_output(&mut self) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("No audio output device found.")?;
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
            if let Ok(mut p) = r.peaks.lock() {
                let v = *p;
                *p = [0.0; 2];
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

    pub fn recorded_seconds(&self) -> f64 {
        match &self.rec {
            Some(r) => {
                let n = r.data.lock().map(|d| d.len()).unwrap_or(0);
                n as f64 / (r.channels.max(1) as f64 * r.rate.max(1) as f64)
            }
            None => 0.0,
        }
    }

    pub fn start_recording(&mut self) -> Result<(), String> {
        if self.rec.is_some() {
            return Ok(());
        }
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or("No audio input device found.")?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let fmt = supported.sample_format();
        let cfg: cpal::StreamConfig = supported.into();
        let data = Arc::new(Mutex::new(Vec::<f32>::with_capacity(cfg.sample_rate.0 as usize * 120)));
        let peaks = Arc::new(Mutex::new([0.0f32; 2]));
        let (d, p) = (data.clone(), peaks.clone());
        let stream = match fmt {
            cpal::SampleFormat::F32 => build_input::<f32>(&device, &cfg, d, p),
            cpal::SampleFormat::I16 => build_input::<i16>(&device, &cfg, d, p),
            cpal::SampleFormat::U16 => build_input::<u16>(&device, &cfg, d, p),
            cpal::SampleFormat::I32 => build_input::<i32>(&device, &cfg, d, p),
            other => Err(format!("Unsupported input sample format {other:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        self.rec = Some(Recording {
            _stream: stream,
            data,
            peaks,
            channels: cfg.channels as usize,
            rate: cfg.sample_rate.0,
        });
        Ok(())
    }

    /// Stop recording and return de-interleaved audio (max two channels).
    pub fn stop_recording(&mut self) -> Option<(Vec<Vec<f32>>, u32)> {
        let rec = self.rec.take()?;
        let rate = rec.rate;
        let n_ch = rec.channels.max(1);
        drop(rec._stream);
        let data = rec.data.lock().ok()?.clone();
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
    data: Arc<Mutex<Vec<f32>>>,
    peaks: Arc<Mutex<[f32; 2]>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = cfg.channels as usize;
    device
        .build_input_stream(
            cfg,
            move |input: &[T], _: &cpal::InputCallbackInfo| {
                let mut pk = [0.0f32; 2];
                if let Ok(mut d) = data.lock() {
                    for (i, s) in input.iter().enumerate() {
                        let v: f32 = cpal::Sample::to_sample::<f32>(*s);
                        let c = (i % channels.max(1)).min(1);
                        pk[c] = pk[c].max(v.abs());
                        d.push(v);
                    }
                }
                if channels == 1 {
                    pk[1] = pk[0];
                }
                if let Ok(mut p) = peaks.try_lock() {
                    p[0] = p[0].max(pk[0]);
                    p[1] = p[1].max(pk[1]);
                }
            },
            |e| eprintln!("audio input error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}
