//! Audio plug-in hosting: finding VST3 plug-ins, running them as effects
//! (menus, Effects Rack, track racks, batch) and keeping their settings.
//!
//! Each plug-in becomes an ordinary effect whose id is `vst3:<class id>`
//! (or `au:<type/subtype/manufacturer>` for Audio Units on macOS). Its
//! settings are two text parameters: `plugin` (the class id) and `state`
//! (the plug-in's own saved state), so presets, sessions and racks need no
//! special handling.

#[cfg(target_os = "macos")]
pub mod au;
pub mod editor;
pub mod view;
pub mod vst3;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime};

use crate::dsp::effects::{rt::RtEffect, Category, Ctx, EffectDef};
use crate::dsp::params::{text, Params};

pub const ID_PREFIX: &str = "vst3:";
pub const AU_PREFIX: &str = "au:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Vst3,
    Au,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Vst3 => "VST 3",
            Format::Au => "Audio Unit",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Format::Vst3 => "vst3",
            Format::Au => "au",
        }
    }
}

/// The format and class id of a plug-in effect id.
pub fn split_id(id: &str) -> Option<(Format, &str)> {
    id.strip_prefix(ID_PREFIX).map(|c| (Format::Vst3, c)).or_else(|| id.strip_prefix(AU_PREFIX).map(|c| (Format::Au, c)))
}

// ------------------------------------------------------------------ main thread

static MAIN: OnceLock<std::thread::ThreadId> = OnceLock::new();
type Task = Box<dyn FnOnce() + Send>;
static QUEUE: Mutex<Vec<Task>> = Mutex::new(Vec::new());
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Call once from the UI thread at start-up. `wake` asks the UI to run a frame.
pub fn init_main_thread(wake: impl Fn() + Send + Sync + 'static) {
    let _ = MAIN.set(std::thread::current().id());
    let _ = WAKE.set(Box::new(wake));
}

pub fn on_main_thread() -> bool {
    MAIN.get().map(|m| *m == std::thread::current().id()).unwrap_or(true)
}

/// Plug-ins expect to be created and destroyed on the UI thread. Run `f`
/// there and wait for its result (directly, if this is the UI thread).
pub fn run_on_main<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    if on_main_thread() {
        return Ok(f());
    }
    let (tx, rx) = channel();
    QUEUE.lock().map_err(|_| "plug-in queue poisoned")?.push(Box::new(move || {
        let _ = tx.send(f());
    }));
    if let Some(w) = WAKE.get() {
        w();
    }
    rx.recv_timeout(Duration::from_secs(60)).map_err(|_| "The plug-in didn't respond.".to_string())
}

/// Run queued plug-in work. Call every UI frame.
pub fn pump() {
    let tasks: Vec<Task> = match QUEUE.try_lock() {
        Ok(mut q) => std::mem::take(&mut *q),
        Err(_) => return,
    };
    for t in tasks {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(t));
    }
}

/// Drop `v` on the UI thread (never blocks).
fn drop_on_main<T: Send + 'static>(v: T) {
    if on_main_thread() {
        drop(v);
    } else if let Ok(mut q) = QUEUE.lock() {
        q.push(Box::new(move || drop(v)));
        if let Some(w) = WAKE.get() {
            w();
        }
    }
}

// ------------------------------------------------------------------ registry

#[derive(Clone, Debug, PartialEq)]
pub struct PluginInfo {
    pub format: Format,
    pub cid: String,
    pub name: String,
    pub vendor: String,
    pub path: PathBuf,
    pub sub_categories: String,
    pub version: String,
    pub enabled: bool,
}

