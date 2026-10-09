//! Persistent preferences in a plain `key=value` file:
//! - macOS:   ~/Library/Application Support/Audemo/preferences.txt
//! - Windows: %APPDATA%\Audemo\preferences.txt
//! - Linux:   $XDG_CONFIG_HOME/audemo/preferences.txt (or ~/.config/audemo)

use std::path::PathBuf;

pub const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub recent: Vec<PathBuf>,
    pub browser_dir: Option<PathBuf>,
    pub browser_autoplay: bool,
    /// Recording latency compensation for multitrack overdubs, in ms.
    pub rec_offset_ms: f32,
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
        let mut p = Prefs { browser_autoplay: true, ..Default::default() };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "input_device" if !v.is_empty() => p.input_device = Some(v.to_string()),
                "output_device" if !v.is_empty() => p.output_device = Some(v.to_string()),
                "recent" if !v.is_empty() => p.recent.push(PathBuf::from(v)),
                "browser_dir" if !v.is_empty() => p.browser_dir = Some(PathBuf::from(v)),
                "browser_autoplay" => p.browser_autoplay = v == "1" || v == "true",
                "rec_offset_ms" => p.rec_offset_ms = v.parse::<f32>().unwrap_or(0.0).clamp(0.0, 1000.0),
                _ => {}
            }
        }
        p.recent.truncate(MAX_RECENT);
        p
    }

    pub fn serialize(&self) -> String {
        let mut s = String::from("# Audemo preferences\n");
        if let Some(d) = &self.input_device {
            s += &format!("input_device={d}\n");
        }
        if let Some(d) = &self.output_device {
            s += &format!("output_device={d}\n");
        }
        if let Some(d) = &self.browser_dir {
            s += &format!("browser_dir={}\n", d.display());
        }
        s += &format!("browser_autoplay={}\n", if self.browser_autoplay { 1 } else { 0 });
        s += &format!("rec_offset_ms={}\n", self.rec_offset_ms);
        for r in &self.recent {
            s += &format!("recent={}\n", r.display());
        }
        s
    }

    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|t| Prefs::parse(&t))
            .unwrap_or(Prefs { browser_autoplay: true, ..Default::default() })
    }

    pub fn save(&self) {
        if let Some(f) = file() {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(f, self.serialize());
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
