//! Check for updates, and install them.
//!
//! The check asks GitHub for the latest Audemo release and offers it when
//! it's newer than this build (at launch unless switched off in
//! Preferences > General, and from Help > Check for Updates).
//!
//! Install and Restart downloads the release's package for how this copy
//! was installed, checks its SHA-256 against the one GitHub publishes, puts
//! it in place and restarts:
//!
//! * Windows, installed (Program Files): the .msi runs after Audemo quits
//!   (it upgrades in place), then Audemo starts again.
//! * Windows, portable: the new Audemo.exe from the portable .zip replaces
//!   this one (a running .exe can be renamed aside, not overwritten).
//! * macOS: the new Audemo.app from the .zip is staged beside this one and
//!   swapped in on restart.
//! * Linux AppImage or tarball: the new file is swapped in on restart.
//! * Linux .deb: installing needs root, so the package is handed to the
//!   system's software installer.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use crate::app::{App, Dialog};

const LATEST: &str = "https://api.github.com/repos/M0JSX/audemo/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/M0JSX/audemo/releases";

pub enum Outcome {
    UpToDate { latest: String },
    Available(Release),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lower-case hex SHA-256, when GitHub gives one.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "1.6.0" (without the tag's leading v).
    pub version: String,
    pub page: String,
    pub notes: String,
    pub assets: Vec<Asset>,
}

pub struct Check {
    rx: Receiver<Outcome>,
    /// From the Help menu: report "up to date" and failures too.
    manual: bool,
}

/// (major, minor, patch) from "v1.5.0", "1.5" or "1.5.0-beta".
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let core = v.split(['-', '+', ' ']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>());
    let major = it.next()?.ok()?;
    let minor = it.next().unwrap_or(Ok(0)).ok()?;
    let patch = it.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

// ------------------------------------------------------------------ JSON

#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|f| f.0 == k).map(|f| &f.1),
            _ => None,
        }
    }
    fn str(&self, k: &str) -> Option<&str> {
        match self.get(k)? {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    fn num(&self, k: &str) -> Option<f64> {
        match self.get(k)? {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
}

fn parse_json(s: &str) -> Option<Json> {
    struct P<'a> {
        b: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn eat(&mut self, lit: &[u8]) -> bool {
            if self.b[self.i..].starts_with(lit) {
                self.i += lit.len();
                true
            } else {
                false
            }
        }
        fn string(&mut self) -> Option<String> {
            if self.b.get(self.i) != Some(&b'"') {
                return None;
            }
            self.i += 1;
            let mut out: Vec<u8> = Vec::new();
            while let Some(&c) = self.b.get(self.i) {
                self.i += 1;
                match c {
                    b'"' => return String::from_utf8(out).ok(),
                    b'\\' => {
                        let e = *self.b.get(self.i)?;
                        self.i += 1;
                        match e {
                            b'n' => out.push(b'\n'),
                            b't' => out.push(b'\t'),
                            b'r' => out.push(b'\r'),
                            b'b' | b'f' => {}
                            b'u' => {
                                let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                                self.i += 4;
                                let mut cp = u32::from_str_radix(hex, 16).ok()?;
                                if (0xD800..0xDC00).contains(&cp) && self.b.get(self.i..self.i + 2) == Some(b"\\u") {
                                    let lo = std::str::from_utf8(self.b.get(self.i + 2..self.i + 6)?).ok()?;
                                    let lo = u32::from_str_radix(lo, 16).ok()?;
                                    self.i += 6;
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                                }
                                let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                                let mut buf = [0u8; 4];
                                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                            }
                            other => out.push(other),
                        }
                    }
                    _ => out.push(c),
                }
            }
            None
        }
        fn value(&mut self, depth: usize) -> Option<Json> {
            if depth > 64 {
                return None;
            }
            self.ws();
            match *self.b.get(self.i)? {
                b'"' => Some(Json::Str(self.string()?)),
                b'{' => {
                    self.i += 1;
                    let mut v = Vec::new();
                    loop {
                        self.ws();
                        if self.eat(b"}") {
                            return Some(Json::Obj(v));
                        }
                        if !v.is_empty() && !self.eat(b",") {
                            return None;
                        }
                        self.ws();
                        let k = self.string()?;
                        self.ws();
                        if !self.eat(b":") {
                            return None;
                        }
                        v.push((k, self.value(depth + 1)?));
                    }
                }
                b'[' => {
                    self.i += 1;
                    let mut v = Vec::new();
                    loop {
                        self.ws();
                        if self.eat(b"]") {
                            return Some(Json::Arr(v));
                        }
                        if !v.is_empty() && !self.eat(b",") {
                            return None;
                        }
                        v.push(self.value(depth + 1)?);
                    }
                }
                b't' if self.eat(b"true") => Some(Json::Bool(true)),
                b'f' if self.eat(b"false") => Some(Json::Bool(false)),
                b'n' if self.eat(b"null") => Some(Json::Null),
                _ => {
                    let st = self.i;
                    while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                        self.i += 1;
                    }
                    std::str::from_utf8(&self.b[st..self.i]).ok()?.parse().ok().map(Json::Num)
                }
            }
        }
    }
    let mut p = P { b: s.as_bytes(), i: 0 };
    p.value(0)
}

/// Read a GitHub "latest release" response.
pub fn parse_release(json: &str) -> Option<Release> {
    let j = parse_json(json)?;
    let tag = j.str("tag_name")?.to_string();
    parse_version(&tag)?;
    let assets = match j.get("assets") {
        Some(Json::Arr(a)) => a
            .iter()
            .filter_map(|a| {
                Some(Asset {
                    name: a.str("name")?.to_string(),
                    url: a.str("browser_download_url")?.to_string(),
                    size: a.num("size").unwrap_or(0.0) as u64,
                    sha256: a.str("digest").and_then(|d| d.strip_prefix("sha256:")).map(|h| h.to_ascii_lowercase()),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: j.str("html_url").unwrap_or(RELEASES_PAGE).to_string(),
        notes: j.str("body").unwrap_or_default().replace("\r\n", "\n"),
        assets,
    })
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("Audemo/{}", env!("CARGO_PKG_VERSION")))
        .build()
}

fn fetch() -> Outcome {
    let body = match agent().get(LATEST).set("Accept", "application/vnd.github+json").call() {
        Ok(r) => match r.into_string() {
            Ok(b) => b,
            Err(e) => return Outcome::Failed(format!("Couldn't read GitHub's reply: {e}")),
        },
        Err(ureq::Error::Status(404, _)) => return Outcome::Failed("No releases have been published yet.".into()),
        Err(ureq::Error::Status(code, _)) => return Outcome::Failed(format!("GitHub answered with HTTP {code}. Try again later.")),
        Err(e) => return Outcome::Failed(format!("Couldn't reach GitHub ({e}). Check your internet connection.")),
    };
    match parse_release(&body) {
        Some(r) if is_newer(&r.version, env!("CARGO_PKG_VERSION")) => Outcome::Available(r),
        Some(r) => Outcome::UpToDate { latest: r.version },
        None => Outcome::Failed("GitHub's reply didn't name a release.".into()),
    }
}

/// Open a web page (or a file) with the system's default app.
pub fn open_url(url: &str) -> Result<(), String> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let r = Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let r = Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let r = Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ()).map_err(|e| format!("Couldn't open {url}: {e}"))
}

// ------------------------------------------------------------------ SHA-256

pub struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    pub fn new() -> Self {
        Sha256 { h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], buf: Vec::with_capacity(64), len: 0 }
    }

    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b2, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b2) ^ (a & c) ^ (b2 & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b2;
            b2 = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in self.h.iter_mut().zip([a, b2, c, d, e, f, g, h]) {
            *x = x.wrapping_add(y);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == 64 {
                let b = std::mem::take(&mut self.buf);
                self.block(&b);
                self.buf = b;
                self.buf.clear();
            }
        }
        while data.len() >= 64 {
            self.block(&data[..64]);
            data = &data[64..];
        }
        self.buf.extend_from_slice(data);
    }

    pub fn hex(mut self) -> String {
        let bits = self.len.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.buf.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_be_bytes());
        let len = self.len;
        self.update(&pad);
        self.len = len;
        self.h.iter().map(|x| format!("{x:08x}")).collect()
    }
}

