//! Append-only history of the transformations `selara serve` generated, so a
//! result (or the text it replaced) can be copied back later from Settings.
//!
//! One JSON object per line in `history.jsonl` next to the config file. The
//! file holds selected text verbatim, so it is kept owner-only (0600) and
//! trimmed back to the newest [`MAX_ENTRIES`] rows as soon as it grows past
//! that many lines.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::commands::CommandKind;
use crate::error::CoreError;

/// Retention limit: the file never holds more than this many entries once an
/// append has finished, which is the last-50 limit Settings advertises.
pub const MAX_ENTRIES: usize = 50;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementOutcome {
    #[default]
    Applied,
    NotApplied,
    PasteUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unix seconds when the command finished.
    pub ts: u64,
    pub command_id: String,
    pub label: String,
    pub kind: CommandKind,
    /// App the selection came from, when Accessibility reported one.
    pub app: Option<String>,
    /// The captured selection (empty for a popup Insert below).
    pub original: String,
    /// What the model produced (and what was written back, for Replace).
    pub result: String,
    /// Older entries were only recorded after a verified replacement.
    #[serde(default)]
    pub outcome: ReplacementOutcome,
}

/// One logical append, retained across I/O retries. Identical completed runs
/// still get separate appends; only retries of this operation skip the write.
pub struct PendingEntry {
    entry: HistoryEntry,
    written: bool,
}

impl PendingEntry {
    pub fn new(entry: HistoryEntry) -> Self {
        Self {
            entry,
            written: false,
        }
    }

    pub fn save(&mut self, path: &Path) -> Result<(), CoreError> {
        if !self.written {
            let line = serde_json::to_string(&self.entry)
                .map_err(|e| CoreError::Config(format!("history entry: {e}")))?;
            let mut file = open_append(path).map_err(io_err)?;
            // Recover a partial record from an earlier failed write.
            if file.metadata().map_err(io_err)?.len() > 0 {
                file.seek(SeekFrom::End(-1)).map_err(io_err)?;
                let mut last = [0];
                file.read_exact(&mut last).map_err(io_err)?;
                if last[0] != b'\n' {
                    file.write_all(b"\n").map_err(io_err)?;
                }
            }
            file.write_all(line.as_bytes()).map_err(io_err)?;
            // A complete JSON record is readable even if its trailing newline
            // or retention fails. Retrying must not append it a second time.
            self.written = true;
            file.write_all(b"\n").map_err(io_err)?;
        }
        let lines = read_lines(path).map_err(io_err)?;
        if lines.len() > MAX_ENTRIES {
            rewrite(path, &lines[lines.len() - MAX_ENTRIES..]).map_err(io_err)?;
        }
        Ok(())
    }
}

/// `history.jsonl` beside the config file.
pub fn history_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("history.jsonl")
}

fn io_err(e: std::io::Error) -> CoreError {
    CoreError::Io(e)
}

fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    // `mode` only applies when the file is created. One that already exists —
    // restored from a backup, or made by hand — keeps whatever permissions it
    // came with, so tighten it before more selected text is appended.
    crate::config::restrict_to_owner(path);
    Ok(file)
}

/// Write the whole file at once with owner-only permissions, via a sibling
/// temp file so a reader never sees a half-written history.
fn rewrite(path: &Path, lines: &[String]) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "history.jsonl".into());
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut file = opts.open(&tmp)?;
        for line in lines {
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        #[cfg(windows)]
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Raw lines of the file, oldest first; blank lines dropped. Missing file = empty.
fn read_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for line in std::io::BufReader::new(file).split(b'\n') {
        // A short write may end inside a UTF-8 character. Skip that damaged
        // record just like malformed JSON rather than hiding later results.
        let Ok(line) = String::from_utf8(line?) else {
            continue;
        };
        if !line.trim().is_empty() {
            out.push(line);
        }
    }
    Ok(out)
}

/// Append one entry as a JSON line, then trim the file back to the newest
/// [`MAX_ENTRIES`] as soon as it holds more than that.
pub fn append(path: &Path, entry: &HistoryEntry) -> Result<(), CoreError> {
    PendingEntry::new(entry.clone()).save(path)
}

/// Every readable entry, newest first. Lines that fail to parse are skipped
/// so one bad write never hides the rest.
pub fn load(path: &Path) -> Result<Vec<HistoryEntry>, CoreError> {
    let lines = read_lines(path).map_err(io_err)?;
    let mut entries: Vec<HistoryEntry> = lines
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    Ok(entries)
}

