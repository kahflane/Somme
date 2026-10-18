use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use somme_application::ports::{GatewayError, OutputSink, PtyGateway, ShellOptions, TermSize, Terminal};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const READ_BUFFER: usize = 16 * 1024;

fn session_error(err: impl std::fmt::Display) -> GatewayError {
    GatewayError::Session(err.to_string())
}

fn pty_size(size: TermSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NativePtyGateway;

struct LocalTerminal {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    shell: Option<u32>,
    hooks: bool,
}

/// The current folder of a process, asked from the OS.
fn process_cwd(pid: u32) -> Option<PathBuf> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        ProcessRefreshKind::new().with_cwd(UpdateKind::Always),
    );
    system.process(pid)?.cwd().map(Path::to_path_buf)
}

#[async_trait]
impl Terminal for LocalTerminal {
    async fn write(&self, data: &[u8]) -> Result<(), GatewayError> {
        let mut writer = self.writer.lock().map_err(session_error)?;
        writer.write_all(data).map_err(session_error)?;
        writer.flush().map_err(session_error)
    }

    async fn resize(&self, size: TermSize) -> Result<(), GatewayError> {
        self.master
            .lock()
            .map_err(session_error)?
            .resize(pty_size(size))
            .map_err(session_error)
    }

    async fn close(&self) {
        if let Ok(mut killer) = self.killer.lock() {
            let _ = killer.kill();
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The program in front (a sub-shell, an editor) when there is one, else the shell.
        #[cfg(unix)]
        let front = self
            .master
            .lock()
            .ok()?
            .process_group_leader()
            .and_then(|pid| u32::try_from(pid).ok());
        #[cfg(not(unix))]
        let front = None;
        front.and_then(process_cwd).or_else(|| process_cwd(self.shell?))
    }

    fn has_hooks(&self) -> bool {
        self.hooks
    }
}

const ZSHENV: &str = include_str!("../shell/zsh/zshenv");
const ZSH_HOOKS: &str = include_str!("../shell/zsh/somme.zsh");

fn write_if_changed(path: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(path, text)
}

/// Writes the zsh hooks to the user's cache folder and returns that folder.
fn zsh_hooks_dir() -> std::io::Result<PathBuf> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| std::io::Error::other("no cache folder"))?
        .join("app.somme.desktop")
        .join("shell")
        .join("zsh");
    std::fs::create_dir_all(&dir)?;
    write_if_changed(&dir.join(".zshenv"), ZSHENV)?;
    write_if_changed(&dir.join("somme.zsh"), ZSH_HOOKS)?;
    Ok(dir)
}

/// Makes zsh load the hooks: zsh reads `.zshenv` from ZDOTDIR first, and ours
/// loads the user's own config before adding the hooks. Other shells start
/// plain. Returns whether the hooks will load.
fn add_shell_hooks(cmd: &mut CommandBuilder, token: &str) -> bool {
    let shell = cmd.get_shell();
    if Path::new(&shell).file_name().and_then(|name| name.to_str()) != Some("zsh") {
        return false;
    }
    let dir = match zsh_hooks_dir() {
        Ok(dir) => dir,
        Err(err) => {
            tracing::warn!(%err, "could not write the shell hooks");
            return false;
        }
    };
    if let Some(user_dir) = std::env::var_os("ZDOTDIR") {
        cmd.env("SOMME_USER_ZDOTDIR", user_dir);
    }
    cmd.env("ZDOTDIR", &dir);
    cmd.env("SOMME_SHELL_HOOKS", &dir);
    // Our .zshenv moves this into a shell variable before anything else runs.
    cmd.env("SOMME_MARK_TOKEN", token);
    true
}

/// Settings that belong to the program that started Somme, not to its
/// terminals. Started from Claude Code, Somme would pass on its session
/// markers, and a `claude` run in Somme would think it was part of that
/// session; Claude Code also points git at its own editor.
fn launcher_settings(names: impl Iterator<Item = std::ffi::OsString>) -> Vec<String> {
    let names: Vec<String> = names.filter_map(|name| name.into_string().ok()).collect();
    if !names.iter().any(|name| name == "CLAUDECODE") {
        return Vec::new();
    }
    names
        .into_iter()
        .filter(|name| {
            name == "CLAUDECODE"
                || name.starts_with("CLAUDE_CODE_")
                || ["CLAUDE_PID", "CLAUDE_EFFORT", "AI_AGENT", "GIT_EDITOR"].contains(&name.as_str())
        })
        .collect()
}

impl PtyGateway for NativePtyGateway {
    fn spawn(
        &self,
        size: TermSize,
        options: ShellOptions<'_>,
        sink: Arc<dyn OutputSink>,
    ) -> Result<Arc<dyn Terminal>, GatewayError> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(session_error)?;
        let mut cmd = CommandBuilder::new_default_prog();
        for name in launcher_settings(std::env::vars_os().map(|(name, _)| name)) {
            cmd.env_remove(name);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let hooks = options.marks.is_some_and(|token| add_shell_hooks(&mut cmd, token));
        let dir = options.dir.filter(|d| d.is_dir()).map(Path::to_path_buf).or_else(dirs::home_dir);
        if let Some(dir) = dir {
            cmd.cwd(dir);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(session_error)?;
        let shell = child.process_id();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(session_error)?;
        let writer = pair.master.take_writer().map_err(session_error)?;
        let killer = child.clone_killer();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_BUFFER];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink.data(&buf[..n]),
                }
            }
            let reason = child
                .wait()
                .ok()
                .filter(|status| !status.success())
                .map(|status| format!("shell exited with code {}", status.exit_code()));
            sink.closed(reason);
        });
        Ok(Arc::new(LocalTerminal {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            shell,
            hooks,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> impl Iterator<Item = std::ffi::OsString> {
        list.iter().map(std::ffi::OsString::from).collect::<Vec<_>>().into_iter()
    }

    #[test]
    fn a_claude_code_session_is_not_passed_on() {
        let dropped = launcher_settings(names(&[
            "HOME",
            "CLAUDECODE",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_PID",
            "AI_AGENT",
            "GIT_EDITOR",
            "CLAUDE_CONFIG_DIR",
            "PATH",
        ]));
        assert_eq!(
            dropped,
            ["CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PID", "AI_AGENT", "GIT_EDITOR"]
        );
        // Started some other way, the user's own settings all stay.
        assert!(launcher_settings(names(&["HOME", "GIT_EDITOR", "CLAUDE_CODE_USE_BEDROCK"])).is_empty());
    }
}
