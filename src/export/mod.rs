//! Writing audio files: WAV, FLAC, MP3 and AAC (M4A), with metadata.

pub mod flac;
#[cfg(not(audemo_std_test))]
mod lossy;
pub mod md5;
pub mod meta;
pub mod mp4;
pub mod wav;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub use meta::{Metadata, Tag, TAGS};
pub use wav::WavFormat;

static WRITE_ENCODER: AtomicBool = AtomicBool::new(true);

/// Preferences > Markers & Metadata: name Audemo as the encoder in tags.
pub fn set_write_encoder(on: bool) {
    WRITE_ENCODER.store(on, Ordering::Relaxed);
}

pub fn encoder_name() -> String {
    if !WRITE_ENCODER.load(Ordering::Relaxed) {
        return String::new();
    }
    format!("Audemo {}", option_env!("CARGO_PKG_VERSION").unwrap_or(""))
}

/// Shared progress (0–1) and a cancel flag for a background save.
#[derive(Clone, Default)]
pub struct Progress(Arc<(AtomicU32, AtomicBool)>);

impl Progress {
    pub fn set(&self, f: f32) {
        self.0 .0.store(f.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0 .0.load(Ordering::Relaxed))
    }
    pub fn cancel(&self) {
        self.0 .1.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.0 .1.load(Ordering::Relaxed)
    }
}

/// Float → integer conversion with optional triangular (TPDF) dither.
pub struct Quantizer {
    rng: u64,
    dither: bool,
}

impl Quantizer {
    pub fn new(dither: bool) -> Self {
        Quantizer { rng: 0x9E37_79B9_7F4A_7C15, dither }
    }
    #[inline]
    fn uniform(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
    /// Quantise to a signed `bits`-bit integer (8, 16, 24 or 32).
    #[inline]
    pub fn quantize(&mut self, s: f32, bits: u32) -> i32 {
        let full = ((1u64 << (bits - 1)) - 1) as f64;
        let tpdf = if self.dither && bits < 32 { (self.uniform() + self.uniform()) * 0.5 } else { 0.0 };
        let s = if s.is_finite() { s as f64 } else { 0.0 };
        (s * full + tpdf).round().clamp(-full - 1.0, full) as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Wav,
    Flac,
    Mp3,
    M4a,
}

impl Container {
    pub const ALL: [Container; 4] = [Container::Wav, Container::Flac, Container::Mp3, Container::M4a];
    pub fn label(self) -> &'static str {
        match self {
            Container::Wav => "Wave PCM (*.wav)",
            Container::Flac => "FLAC (*.flac)",
            Container::Mp3 => "MP3 Audio (*.mp3)",
            Container::M4a => "AAC Audio (*.m4a)",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Container::Wav => "wav",
            Container::Flac => "flac",
            Container::Mp3 => "mp3",
            Container::M4a => "m4a",
        }
    }
    pub fn from_path(p: &Path) -> Option<Container> {
        let e = p.extension()?.to_str()?.to_ascii_lowercase();
        Some(match e.as_str() {
            "wav" | "wave" => Container::Wav,
            "flac" => Container::Flac,
            "mp3" => Container::Mp3,
            "m4a" | "aac" | "mp4" => Container::M4a,
            _ => return None,
        })
    }
    pub fn lossy(self) -> bool {
        matches!(self, Container::Mp3 | Container::M4a)
    }
}

pub const MP3_BITRATES: [u32; 10] = [32, 48, 64, 96, 128, 160, 192, 224, 256, 320];
pub const AAC_BITRATES: [u32; 8] = [64, 96, 128, 160, 192, 224, 256, 320];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportSettings {
    pub container: Container,
    pub wav: WavFormat,
    /// 16 or 24.
    pub flac_bits: u32,
    pub dither: bool,
    pub mp3_vbr: bool,
    pub mp3_kbps: u32,
    /// 0 (best) … 9.
    pub mp3_vbr_quality: u8,
    pub aac_kbps: u32,
    pub include_meta: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            container: Container::Wav,
            wav: WavFormat::Pcm24,
            flac_bits: 24,
            dither: true,
            mp3_vbr: false,
            mp3_kbps: 320,
            mp3_vbr_quality: 2,
            aac_kbps: 256,
            include_meta: true,
        }
    }
}