impl PluginInfo {
    pub fn effect_id(&self) -> String {
        match self.format {
            Format::Vst3 => format!("{ID_PREFIX}{}", self.cid),
            Format::Au => format!("{AU_PREFIX}{}", self.cid),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub plugins: Vec<PluginInfo>,
    /// Bundles that couldn't be loaded: (path, reason).
    pub failed: Vec<(PathBuf, String)>,
    /// Extra folders to search.
    pub folders: Vec<PathBuf>,
    /// Bundle modification times when last scanned.
    pub scanned: HashMap<PathBuf, u64>,
}

static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();

fn registry() -> &'static RwLock<Registry> {
    REGISTRY.get_or_init(|| RwLock::new(Registry::load()))
}

pub fn snapshot() -> Registry {
    registry().read().map(|r| r.clone()).unwrap_or_default()
}

pub fn update(f: impl FnOnce(&mut Registry)) {
    if let Ok(mut r) = registry().write() {
        f(&mut r);
        r.save();
    }
}

pub fn find(cid: &str) -> Option<PluginInfo> {
    registry().read().ok()?.plugins.iter().find(|p| p.cid == cid).cloned()
}

/// True on first run (no plug-in list saved yet).
pub fn needs_first_scan() -> bool {
    cache_file().map(|f| !f.exists()).unwrap_or(false)
}

fn cache_file() -> Option<PathBuf> {
    crate::prefs::config_dir().map(|d| d.join("plugins.txt"))
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl Registry {
    fn load() -> Registry {
        let mut r = Registry::default();
        let Some(text) = cache_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return r };
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                [tag @ ("vst3" | "au"), cid, path, name, vendor, subs, version, enabled] => r.plugins.push(PluginInfo {
                    format: if *tag == "au" { Format::Au } else { Format::Vst3 },
                    cid: cid.to_string(),
                    path: PathBuf::from(path),
                    name: name.to_string(),
                    vendor: vendor.to_string(),
                    sub_categories: subs.to_string(),
                    version: version.to_string(),
                    enabled: *enabled != "0",
                }),
                ["failed", path, reason] => r.failed.push((PathBuf::from(path), reason.to_string())),
                ["folder", path] => r.folders.push(PathBuf::from(path)),
                ["scanned", path, mtime] => {
                    r.scanned.insert(PathBuf::from(path), mtime.parse().unwrap_or(0));
                }
                _ => {}
            }
        }
        r
    }

    fn save(&self) {
        let Some(f) = cache_file() else { return };
        let mut s = String::from("# Audemo plug-in list\n");
        for p in &self.plugins {
            s += &format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                p.format.tag(),
                p.cid,
                p.path.display(),
                clean(&p.name),
                clean(&p.vendor),
                clean(&p.sub_categories),
                clean(&p.version),
                p.enabled as u8
            );
        }
        for (p, why) in &self.failed {
            s += &format!("failed\t{}\t{}\n", p.display(), clean(why));
        }
        for d in &self.folders {
            s += &format!("folder\t{}\n", d.display());
        }
        for (p, m) in &self.scanned {
            s += &format!("scanned\t{}\t{m}\n", p.display());
        }
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(f, s);
    }
}

/// Standard VST3 folders for this platform.
pub fn default_folders() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        if let Some(c) = std::env::var_os("COMMONPROGRAMFILES") {
            v.push(PathBuf::from(c).join("VST3"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(l).join("Programs").join("Common").join("VST3"));
        }
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(h) = crate::prefs::home_dir() {
            v.push(h.join("Library/Audio/Plug-Ins/VST3"));
        }
    } else {
        if let Some(h) = crate::prefs::home_dir() {
            v.push(h.join(".vst3"));
        }
        v.push(PathBuf::from("/usr/lib/vst3"));
        v.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    v
}

/// Every `.vst3` bundle (or single-file plug-in) under `dir`.
fn find_bundles(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let is_vst3 = p.extension().map(|x| x.eq_ignore_ascii_case("vst3")).unwrap_or(false);
        if is_vst3 {
            out.push(p);
        } else if depth > 0 && p.is_dir() {
            find_bundles(&p, depth - 1, out);
        }
    }
}

