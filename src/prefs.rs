//! Persistent preferences in a plain `key=value` file:
//! - macOS:   ~/Library/Application Support/Audemo/preferences.txt
//! - Windows: %APPDATA%\Audemo\preferences.txt
//! - Linux:   $XDG_CONFIG_HOME/audemo/preferences.txt (or ~/.config/audemo)

use std::path::PathBuf;

pub const MAX_RECENT: usize = 10;

/// A value stored in the preferences file.
pub trait PrefValue: Sized {
    fn text(&self) -> String;
    fn parse(s: &str) -> Option<Self>;
}

impl PrefValue for bool {
    fn text(&self) -> String {
        (*self as u8).to_string()
    }
    fn parse(s: &str) -> Option<Self> {
        Some(s == "1" || s.eq_ignore_ascii_case("true"))
    }
}
impl PrefValue for f32 {
    fn text(&self) -> String {
        self.to_string()
    }
    fn parse(s: &str) -> Option<Self> {
        s.parse().ok().filter(|v: &f32| v.is_finite())
    }
}
impl PrefValue for u32 {
    fn text(&self) -> String {
        self.to_string()
    }
    fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}
impl PrefValue for String {
    fn text(&self) -> String {
        self.clone()
    }
    fn parse(s: &str) -> Option<Self> {
        Some(s.to_string())
    }
}
impl PrefValue for Option<String> {
    fn text(&self) -> String {
        self.clone().unwrap_or_default()
    }
    fn parse(s: &str) -> Option<Self> {
        Some((!s.is_empty()).then(|| s.to_string()))
    }
}
impl PrefValue for Option<PathBuf> {
    fn text(&self) -> String {
        self.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
    }
    fn parse(s: &str) -> Option<Self> {
        Some((!s.is_empty()).then(|| PathBuf::from(s)))
    }
}
impl<const N: usize> PrefValue for [u32; N] {
    fn text(&self) -> String {
        self.iter().map(|v| format!("{v:x}")).collect::<Vec<_>>().join(",")
    }
    fn parse(s: &str) -> Option<Self> {
        let v: Vec<u32> = s.split(',').filter_map(|x| u32::from_str_radix(x.trim(), 16).ok()).collect();
        v.try_into().ok()
    }
}

/// An enum stored by name, with the labels shown in Preferences.
macro_rules! pref_enum {
    ($name:ident { $($v:ident = $key:literal : $label:literal),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name { $($v),* }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),*];
            pub fn label(self) -> &'static str {
                match self { $($name::$v => $label),* }
            }
        }
        impl PrefValue for $name {
            fn text(&self) -> String {
                match self { $($name::$v => $key.to_string()),* }
            }
            fn parse(s: &str) -> Option<Self> {
                match s { $($key => Some($name::$v),)* _ => None }
            }
        }
    };
}

pref_enum!(Startup { MostRecent = "recent": "Open Most Recent File", Nothing = "nothing": "Open Nothing" });
pref_enum!(AutoScroll { Paged = "paged": "Paged", Centered = "centered": "Centered" });
pref_enum!(TimeFormat {
    Decimal = "decimal": "Decimal (mm:ss.ddd)",
    Cd75 = "cd": "Compact Disc (75 fps)",
    Smpte30 = "smpte30": "SMPTE (30 fps)",
    Smpte2997Drop = "smpte2997df": "SMPTE Drop (29.97 fps)",
    Smpte2997 = "smpte2997": "SMPTE 30 fps (29.97 fps)",
    Smpte25 = "smpte25": "SMPTE EBU (25 fps)",
    Smpte24 = "smpte24": "SMPTE Film Speed (24 fps)",
    Samples = "samples": "Samples",
    BarsBeats = "bars": "Bars and Beats",
    Custom = "custom": "Custom",
});
pref_enum!(WindowFn {
    BlackmanHarris = "blackman_harris": "Blackman-Harris",
    Blackman = "blackman": "Blackman",
    Hann = "hann": "Hann",
    Hamming = "hamming": "Hamming",
    Welch = "welch": "Welch",
    Rectangular = "rectangular": "Rectangular",
});
pref_enum!(SrcQuality { Low = "low": "Low (faster)", Medium = "medium": "Medium", High = "high": "High (slower)" });
pref_enum!(PanLaw { LrCut = "lr_cut": "L/R Cut (Logarithmic)", EqualPower = "equal_power": "Equal Power (Sinusoidal)" });
pref_enum!(FadeCurve { Linear = "linear": "Linear (clip fade shapes)", EqualPower = "equal_power": "Equal Power" });
pref_enum!(BackupLocation { WithSession = "session": "Same folder as the session (Backup)", Folder = "folder": "Centralized folder" });
pref_enum!(AppearancePreset { Default = "default": "Default", Darkest = "darkest": "Darkest", Light = "light": "Light", Custom = "custom": "Custom" });