// ------------------------------------------------------------------ installing

/// How this copy of Audemo was installed.
#[derive(Clone, Debug, PartialEq)]
pub enum InstallKind {
    WinMsi { exe: PathBuf },
    WinPortable { exe: PathBuf },
    MacApp { bundle: PathBuf },
    AppImage { file: PathBuf },
    LinuxTar { exe: PathBuf },
    LinuxDeb,
}

pub fn install_kind() -> Option<InstallKind> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if cfg!(windows) {
        let lower = exe.to_string_lossy().to_ascii_lowercase();
        return Some(if lower.contains("\\program files") { InstallKind::WinMsi { exe } } else { InstallKind::WinPortable { exe } });
    }
    if cfg!(target_os = "macos") {
        let bundle = exe.ancestors().find(|p| p.extension().map(|e| e == "app").unwrap_or(false))?.to_path_buf();
        return Some(InstallKind::MacApp { bundle });
    }
    if let Some(a) = std::env::var_os("APPIMAGE") {
        return Some(InstallKind::AppImage { file: PathBuf::from(a) });
    }
    if exe.starts_with("/usr/") {
        return Some(InstallKind::LinuxDeb);
    }
    Some(InstallKind::LinuxTar { exe })
}

/// The release package for an install kind.
pub fn pick_asset<'a>(kind: &InstallKind, assets: &'a [Asset]) -> Option<&'a Asset> {
    let has = |a: &&Asset, pre: &str, suf: &str| {
        let n = a.name.to_ascii_lowercase();
        n.contains(pre) && n.ends_with(suf)
    };
    match kind {
        InstallKind::WinMsi { .. } => assets.iter().find(|a| has(a, "", ".msi")),
        InstallKind::WinPortable { .. } => assets.iter().find(|a| has(a, "windows", ".zip")),
        InstallKind::MacApp { .. } => assets.iter().find(|a| has(a, "macos", ".zip")),
        InstallKind::AppImage { .. } => assets.iter().find(|a| has(a, "", ".appimage")),
        InstallKind::LinuxTar { .. } => assets.iter().find(|a| has(a, "linux", ".tar.gz")),
        InstallKind::LinuxDeb => assets.iter().find(|a| has(a, "", ".deb")),
    }
}