fn mtime(p: &Path) -> u64 {
    let m = std::fs::metadata(vst3::binary_path(p)).or_else(|_| std::fs::metadata(p));
    m.and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

// ------------------------------------------------------------------ scanning

/// Entry point for `audemo --scan-vst3 <bundle>`: list the bundle's effect
/// classes on stdout. Runs in a child process so a crashing plug-in can't
/// take Audemo down with it.
pub fn scan_child(path: &Path) -> i32 {
    match vst3::Module::load(path) {
        Ok(m) => {
            for c in m.classes() {
                if c.is_effect() {
                    println!("class\t{}\t{}\t{}\t{}\t{}", vst3::tuid_hex(&c.cid), clean(&c.name), clean(&c.vendor), clean(&c.sub_categories), clean(&c.version));
                }
            }
            0
        }
        Err(e) => {
            println!("error\t{}", clean(&e));
            2
        }
    }
}

#[derive(Default)]
pub struct ScanProgress {
    pub total: usize,
    pub done: usize,
    pub current: String,
    pub finished: bool,
    pub found: usize,
}

/// Scan for plug-ins in the background. `full` rescans bundles already known.
pub fn start_scan(full: bool) -> Arc<Mutex<ScanProgress>> {
    let progress = Arc::new(Mutex::new(ScanProgress::default()));
    let prog = progress.clone();
    std::thread::spawn(move || {
        let reg = snapshot();
        let mut folders = default_folders();
        folders.extend(reg.folders.iter().cloned());
        let mut bundles = Vec::new();
        for f in &folders {
            find_bundles(f, 6, &mut bundles);
        }
        bundles.sort();
        bundles.dedup();
        let todo: Vec<PathBuf> = bundles.iter().filter(|b| full || reg.scanned.get(*b) != Some(&mtime(b))).cloned().collect();
        if let Ok(mut p) = prog.lock() {
            p.total = todo.len();
        }
        let exe = std::env::current_exe().ok();
        let mut results: Vec<(PathBuf, Result<Vec<PluginInfo>, String>)> = Vec::new();
        for b in &todo {
            if let Ok(mut p) = prog.lock() {
                p.current = b.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            }
            let r = match &exe {
                Some(exe) => scan_one(exe, b),
                None => Err("Can't find Audemo's own executable.".into()),
            };
            if let Ok(mut p) = prog.lock() {
                p.done += 1;
                if let Ok(v) = &r {
                    p.found += v.len();
                }
            }
            results.push((b.clone(), r));
        }
        #[cfg(target_os = "macos")]
        let units = au::list();
        update(|r| {
            // Forget bundles that are gone (Audio Units are relisted below).
            r.plugins.retain(|p| p.format == Format::Vst3 && bundles.contains(&p.path) || p.format == Format::Au);
            r.failed.retain(|(p, _)| bundles.contains(p) && !todo.contains(p));
            r.scanned.retain(|p, _| bundles.contains(p));
            for (b, res) in results {
                let was: Vec<PluginInfo> = r.plugins.iter().filter(|p| p.path == b).cloned().collect();
                r.plugins.retain(|p| p.path != b);
                match res {
                    Ok(list) => {
                        for mut p in list {
                            p.enabled = was.iter().find(|w| w.cid == p.cid).map(|w| w.enabled).unwrap_or(true);
                            // The same class in two places: keep the first.
                            if !r.plugins.iter().any(|q| q.cid == p.cid) {
                                r.plugins.push(p);
                            }
                        }
                    }
                    Err(e) => r.failed.push((b.clone(), e)),
                }
                r.scanned.insert(b.clone(), mtime(&b));
            }
            #[cfg(target_os = "macos")]
            {
                let was: Vec<PluginInfo> = r.plugins.iter().filter(|p| p.format == Format::Au).cloned().collect();
                r.plugins.retain(|p| p.format != Format::Au);
                for u in &units {
                    if r.plugins.iter().any(|q| q.format == Format::Au && q.cid == u.cid) {
                        continue;
                    }
                    r.plugins.push(PluginInfo {
                        format: Format::Au,
                        cid: u.cid.clone(),
                        name: u.name.clone(),
                        vendor: u.vendor.clone(),
                        path: PathBuf::new(),
                        sub_categories: String::new(),
                        version: u.version.clone(),
                        enabled: was.iter().find(|w| w.cid == u.cid).map(|w| w.enabled).unwrap_or(true),
                    });
                }
            }
            #[cfg(not(target_os = "macos"))]
            r.plugins.retain(|p| p.format != Format::Au);
            r.plugins.sort_by(|a, b| a.vendor.to_lowercase().cmp(&b.vendor.to_lowercase()).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        });
        if let Ok(mut p) = prog.lock() {
            p.finished = true;
        }
        if let Some(w) = WAKE.get() {
            w();
        }
    });
    progress
}

fn scan_one(exe: &Path, bundle: &Path) -> Result<Vec<PluginInfo>, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .arg("--scan-vst3")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the scanner: {e}"))?;
    let mut out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let text = reader.join().unwrap_or_default();
    let mut list = Vec::new();
    let mut error = None;
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.as_slice() {
            ["class", cid, name, vendor, subs, version] => list.push(PluginInfo {
                format: Format::Vst3,
                cid: cid.to_string(),
                name: name.to_string(),
                vendor: vendor.to_string(),
                path: bundle.to_path_buf(),
                sub_categories: subs.to_string(),
                version: version.to_string(),
                enabled: true,
            }),
            ["error", e] => error = Some(e.to_string()),
            _ => {}
        }
    }
    match status {
        None => Err("timed out while loading".into()),
        Some(s) if !s.success() && list.is_empty() => Err(error.unwrap_or_else(|| "crashed while loading".into())),
        _ if list.is_empty() => Err(error.unwrap_or_else(|| "no audio effects inside".into())),
        _ => Ok(list),
    }
}

