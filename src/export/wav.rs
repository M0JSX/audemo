//! WAV writer with RIFF INFO metadata and cue-point markers.

use std::io::{Seek, SeekFrom, Write};

use super::{Progress, Quantizer};

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
            Some(32) => WavFormat::Pcm32,
            _ => WavFormat::Float32,
        }
    }
    pub fn bits(self) -> Option<u32> {
        match self {
            WavFormat::Pcm16 => Some(16),
            WavFormat::Pcm24 => Some(24),
            WavFormat::Pcm32 => Some(32),
            WavFormat::Float32 => None,
        }
    }
    fn bytes(self) -> usize {
        match self {
            WavFormat::Pcm16 => 2,
            WavFormat::Pcm24 => 3,
            _ => 4,
        }
    }
}

pub fn write<W: Write + Seek>(
    out: &mut W,
    chs: &[Vec<f32>],
    sample_rate: u32,
    fmt: WavFormat,
    dither: bool,
    info: &[u8],
    markers: &[u8],
    progress: &Progress,
) -> Result<(), String> {
    let err = |e: std::io::Error| format!("Write failed: {e}");
    let n_ch = chs.len().max(1);
    let len = chs.first().map(|c| c.len()).unwrap_or(0);
    let bps = fmt.bytes();
    let data_len = len as u64 * n_ch as u64 * bps as u64;
    let float = fmt == WavFormat::Float32;
    let fmt_len: u32 = if float { 18 } else { 16 };
    let fact_len: u64 = if float { 12 } else { 0 };
    let total = 4 + (8 + fmt_len as u64) + fact_len + info.len() as u64 + 8 + data_len + (data_len & 1) + markers.len() as u64;
    if total > u32::MAX as u64 {
        return Err("The file is too large for WAV (over 4 GB). Save it as FLAC or shorten it.".into());
    }
    let mut h = Vec::with_capacity(64);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(total as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&fmt_len.to_le_bytes());
    h.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    h.extend_from_slice(&(n_ch as u16).to_le_bytes());
    h.extend_from_slice(&sample_rate.to_le_bytes());
    h.extend_from_slice(&(sample_rate * (n_ch * bps) as u32).to_le_bytes());
    h.extend_from_slice(&((n_ch * bps) as u16).to_le_bytes());
    h.extend_from_slice(&((bps * 8) as u16).to_le_bytes());
    if float {
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(b"fact");
        h.extend_from_slice(&4u32.to_le_bytes());
        h.extend_from_slice(&(len.min(u32::MAX as usize) as u32).to_le_bytes());
    }
    h.extend_from_slice(info);
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.write_all(&h).map_err(err)?;

    let mut q = Quantizer::new(dither);
    let block = 16384;
    let mut buf: Vec<u8> = Vec::with_capacity(block * n_ch * bps);
    let mut i = 0;
    while i < len {
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
        let end = (i + block).min(len);
        buf.clear();
        for f in i..end {
            for c in chs {
                let s = c[f];
                match fmt {
                    WavFormat::Float32 => buf.extend_from_slice(&s.to_le_bytes()),
                    WavFormat::Pcm16 => buf.extend_from_slice(&(q.quantize(s, 16) as i16).to_le_bytes()),
                    WavFormat::Pcm24 => buf.extend_from_slice(&q.quantize(s, 24).to_le_bytes()[..3]),
                    WavFormat::Pcm32 => buf.extend_from_slice(&q.quantize(s, 32).to_le_bytes()),
                }
            }
        }
        out.write_all(&buf).map_err(err)?;
        progress.set(end as f32 / len as f32);
        i = end;
    }
    if data_len & 1 == 1 {
        out.write_all(&[0]).map_err(err)?;
    }
    out.write_all(markers).map_err(err)?;
    out.flush().map_err(err)?;
    let _ = out.seek(SeekFrom::End(0));
    Ok(())
}
