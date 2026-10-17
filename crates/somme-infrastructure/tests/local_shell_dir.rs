//! A local shell starts in the folder it is given and reports where it is.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use somme_application::ports::{OutputSink, PtyGateway, ShellOptions, TermSize, Terminal};
use somme_infrastructure::pty::NativePtyGateway;

const SIZE: TermSize = TermSize { cols: 80, rows: 24 };

struct Discard;

impl OutputSink for Discard {
    fn data(&self, _: &[u8]) {}
    fn closed(&self, _: Option<String>) {}
}

async fn reaches(terminal: &dyn Terminal, want: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if terminal.cwd().as_deref() == Some(want) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[tokio::test]
async fn shell_starts_in_the_given_folder_and_follows_cd() {
    let (start, next) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (start, next) = (real(start.path()), real(next.path()));
    let options = ShellOptions { dir: Some(&start), marks: None };
    let terminal = NativePtyGateway.spawn(SIZE, options, Arc::new(Discard)).unwrap();
    assert!(reaches(&*terminal, &start).await, "got {:?}", terminal.cwd());
    terminal.write(format!("cd '{}'\n", next.display()).as_bytes()).await.unwrap();
    assert!(reaches(&*terminal, &next).await, "got {:?}", terminal.cwd());
    terminal.close().await;
}

#[tokio::test]
async fn missing_folder_starts_at_home() {
    let home = real(&dirs::home_dir().unwrap());
    let terminal = NativePtyGateway
        .spawn(SIZE, ShellOptions { dir: Some(Path::new("/no/such/folder")), marks: None }, Arc::new(Discard))
        .unwrap();
    assert!(reaches(&*terminal, &home).await, "got {:?}", terminal.cwd());
    terminal.close().await;
}