// ------------------------------------------------------------------ modules

static MODULES: Mutex<Vec<(PathBuf, Weak<vst3::Module>)>> = Mutex::new(Vec::new());

/// Load (or reuse) the module at `path`. UI thread only.
fn module(path: &Path) -> Result<Arc<vst3::Module>, String> {
    let mut mods = MODULES.lock().map_err(|_| "module list poisoned")?;
    mods.retain(|(_, w)| w.strong_count() > 0);
    if let Some(m) = mods.iter().find(|(p, _)| p == path).and_then(|(_, w)| w.upgrade()) {
        return Ok(m);
    }
    let m = vst3::Module::load(path)?;
    mods.push((path.to_path_buf(), Arc::downgrade(&m)));
    Ok(m)
}

/// A running plug-in of either format.
pub enum Instance {
    Vst3(vst3::Instance),
    #[cfg(target_os = "macos")]
    Au(au::Instance),
}

macro_rules! each {
    ($self:ident, $i:ident => $e:expr) => {
        match $self {
            Instance::Vst3($i) => $e,
            #[cfg(target_os = "macos")]
            Instance::Au($i) => $e,
        }
    };
}

impl Instance {
    pub fn name(&self) -> String {
        each!(self, i => i.name.clone())
    }
    pub fn latency(&self) -> usize {
        each!(self, i => i.latency)
    }
    pub fn params(&self) -> Vec<vst3::ParamInfo> {
        each!(self, i => i.params())
    }
    pub fn get_param(&self, id: u32) -> f64 {
        each!(self, i => i.get_param(id))
    }
    pub fn param_text(&self, id: u32, v: f64) -> String {
        each!(self, i => i.param_text(id, v))
    }
    pub fn set_param(&mut self, id: u32, v: f64) {
        each!(self, i => i.set_param(id, v))
    }
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        each!(self, i => i.process(l, r))
    }
    pub fn reset(&mut self) {
        each!(self, i => i.reset())
    }
    /// Parameters the plug-in's own window changed since the last call:
    /// `Some(changes)` (possibly empty for changes it didn't itemise), or
    /// `None` if nothing changed.
    pub fn take_touched(&mut self) -> Option<Vec<(u32, f64)>> {
        match self {
            Instance::Vst3(i) => {
                let edits = {
                    let q = i.edits.lock().ok()?;
                    if !q.touched {
                        return None;
                    }
                    q.edits.clone()
                };
                let restart = i.take_edits();
                if restart & vst3::RESTART_LATENCY != 0 {
                    i.refresh_latency();
                }
                i.flush_params();
                Some(edits)
            }
            #[cfg(target_os = "macos")]
            Instance::Au(i) => i.take_touched(),
        }
    }
    /// The saved state as text (the `state` parameter).
    pub fn state_text(&mut self) -> String {
        match self {
            Instance::Vst3(i) => {
                let (a, b) = i.get_state();
                encode_state(&a, &b)
            }
            #[cfg(target_os = "macos")]
            Instance::Au(i) => base64(&i.get_state()),
        }
    }
    pub fn set_state_text(&mut self, s: &str) -> bool {
        match self {
            Instance::Vst3(i) => {
                let (a, b) = decode_state(s);
                i.set_state(&a, &b)
            }
            #[cfg(target_os = "macos")]
            Instance::Au(i) => i.set_state(&unbase64(s)),
        }
    }
    pub fn has_window(&self) -> bool {
        match self {
            Instance::Vst3(i) => view::SUPPORTED && i.controller().is_some(),
            #[cfg(target_os = "macos")]
            Instance::Au(_) => true,
        }
    }
    pub fn open_window(&self, title: &str) -> Result<Box<dyn view::Window>, String> {
        match self {
            Instance::Vst3(i) => {
                let ctrl = i.controller().ok_or("This plug-in has no window of its own.")?;
                Ok(Box::new(view::EditorWindow::open(ctrl, title)?))
            }
            #[cfg(target_os = "macos")]
            Instance::Au(i) => Ok(Box::new(i.open_window(title)?)),
        }
    }
}