impl ExportSettings {
    /// Settings that keep a file's original format and bit depth.
    pub fn for_source(container: Container, bits: Option<u32>) -> Self {
        let wav = WavFormat::from_bits(bits);
        ExportSettings {
            container,
            wav,
            flac_bits: if bits == Some(16) || bits == Some(8) { 16 } else { 24 },
            dither: true,
            ..Default::default()
        }
    }
    /// Whether the chosen format quantises to integers (where dither applies).
    pub fn integer(&self) -> bool {
        match self.container {
            Container::Wav => self.wav != WavFormat::Float32,
            Container::Flac | Container::M4a => true,
            Container::Mp3 => false,
        }
    }
    /// Bit depth to remember for the saved file.
    pub fn bits(&self) -> Option<u32> {
        match self.container {
            Container::Wav => self.wav.bits(),
            Container::Flac => Some(self.flac_bits),
            _ => None,
        }
    }
    /// One-line form for the preferences file.
    pub fn to_text(&self) -> String {
        let wav = match self.wav {
            WavFormat::Pcm16 => 16,
            WavFormat::Pcm24 => 24,
            WavFormat::Pcm32 => 32,
            WavFormat::Float32 => 0,
        };
        format!(
            "{} wav={} flac={} dither={} vbr={} kbps={} q={} aac={} meta={}",
            self.container.ext(),
            wav,
            self.flac_bits,
            self.dither as u8,
            self.mp3_vbr as u8,
            self.mp3_kbps,
            self.mp3_vbr_quality,
            self.aac_kbps,
            self.include_meta as u8
        )
    }

    pub fn from_text(t: &str) -> Option<Self> {
        let mut it = t.split_whitespace();
        let container = Container::from_path(Path::new(&format!("x.{}", it.next()?)))?;
        let mut s = ExportSettings { container, ..Default::default() };
        for kv in it {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let Ok(n) = v.parse::<u32>() else { continue };
            match k {
                "wav" => {
                    s.wav = match n {
                        16 => WavFormat::Pcm16,
                        24 => WavFormat::Pcm24,
                        32 => WavFormat::Pcm32,
                        _ => WavFormat::Float32,
                    }
                }
                "flac" => s.flac_bits = if n == 16 { 16 } else { 24 },
                "dither" => s.dither = n != 0,
                "vbr" => s.mp3_vbr = n != 0,
                "kbps" => s.mp3_kbps = *MP3_BITRATES.iter().find(|b| **b >= n).unwrap_or(&320),
                "q" => s.mp3_vbr_quality = n.min(9) as u8,
                "aac" => s.aac_kbps = *AAC_BITRATES.iter().find(|b| **b >= n).unwrap_or(&320),
                "meta" => s.include_meta = n != 0,
                _ => {}
            }
        }
        Some(s)
    }

    pub fn summary(&self) -> String {
        match self.container {
            Container::Wav => format!("WAV, {}", self.wav.label()),
            Container::Flac => format!("FLAC, {}-bit", self.flac_bits),
            Container::Mp3 if self.mp3_vbr => format!("MP3, VBR quality V{}", self.mp3_vbr_quality),
            Container::Mp3 => format!("MP3, {} kbps CBR", self.mp3_kbps),
            Container::M4a => format!("AAC, {} kbps", self.aac_kbps),
        }
    }
}

/// A sibling temporary path so a failed save never damages the target.
fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "audio".into());
    path.with_file_name(format!(".{name}.audemo-part"))
}

/// Write `chs` to `path` in the chosen format.
pub fn save(
    path: &Path,
    chs: &[Vec<f32>],
    sample_rate: u32,
    s: &ExportSettings,
    meta: &Metadata,
    markers: &[(usize, String)],
    progress: &Progress,
) -> Result<(), String> {
    if chs.is_empty() || chs[0].is_empty() {
        return Err("There is no audio to save.".into());
    }
    let tmp = temp_path(path);
    let res = (|| {
        let file = std::fs::File::create(&tmp).map_err(|e| format!("Couldn't create file: {e}"))?;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, file);
        let empty = Metadata::default();
        let meta = if s.include_meta { meta } else { &empty };
        let enc = encoder_name();
        match s.container {
            Container::Wav => {
                let info = meta::riff_info(meta, &enc);
                let cues = if s.include_meta { meta::wav_markers(markers) } else { Vec::new() };
                wav::write(&mut w, chs, sample_rate, s.wav, s.dither && s.wav != WavFormat::Float32, &info, &cues, progress)?;
            }
            Container::Flac => {
                let vc = meta::vorbis_comment(meta, &enc);
                flac::write(&mut w, chs, sample_rate, s.flac_bits, s.dither, &vc, progress)?;
            }
            #[cfg(not(audemo_std_test))]
            Container::Mp3 => lossy::write_mp3(&mut w, chs, sample_rate, s, meta, progress)?,
            #[cfg(not(audemo_std_test))]
            Container::M4a => lossy::write_m4a(&mut w, chs, sample_rate, s, meta, progress)?,
            #[cfg(audemo_std_test)]
            _ => return Err("not built".into()),
        }
        let file = w.into_inner().map_err(|e| format!("Write failed: {}", e.error()))?;
        file.sync_all().map_err(|e| format!("Write failed: {e}"))?;
        Ok(())
    })();
    match res {
        Ok(()) => std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Couldn't replace {}: {e}", path.display())
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(all(test, not(audemo_std_test)))]
mod tests;
