//! Check for updates: asks GitHub for the latest Audemo release and offers
//! it when it's newer than this build. Runs at launch (Preferences >
//! General, "Check for updates at startup") and from Help > Check for
//! Updates.

use std::sync::mpsc::{channel, Receiver};
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
pub struct Release {
    /// "1.6.0" (without the tag's leading v).
    pub version: String,
    pub page: String,
    pub notes: String,
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

/// The top-level string fields of a JSON object (nested values skipped).
fn json_top_strings(s: &str) -> Vec<(String, String)> {
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
                                // Surrogate pair.
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
        /// Skip any value; Some(text) when it was a string.
        fn value(&mut self) -> Option<Option<String>> {
            self.ws();
            match *self.b.get(self.i)? {
                b'"' => Some(Some(self.string()?)),
                b'{' | b'[' => {
                    let mut depth = 0usize;
                    while let Some(&c) = self.b.get(self.i) {
                        match c {
                            b'"' => {
                                self.string()?;
                                continue;
                            }
                            b'{' | b'[' => depth += 1,
                            b'}' | b']' => {
                                depth -= 1;
                                if depth == 0 {
                                    self.i += 1;
                                    return Some(None);
                                }
                            }
                            _ => {}
                        }
                        self.i += 1;
                    }
                    None
                }
                _ => {
                    while self.i < self.b.len() && !matches!(self.b[self.i], b',' | b'}' | b']') {
                        self.i += 1;
                    }
                    Some(None)
                }
            }
        }
    }
    let mut p = P { b: s.as_bytes(), i: 0 };
    let mut out = Vec::new();
    p.ws();
    if p.b.get(p.i) != Some(&b'{') {
        return out;
    }
    p.i += 1;
    loop {
        p.ws();
        match p.b.get(p.i) {
            Some(b'}') | None => break,
            Some(b',') => {
                p.i += 1;
                continue;
            }
            _ => {}
        }
        let Some(key) = p.string() else { break };
        p.ws();
        if p.b.get(p.i) != Some(&b':') {
            break;
        }
        p.i += 1;
        match p.value() {
            Some(Some(v)) => out.push((key, v)),
            Some(None) => {}
            None => break,
        }
    }
    out
}

/// Read a GitHub "latest release" response.
pub fn parse_release(json: &str) -> Option<Release> {
    let fields = json_top_strings(json);
    let get = |k: &str| fields.iter().find(|f| f.0 == k).map(|f| f.1.clone());
    let tag = get("tag_name")?;
    parse_version(&tag)?;
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: get("html_url").unwrap_or_else(|| RELEASES_PAGE.to_string()),
        notes: get("body").unwrap_or_default().replace("\r\n", "\n"),
    })
}

fn fetch() -> Outcome {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(&format!("Audemo/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let body = match agent.get(LATEST).set("Accept", "application/vnd.github+json").call() {
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

/// Open a web page in the default browser.
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
          "author": {"login": "M0JSX", "html_url": "https://github.com/M0JSX"},
          "tag_name": "v1.6.0", "draft": false, "prerelease": false, "id": 42,
          "assets": [{"name": "Audemo.dmg", "size": 1}],
          "body": "## What's new\r\n- Caf\u00e9 \"quotes\" \\ and \ud83c\udfb5"
        }"###;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "1.6.0");
        assert_eq!(r.page, "https://github.com/M0JSX/audemo/releases/tag/v1.6.0");
        assert_eq!(r.notes, "## What's new\n- Café \"quotes\" \\ and 🎵");
        assert!(parse_release("{\"message\": \"Not Found\"}").is_none());
    }
}