/// A plug-in instance that is always destroyed on the UI thread.
pub struct Plugin(Option<Instance>);

impl Plugin {
    /// Create an instance of plug-in `cid` with saved `state` (from any thread).
    pub fn create(cid: &str, state: &str, sample_rate: u32, channels: usize, max_block: usize, offline: bool) -> Result<Plugin, String> {
        let info = find(cid).ok_or_else(|| format!("The plug-in {cid} isn't installed (or hasn't been scanned)."))?;
        let state = state.to_string();
        let inst = run_on_main(move || -> Result<Instance, String> {
            let mut inst = match info.format {
                Format::Vst3 => {
                    let tuid = vst3::tuid_from_hex(&info.cid).ok_or("bad plug-in id")?;
                    let m = module(&info.path)?;
                    Instance::Vst3(vst3::Instance::new(m, &tuid, sample_rate as f64, channels, max_block, offline)?)
                }
                #[cfg(target_os = "macos")]
                Format::Au => Instance::Au(au::Instance::new(&info.cid, &info.name, sample_rate as f64, channels, max_block, offline)?),
                #[cfg(not(target_os = "macos"))]
                Format::Au => return Err("Audio Units are only available on macOS.".into()),
            };
            if !state.is_empty() {
                inst.set_state_text(&state);
            }
            Ok(inst)
        })??;
        Ok(Plugin(Some(inst)))
    }

    pub fn get(&mut self) -> &mut Instance {
        self.0.as_mut().expect("plug-in instance")
    }

    pub fn state(&mut self) -> String {
        self.get().state_text()
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        if let Some(i) = self.0.take() {
            drop_on_main(i);
        }
    }
}

// ------------------------------------------------------------------ state text

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut s = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

pub fn unbase64(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Component and controller state as one text value.
pub fn encode_state(comp: &[u8], ctrl: &[u8]) -> String {
    let mut v = (comp.len() as u32).to_le_bytes().to_vec();
    v.extend_from_slice(comp);
    v.extend_from_slice(ctrl);
    base64(&v)
}

pub fn decode_state(s: &str) -> (Vec<u8>, Vec<u8>) {
    let v = unbase64(s);
    if v.len() < 4 {
        return (Vec::new(), Vec::new());
    }
    let n = (u32::from_le_bytes([v[0], v[1], v[2], v[3]]) as usize).min(v.len() - 4);
    (v[4..4 + n].to_vec(), v[4 + n..].to_vec())
}

// ------------------------------------------------------------------ effects

/// A plug-in in a real-time rack.
struct PluginRt {
    plugin: Plugin,
    cid: String,
    sample_rate: u32,
    state: String,
}

/// A stable fingerprint of a state text (FNV-1a).
pub fn state_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The `edits` parameter: the changes that turn the state with fingerprint
/// `from` into the new one, so a running instance can apply them cheaply.
pub fn encode_edits(from: &str, edits: &[(u32, f64)]) -> String {
    let list: Vec<String> = edits.iter().map(|(id, v)| format!("{id}:{v}")).collect();
    format!("{}|{}", state_hash(from), list.join(","))
}

fn decode_edits(s: &str, current: &str) -> Option<Vec<(u32, f64)>> {
    let (from, list) = s.split_once('|')?;
    if from != state_hash(current) || list.is_empty() {
        return None;
    }
    list.split(',').map(|e| e.split_once(':').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))).collect()
}