/// Remove the history file. A missing file is not an error.
pub fn clear(path: &Path) -> Result<(), CoreError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(i: u64) -> HistoryEntry {
        HistoryEntry {
            ts: 1_700_000_000 + i,
            command_id: format!("cmd-{i}"),
            label: format!("Label {i}"),
            kind: if i.is_multiple_of(2) {
                CommandKind::Replace
            } else {
                CommandKind::Popup
            },
            app: Some("Notes".into()),
            original: format!("orig {i}"),
            result: format!("result {i}"),
            outcome: ReplacementOutcome::Applied,
        }
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir() -> TempDir {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("selara-history-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    #[test]
    fn outcome_round_trips_and_old_entries_default_to_applied() {
        let mut saved = entry(1);
        let mut old = serde_json::to_value(&saved).unwrap();
        old.as_object_mut().unwrap().remove("outcome");
        assert_eq!(serde_json::from_value::<HistoryEntry>(old).unwrap(), saved);
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        for outcome in [
            ReplacementOutcome::NotApplied,
            ReplacementOutcome::PasteUnverified,
        ] {
            saved.outcome = outcome;
            append(&path, &saved).unwrap();
            assert_eq!(load(&path).unwrap()[0], saved);
        }
    }

    #[test]
    fn history_path_is_sibling_of_config() {
        assert_eq!(
            history_path(Path::new("/home/u/.config/selara/config.toml")),
            PathBuf::from("/home/u/.config/selara/history.jsonl")
        );
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn append_then_load_newest_first() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        for i in 0..3 {
            append(&path, &entry(i)).unwrap();
        }
        let got = load(&path).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], entry(2));
        assert_eq!(got[2], entry(0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "history must be owner-only, got {mode:o}");
        }
    }

    #[test]
    fn identical_runs_are_distinct_but_retries_do_not_duplicate_them() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        let saved = entry(1);
        let mut first = PendingEntry::new(saved.clone());
        first.save(&path).unwrap();
        first.save(&path).unwrap();
        let mut second = PendingEntry::new(saved.clone());
        second.save(&path).unwrap();
        second.save(&path).unwrap();
        assert_eq!(load(&path).unwrap(), vec![saved.clone(), saved]);
    }

    #[test]
    fn retention_failure_retries_without_appending_a_second_record() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        for i in 0..MAX_ENTRIES {
            append(&path, &entry(i as u64)).unwrap();
        }
        // A directory at the retention staging path makes its open fail after
        // the new record has been appended, without relying on disk capacity.
        let staging = path.with_file_name(format!(".history.jsonl.tmp-{}", std::process::id()));
        std::fs::create_dir(&staging).unwrap();
        let saved = entry(MAX_ENTRIES as u64);
        let mut pending = PendingEntry::new(saved.clone());
        assert!(pending.save(&path).is_err());
        assert_eq!(load(&path).unwrap().len(), MAX_ENTRIES + 1);
        std::fs::remove_dir(staging).unwrap();
        pending.save(&path).unwrap();
        let got = load(&path).unwrap();
        assert_eq!(got.len(), MAX_ENTRIES);
        assert_eq!(got.iter().filter(|e| **e == saved).count(), 1);
        assert_eq!(got.last(), Some(&entry(1)));
    }

    #[test]
    fn append_never_keeps_more_than_the_advertised_limit() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        let max = MAX_ENTRIES as u64;
        for i in 0..max {
            append(&path, &entry(i)).unwrap();
        }
        assert_eq!(
            load(&path).unwrap().len(),
            MAX_ENTRIES,
            "no trim at exactly the limit"
        );
        // Every append past the limit trims: the file must stay at 50 rather
        // than drift up to a looser threshold before the first trim.
        for i in max..max + 12 {
            append(&path, &entry(i)).unwrap();
            let got = load(&path).unwrap();
            assert_eq!(got.len(), MAX_ENTRIES, "after entry {i}");
            assert_eq!(got[0], entry(i));
            assert_eq!(got[MAX_ENTRIES - 1], entry(i + 1 - max));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn append_tightens_a_preexisting_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        // A restored backup or a hand-made file: it already exists, so the
        // create-time 0600 never applies to it.
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        append(&path, &entry(1)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "history must be owner-only, got {mode:o}");
        assert_eq!(load(&path).unwrap(), vec![entry(1)]);
    }

    #[test]
    fn clear_removes_file_and_tolerates_missing() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        append(&path, &entry(1)).unwrap();
        clear(&path).unwrap();
        assert!(!path.exists());
        clear(&path).unwrap();
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn retry_after_partial_write_preserves_the_completed_result() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        append(&path, &entry(1)).unwrap();
        open_append(&path)
            .unwrap()
            .write_all(b"{\"result\":\"\xe2")
            .unwrap();
        let mut recovered = entry(2);
        recovered.outcome = ReplacementOutcome::NotApplied;
        append(&path, &recovered).unwrap();
        assert_eq!(load(&path).unwrap(), vec![recovered, entry(1)]);
    }

    #[test]
    fn corrupt_line_is_skipped() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        append(&path, &entry(1)).unwrap();
        {
            let mut f = open_append(&path).unwrap();
            f.write_all(b"{not json\n\n").unwrap();
        }
        append(&path, &entry(2)).unwrap();
        let got = load(&path).unwrap();
        assert_eq!(got, vec![entry(2), entry(1)]);
    }
}
