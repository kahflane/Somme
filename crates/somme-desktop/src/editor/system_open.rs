//! Files the editor can't show, like images and PDFs, open in the app the
//! system picks for them. Files on a server are downloaded first.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::terminal::disk::{Disk, DiskError};

/// Kinds of files that open in another app. Programs, scripts and installers
/// are left out on purpose: the system would run them.
const OUTSIDE: &[&str] = &[
    // Images
    "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "avif", "ico", "icns", "psd",
    // Documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "pages", "numbers", "key", "epub",
    // Audio and video
    "mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "mp4", "m4v", "mov", "mkv", "avi", "webm",
    // Archives
    "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar",
    // Fonts
    "ttf", "otf", "woff", "woff2",
];

/// Whether `path` opens in the system's app instead of the editor.
pub fn opens_outside(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| OUTSIDE.iter().any(|known| known.eq_ignore_ascii_case(ext)))
}

/// A copy of `path` on this computer, for the system's app to open. Files on
/// a server are downloaded to a new temporary folder, keeping their name so
/// the system knows the kind.
pub async fn local_copy(disk: Disk, path: PathBuf) -> Result<PathBuf, DiskError> {
    if disk.is_local() {
        return Ok(path);
    }
    disk.run(move |disk| {
        let name = path.file_name().ok_or_else(|| DiskError::Other("It has no file name.".into()))?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
        let dir = std::env::temp_dir().join("somme-open").join(stamp.to_string());
        std::fs::create_dir_all(&dir)?;
        let local = dir.join(name);
        disk.download(&path, &local)?;
        Ok(local)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_on_a_server_are_downloaded_first() {
        use crate::terminal::disk::fake;

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("home/me")).unwrap();
        std::fs::write(root.path().join("home/me/photo.png"), b"png").unwrap();
        let disk = fake::disk(root.path());
        let local = futures::executor::block_on(local_copy(disk, PathBuf::from("/home/me/photo.png"))).unwrap();
        let bytes = std::fs::read(&local);
        std::fs::remove_dir_all(local.parent().unwrap()).unwrap();
        assert_eq!(local.file_name(), Some("photo.png".as_ref()));
        assert_eq!(bytes.unwrap(), b"png");
    }

    #[test]
    fn documents_open_outside_but_programs_and_text_do_not() {
        assert!(opens_outside(Path::new("/tmp/Photo.JPG")));
        assert!(opens_outside(Path::new("report.pdf")));
        assert!(!opens_outside(Path::new("setup.exe")));
        assert!(!opens_outside(Path::new("run.sh")));
        assert!(!opens_outside(Path::new("notes.txt")));
        assert!(!opens_outside(Path::new("Makefile")));
    }
}