impl RtEffect for PluginRt {
    fn set_params(&mut self, p: &Params) {
        let s = p.s("state");
        if s == self.state {
            return;
        }
        // Edits made in the effect window arrive as individual parameter
        // changes: cheap enough to apply while the slot is playing.
        if let Some(edits) = decode_edits(&p.s("edits"), &self.state) {
            let inst = self.plugin.get();
            for (id, v) in edits {
                inst.set_param(id, v);
            }
        } else if s.is_empty() {
            // Back to defaults (Reset, undo): only a fresh instance has them.
            match Plugin::create(&self.cid, "", self.sample_rate, 2, 4096, false) {
                Ok(fresh) => self.plugin = fresh,
                Err(e) => eprintln!("{e}"),
            }
        } else {
            self.plugin.get().set_state_text(&s);
        }
        self.state = s;
    }
    fn reset(&mut self) {
        self.plugin.get().reset();
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.plugin.get().process(l, r);
    }
}

fn make_rt(id: &str, p: &Params, sr: u32) -> Option<Box<dyn RtEffect>> {
    let (_, cid) = split_id(id)?;
    let state = p.s("state");
    match Plugin::create(cid, &state, sr, 2, 4096, false) {
        Ok(plugin) => Some(Box::new(PluginRt { plugin, cid: cid.to_string(), sample_rate: sr, state })),
        Err(e) => {
            eprintln!("{e}");
            None
        }
    }
}

// Offline rendering reuses instances: the live Effects Rack renders a file
// in chunks and previews re-render after every edit, and creating a plug-in
// each time would be slow. Instances are kept per (plug-in, rate, channels)
// with the state they last had.
struct Pooled {
    cid: String,
    sr: u32,
    ch: usize,
    state: String,
    plugin: Plugin,
}

static POOL: Mutex<Vec<Pooled>> = Mutex::new(Vec::new());
const POOL_SIZE: usize = 6;

/// An offline instance with `state` and no leftover sound (from any thread).
fn pooled(cid: &str, state: &str, sr: u32, ch: usize) -> Result<Pooled, String> {
    let mut found = None;
    if let Ok(mut pool) = POOL.lock() {
        // Prefer an instance already in this state; any other will do unless
        // defaults are wanted (only a fresh instance has those).
        let pos = pool
            .iter()
            .position(|e| e.cid == cid && e.sr == sr && e.ch == ch && e.state == state)
            .or_else(|| if state.is_empty() { None } else { pool.iter().position(|e| e.cid == cid && e.sr == sr && e.ch == ch) });
        if let Some(i) = pos {
            found = Some(pool.remove(i));
        }
    }
    if let Some(mut e) = found {
        // Plug-ins accept these while not processing; doing them here keeps
        // rendering going even while the UI thread is busy (a file dialog).
        if e.state != state {
            e.plugin.get().set_state_text(state);
            e.state = state.to_string();
        }
        e.plugin.get().reset();
        return Ok(e);
    }
    let plugin = Plugin::create(cid, state, sr, ch, 4096, true)?;
    Ok(Pooled { cid: cid.to_string(), sr, ch, state: state.to_string(), plugin })
}

fn unpool(e: Pooled) {
    if let Ok(mut p) = POOL.lock() {
        p.push(e);
        if p.len() > POOL_SIZE {
            p.remove(0);
        }
    }
}

/// Offline processing (Apply, preview, Effects Rack, batch). Plug-ins see
/// the first two channels; any others pass through unchanged.
fn process_offline(audio: &[Vec<f32>], ctx: &Ctx, p: &Params) -> Result<Vec<Vec<f32>>, String> {
    let cid = p.s("plugin");
    let n_ch = audio.len().clamp(1, 2);
    let mut entry = pooled(&cid, &p.s("state"), ctx.sample_rate, n_ch)?;
    let inst = entry.plugin.get();
    let len = audio.first().map(|c| c.len()).unwrap_or(0);
    let lat = inst.latency();
    // Feed the latency's worth of silence after the audio and drop the
    // same amount from the start, so the result lines up with the input.
    let total = len + lat;
    let mut out: Vec<Vec<f32>> = audio.to_vec();
    if out.is_empty() {
        out.push(Vec::new());
    }
    let mut l = vec![0.0f32; 4096];
    let mut r = vec![0.0f32; 4096];
    let mut pos = 0;
    while pos < total {
        let n = (total - pos).min(4096);
        for i in 0..n {
            let k = pos + i;
            l[i] = if k < len { audio[0][k] } else { 0.0 };
            r[i] = if k < len { audio.get(1).map(|c| c[k]).unwrap_or(l[i]) } else { 0.0 };
        }
        inst.process(&mut l[..n], &mut r[..n]);
        for i in 0..n {
            let k = pos + i;
            if k >= lat && k - lat < len {
                out[0][k - lat] = l[i];
                if n_ch == 2 {
                    out[1][k - lat] = r[i];
                }
            }
        }
        pos += n;
    }
    unpool(entry);
    Ok(out)
}

