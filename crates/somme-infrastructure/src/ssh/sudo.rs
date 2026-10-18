//! Reads and saves files as root with sudo, for files the login user may not
//! touch. Each call runs a small script on its own channel. The script takes
//! the sudo password, when there is one, as its first line of input and hands
//! only that line to sudo, so a wrong password cannot eat the file's bytes.

use russh::ChannelMsg;
use somme_application::ports::GatewayError;
use zeroize::Zeroizing;

use crate::ssh::connect::Connection;
use crate::ssh::{connection_error, session_error};

/// `$1` is the file, and the input is the password line.
const READ: &str = r#"IFS= read -r p
if [ -n "$p" ]; then
  printf '%s\n' "$p" | LC_ALL=C sudo -S -p '' -- cat -- "$1"
else
  LC_ALL=C sudo -n -- cat -- "$1"
fi"#;

/// `$1` is the file. The input is the password line, then the new contents,
/// which wait in a private file while sudo reads the password.
const WRITE: &str = r#"IFS= read -r p
t=$(mktemp "${TMPDIR:-/tmp}/somme.XXXXXXXX") || exit 70
if ! cat > "$t"; then rm -f -- "$t"; exit 70; fi
if [ -n "$p" ]; then
  printf '%s\n' "$p" | LC_ALL=C sudo -S -p '' -- sh -c 'cat -- "$1" > "$2"' sh "$t" "$1"
else
  LC_ALL=C sudo -n -- sh -c 'cat -- "$1" > "$2"' sh "$t" "$1"
fi
s=$?
rm -f -- "$t"
exit $s"#;

/// The script could not make its temporary file.
const NO_TEMP_FILE: u32 = 70;
const MAX_ERROR: usize = 16 * 1024;

#[derive(Default)]
struct Output {
    status: Option<u32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub async fn read(conn: &Connection, path: &str, limit: u64, password: Option<&str>) -> Result<Vec<u8>, GatewayError> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let out = run(conn, READ, path, password, &[], limit).await?;
    match out.status {
        Some(0) => Ok(out.stdout),
        _ => Err(failure(&out)),
    }
}

pub async fn write(conn: &Connection, path: &str, data: &[u8], password: Option<&str>) -> Result<(), GatewayError> {
    let out = run(conn, WRITE, path, password, data, 0).await?;
    match out.status {
        Some(0) => Ok(()),
        _ => Err(failure(&out)),
    }
}

/// Runs `script` with `path` as `$1`, the password line and `input` on its
/// input. Output past `limit` bytes stops it.
async fn run(
    conn: &Connection,
    script: &str,
    path: &str,
    password: Option<&str>,
    input: &[u8],
    limit: usize,
) -> Result<Output, GatewayError> {
    let mut channel = conn.target().channel_open_session().await.map_err(connection_error)?;
    channel.exec(true, command(script, path)).await.map_err(session_error)?;
    let line = Zeroizing::new(format!("{}\n", password.unwrap_or_default()));
    channel.data(line.as_bytes()).await.map_err(session_error)?;
    if !input.is_empty() {
        channel.data(input).await.map_err(session_error)?;
    }
    channel.eof().await.map_err(session_error)?;
    let mut out = Output::default();
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                out.stdout.extend_from_slice(&data);
                if out.stdout.len() > limit {
                    let _ = channel.close().await;
                    return Err(GatewayError::Transfer(format!("the file is over {} MB", limit / (1024 * 1024))));
                }
            }
            ChannelMsg::ExtendedData { data, .. } if out.stderr.len() < MAX_ERROR => out.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
            ChannelMsg::Failure => return Err(GatewayError::Session("the server would not run the command".into())),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(out)
}

/// The command line for the login shell: `script` run by `/bin/sh`, with
/// `path` as `$1`.
fn command(script: &str, path: &str) -> String {
    format!("/bin/sh -c {} sh {}", quote(script), quote(path))
}

