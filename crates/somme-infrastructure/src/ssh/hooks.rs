//! Loads Somme's shell hooks into bash or zsh on a server, so an SSH terminal
//! gets command blocks like a local one. A side channel writes the hook files
//! to a private folder there, then the terminal starts the shell through them.
//! Anything unexpected leaves the plain login shell.

use std::time::Duration;

use russh::ChannelMsg;

use crate::ssh::connect::Connection;

const ZSHENV: &str = include_str!("../../shell/zsh/zshenv");
const ZSH_HOOKS: &str = include_str!("../../shell/zsh/somme.zsh");
const BASH_HOOKS: &str = include_str!("../../shell/bash/somme.bash");
const START: &str = include_str!("../../shell/remote.sh");

/// The side channel gets this long before the shell starts without hooks.
const SETUP_WAIT: Duration = Duration::from_secs(5);
const MAX_REPLY: usize = 64 * 1024;
/// Printed by the setup script, followed by the folder it made.
const READY: &str = "somme-hooks:";
const END_OF_FILE: &str = "SOMME_END_OF_FILE";

/// The command that starts the shell with the hooks, or None to start it
/// plainly. `token` is the marks' secret.
pub async fn prepare(conn: &Connection, token: &str) -> Option<String> {
    // Exec requests on network gear run its own CLI, not a POSIX shell.
    let server = conn.server_id()?.to_ascii_lowercase();
    if !(server.contains("openssh") || server.contains("dropbear")) {
        return None;
    }
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let dir = tokio::time::timeout(SETUP_WAIT, setup(conn, token)).await.ok()??;
    Some(format!("exec /bin/sh '{dir}/start'"))
}

/// Runs the setup script on its own channel. The script goes in on stdin, so
/// the secret never shows in the server's process list.
async fn setup(conn: &Connection, token: &str) -> Option<String> {
    let mut channel = conn.target().channel_open_session().await.ok()?;
    channel.exec(true, "/bin/sh -s").await.ok()?;
    channel.data(setup_script(token).as_bytes()).await.ok()?;
    channel.eof().await.ok()?;
    let mut out = Vec::new();
    let mut dir = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                out.extend_from_slice(&data);
                dir = ready_dir(&out);
                if dir.is_some() || out.len() > MAX_REPLY {
                    break;
                }
            }
            ChannelMsg::Failure | ChannelMsg::Close => break,
            _ => {}
        }
    }
    let _ = channel.close().await;
    dir
}

/// Writes the hooks to a new folder only its owner can read, then prints the
/// folder. Shells other than bash and zsh get nothing.
fn setup_script(token: &str) -> String {
    let mut script = String::from(
        "case ${SHELL##*/} in zsh|bash) ;; *) exit 0 ;; esac\n\
         umask 077\n\
         d=$(mktemp -d \"${TMPDIR:-/tmp}/somme.XXXXXXXX\") || exit 0\n\
         case $d in /*) ;; *) exit 0 ;; esac\n\
         case $d in *[!A-Za-z0-9/._-]*) rm -rf \"$d\"; exit 0 ;; esac\n",
    );
    for (name, text) in [(".zshenv", ZSHENV), ("somme.zsh", ZSH_HOOKS), ("somme.bash", BASH_HOOKS), ("start", START)] {
        script.push_str(&format!("cat > \"$d/{name}\" <<'{END_OF_FILE}'\n{}\n{END_OF_FILE}\n", text.trim_end()));
    }
    script.push_str(&format!("printf '%s\\n' '{token}' > \"$d/token\"\n"));
    script.push_str(&format!("printf '{READY}%s\\n' \"$d\"\n"));
    script
}

/// The folder from a finished `somme-hooks:` line, when it is safe to quote.
fn ready_dir(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(out);
    let (complete, _) = text.rsplit_once('\n')?;
    complete
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix(READY))
        .find(|dir| {
            dir.starts_with('/')
                && dir.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_folder_past_login_noise() {
        assert_eq!(ready_dir(b"Welcome!\nsomme-hooks:/tmp/somme.Ab12\n"), Some("/tmp/somme.Ab12".into()));
        assert_eq!(ready_dir(b"somme-hooks:/tmp/somme.Ab12"), None, "line not finished yet");
        assert_eq!(ready_dir(b"somme-hooks:/tmp/it's\n"), None);
        assert_eq!(ready_dir(b"somme-hooks:relative\n"), None);
    }

    #[test]
    fn every_hook_file_survives_the_here_document() {
        let script = setup_script("abc123");
        for text in [ZSHENV, ZSH_HOOKS, BASH_HOOKS, START] {
            assert!(!text.contains(END_OF_FILE));
            assert!(script.contains(text.trim_end()));
        }
        assert!(script.contains("printf '%s\\n' 'abc123' > \"$d/token\""));
    }

    /// Runs the setup script and the start script with the local /bin/sh,
    /// the way a server would.
    #[test]
    fn setup_writes_a_private_folder_the_shell_starts_from() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::{Command, Stdio};

        let tmp = tempfile::tempdir().unwrap();
        let mut child = Command::new("/bin/sh")
            .arg("-s")
            .env("SHELL", "/bin/bash")
            .env("TMPDIR", tmp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(setup_script("t0k3n").as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let dir = ready_dir(&out.stdout).expect("folder line");
        let dir = std::path::Path::new(&dir);
        assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read_to_string(dir.join("token")).unwrap(), "t0k3n\n");
        assert_eq!(std::fs::read_to_string(dir.join("somme.bash")).unwrap(), format!("{}\n", BASH_HOOKS.trim_end()));

        // bash starts through the folder, marks its prompts, and removes the folder.
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new("/bin/sh")
            .arg(dir.join("start"))
            .env_clear()
            .env("SHELL", "/bin/bash")
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .current_dir(home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"echo hi\nexit\n").unwrap();
        let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
        assert!(out.contains("\x1b]6973;t0k3n;A;0;"), "{out:?}");
        assert!(out.contains("\x1b]6973;t0k3n;C;echo hi\x07hi\n"), "{out:?}");
        assert!(!dir.exists());

        // A shell other than bash or zsh gets nothing written.
        let out = Command::new("/bin/sh")
            .arg("-s")
            .env("SHELL", "/usr/bin/fish")
            .env("TMPDIR", tmp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child.stdin.take().unwrap().write_all(setup_script("t0k3n").as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(ready_dir(&out.stdout).is_none());
    }
}
