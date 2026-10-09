//! File decoding (symphonia) and WAV export (hound).

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const OPEN_EXTENSIONS: &[&str] = &[
    "wav", "wave", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "mp4", "aac", "alac",
    "caf", "mkv", "webm",
];

pub struct Decoded {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
    pub bits: Option<u32>,
}

pub fn load(path: &Path) -> Result<Decoded, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Couldn't open file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Unrecognised audio format: {e}"))?;
    let mut format = probed.format;
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
    Ok(Decoded { sample_rate, channels, bits })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavFormat {
    Pcm16,
    Pcm24,
    Pcm32,
    Float32,
}

impl WavFormat {
    pub const ALL: [WavFormat; 4] = [WavFormat::Pcm16, WavFormat::Pcm24, WavFormat::Pcm32, WavFormat::Float32];
    pub fn label(self) -> &'static str {
        match self {
            WavFormat::Pcm16 => "16-bit integer",
            WavFormat::Pcm24 => "24-bit integer",
            WavFormat::Pcm32 => "32-bit integer",
            WavFormat::Float32 => "32-bit floating point",
        }
    }
    pub fn from_bits(bits: Option<u32>) -> Self {
        match bits {
            Some(16) | Some(8) => WavFormat::Pcm16,
            Some(24) => WavFormat::Pcm24,
            _ => WavFormat::Float32,
        }
    }
}

pub fn save_wav(
    path: &Path,
    chs: &[Vec<f32>],
    sample_rate: u32,
    fmt: WavFormat,
    dither: bool,
) -> Result<(), String> {
    let n_ch = chs.len().max(1);
    let (bits, sample_format) = match fmt {
        WavFormat::Pcm16 => (16, hound::SampleFormat::Int),
        WavFormat::Pcm24 => (24, hound::SampleFormat::Int),
        WavFormat::Pcm32 => (32, hound::SampleFormat::Int),
        WavFormat::Float32 => (32, hound::SampleFormat::Float),
    };
    let spec = hound::WavSpec { channels: n_ch as u16, sample_rate, bits_per_sample: bits, sample_format };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("Couldn't create file: {e}"))?;
    let len = chs.first().map(|c| c.len()).unwrap_or(0);
    let mut rng = crate::dsp::util::Rng::new(0xF3_77_17E);
    let err = |e: hound::Error| format!("Write failed: {e}");
    for i in 0..len {
        for c in chs {
            let s = c[i];
            match fmt {
                WavFormat::Float32 => w.write_sample(s).map_err(err)?,
                WavFormat::Pcm16 => {
                    let tpdf = if dither { (rng.bipolar() + rng.bipolar()) * 0.5 } else { 0.0 };
                    let v = (s * 32767.0 + tpdf).round().clamp(-32768.0, 32767.0) as i16;
                    w.write_sample(v).map_err(err)?
                }
                WavFormat::Pcm24 => {
                    let tpdf = if dither { (rng.bipolar() + rng.bipolar()) * 0.5 } else { 0.0 };
                    let v = (s as f64 * 8_388_607.0 + tpdf as f64).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                    w.write_sample(v).map_err(err)?
                }
                WavFormat::Pcm32 => {
                    let v = (s as f64 * 2_147_483_647.0).round().clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                    w.write_sample(v).map_err(err)?
                }
            }
        }
    }
    w.finalize().map_err(|e| format!("Couldn't finish file: {e}"))
}