/// What to do on restart once the update is downloaded and staged.
#[derive(Clone, Debug)]
pub enum Plan {
    /// Run the .msi after quitting, then start Audemo.
    Msi { msi: PathBuf, exe: PathBuf },
    /// Swap `new` (staged beside it) in for `target`, then start it.
    Swap { new: PathBuf, target: PathBuf, bundle: bool },
    /// Hand a package to the system (with a note for the user).
    Open { file: PathBuf, note: String },
}

pub enum InstallState {
    Downloading,
    Ready(Plan),
    Failed(String),
}

pub struct Install {
    pub version: String,
    pub total: u64,
    pub done: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Result<Plan, String>>>,
    pub state: InstallState,
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".audemo-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn run(cmd: &mut std::process::Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("Couldn't run {what}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The first file called `name` (any case) under `dir`.
fn find_file(dir: &Path, name: &str, want_dir: bool) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false) && p.is_dir() == want_dir {
            return Some(p);
        }
        if p.is_dir() && p.extension().map(|x| x != "app").unwrap_or(true) {
            if let Some(f) = find_file(&p, name, want_dir) {
                return Some(f);
            }
        }
    }
    None
}

fn work_dir(version: &str) -> PathBuf {
    std::env::temp_dir().join(format!("audemo-update-{version}"))
}