/// `text` in single quotes, for any POSIX shell.
pub(super) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// What went wrong, from sudo's own words. The scripts run sudo in the C
/// locale, so these are always in English.
fn failure(out: &Output) -> GatewayError {
    let text = String::from_utf8_lossy(&out.stderr);
    let has = |needle: &str| text.contains(needle);
    if has("a password is required") {
        return GatewayError::SudoNeedsPassword;
    }
    if has("incorrect password") || has("Sorry, try again") {
        return GatewayError::SudoWrongPassword;
    }
    let reason = if has("not in the sudoers") || has("not allowed to") || has("may not run sudo") {
        "your account may not use sudo on this server".to_owned()
    } else if has("terminal is required") || has("must have a tty") {
        "sudo on this server only runs in a terminal".to_owned()
    } else if out.status == Some(127) {
        "sudo is not installed on this server".to_owned()
    } else if out.status == Some(NO_TEMP_FILE) {
        "could not make a temporary file on the server".to_owned()
    } else {
        match text.lines().rev().find(|line| !line.trim().is_empty()) {
            Some(line) => line.trim().to_owned(),
            None => format!("sudo stopped with code {}", out.status.map_or("unknown".into(), |code| code.to_string())),
        }
    };
    GatewayError::Session(reason)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use super::*;

    /// Stands in for sudo: `-n` fails while a password is set, and `-S`
    /// checks one line of input against it.
    const FAKE_SUDO: &str = r#"#!/bin/sh
mode=$1; shift
case $mode in
  -n) if [ -n "$FAKE_SUDO_PASSWORD" ]; then echo "sudo: a password is required" >&2; exit 1; fi ;;
  -S) shift 2
      IFS= read -r given
      if [ "$given" != "$FAKE_SUDO_PASSWORD" ]; then
        echo "Sorry, try again." >&2
        echo "sudo: 1 incorrect password attempt" >&2
        exit 1
      fi ;;
esac
[ "$1" = -- ] && shift
exec "$@"
"#;

    struct Server {
        dir: tempfile::TempDir,
    }

    impl Server {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            for sub in ["bin", "tmp"] {
                std::fs::create_dir(dir.path().join(sub)).unwrap();
            }
            let sudo = dir.path().join("bin/sudo");
            std::fs::write(&sudo, FAKE_SUDO).unwrap();
            std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn path(&self, name: &str) -> String {
            self.dir.path().join(name).display().to_string()
        }

        /// Runs a script the way the login shell would, with `sudo_password`
        /// as the one sudo wants.
        fn run(&self, script: &str, path: &str, sudo_password: &str, password: Option<&str>, input: &[u8]) -> Output {
            let bin = self.dir.path().join("bin");
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg(command(script, path))
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("TMPDIR", self.dir.path().join("tmp"))
                .env("FAKE_SUDO_PASSWORD", sudo_password)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            stdin.write_all(format!("{}\n", password.unwrap_or_default()).as_bytes()).unwrap();
            stdin.write_all(input).unwrap();
            drop(stdin);
            let done = child.wait_with_output().unwrap();
            Output {
                status: done.status.code().and_then(|code| u32::try_from(code).ok()),
                stdout: done.stdout,
                stderr: done.stderr,
            }
        }

        fn temp_files(&self) -> usize {
            std::fs::read_dir(self.dir.path().join("tmp")).unwrap().count()
        }
    }

    #[test]
    fn reads_and_writes_when_sudo_needs_no_password() {
        let server = Server::new();
        let file = server.path("it's a file.conf");
        std::fs::write(&file, "old contents that are longer\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();

        let out = server.run(WRITE, &file, "", None, b"new\nlines\n");
        assert_eq!(out.status, Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\nlines\n");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(server.temp_files(), 0);

        let out = server.run(READ, &file, "", None, b"");
        assert_eq!(out.status, Some(0));
        assert_eq!(out.stdout, b"new\nlines\n");
    }

    #[test]
    fn asks_for_the_password_and_keeps_the_file_when_it_is_wrong() {
        let server = Server::new();
        let file = server.path("hosts");
        std::fs::write(&file, "kept\n").unwrap();

        let out = server.run(WRITE, &file, "s3cret", None, b"changed\n");
        assert!(matches!(failure(&out), GatewayError::SudoNeedsPassword));
        let out = server.run(WRITE, &file, "s3cret", Some("guess"), b"s3cret\nchanged\n");
        assert!(matches!(failure(&out), GatewayError::SudoWrongPassword));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept\n");
        assert_eq!(server.temp_files(), 0);

        // The contents start with what looks like a password line, and still arrive whole.
        let out = server.run(WRITE, &file, "s3cret", Some("s3cret"), b"s3cret\nchanged\n");
        assert_eq!(out.status, Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "s3cret\nchanged\n");
        let out = server.run(READ, &file, "s3cret", Some("s3cret"), b"");
        assert_eq!(out.stdout, b"s3cret\nchanged\n");
        assert_eq!(server.temp_files(), 0);
    }

    #[test]
    fn explains_other_failures() {
        let server = Server::new();
        let missing = server.path("missing/file");
        let out = server.run(WRITE, &missing, "", None, b"x");
        let GatewayError::Session(reason) = failure(&out) else { panic!("expected a reason") };
        assert!(reason.contains("missing/file"), "{reason}");
        assert!(!Path::new(&missing).exists());

        let not_allowed = Output {
            status: Some(1),
            stderr: b"user is not in the sudoers file.  This incident will be reported.\n".to_vec(),
            ..Output::default()
        };
        assert!(matches!(failure(&not_allowed), GatewayError::Session(reason) if reason.contains("may not use sudo")));
    }
}