/// Audemo's default element colours: waveform, selection, playhead,
/// markers, time display, accent.
pub const DEFAULT_COLORS: [u32; 6] = [0x3fdc9b, 0x47d4a0, 0xf2c230, 0xe87a30, 0xe8a13a, 0x2d7fd9];
pub const COLOR_NAMES: [&str; 6] = ["Waveform", "Selection", "Playhead and cursor", "Markers", "Time display", "Highlights"];

macro_rules! prefs {
    ($( $(#[$m:meta])* $field:ident : $ty:ty = $default:expr ),* $(,)?) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct Prefs {
            pub recent: Vec<PathBuf>,
            /// Format last chosen in Save As.
            pub export: Option<crate::export::ExportSettings>,
            $( $(#[$m])* pub $field: $ty, )*
        }
        impl Default for Prefs {
            fn default() -> Self {
                Prefs { recent: Vec::new(), export: None, $( $field: $default, )* }
            }
        }
        impl Prefs {
            fn set_key(&mut self, k: &str, v: &str) {
                match k {
                    $( stringify!($field) => {
                        if let Some(x) = <$ty as PrefValue>::parse(v) {
                            self.$field = x;
                        }
                    } )*
                    _ => {}
                }
            }
            fn write_keys(&self, s: &mut String) {
                $( s.push_str(&format!("{}={}\n", stringify!($field), PrefValue::text(&self.$field))); )*
            }
        }
    };
}

prefs! {
    // Audio hardware
    /// Audio API ("CoreAudio", "WASAPI", "ALSA" …); None = the system's default.
    host: Option<String> = None,
    input_device: Option<String> = None,
    output_device: Option<String> = None,
    /// I/O buffer size in frames (0 = the device's default).
    buffer_size: u32 = 0,
    /// Output sample rate (0 = the device's default).
    sample_rate: u32 = 0,
    /// Reopen the output at each file's own sample rate when playing it.
    force_doc_rate: bool = false,
    // Audio channel mapping: device channels (0-based) for Audemo's L and R.
    out_map_l: u32 = 0,
    out_map_r: u32 = 1,
    in_map_l: u32 = 0,
    in_map_r: u32 = 1,
    // General
    startup: Startup = Startup::Nothing,
    show_tooltips: bool = true,
    /// Zoom in/out step, percent.
    zoom_factor: f32 = 50.0,
    /// Mouse-wheel zoom strength, percent.
    wheel_zoom: f32 = 50.0,
    autoscroll: AutoScroll = AutoScroll::Paged,
    browser_dir: Option<PathBuf> = None,
    browser_autoplay: bool = true,
    // Appearance
    appearance: AppearancePreset = AppearancePreset::Default,
    /// Interface brightness, 0 (darkest) … 100 (light).
    brightness: f32 = 30.0,
    gradients: bool = false,
    ui_scale: f32 = 1.0,
    colors: [u32; 6] = DEFAULT_COLORS,
    // Auto save
    autosave: bool = true,
    autosave_minutes: u32 = 5,
    autosave_max: u32 = 10,
    backup_location: BackupLocation = BackupLocation::WithSession,
    backup_dir: Option<PathBuf> = None,
    /// Also back up open audio files with unsaved changes.
    backup_files: bool = true,
    // Data
    dither: bool = true,
    smooth_delete: bool = false,
    smooth_delete_ms: f32 = 2.0,
    smooth_edits: bool = false,
    smooth_edits_ms: f32 = 2.0,
    src_quality: SrcQuality = SrcQuality::Medium,
    // Effects
    show_plugin_window: bool = false,
    scan_at_startup: bool = false,
    rack_entire: bool = false,
    // Markers & metadata
    copy_markers: bool = true,
    marker_name: String = "Marker".to_string(),
    save_meta: bool = true,
    write_encoder: bool = true,
    // Memory
    undo_levels: u32 = 60,
    // Multitrack
    pan_law: PanLaw = PanLaw::LrCut,
    // Multitrack clips
    auto_crossfade: bool = true,
    crossfade_curve: FadeCurve = FadeCurve::Linear,
    clip_fade_ms: f32 = 0.0,
    show_clip_names: bool = true,
    // Playback and recording
    return_to_start: bool = false,
    preroll_s: f32 = 2.0,
    postroll_s: f32 = 2.0,
    follow: bool = true,
    /// Recording latency compensation for multitrack overdubs, in ms.
    rec_offset_ms: f32 = 0.0,
    // Spectral displays
    spec_window: WindowFn = WindowFn::Hann,
    spec_size: u32 = 1024,
    spec_range_db: f32 = 115.0,
    spec_log: bool = false,
    // Time display
    time_format: TimeFormat = TimeFormat::Decimal,
    custom_fps: f32 = 30.0,
    tempo: f32 = 120.0,
    beats_per_bar: u32 = 4,
    beat_unit: u32 = 4,
}

pub fn config_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Audemo"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Audemo"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("audemo"))
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// Bytes free for the current user on the volume containing `path`.
pub fn disk_free(path: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free: u64 = 0;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        if ok != 0 {
            Some(free)
        } else {
            None
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
}

fn file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("preferences.txt"))
}

impl Prefs {
    pub fn parse(text: &str) -> Self {
        let mut p = Prefs::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "recent" if !v.is_empty() => p.recent.push(PathBuf::from(v)),
                "export" => p.export = crate::export::ExportSettings::from_text(v),
                _ => p.set_key(k, v),
            }
        }
        p.recent.truncate(MAX_RECENT);
        p.sanitize();
        p
    }

    /// Keep values in the ranges the Preferences window allows.
    pub fn sanitize(&mut self) {
        self.rec_offset_ms = self.rec_offset_ms.clamp(0.0, 1000.0);
        self.zoom_factor = self.zoom_factor.clamp(10.0, 90.0);
        self.wheel_zoom = self.wheel_zoom.clamp(5.0, 100.0);
        self.brightness = self.brightness.clamp(0.0, 100.0);
        self.ui_scale = self.ui_scale.clamp(0.75, 2.0);
        self.autosave_minutes = self.autosave_minutes.clamp(1, 120);
        self.autosave_max = self.autosave_max.clamp(1, 100);
        self.smooth_delete_ms = self.smooth_delete_ms.clamp(0.1, 50.0);
        self.smooth_edits_ms = self.smooth_edits_ms.clamp(0.1, 50.0);
        self.undo_levels = self.undo_levels.clamp(5, 500);
        self.clip_fade_ms = self.clip_fade_ms.clamp(0.0, 10_000.0);
        self.preroll_s = self.preroll_s.clamp(0.0, 30.0);
        self.postroll_s = self.postroll_s.clamp(0.0, 30.0);
        if ![256, 512, 1024, 2048, 4096, 8192].contains(&self.spec_size) {
            self.spec_size = 1024;
        }
        self.spec_range_db = self.spec_range_db.clamp(48.0, 180.0);
        self.custom_fps = self.custom_fps.clamp(1.0, 1000.0);
        self.tempo = self.tempo.clamp(20.0, 400.0);
        self.beats_per_bar = self.beats_per_bar.clamp(1, 32);
        if ![1, 2, 4, 8, 16].contains(&self.beat_unit) {
            self.beat_unit = 4;
        }
        for v in [&mut self.out_map_l, &mut self.out_map_r, &mut self.in_map_l, &mut self.in_map_r] {
            *v = (*v).min(63);
        }
        if self.marker_name.trim().is_empty() {
            self.marker_name = "Marker".into();
        }
    }

    pub fn serialize(&self) -> String {
        let mut s = String::from("# Audemo preferences\n");
        self.write_keys(&mut s);
        if let Some(e) = &self.export {
            s += &format!("export={}\n", e.to_text());
        }
        for r in &self.recent {
            s += &format!("recent={}\n", r.display());
        }
        s
    }

    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| Prefs::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(f) = file() {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(f, self.serialize());
        }
    }

    /// The hardware setup for the audio engine.
    pub fn audio_config(&self) -> crate::engine::AudioConfig {
        crate::engine::AudioConfig {
            host: self.host.clone(),
            output: self.output_device.clone(),
            input: self.input_device.clone(),
            buffer: self.buffer_size,
            rate: self.sample_rate,
            out_map: [self.out_map_l as usize, self.out_map_r as usize],
            in_map: [self.in_map_l as usize, self.in_map_r as usize],
        }
    }

    pub fn add_recent(&mut self, p: PathBuf) {
        self.recent.retain(|r| r != &p);
        self.recent.insert(0, p);
        self.recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_round_trips() {
        let mut p = Prefs {
            host: Some("CoreAudio".into()),
            buffer_size: 256,
            sample_rate: 96000,
            startup: Startup::MostRecent,
            time_format: TimeFormat::Smpte2997Drop,
            colors: [1, 2, 3, 4, 5, 0xffffff],
            spec_window: WindowFn::Hamming,
            backup_dir: Some(PathBuf::from("/Backups here")),
            marker_name: "Cue".into(),
            pan_law: PanLaw::EqualPower,
            ..Default::default()
        };
        p.sanitize();
        assert_eq!(Prefs::parse(&p.serialize()), p);
        // Unknown keys and bad values are ignored; ranges are enforced.
        let q = Prefs::parse("bogus=1\nzoom_factor=500\nstartup=sideways\nspec_size=3\n");
        assert_eq!(q.zoom_factor, 90.0);
        assert_eq!(q.startup, Startup::Nothing);
        assert_eq!(q.spec_size, 1024);
    }

    #[test]
    fn round_trip() {
        let mut p = Prefs { input_device: Some("MacBook Air Microphone".into()), browser_autoplay: false, rec_offset_ms: 12.5, ..Default::default() };
        p.add_recent(PathBuf::from("/a/b.wav"));
        p.add_recent(PathBuf::from("/c d/e=f.wav"));
        p.add_recent(PathBuf::from("/a/b.wav"));
        let q = Prefs::parse(&p.serialize());
        assert_eq!(p, q);
        assert_eq!(q.recent[0], PathBuf::from("/a/b.wav"));
        assert_eq!(q.recent.len(), 2);
    }
}
