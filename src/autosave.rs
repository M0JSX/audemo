//! Preferences > Auto Save: timed backups of multitrack sessions (and,
//! optionally, of audio files with unsaved changes). Backups are written to
//! their own files in the background and never replace the originals.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::app::App;
use crate::export::{self, Container, ExportSettings, WavFormat};
use crate::prefs::BackupLocation;

#[derive(Default)]
pub struct AutoSave {
    last: Option<Instant>,
    busy: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Backups written while Audemo has been running (files, and the
    /// "Files" folders of session backups): removed again on a normal quit.
    /// After a crash they stay, for recovery.
    pub(crate) written: Vec<PathBuf>,
    /// The newest backup of each file and session this run, by backup name.
    entries: BTreeMap<String, Recovery>,
    /// Backups left by an earlier run that the user hasn't dealt with yet
    /// ("Not Now"): kept in the list until they are.
    pub(crate) stale: Vec<Recovery>,
}

/// A backup that can be reopened after Audemo didn't close normally.
#[derive(Clone, Debug, PartialEq)]
pub struct Recovery {
    pub session: bool,
    pub backup: PathBuf,
    /// The file or session it backs up (None if never saved).
    pub original: Option<PathBuf>,
    pub name: String,
}

impl AutoSave {
    /// Start the interval again (after the settings change).
    pub fn reset(&mut self) {
        self.last = Some(Instant::now());
    }
}

/// The list of backups in use, in the settings folder. It's rewritten after
/// each round of backups and removed on a normal quit, so finding it at
/// startup means Audemo didn't close normally last time.
fn manifest() -> Option<PathBuf> {
    crate::prefs::config_dir().map(|d| d.join("backups-in-use.txt"))
}

fn write_manifest(list: &[Recovery]) {
    let Some(f) = manifest() else { return };
    if list.is_empty() {
        let _ = std::fs::remove_file(f);
        return;
    }
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let text: String = list
        .iter()
        .map(|r| format!("{}\t{}\t{}\t{}\n", if r.session { "session" } else { "audio" }, r.backup.display(), r.original.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), clean(&r.name)))
        .collect();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, text);
}

/// Backups left by a run that didn't close normally (those still on disk).
pub fn leftover_backups() -> Vec<Recovery> {
    let Some(text) = manifest().and_then(|f| std::fs::read_to_string(f).ok()) else { return Vec::new() };
    parse_manifest(&text).into_iter().filter(|r| r.backup.is_file()).collect()
}

fn parse_manifest(text: &str) -> Vec<Recovery> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 4 {
                return None;
            }
            Some(Recovery { session: f[0] == "session", backup: PathBuf::from(f[1]), original: (!f[2].is_empty()).then(|| PathBuf::from(f[2])), name: f[3].to_string() })
        })
        .collect()
}

/// The "Files" folder holding a session backup's unsaved audio.
fn files_dir_of(backup: &Path) -> PathBuf {
    PathBuf::from(format!("{} Files", backup.with_extension("").display()))
}

/// The centralized backup folder used when no other is chosen.
pub fn default_dir() -> Option<PathBuf> {
    crate::prefs::config_dir().map(|d| d.join("Backups"))
}

/// "2026-10-10 193512" (UTC): sorts in time order.
fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

enum Item {
    Text { path: PathBuf, text: String },
    Audio { path: PathBuf, audio: crate::audio::Audio, rate: u32, meta: export::Metadata, markers: Vec<crate::export::Marker> },
    /// Keep the newest `keep` backups in `dir` whose names start with `prefix`.
    Prune { dir: PathBuf, prefix: String, keep: usize },
}

/// A short tag telling apart backups of different things with the same
/// name: a hash of the saved path, or of the open item's id when unsaved.
fn tag(path: Option<&Path>, id: u64) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let bytes: Vec<u8> = match path {
        Some(p) => p.to_string_lossy().as_bytes().to_vec(),
        None => format!("untitled:{id}").into_bytes(),
    };
    for b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{:06x}", h & 0xff_ffff)
}