/// Download `asset` (checking size and SHA-256) and stage it for `kind`.
fn download_and_stage(kind: InstallKind, asset: Asset, version: String, done: Arc<AtomicU64>, cancel: Arc<AtomicBool>) -> Result<Plan, String> {
    let dir = work_dir(&version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make a folder for the download: {e}"))?;
    let file = dir.join(&asset.name);
    let resp = agent().get(&asset.url).call().map_err(|e| format!("Download failed ({e})."))?;
    let mut reader = resp.into_reader();
    let mut out = std::fs::File::create(&file).map_err(|e| format!("Couldn't save the download: {e}"))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n_total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled.".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("Download interrupted ({e})."))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| format!("Couldn't save the download: {e}"))?;
        hash.update(&buf[..n]);
        n_total += n as u64;
        done.store(n_total, Ordering::Relaxed);
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    if asset.size > 0 && n_total != asset.size {
        return Err(format!("The download is incomplete ({n_total} of {} bytes).", asset.size));
    }
    if let Some(want) = &asset.sha256 {
        let got = hash.hex();
        if &got != want {
            return Err("The download doesn't match GitHub's checksum, so it wasn't installed.".into());
        }
    }
    stage(kind, &file, &dir)
}

#[cfg(unix)]
fn make_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

/// Put the downloaded package where the restart can swap it in.
fn stage(kind: InstallKind, file: &Path, dir: &Path) -> Result<Plan, String> {
    use std::process::Command;
    match kind {
        InstallKind::WinMsi { exe } => Ok(Plan::Msi { msi: file.to_path_buf(), exe }),
        InstallKind::WinPortable { exe } => {
            let parent = exe.parent().ok_or("No folder for Audemo.exe.")?.to_path_buf();
            if !dir_writable(&parent) {
                return Err(format!("Audemo can't write to {}. Download the update and replace Audemo.exe yourself.", parent.display()));
            }
            let x = dir.join("unpacked");
            std::fs::create_dir_all(&x).map_err(|e| e.to_string())?;
            run(Command::new("tar").arg("-xf").arg(file).arg("-C").arg(&x), "Unpacking the update")?;
            let new_exe = find_file(&x, "Audemo.exe", false).ok_or("The update has no Audemo.exe.")?;
            let staged = parent.join("Audemo.new.exe");
            std::fs::copy(&new_exe, &staged).map_err(|e| format!("Couldn't stage the update: {e}"))?;
            Ok(Plan::Swap { new: staged, target: exe, bundle: false })
        }
        InstallKind::MacApp { bundle } => {
            let parent = bundle.parent().ok_or("No folder for Audemo.app.")?.to_path_buf();
            if !dir_writable(&parent) {
                return Err(format!("Audemo can't write to {}. Download the update and drag Audemo.app there yourself.", parent.display()));
            }
            let x = dir.join("unpacked");
            std::fs::create_dir_all(&x).map_err(|e| e.to_string())?;
            run(Command::new("ditto").arg("-x").arg("-k").arg(file).arg(&x), "Unpacking the update")?;
            let new_app = find_file(&x, "Audemo.app", true).ok_or("The update has no Audemo.app.")?;
            let staged = parent.join(".Audemo-update.app");
            let _ = std::fs::remove_dir_all(&staged);
            run(Command::new("ditto").arg(&new_app).arg(&staged), "Copying the update")?;
            let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&staged).output();
            Ok(Plan::Swap { new: staged, target: bundle, bundle: true })
        }
        InstallKind::AppImage { file: target } => {
            let parent = target.parent().ok_or("No folder for the AppImage.")?.to_path_buf();
            if !dir_writable(&parent) {
                return Err(format!("Audemo can't write to {}.", parent.display()));
            }
            let staged = parent.join(".Audemo-update.AppImage");
            std::fs::copy(file, &staged).map_err(|e| format!("Couldn't stage the update: {e}"))?;
            make_executable(&staged)?;
            Ok(Plan::Swap { new: staged, target, bundle: false })
        }
        InstallKind::LinuxTar { exe } => {
            let parent = exe.parent().ok_or("No folder for audemo.")?.to_path_buf();
            if !dir_writable(&parent) {
                return Err(format!("Audemo can't write to {}.", parent.display()));
            }
            let x = dir.join("unpacked");
            std::fs::create_dir_all(&x).map_err(|e| e.to_string())?;
            run(Command::new("tar").arg("-xzf").arg(file).arg("-C").arg(&x), "Unpacking the update")?;
            let new_exe = find_file(&x, "audemo", false).ok_or("The update has no audemo program.")?;
            let staged = parent.join(".audemo-update");
            std::fs::copy(&new_exe, &staged).map_err(|e| format!("Couldn't stage the update: {e}"))?;
            make_executable(&staged)?;
            Ok(Plan::Swap { new: staged, target: exe, bundle: false })
        }
        InstallKind::LinuxDeb => Ok(Plan::Open {
            file: file.to_path_buf(),
            note: format!("Installing a .deb needs your password. It's opening in your software installer; or run:\n\nsudo apt install {}", shell_quote(&file.to_string_lossy())),
        }),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Swap the staged update in and start it (Audemo quits straight after).
pub fn apply(plan: &Plan) -> Result<(), String> {
    use std::process::Command;
    let pid = std::process::id();
    match plan {
        Plan::Open { file, .. } => open_url(&file.to_string_lossy()),
        Plan::Msi { msi, exe } => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // Wait a moment for Audemo to close, install (Windows asks
                // for permission), then start the new version.
                let script = format!(
                    "/S /C \"ping -n 3 127.0.0.1 >nul & msiexec /i \"{}\" /passive & start \"\" \"{}\"\"",
                    msi.display(),
                    exe.display()
                );
                Command::new("cmd").raw_arg(script).creation_flags(0x0800_0000).spawn().map_err(|e| format!("Couldn't start the installer: {e}"))?;
                Ok(())
            }
            #[cfg(not(windows))]
            {
                let _ = (msi, exe);
                Err("Not Windows.".into())
            }
        }
        Plan::Swap { new, target, bundle } => {
            let parent = target.parent().ok_or("No folder.")?;
            let old = if *bundle {
                parent.join(format!(".Audemo-old-{pid}.app"))
            } else if cfg!(windows) {
                parent.join("Audemo.old.exe")
            } else {
                parent.join(format!(".audemo-old-{pid}"))
            };
            if old.is_dir() {
                let _ = std::fs::remove_dir_all(&old);
            } else {
                let _ = std::fs::remove_file(&old);
            }
            std::fs::rename(target, &old).map_err(|e| format!("Couldn't move the current version aside: {e}"))?;
            if let Err(e) = std::fs::rename(new, target) {
                let _ = std::fs::rename(&old, target);
                return Err(format!("Couldn't put the new version in place: {e}"));
            }
            if !cfg!(windows) {
                if *bundle {
                    let _ = std::fs::remove_dir_all(&old);
                } else {
                    let _ = std::fs::remove_file(&old);
                }
            }
            // Start the new version once this one has gone.
            #[cfg(windows)]
            {
                Command::new(target).spawn().map_err(|e| format!("Updated, but couldn't restart: {e}"))?;
            }
            #[cfg(unix)]
            {
                let launch = if *bundle { "open \"$2\"" } else { "exec \"$2\"" };
                Command::new("sh")
                    .arg("-c")
                    .arg(format!("while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; {launch}"))
                    .arg("sh")
                    .arg(pid.to_string())
                    .arg(target)
                    .spawn()
                    .map_err(|e| format!("Updated, but couldn't restart: {e}"))?;
            }
            Ok(())
        }
    }
}

