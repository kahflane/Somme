//! Scratch files: editor tabs not saved anywhere yet. Their text is kept in the
//! app data folder, so it survives a restart, until it is saved to a real path.
//! Also the list of editor files to open again on the next start.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const LIST_FILE: &str = "open-files.json";

/// An editor file open when the app last ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenEntry {
    pub path: PathBuf,
    pub scratch: bool,
}

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("scratch")
}

/// Makes a new empty scratch file, named after the first free number.
pub fn create(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for n in 1.. {
        let path = dir.join(format!("untitled-{n}.txt"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    unreachable!("ran out of numbers")
}

pub fn load_list(data_dir: &Path) -> Vec<OpenEntry> {
    std::fs::read_to_string(data_dir.join(LIST_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_list(data_dir: &Path, entries: &[OpenEntry]) {
    let result = serde_json::to_string_pretty(entries)
        .map_err(std::io::Error::other)
        .and_then(|raw| std::fs::write(data_dir.join(LIST_FILE), raw));
    if let Err(err) = result {
        tracing::warn!(%err, "could not save the open files");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_files_take_the_first_free_number_and_the_list_round_trips() {
        let data = std::env::temp_dir().join(format!("somme-scratch-{}", std::process::id()));
        let dir = dir(&data);
        let first = create(&dir).unwrap();
        let second = create(&dir).unwrap();
        std::fs::remove_file(&first).unwrap();
        let third = create(&dir).unwrap();
        let entries = vec![
            OpenEntry { path: second.clone(), scratch: true },
            OpenEntry { path: "/notes/todo.md".into(), scratch: false },
        ];
        save_list(&data, &entries);
        let loaded = load_list(&data);
        std::fs::remove_dir_all(&data).unwrap();
        assert_eq!(second.file_name().unwrap(), "untitled-2.txt");
        assert_eq!(third, first);
        assert_eq!(loaded, entries);
    }
}
