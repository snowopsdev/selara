//! Append-only history of the transformations `selara serve` applied, so a
//! result (or the text it replaced) can be copied back later from Settings.
//!
//! One JSON object per line in `history.jsonl` next to the config file. The
//! file holds selected text verbatim, so it is created owner-only (0600) and
//! trimmed to the newest [`MAX_ENTRIES`] rows once it grows past
//! [`TRIM_ABOVE`] lines.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use crate::commands::CommandKind;
use crate::error::CoreError;

/// Entries kept after a trim.
pub const MAX_ENTRIES: usize = 50;
/// Line count above which `append` trims the file back to `MAX_ENTRIES`.
pub const TRIM_ABOVE: usize = 60;

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
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
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
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(line);
        }
    }
    Ok(out)
}

/// Append one entry as a JSON line, then trim the file to the newest
/// [`MAX_ENTRIES`] once it exceeds [`TRIM_ABOVE`] lines.
pub fn append(path: &Path, entry: &HistoryEntry) -> Result<(), CoreError> {
    let line = serde_json::to_string(entry)
        .map_err(|e| CoreError::Config(format!("history entry: {e}")))?;
    {
        let mut file = open_append(path).map_err(io_err)?;
        file.write_all(line.as_bytes()).map_err(io_err)?;
        file.write_all(b"\n").map_err(io_err)?;
    }
    let lines = read_lines(path).map_err(io_err)?;
    if lines.len() > TRIM_ABOVE {
        let keep = &lines[lines.len() - MAX_ENTRIES..];
        rewrite(path, keep).map_err(io_err)?;
    }
    Ok(())
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
    fn append_trims_to_newest_fifty_above_sixty() {
        let dir = temp_dir();
        let path = dir.path().join("history.jsonl");
        for i in 0..TRIM_ABOVE as u64 {
            append(&path, &entry(i)).unwrap();
        }
        assert_eq!(
            load(&path).unwrap().len(),
            TRIM_ABOVE,
            "no trim at exactly the threshold"
        );
        append(&path, &entry(TRIM_ABOVE as u64)).unwrap();
        let got = load(&path).unwrap();
        assert_eq!(got.len(), MAX_ENTRIES);
        assert_eq!(got[0], entry(TRIM_ABOVE as u64));
        assert_eq!(
            got[MAX_ENTRIES - 1],
            entry((TRIM_ABOVE + 1 - MAX_ENTRIES) as u64)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
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
