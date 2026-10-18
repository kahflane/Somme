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
/// the system knows the kind, and marked as downloaded.
pub async fn local_copy(disk: Disk, path: PathBuf) -> Result<PathBuf, DiskError> {
    if disk.is_local() {
        return Ok(path);
    }
    disk.run(move |disk| {
        let name = path.file_name().ok_or_else(|| DiskError::Other("It has no file name.".into()))?;
        // A new folder with a random name that only this user can open, so no
        // one else on this computer can put something in its place. It stays
        // for the app that opens the file.
        let mut folder = tempfile::Builder::new();
        folder.prefix("somme-open-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            folder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = folder.tempdir()?.keep();
        let local = dir.join(name);
        disk.download(&path, &local)?;
        mark_downloaded(&local);
        Ok(local)
    })
    .await
}

/// Marks a file the way browsers mark downloads, so the system treats it as
/// from the internet: macOS checks apps and scripts inside it before they
/// run, and Office on Windows opens it in Protected View.
#[cfg(target_os = "macos")]
fn mark_downloaded(path: &Path) {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs());
    let mark = format!("0081;{stamp:x};Somme;");
    let set = std::process::Command::new("/usr/bin/xattr")
        .args(["-w", "com.apple.quarantine", &mark])
        .arg(path)
        .status();
    if !set.is_ok_and(|status| status.success()) {
        tracing::warn!("could not mark a downloaded file");
    }
}

#[cfg(windows)]
fn mark_downloaded(path: &Path) {
    let mut zone = path.as_os_str().to_owned();
    zone.push(":Zone.Identifier");
    if let Err(err) = std::fs::write(zone, "[ZoneTransfer]\r\nZoneId=3\r\n") {
        tracing::warn!(%err, "could not mark a downloaded file");
    }
}

/// Other systems have no such mark.
#[cfg(not(any(target_os = "macos", windows)))]
fn mark_downloaded(_: &Path) {}

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
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::metadata(local.parent().unwrap()).unwrap().permissions().mode() & 0o777
        };
        #[cfg(target_os = "macos")]
        let mark = std::process::Command::new("/usr/bin/xattr").args(["-p", "com.apple.quarantine"]).arg(&local).output();
        std::fs::remove_dir_all(local.parent().unwrap()).unwrap();
        assert_eq!(local.file_name(), Some("photo.png".as_ref()));
        assert_eq!(bytes.unwrap(), b"png");
        #[cfg(unix)]
        assert_eq!(mode, 0o700, "only this user can open the folder");
        #[cfg(target_os = "macos")]
        assert!(String::from_utf8_lossy(&mark.unwrap().stdout).contains(";Somme;"));
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