/// At startup: tidy up after an update (the old .exe, staged copies and
/// downloads).
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::fs::remove_file(dir.join("Audemo.old.exe"));
            let _ = std::fs::remove_file(dir.join("Audemo.new.exe"));
        }
    }
    if let Some(InstallKind::MacApp { bundle }) = install_kind() {
        if let Some(parent) = bundle.parent() {
            let _ = std::fs::remove_dir_all(parent.join(".Audemo-update.app"));
        }
    }
    if let Some(InstallKind::AppImage { file: f }) = install_kind() {
        if let Some(parent) = f.parent() {
            let _ = std::fs::remove_file(parent.join(".Audemo-update.AppImage"));
        }
    }
    if let Some(InstallKind::LinuxTar { exe }) = install_kind() {
        if let Some(parent) = exe.parent() {
            let _ = std::fs::remove_file(parent.join(".audemo-update"));
        }
    }
    if let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("audemo-update-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

impl App {
    /// Start a check in the background. `manual` reports every outcome;
    /// otherwise only a new version (that wasn't skipped) is mentioned.
    pub fn check_for_updates(&mut self, manual: bool) {
        if self.update_check.is_some() {
            if manual {
                self.update_check.as_mut().unwrap().manual = true;
            }
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
        });
        self.update_check = Some(Check { rx, manual });
        if manual {
            self.set_status("Checking for updates…");
        }
    }

    pub fn poll_update_check(&mut self) {
        self.poll_install();
        let Some(c) = &self.update_check else { return };
        let Ok(outcome) = c.rx.try_recv() else { return };
        let manual = c.manual;
        self.update_check = None;
        match outcome {
            Outcome::Available(r) => {
                if manual || r.version != self.prefs.skip_version {
                    // Don't cover a dialog the user is working in.
                    if self.dialog.is_none() || manual {
                        self.dialog = Some(Dialog::Update(r));
                    } else {
                        self.set_status(format!("Audemo {} is available: Help > Check for Updates", r.version));
                    }
                }
            }
            Outcome::UpToDate { latest } if manual => {
                self.set_status("Audemo is up to date.");
                self.dialog = Some(Dialog::Message {
                    title: "Check for Updates".into(),
                    text: format!("Audemo {} is the latest version (newest release: {latest}).", env!("CARGO_PKG_VERSION")),
                });
            }
            Outcome::Failed(e) if manual => {
                self.set_status("Couldn't check for updates.");
                self.dialog = Some(Dialog::Message { title: "Check for Updates".into(), text: e });
            }
            _ => {}
        }
    }

    /// Install and Restart: download and stage `r` in the background.
    pub fn start_install(&mut self, r: &Release) {
        let Some(kind) = install_kind() else {
            self.set_status("Audemo can't tell how it was installed; use Download instead.");
            return;
        };
        let Some(asset) = pick_asset(&kind, &r.assets).cloned() else {
            self.set_status("This release has no package for this kind of install; use Download instead.");
            return;
        };
        let done = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (d, c, v, size) = (done.clone(), cancel.clone(), r.version.clone(), asset.size);
        std::thread::spawn(move || {
            let _ = tx.send(download_and_stage(kind, asset, v, d, c));
        });
        self.updater = Some(Install { version: r.version.clone(), total: size, done, cancel, rx: Some(rx), state: InstallState::Downloading });
    }

    pub fn cancel_install(&mut self) {
        if let Some(u) = &self.updater {
            u.cancel.store(true, Ordering::Relaxed);
        }
        self.updater = None;
    }

    fn poll_install(&mut self) {
        let Some(u) = &mut self.updater else { return };
        let Some(rx) = &u.rx else { return };
        let Ok(res) = rx.try_recv() else { return };
        u.rx = None;
        let version = u.version.clone();
        u.state = match res {
            Ok(p) => InstallState::Ready(p),
            Err(e) => InstallState::Failed(e),
        };
        if matches!(u.state, InstallState::Ready(_)) && !matches!(self.dialog, Some(Dialog::Update(_))) {
            self.set_status(format!("Audemo {version} is ready to install: Help > Check for Updates > Restart Now"));
        }
    }

    /// Swap the update in and quit (the new version starts).
    pub fn restart_into_update(&mut self, ctx: &eframe::egui::Context) {
        let Some(u) = &mut self.updater else { return };
        let InstallState::Ready(plan) = &u.state else { return };
        let plan = plan.clone();
        match apply(&plan) {
            Ok(()) if matches!(plan, Plan::Open { .. }) => {
                if let Plan::Open { note, .. } = plan {
                    self.dialog = Some(Dialog::Message { title: "Install the Update".into(), text: note });
                }
                self.updater = None;
            }
            Ok(()) => {
                self.allow_quit = true;
                ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
            }
            Err(e) => u.state = InstallState::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert_eq!(parse_version("v1.5.0"), Some((1, 5, 0)));
        assert_eq!(parse_version("1.6"), Some((1, 6, 0)));
        assert_eq!(parse_version("2.0.1-beta"), Some((2, 0, 1)));
        assert!(is_newer("v1.5.1", "1.5.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(!is_newer("v1.5.0", "1.5.0"));
        assert!(!is_newer("1.4.9", "1.5.0"));
        assert!(!is_newer("nightly", "1.5.0"));
    }

    #[test]
    fn reads_github_release() {
        let json = r###"{
          "url": "https://api.github.com/repos/M0JSX/audemo/releases/1",
          "html_url": "https://github.com/M0JSX/audemo/releases/tag/v1.6.0",
          "author": {"login": "M0JSX", "html_url": "https://github.com/M0JSX", "site_admin": false},
          "tag_name": "v1.6.0", "draft": false, "prerelease": false, "id": 42, "published_at": null,
          "assets": [
            {"name": "Audemo-1.6.0-x64.msi", "size": 5570560, "digest": "sha256:3770BE8B", "browser_download_url": "https://example.com/a.msi", "uploader": {"login": "x"}},
            {"name": "Audemo.dmg", "size": 1, "digest": null, "browser_download_url": "https://example.com/a.dmg"},
            {"name": "audemo-macos-universal.zip", "size": 2, "browser_download_url": "https://example.com/m.zip"},
            {"name": "audemo-windows-x64-portable.zip", "size": 3, "browser_download_url": "https://example.com/w.zip"},
            {"name": "Audemo-x86_64.AppImage", "size": 4, "browser_download_url": "https://example.com/a.AppImage"},
            {"name": "audemo-linux-x64.tar.gz", "size": 5, "browser_download_url": "https://example.com/l.tgz"},
            {"name": "audemo_1.6.0_amd64.deb", "size": 6, "browser_download_url": "https://example.com/a.deb"}
          ],
          "body": "## What's new\r\n- Café \"quotes\" \\ and 🎵"
        }"###;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "1.6.0");
        assert_eq!(r.page, "https://github.com/M0JSX/audemo/releases/tag/v1.6.0");
        assert_eq!(r.notes, "## What's new\n- Café \"quotes\" \\ and 🎵");
        assert_eq!(r.assets.len(), 7);
        assert_eq!(r.assets[0].sha256.as_deref(), Some("3770be8b"));
        assert_eq!(r.assets[0].size, 5570560);
        assert!(r.assets[1].sha256.is_none());
        let p = PathBuf::from("/x");
        let name = |k: InstallKind| pick_asset(&k, &r.assets).map(|a| a.name.clone()).unwrap_or_default();
        assert_eq!(name(InstallKind::WinMsi { exe: p.clone() }), "Audemo-1.6.0-x64.msi");
        assert_eq!(name(InstallKind::WinPortable { exe: p.clone() }), "audemo-windows-x64-portable.zip");
        assert_eq!(name(InstallKind::MacApp { bundle: p.clone() }), "audemo-macos-universal.zip");
        assert_eq!(name(InstallKind::AppImage { file: p.clone() }), "Audemo-x86_64.AppImage");
        assert_eq!(name(InstallKind::LinuxTar { exe: p }), "audemo-linux-x64.tar.gz");
        assert_eq!(name(InstallKind::LinuxDeb), "audemo_1.6.0_amd64.deb");
        assert!(parse_release("{\"message\": \"Not Found\"}").is_none());
    }

    #[test]
    fn sha256_matches_known_vectors() {
        let h = |d: &[u8]| {
            let mut s = Sha256::new();
            s.update(d);
            s.hex()
        };
        assert_eq!(h(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(h(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(h(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        // Fed in pieces across block boundaries.
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut s = Sha256::new();
        for c in data.chunks(37) {
            s.update(c);
        }
        assert_eq!(s.hex(), h(&data));
        let mut big = Sha256::new();
        for _ in 0..1000 {
            big.update(&[b'a'; 1000]);
        }
        assert_eq!(big.hex(), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }
}
