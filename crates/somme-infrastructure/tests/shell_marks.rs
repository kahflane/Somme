//! zsh started with the hooks marks each prompt and command.

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use somme_application::ports::{OutputSink, PtyGateway, ShellOptions, TermSize};
use somme_infrastructure::pty::NativePtyGateway;

#[derive(Default)]
struct Collect(Mutex<Vec<u8>>);

impl OutputSink for Collect {
    fn data(&self, bytes: &[u8]) {
        self.0.lock().unwrap().extend_from_slice(bytes);
    }
    fn closed(&self, _: Option<String>) {}
}

impl Collect {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    async fn wait_for(&self, needle: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }
}

/// Set in the child run, which starts zsh with an empty config folder.
const CHILD: &str = "SOMME_MARKS_TEST_HOME";

#[tokio::test]
async fn zsh_marks_prompts_and_commands() {
    let zsh = Path::new("/bin/zsh");
    if !zsh.exists() {
        return;
    }
    let Some(home) = std::env::var_os(CHILD) else {
        // Run again in a child process, so the user's own zsh setup stays out.
        let home = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "zsh_marks_prompts_and_commands", "--nocapture"])
            .env(CHILD, home.path())
            .env("ZDOTDIR", home.path())
            .env("SHELL", zsh)
            .status()
            .unwrap();
        assert!(status.success());
        return;
    };
    let home = Path::new(&home);
    let out = Arc::new(Collect::default());
    let options = ShellOptions { dir: Some(home), marks: Some("t0k3n") };
    let size = TermSize { cols: 80, rows: 24 };
    let terminal = NativePtyGateway.spawn(size, options, out.clone()).unwrap();

    assert!(out.wait_for("\x1b]6973;t0k3n;A;0;").await, "no first prompt mark: {:?}", out.text());
    terminal.write(b"echo 'a%b'\r").await.unwrap();
    assert!(out.wait_for("a%b\r\n\x1b]6973;t0k3n;A;0;").await, "no mark after the command: {:?}", out.text());

    let text = out.text();
    let accepted = text.find("\x1b]6973;t0k3n;B\x07").expect("line accepted mark");
    let started = text.find("\x1b]6973;t0k3n;C;echo 'a%25b'\x07").expect("command mark with encoded text");
    assert!(accepted < started);
    // Programs started from the shell cannot read the secret.
    terminal.write(b"echo token=${SOMME_MARK_TOKEN-none}\r").await.unwrap();
    assert!(out.wait_for("token=none\r\n").await, "secret leaked: {:?}", out.text());

    let folder = home.canonicalize().unwrap();
    assert!(text.contains(&format!("A;0;{}\x07", folder.display())), "{text:?}");
    terminal.close().await;
}