fn prune(dir: &Path, prefix: &str, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut found: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().map(|n| n.to_string_lossy().starts_with(prefix)).unwrap_or(false))
        .filter(|p| !p.to_string_lossy().ends_with(" Files"))
        .collect();
    found.sort();
    let excess = found.len().saturating_sub(keep);
    for p in found.into_iter().take(excess) {
        let files = PathBuf::from(format!("{} Files", p.with_extension("").display()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(files);
    }
}

impl App {
    /// On a normal quit: delete the backups written this time (finishing any
    /// in progress first), and Backup folders left empty. Backups from a
    /// run that crashed are kept.
    pub fn clear_backups(&mut self) {
        if let Some(rx) = self.autosave.busy.take() {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(20));
        }
        let mut dirs = std::collections::BTreeSet::new();
        for p in std::mem::take(&mut self.autosave.written) {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
            if let Some(d) = p.parent() {
                dirs.insert(d.to_path_buf());
            }
        }
        for d in dirs {
            let empty = std::fs::read_dir(&d).map(|mut r| r.next().is_none()).unwrap_or(false);
            if empty && d.file_name().map(|n| n == "Backup").unwrap_or(false) {
                let _ = std::fs::remove_dir(&d);
            }
        }
        self.autosave.entries.clear();
        // Only backups put off with "Not Now" stay listed.
        let stale = self.autosave.stale.clone();
        write_manifest(&stale);
    }

    /// Reopen backups left after Audemo didn't close normally: each comes
    /// back with its original name (and file, so Save puts it back there),
    /// marked as unsaved.
    pub fn recover_backups(&mut self, list: Vec<Recovery>) {
        for r in &list {
            if r.session {
                let n = self.sessions.len();
                self.open_session(&r.backup);
                if self.sessions.len() > n {
                    let s = self.sessions.last_mut().unwrap();
                    s.path = r.original.clone();
                    s.name = r.name.clone();
                    s.dirty = true;
                }
            } else {
                self.load_async(r.backup.clone(), crate::app::LoadIntent::Recover { original: r.original.clone(), name: r.name.clone() });
            }
            // The old backup goes when Audemo next quits normally (its
            // session audio is kept until the session has been saved).
            self.autosave.written.push(r.backup.clone());
        }
        self.autosave.stale.retain(|s| !list.contains(s));
        let mut all = self.autosave.stale.clone();
        all.extend(list);
        write_manifest(&all);
    }

    /// Delete backups left by an earlier run.
    pub fn discard_backups(&mut self, list: Vec<Recovery>) {
        for r in &list {
            let _ = std::fs::remove_file(&r.backup);
            if r.session {
                let _ = std::fs::remove_dir_all(files_dir_of(&r.backup));
            }
        }
        self.autosave.stale.retain(|s| !list.contains(s));
        let mut all = self.autosave.stale.clone();
        all.extend(self.autosave.entries.values().cloned());
        write_manifest(&all);
    }

    /// Where a session's backups go: a Backup folder beside the session, or
    /// the centralized folder.
    fn backup_dir_for(&self, near: Option<&Path>) -> Option<PathBuf> {
        match (self.prefs.backup_location, near.and_then(|p| p.parent())) {
            (BackupLocation::WithSession, Some(dir)) => Some(dir.join("Backup")),
            _ => self.central_backup_dir(),
        }
    }

    /// Audio files are always backed up to the centralized folder, so no
    /// Backup folders appear beside your recordings.
    fn central_backup_dir(&self) -> Option<PathBuf> {
        self.prefs.backup_dir.clone().filter(|_| self.prefs.backup_location == BackupLocation::Folder).or_else(default_dir)
    }

    /// Called every frame: write backups when the interval has passed.
    pub fn autosave_tick(&mut self) {
        if let Some(rx) = &self.autosave.busy {
            match rx.try_recv() {
                Ok(errors) => {
                    self.autosave.busy = None;
                    if let Some(e) = errors.first() {
                        self.set_status(format!("Auto Save: {e}"));
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => self.autosave.busy = None,
            }
        }
        if !self.prefs.autosave {
            return;
        }
        let now = Instant::now();
        let last = *self.autosave.last.get_or_insert(now);
        if now.duration_since(last).as_secs() < self.prefs.autosave_minutes as u64 * 60 {
            return;
        }
        self.autosave.last = Some(now);
        self.write_backups();
    }

    /// Back up every session (and, if chosen, audio file) with unsaved changes.
    pub fn write_backups(&mut self) {
        let keep = self.prefs.autosave_max as usize;
        let t = stamp();
        let mut items = Vec::new();
        for s in self.sessions.iter().filter(|s| s.dirty) {
            let Some(dir) = self.backup_dir_for(s.path.as_deref()) else { continue };
            let name = crate::mt_ui::safe_file_name(&s.name);
            let prefix = format!("{name} [{}] session backup ", tag(s.path.as_deref(), s.id));
            let file = dir.join(format!("{prefix}{t}.{}", crate::session::SESSION_EXT));
            let files_dir = dir.join(format!("{prefix}{t} Files"));
            let mut paths: BTreeMap<u64, PathBuf> = BTreeMap::new();
            for (id, src) in &s.sources {
                let doc = src.doc_id.and_then(|d| self.docs.iter().find(|x| x.id == d));
                let on_disk = match doc {
                    Some(d) if !d.dirty => d.path.clone(),
                    Some(_) => None,
                    None => src.path.clone().filter(|p| p.is_file()),
                };
                match on_disk {
                    Some(p) => {
                        paths.insert(*id, p);
                    }
                    None if src.len() > 0 => {
                        // Unsaved audio goes beside the backup.
                        let p = files_dir.join(format!("{} {id}.wav", crate::mt_ui::safe_file_name(&src.name)));
                        items.push(Item::Audio { path: p.clone(), audio: crate::audio::Audio::from(src.audio.clone()), rate: s.sample_rate, meta: Default::default(), markers: Vec::new() });
                        paths.insert(*id, p);
                    }
                    None => {
                        if let Some(p) = src.path.clone() {
                            paths.insert(*id, p);
                        }
                    }
                }
            }
            let text = crate::session::to_text(s, &paths, Some(&dir));
            self.autosave.entries.insert(prefix.clone(), Recovery { session: true, backup: file.clone(), original: s.path.clone(), name: s.name.clone() });
            items.push(Item::Text { path: file, text });
            items.push(Item::Prune { dir, prefix, keep });
        }
        if self.prefs.backup_files {
            for d in self.docs.iter().filter(|d| d.dirty && d.len() > 0) {
                let Some(dir) = self.central_backup_dir() else { continue };
                let stem = Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
                let prefix = format!("{} [{}] backup ", crate::mt_ui::safe_file_name(&stem), tag(d.path.as_deref(), d.id));
                let markers = d.markers.clone();
                let file = dir.join(format!("{prefix}{t}.wav"));
                self.autosave.entries.insert(prefix.clone(), Recovery { session: false, backup: file.clone(), original: d.path.clone(), name: d.name.clone() });
                items.push(Item::Audio { path: file, audio: d.audio.clone(), rate: d.sample_rate, meta: d.meta.clone(), markers });
                items.push(Item::Prune { dir, prefix, keep });
            }
        }
        if items.is_empty() {
            return;
        }
        for it in &items {
            match it {
                Item::Text { path, .. } => self.autosave.written.push(path.clone()),
                Item::Audio { path, .. } => {
                    // Unsaved session audio lives in the backup's "Files" folder.
                    match path.parent().filter(|d| d.to_string_lossy().ends_with(" Files")) {
                        Some(d) => self.autosave.written.push(d.to_path_buf()),
                        None => self.autosave.written.push(path.clone()),
                    }
                }
                Item::Prune { .. } => {}
            }
        }
        self.autosave.written.sort();
        self.autosave.written.dedup();
        let mut list: Vec<Recovery> = self.autosave.stale.clone();
        list.extend(self.autosave.entries.values().cloned());
        write_manifest(&list);
        let (tx, rx) = std::sync::mpsc::channel();
        self.autosave.busy = Some(rx);
        std::thread::spawn(move || {
            let mut errors = Vec::new();
            let float = ExportSettings { container: Container::Wav, wav: WavFormat::Float32, dither: false, include_meta: true, ..Default::default() };
            for it in items {
                let r = match it {
                    Item::Text { path, text } => path.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&path, text)).map_err(|e| format!("{}: {e}", path.display())),
                    Item::Audio { path, audio, rate, meta, markers } => match path.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())) {
                        Ok(()) => export::save_audio(&path, &audio, rate, &float, &meta, &markers, &export::Progress::default()),
                        Err(e) => Err(format!("{}: {e}", path.display())),
                    },
                    Item::Prune { dir, prefix, keep } => {
                        prune(&dir, &prefix, keep);
                        Ok(())
                    }
                };
                if let Err(e) = r {
                    errors.push(e);
                }
            }
            let _ = tx.send(errors);
        });
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn manifest_round_trips() {
        let list = vec![
            Recovery { session: false, backup: PathBuf::from("/b/Take [abc123] backup 2026-10-15 101500.wav"), original: Some(PathBuf::from("/music/Take.wav")), name: "Take.wav".into() },
            Recovery { session: true, backup: PathBuf::from("/s/Backup/Song [def456] session backup 2026-10-15 101500.ases"), original: None, name: "Song\twith tab".into() },
        ];
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        let text: String = list.iter().map(|r| format!("{}\t{}\t{}\t{}\n", if r.session { "session" } else { "audio" }, r.backup.display(), r.original.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), clean(&r.name))).collect();
        let back = parse_manifest(&text);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], list[0]);
        assert_eq!((back[1].session, back[1].original.clone(), back[1].name.as_str()), (true, None, "Song with tab"));
        assert_eq!(files_dir_of(&list[1].backup), PathBuf::from("/s/Backup/Song [def456] session backup 2026-10-15 101500 Files"));
    }

    use super::*;

    #[test]
    fn stamps_sort_and_pruning_keeps_the_newest() {
        let s = stamp();
        assert_eq!(s.len(), 17);
        assert!(s.starts_with("20"));
        let dir = std::env::temp_dir().join(format!("audemo-autosave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for d in ["2026-01-01 000000", "2026-01-02 000000", "2026-01-03 000000"] {
            std::fs::write(dir.join(format!("Song backup {d}.ases")), "x").unwrap();
        }
        std::fs::create_dir_all(dir.join("Song backup 2026-01-01 000000 Files")).unwrap();
        std::fs::write(dir.join("Other backup 2026-01-01 000000.ases"), "x").unwrap();
        prune(&dir, "Song backup ", 2);
        assert!(!dir.join("Song backup 2026-01-01 000000.ases").exists());
        assert!(!dir.join("Song backup 2026-01-01 000000 Files").exists());
        assert!(dir.join("Song backup 2026-01-03 000000.ases").exists());
        assert!(dir.join("Other backup 2026-01-01 000000.ases").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
