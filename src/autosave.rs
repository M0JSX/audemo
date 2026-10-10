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
}

impl AutoSave {
    /// Start the interval again (after the settings change).
    pub fn reset(&mut self) {
        self.last = Some(Instant::now());
    }
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
    Audio { path: PathBuf, audio: crate::engine::Buffer, rate: u32, meta: export::Metadata, markers: Vec<(usize, String)> },
    /// Keep the newest `keep` backups in `dir` whose names start with `prefix`.
    Prune { dir: PathBuf, prefix: String, keep: usize },
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
    fn backup_dir_for(&self, near: Option<&Path>) -> Option<PathBuf> {
        match (self.prefs.backup_location, near.and_then(|p| p.parent())) {
            (BackupLocation::WithSession, Some(dir)) => Some(dir.join("Backup")),
            _ => self.prefs.backup_dir.clone().or_else(default_dir),
        }
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
            let prefix = format!("{name} backup ");
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
                        items.push(Item::Audio { path: p.clone(), audio: src.audio.clone(), rate: s.sample_rate, meta: Default::default(), markers: Vec::new() });
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
            items.push(Item::Text { path: file, text });
            items.push(Item::Prune { dir, prefix, keep });
        }
        if self.prefs.backup_files {
            for d in self.docs.iter().filter(|d| d.dirty && d.len() > 0) {
                let Some(dir) = self.backup_dir_for(d.path.as_deref()) else { continue };
                let stem = Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
                let prefix = format!("{} backup ", crate::mt_ui::safe_file_name(&stem));
                let markers = d.markers.iter().map(|m| (m.pos, m.name.clone())).collect();
                items.push(Item::Audio { path: dir.join(format!("{prefix}{t}.wav")), audio: d.audio.clone(), rate: d.sample_rate, meta: d.meta.clone(), markers });
                items.push(Item::Prune { dir, prefix, keep });
            }
        }
        if items.is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.autosave.busy = Some(rx);
        std::thread::spawn(move || {
            let mut errors = Vec::new();
            let float = ExportSettings { container: Container::Wav, wav: WavFormat::Float32, dither: false, include_meta: true, ..Default::default() };
            for it in items {
                let r = match it {
                    Item::Text { path, text } => path.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&path, text)).map_err(|e| format!("{}: {e}", path.display())),
                    Item::Audio { path, audio, rate, meta, markers } => match path.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())) {
                        Ok(()) => export::save(&path, &audio, rate, &float, &meta, &markers, &export::Progress::default()),
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
    use super::*;

    #[test]
    fn stamps_sort_and_pruning_keeps_the_newest() {
        let s = stamp();
        assert_eq!(s.len(), 17);
        assert!(s.starts_with("20"));
        let dir = std::env::temp_dir().join(format!("audemo-autosave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for d in ["2026-01-01 000000", "2026-01-02 000000", "2026-01-03 000000"] {
            std::fs::write(dir.join(format!("Song backup {d}.audemo")), "x").unwrap();
        }
        std::fs::create_dir_all(dir.join("Song backup 2026-01-01 000000 Files")).unwrap();
        std::fs::write(dir.join("Other backup 2026-01-01 000000.audemo"), "x").unwrap();
        prune(&dir, "Song backup ", 2);
        assert!(!dir.join("Song backup 2026-01-01 000000.audemo").exists());
        assert!(!dir.join("Song backup 2026-01-01 000000 Files").exists());
        assert!(dir.join("Song backup 2026-01-03 000000.audemo").exists());
        assert!(dir.join("Other backup 2026-01-01 000000.audemo").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
