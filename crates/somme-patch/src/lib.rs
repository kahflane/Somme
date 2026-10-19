// Changed by Somme from OpenAI Codex (codex-rs/apply-patch). Apache-2.0;
// see LICENSE and NOTICE in this crate.

//! Reads and applies patches in the "*** Begin Patch" format that coding
//! models know. Files are read and written through [`Files`], so the same
//! patch can change files on this computer or on a server.

mod file_update;
mod line_parser;
mod parser;
mod seek_sequence;
mod text_file;

use std::io;
use std::path::PathBuf;

use async_trait::async_trait;
pub use parser::Hunk;
pub use parser::ParseError;
pub use parser::UpdateFileChunk;
pub use parser::parse_patch;
use thiserror::Error;

/// Where a patch reads and writes files. Paths are as the patch wrote them:
/// relative ones are for the files to place.
#[async_trait]
pub trait Files: Send + Sync {
    /// Reads a text file. The error kind is `NotFound` when it is missing.
    async fn read(&self, path: &str) -> io::Result<String>;
    /// Writes a file, making missing folders first.
    async fn write(&self, path: &str, contents: &str) -> io::Result<()>;
    /// Removes a file. Fails for a folder.
    async fn remove(&self, path: &str) -> io::Result<()>;
}

/// The files a patch changed, as the patch wrote their paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

impl Applied {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// In git's short style, one file per line.
    pub fn summary(&self) -> String {
        let mut out = String::from("Success. Updated the following files:");
        for (mark, paths) in [("A", &self.added), ("M", &self.modified), ("D", &self.deleted)] {
            for path in paths {
                out.push_str(&format!("\n{mark} {path}"));
            }
        }
        out
    }
}

/// A patch that could not be applied, with the files it changed before it
/// stopped.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct Failed {
    pub message: String,
    pub done: Applied,
}

impl Failed {
    fn new(message: impl Into<String>, done: &Applied) -> Self {
        Self { message: message.into(), done: done.clone() }
    }
}

/// Parses the patch and applies it, one hunk after another. A hunk that fails
/// stops the patch; the hunks before it stay applied.
pub async fn apply(patch: &str, files: &dyn Files) -> Result<Applied, Failed> {
    let hunks = parse_patch(patch).map_err(|error| {
        let message = match error {
            ParseError::InvalidPatchError(message) => format!("Invalid patch: {message}"),
            ParseError::InvalidHunkError { message, line_number } => format!("Invalid patch hunk on line {line_number}: {message}"),
        };
        Failed::new(message, &Applied::default())
    })?;
    apply_hunks(&hunks, files).await
}

/// Applies parsed hunks. See [`apply`].
pub async fn apply_hunks(hunks: &[Hunk], files: &dyn Files) -> Result<Applied, Failed> {
    if hunks.is_empty() {
        return Err(Failed::new("No files were modified.", &Applied::default()));
    }
    let mut done = Applied::default();
    for hunk in hunks {
        match hunk {
            Hunk::AddFile { path, contents } => {
                files.write(path, contents).await.map_err(|error| Failed::new(format!("Failed to write file {path}: {error}"), &done))?;
                done.added.push(path.clone());
            }
            Hunk::DeleteFile { path } => {
                files.remove(path).await.map_err(|error| Failed::new(format!("Failed to delete file {path}: {error}"), &done))?;
                done.deleted.push(path.clone());
            }
            Hunk::UpdateFile { path, move_path, chunks } => {
                let original = files.read(path).await.map_err(|error| Failed::new(format!("Failed to read file to update {path}: {error}"), &done))?;
                let new_contents = file_update::new_contents(&original, path, chunks).map_err(|message| Failed::new(message, &done))?;
                match move_path {
                    Some(dest) => {
                        files.write(dest, &new_contents).await.map_err(|error| Failed::new(format!("Failed to write file {dest}: {error}"), &done))?;
                        files.remove(path).await.map_err(|error| Failed::new(format!("Failed to remove original {path}: {error}"), &done))?;
                        done.modified.push(dest.clone());
                    }
                    None => {
                        files.write(path, &new_contents).await.map_err(|error| Failed::new(format!("Failed to write file {path}: {error}"), &done))?;
                        done.modified.push(path.clone());
                    }
                }
            }
        }
    }
    Ok(done)
}

/// Files on this computer. Relative paths are inside `root`.
pub struct LocalFiles {
    pub root: PathBuf,
}

impl LocalFiles {
    pub fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

#[async_trait]
impl Files for LocalFiles {
    async fn read(&self, path: &str) -> io::Result<String> {
        std::fs::read_to_string(self.path(path))
    }

    async fn write(&self, path: &str, contents: &str) -> io::Result<()> {
        let path = self.path(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    }

    async fn remove(&self, path: &str) -> io::Result<()> {
        let path = self.path(path);
        if std::fs::metadata(&path)?.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "path is a directory"));
        }
        std::fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    #[tokio::test]
    async fn adds_updates_moves_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let files = LocalFiles { root: dir.path().to_path_buf() };
        std::fs::write(dir.path().join("old.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.path().join("gone.txt"), "bye\n").unwrap();
        let patch = wrap_patch("*** Add File: new/add.txt\n+ab\n+cd\n*** Update File: old.txt\n*** Move to: moved.txt\n@@\n one\n-two\n+TWO\n*** Delete File: gone.txt");
        let applied = apply(&patch, &files).await.unwrap();
        assert_eq!(applied.summary(), "Success. Updated the following files:\nA new/add.txt\nM moved.txt\nD gone.txt");
        assert_eq!(std::fs::read_to_string(dir.path().join("new/add.txt")).unwrap(), "ab\ncd\n");
        assert_eq!(std::fs::read_to_string(dir.path().join("moved.txt")).unwrap(), "one\nTWO\n");
        assert!(!dir.path().join("old.txt").exists());
        assert!(!dir.path().join("gone.txt").exists());
    }

    #[tokio::test]
    async fn a_failed_hunk_says_what_was_done_before() {
        let dir = tempfile::tempdir().unwrap();
        let files = LocalFiles { root: dir.path().to_path_buf() };
        let patch = wrap_patch("*** Add File: made.txt\n+hi\n*** Update File: missing.txt\n@@\n-a\n+b");
        let failed = apply(&patch, &files).await.unwrap_err();
        assert!(failed.message.starts_with("Failed to read file to update missing.txt"), "{}", failed.message);
        assert_eq!(failed.done.added, vec!["made.txt".to_string()]);
    }

    #[tokio::test]
    async fn bad_patches_are_refused_before_anything_changes() {
        let dir = tempfile::tempdir().unwrap();
        let files = LocalFiles { root: dir.path().to_path_buf() };
        assert_eq!(apply("nope", &files).await.unwrap_err().message, "Invalid patch: The first line of the patch must be '*** Begin Patch'");
        assert_eq!(apply("*** Begin Patch\n*** End Patch", &files).await.unwrap_err().message, "No files were modified.");
    }
}