/// A plug-in effect's parameters: the class id, its saved state, and the
/// last edits (see [`encode_edits`]).
pub fn param_defs(cid: &'static str) -> Vec<crate::dsp::params::ParamDef> {
    vec![text("plugin", "Plug-in", cid), text("state", "State", ""), text("edits", "Edits", "")]
}

/// Effect definitions for the enabled plug-ins, and the hooks that let racks
/// create them.
pub fn effect_defs() -> Vec<EffectDef> {
    crate::dsp::effects::rt::set_plugin_maker(make_rt);
    static LEAKED: Mutex<Vec<(String, &'static str, &'static str, &'static str)>> = Mutex::new(Vec::new());
    let reg = snapshot();
    let mut leaked = LEAKED.lock().unwrap_or_else(|e| e.into_inner());
    reg.plugins
        .iter()
        .filter(|p| p.enabled)
        .map(|p| {
            // Effect definitions need 'static strings; intern them once per plug-in.
            let (id, name, desc) = match leaked.iter().find(|l| l.0 == p.cid && l.2 == p.name) {
                Some(l) => (l.1, l.2, l.3),
                None => {
                    let id: &'static str = Box::leak(p.effect_id().into_boxed_str());
                    let name: &'static str = Box::leak(p.name.clone().into_boxed_str());
                    let desc: &'static str = Box::leak(format!("{} • {} plug-in{}", p.vendor, p.format.label(), if p.version.is_empty() { String::new() } else { format!(" • {}", p.version) }).into_boxed_str());
                    leaked.push((p.cid.clone(), id, name, desc));
                    (id, name, desc)
                }
            };
            let cid: &'static str = split_id(id).map(|(_, c)| c).unwrap_or("");
            EffectDef {
                id,
                name,
                category: Category::Plugin,
                description: desc,
                params: param_defs(cid),
                presets: Vec::new(),
                process: process_offline,
                response: None,
                handles: Vec::new(),
                generator: false,
                changes_length: false,
                stereo_only: false,
            }
        })
        .collect()
}

pub fn is_plugin(def: &EffectDef) -> bool {
    split_id(def.id).is_some()
}

pub fn vendor_of(def: &EffectDef) -> String {
    split_id(def.id).and_then(|(_, c)| find(c)).map(|p| p.vendor).unwrap_or_default()
}

pub fn format_of(def: &EffectDef) -> Option<Format> {
    split_id(def.id).map(|(f, _)| f)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for n in 0..40 {
            let d: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(unbase64(&base64(&d)), d);
        }
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        let (a, b) = decode_state(&encode_state(b"component", b"ctrl"));
        assert_eq!((a.as_slice(), b.as_slice()), (&b"component"[..], &b"ctrl"[..]));
        assert_eq!(decode_state(""), (Vec::new(), Vec::new()));
    }

    #[test]
    fn edits_apply_only_to_the_state_they_came_from() {
        let e = encode_edits("STATE_A", &[(3, 0.25), (7, 1.0)]);
        assert_eq!(decode_edits(&e, "STATE_A"), Some(vec![(3, 0.25), (7, 1.0)]));
        // Undo/redo to another state: apply the whole state instead.
        assert_eq!(decode_edits(&e, "STATE_B"), None);
        assert_eq!(decode_edits(&encode_edits("X", &[]), "X"), None);
        assert_eq!(decode_edits("", "X"), None);
        // Survives the session file's text escaping (no ; or =).
        assert!(!e.contains(';') && !e.contains('='));
        assert_eq!(split_id("vst3:ABC"), Some((Format::Vst3, "ABC")));
        assert_eq!(split_id("au:00"), Some((Format::Au, "00")));
        assert_eq!(split_id("amplify"), None);
    }
}
