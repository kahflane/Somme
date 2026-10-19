//! Command blocks for local shells: each command keeps its own screen, so the
//! view can show it with its folder, branch, time and exit code.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions as _, Row};
use alacritty_terminal::index::Line;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::terminal::session::{GridSize, Listener};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockState {
    /// The shell took the line but has not run it yet. It may be asking for
    /// the rest of the line, or about to report a syntax error.
    Accepted,
    Running,
    Done { exit: i32, took: Duration },
}

pub struct Block {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub started: Instant,
    pub state: BlockState,
    pub term: Term<Listener>,
    /// Blank rows at the top that are not shown.
    pub skip: usize,
    /// Rows in use, kept for finished blocks.
    rows: usize,
    /// Text the shell printed before its first prompt, like a server's
    /// welcome message. It has no header or command.
    pub startup: bool,
}

impl Block {
    pub fn new(command: String, cwd: Option<PathBuf>, branch: Option<String>, state: BlockState, term: Term<Listener>) -> Self {
        Self {
            command,
            cwd,
            branch,
            started: Instant::now(),
            state,
            term,
            skip: 0,
            rows: 0,
            startup: false,
        }
    }

    /// A block for what the shell printed before its first prompt. None when
    /// that was nothing.
    pub fn startup(term: Term<Listener>, size: GridSize) -> Option<Self> {
        // Accepted, so finishing it drops blank rows at the top as well.
        let mut block = Self::new(String::new(), None, None, BlockState::Accepted, term);
        block.startup = true;
        block.finish(0, size);
        (block.rows() > 0).then_some(block)
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.state, BlockState::Done { .. })
    }

    /// Rows to show, from the top of the history through the last one in use.
    pub fn rows(&self) -> usize {
        if self.is_live() {
            used_rows(&self.term, true).saturating_sub(self.skip)
        } else {
            self.rows
        }
    }

    /// The grid line drawn as the block's first row.
    pub fn first_line(&self) -> Line {
        Line(self.skip as i32 - self.term.grid().history_size() as i32)
    }

    /// Marks the block done and trims its screen to the rows it used.
    pub fn finish(&mut self, exit: i32, size: GridSize) {
        if !self.is_live() {
            return;
        }
        if self.state == BlockState::Accepted {
            // Lines the shell never ran start with the shell's own line break.
            self.skip = leading_blank_rows(&self.term);
        }
        self.state = BlockState::Done {
            exit,
            took: self.started.elapsed(),
        };
        self.term.selection = None;
        let screen = used_rows(&self.term, false).saturating_sub(self.term.grid().history_size());
        self.term.resize(GridSize {
            lines: screen.max(1),
            ..size
        });
        self.refresh_rows();
    }

    /// Follows a new width. Finished blocks keep their own height.
    pub fn resize(&mut self, size: GridSize) {
        if self.is_live() {
            self.term.resize(size);
            return;
        }
        let lines = self.term.grid().screen_lines();
        self.term.resize(GridSize { lines, ..size });
        self.refresh_rows();
    }

    fn refresh_rows(&mut self) {
        self.skip = self.skip.min(used_rows(&self.term, false));
        self.rows = used_rows(&self.term, false).saturating_sub(self.skip);
    }

    /// Everything the command printed, as plain text.
    pub fn output_text(&self) -> String {
        let grid = self.term.grid();
        let first = self.first_line();
        let mut out = String::new();
        for row in 0..self.rows() {
            let line = &grid[first + row as i32];
            let text: String = (0..grid.columns()).map(|col| line[alacritty_terminal::index::Column(col)].c).collect();
            out.push_str(text.trim_end_matches([' ', '\0']));
            if !line[alacritty_terminal::index::Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE) {
                out.push('\n');
            }
        }
        out.trim_end_matches('\n').to_owned()
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self.state {
            BlockState::Done { exit, .. } => Some(exit),
            _ => None,
        }
    }
}

fn row_is_blank(row: &Row<Cell>) -> bool {
    row.into_iter().all(|cell| {
        matches!(cell.c, ' ' | '\0')
            && cell.bg == Color::Named(NamedColor::Background)
            && !cell.flags.intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
    })
}

/// Rows from the top of the history through the last non-blank one. A live
/// block also keeps the cursor's row, where the program may be waiting.
fn used_rows(term: &Term<Listener>, live: bool) -> usize {
    let grid = term.grid();
    let screen = grid.screen_lines();
    let mut last = (0..screen).rev().find(|&line| !row_is_blank(&grid[Line(line as i32)]));
    let cursor = grid.cursor.point;
    if live || cursor.column.0 > 0 {
        let line = cursor.line.0.max(0) as usize;
        last = Some(last.map_or(line, |last| last.max(line)));
    }
    grid.history_size() + last.map_or(0, |line| line + 1)
}

fn leading_blank_rows(term: &Term<Listener>) -> usize {
    let grid = term.grid();
    let top = -(grid.history_size() as i32);
    (0..used_rows(term, false))
        .take_while(|&row| row_is_blank(&grid[Line(top + row as i32)]))
        .count()
}

/// Commands that wipe the screen also wipe the blocks.
pub fn clears_screen(command: &str) -> bool {
    matches!(command.trim(), "clear" | "reset" | "tput clear" | "tput reset")
}

/// `~/Downloads/somme` instead of `/Users/me/Downloads/somme`.
pub fn short_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// `0.074s`, `12.3s`, `4m 05s`.
pub fn format_took(took: Duration) -> String {
    let secs = took.as_secs_f64();
    if secs < 10. {
        format!("{secs:.3}s")
    } else if secs < 60. {
        format!("{secs:.1}s")
    } else {
        let whole = took.as_secs();
        format!("{}m {:02}s", whole / 60, whole % 60)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitInfo {
    /// The repo's top folder.
    pub root: PathBuf,
    pub branch: String,
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

/// The git program, if one can run without macOS asking to install tools.
fn git_program() -> Option<&'static str> {
    static GIT: OnceLock<Option<&'static str>> = OnceLock::new();
    *GIT.get_or_init(|| {
        // Apple's /usr/bin/git opens an install dialog when the developer tools are missing.
        if cfg!(target_os = "macos")
            && Command::new("xcode-select")
                .arg("-p")
                .output()
                .map_or(true, |out| !out.status.success())
        {
            return None;
        }
        Some("git")
    })
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    git_raw(dir, args).map(|out| out.trim().to_owned())
}

/// Git in `dir`, ready for arguments.
fn git_command(dir: &Path) -> Option<Command> {
    let mut command = Command::new(git_program()?);
    // A repo's own config can name programs for git to run. Turn off the ones
    // these lookups could reach, so opening a folder never runs its code.
    // Checking signatures runs the gpg programs; Somme never signs anything.
    command
        .arg("-C")
        .arg(dir)
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"])
        .args(["-c", "log.showSignature=false", "-c", "gpg.program=false"])
        .args(["-c", "gpg.ssh.program=false", "-c", "gpg.x509.program=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(std::process::Stdio::null());
    Some(command)
}

/// Runs git in `dir` and returns what it printed, as is. None when it fails.
/// Commands that compare files, like `diff`, go through [`git_diff_raw`].
pub(crate) fn git_raw(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git_command(dir)?.args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`git_raw`], for commands that read changed files. A repo's own config can
/// name filter programs that git runs on each changed file, so those are blanked
/// out. Filters from the user's own config, like Git LFS, still run.
pub(crate) fn git_diff_raw(dir: &Path, args: &[&str]) -> Option<String> {
    let out = filtered_git_command(dir)?.args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`git_diff_raw`], with `input` given to git, as for `blame --contents -`.
pub(crate) fn git_input_raw(dir: &Path, args: &[&str], input: &str) -> Option<String> {
    let mut child = filtered_git_command(dir)?
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = input.to_owned();
    // Written beside the read, so a big file can't fill both pipes and stall.
    let writer = std::thread::spawn(move || std::io::Write::write_all(&mut stdin, input.as_bytes()));
    let out = child.wait_with_output().ok()?;
    let _ = writer.join();
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Git in `dir` with the filters a repo's own config names switched off.
fn filtered_git_command(dir: &Path) -> Option<Command> {
    let listed = git_command(dir)?
        .args(["config", "--show-scope", "-z", "--get-regexp", r"^filter\."])
        .output()
        .ok()?;
    // 1 means no filter is set. Anything else: not sure, so don't run.
    if !listed.status.success() && listed.status.code() != Some(1) {
        return None;
    }
    let blanked = blanked_filters(&String::from_utf8_lossy(&listed.stdout));
    let mut command = git_command(dir)?;
    // Set this way, not with `-c`, so a filter name with `=` in it can't slip past.
    command.env("GIT_CONFIG_COUNT", blanked.len().to_string());
    for (i, (key, value)) in blanked.iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{i}"), key).env(format!("GIT_CONFIG_VALUE_{i}"), value);
    }
    Some(command)
}

/// Settings that switch off each filter named outside the user's and system's own
/// config. Reads `git config --show-scope -z` output: `scope NUL key NEWLINE value NUL`.
fn blanked_filters(listed: &str) -> Vec<(String, &'static str)> {
    let mut names: Vec<&str> = Vec::new();
    let mut fields = listed.split('\0');
    while let (Some(scope), Some(entry)) = (fields.next(), fields.next()) {
        let key = entry.split_once('\n').map_or(entry, |(key, _)| key);
        let Some(name) = key.strip_prefix("filter.").and_then(|rest| rest.rsplit_once('.')).map(|(name, _)| name)
        else {
            continue;
        };
        if !matches!(scope, "global" | "system") && !names.contains(&name) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .flat_map(|name| {
            [("clean", ""), ("smudge", ""), ("process", ""), ("required", "false")]
                .map(|(var, value)| (format!("filter.{name}.{var}"), value))
        })
        .collect()
}

/// Branch and uncommitted changes for the repo that holds `dir`. Slow: run it off the UI thread.
pub fn git_info(dir: &Path) -> Option<GitInfo> {
    let root = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?);
    let mut branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        branch = git(dir, &["rev-parse", "--short", "HEAD"]).unwrap_or(branch);
    }
    let stat = git_diff_raw(
        dir,
        &["diff", "HEAD", "--shortstat", "--no-ext-diff", "--no-textconv", "--ignore-submodules=all"],
    )
    .unwrap_or_default();
    let stat = stat.trim();
    let (files, added, removed) = parse_shortstat(stat);
    Some(GitInfo {
        root,
        branch,
        files,
        added,
        removed,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Like `main`, or `origin/fix` for a remote one.
    pub name: String,
    /// Checked out now.
    pub current: bool,
    /// Only on a remote: there is no local branch of the same name.
    pub remote: bool,
}

/// What the branch menu offers: branches to switch to, and what can be
/// committed, pushed or renamed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchState {
    pub branches: Vec<Branch>,
    /// The checked-out branch. None when HEAD is detached.
    pub current: Option<String>,
    /// Like `origin/main`, when the current branch follows one.
    pub upstream: Option<String>,
    pub remotes: Vec<String>,
    /// Files staged for the next commit.
    pub staged: usize,
    /// Files changed but not staged, new ones included.
    pub unstaged: usize,
}

impl BranchState {
    /// The upstream, when it has the current branch's name. Only then does a
    /// plain `git push` work, as git refuses to push to a differently named branch.
    pub fn same_name_upstream(&self) -> Option<&str> {
        let current = self.current.as_deref()?;
        let upstream = self.upstream.as_deref()?;
        upstream.strip_suffix(current)?.ends_with('/').then_some(upstream)
    }

    /// Where a branch with no upstream is pushed to.
    pub fn push_remote(&self) -> Option<&str> {
        let origin = self.remotes.iter().find(|remote| *remote == "origin");
        origin.or(self.remotes.first()).map(String::as_str)
    }
}

/// Branches and the state of the checked-out one. Slow: run it off the UI thread.
pub fn git_branch_state(dir: &Path) -> BranchState {
    let local = git(dir, &["branch", "--sort=-committerdate", "--format=%(refname:short)%09%(HEAD)"]);
    let remote = git(dir, &["branch", "--remotes", "--sort=-committerdate", "--format=%(refname:short)"]);
    let current = git(dir, &["branch", "--show-current"]).filter(|name| !name.is_empty());
    let upstream = git(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]);
    let remotes = git(dir, &["remote"]).unwrap_or_default();
    let status = git_diff_raw(
        dir,
        &["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all", "--ignore-submodules=all"],
    );
    let (staged, unstaged) = count_status(&status.unwrap_or_default());
    BranchState {
        branches: parse_branches(&local.unwrap_or_default(), &remote.unwrap_or_default()),
        current,
        upstream: upstream.filter(|name| !name.is_empty()),
        remotes: remotes.lines().map(str::to_owned).collect(),
        staged,
        unstaged,
    }
}

/// Counts staged and unstaged files in `git status --porcelain -z --no-renames`.
fn count_status(status: &str) -> (usize, usize) {
    let mut counts = (0, 0);
    for entry in status.split('\0') {
        let mut codes = entry.chars();
        let (Some(index), Some(tree)) = (codes.next(), codes.next()) else { continue };
        if !matches!(index, ' ' | '?' | '!') {
            counts.0 += 1;
        }
        if tree != ' ' && tree != '!' {
            counts.1 += 1;
        }
    }
    counts
}

fn parse_branches(local: &str, remote: &str) -> Vec<Branch> {
    let mut branches: Vec<Branch> = local
        .lines()
        .filter_map(|line| {
            let (name, head) = line.split_once('\t').unwrap_or((line, ""));
            // A detached HEAD lists as `(HEAD detached at …)`.
            (!name.is_empty() && !name.starts_with('(')).then(|| Branch {
                name: name.to_owned(),
                current: head.trim() == "*",
                remote: false,
            })
        })
        .collect();
    let remote_only: Vec<Branch> = remote
        .lines()
        // `origin` alone, or `origin/HEAD`, points at another remote branch.
        .filter_map(|name| Some((name, name.split_once('/')?.1)))
        .filter(|(_, short)| *short != "HEAD" && !branches.iter().any(|b| b.name == *short))
        .map(|(name, _)| Branch {
            name: name.to_owned(),
            current: false,
            remote: true,
        })
        .collect();
    branches.extend(remote_only);
    branches
}

/// Reads ` 3 files changed, 10 insertions(+), 2 deletions(-)`.
fn parse_shortstat(stat: &str) -> (usize, usize, usize) {
    let mut out = (0, 0, 0);
    for part in stat.split(',') {
        let mut words = part.split_whitespace();
        let Some(count) = words.next().and_then(|n| n.parse().ok()) else {
            continue;
        };
        match words.next() {
            Some(word) if word.starts_with("file") => out.0 = count,
            Some(word) if word.starts_with("insertion") => out.1 = count,
            Some(word) if word.starts_with("deletion") => out.2 = count,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    fn block(output: &[u8]) -> Block {
        let size = GridSize::default();
        let mut term = Term::new(Config::default(), &size, Listener::default());
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, output);
        Block::new("cmd".into(), None, None, BlockState::Running, term)
    }

    #[test]
    fn finished_blocks_keep_only_used_rows() {
        let mut b = block(b"one\r\ntwo\r\n");
        assert_eq!(b.rows(), 3, "a live block keeps the cursor row");
        b.finish(0, GridSize::default());
        assert_eq!(b.rows(), 2);
        assert_eq!(b.output_text(), "one\ntwo");
        assert_eq!(b.term.grid().screen_lines(), 2);
    }

    #[test]
    fn long_output_survives_the_trim() {
        let text: String = (0..60).map(|i| format!("line {i}\r\n")).collect();
        let mut b = block(text.as_bytes());
        b.finish(0, GridSize::default());
        assert_eq!(b.rows(), 60);
        assert!(b.output_text().starts_with("line 0\nline 1\n"));
        assert!(b.output_text().ends_with("line 59"));
    }

    #[test]
    fn lines_the_shell_never_ran_skip_the_line_break() {
        let mut b = block(b"\r\r\nzsh: parse error near `)'\r\n");
        b.state = BlockState::Accepted;
        b.finish(1, GridSize::default());
        assert_eq!(b.output_text(), "zsh: parse error near `)'");
        assert_eq!(b.exit_code(), Some(1));
    }

    #[test]
    fn reads_git_shortstat() {
        assert_eq!(
            parse_shortstat(" 178 files changed, 2382 insertions(+), 20133 deletions(-)"),
            (178, 2382, 20133)
        );
        assert_eq!(parse_shortstat(" 1 file changed, 1 deletion(-)"), (1, 0, 1));
        assert_eq!(parse_shortstat(""), (0, 0, 0));
    }

    #[test]
    fn blanks_only_filters_the_repo_names() {
        let listed = "global\0filter.lfs.clean\ngit-lfs clean -- %f\0\
                      local\0filter.evil.clean\ntouch pwned\0\
                      local\0filter.evil.required\ntrue\0\
                      worktree\0filter.a.b=c.process\nrun\0";
        let keys: Vec<_> = blanked_filters(listed).into_iter().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            [
                "filter.evil.clean",
                "filter.evil.smudge",
                "filter.evil.process",
                "filter.evil.required",
                "filter.a.b=c.clean",
                "filter.a.b=c.smudge",
                "filter.a.b=c.process",
                "filter.a.b=c.required",
            ]
        );
    }

    #[test]
    fn plain_push_needs_the_same_name_upstream() {
        let state = |current: &str, upstream: &str| BranchState {
            current: Some(current.to_owned()),
            upstream: Some(upstream.to_owned()),
            ..BranchState::default()
        };
        assert_eq!(state("main", "origin/main").same_name_upstream(), Some("origin/main"));
        assert_eq!(state("trunk", "origin/main").same_name_upstream(), None);
        assert_eq!(state("main", "origin/xmain").same_name_upstream(), None);
    }

    #[test]
    fn counts_staged_and_unstaged_files() {
        let status = "M  staged.rs\0 M edited.rs\0MM both.rs\0?? new.txt\0A  added.txt\0";
        assert_eq!(count_status(status), (3, 3));
        assert_eq!(count_status(""), (0, 0));
    }

    #[test]
    fn lists_local_then_remote_only_branches() {
        let local = "gpui\t*\nmain\t\n(HEAD detached at 1a2b3c)\t";
        let remote = "origin\norigin/HEAD\norigin/main\norigin/feature/login";
        let branches = parse_branches(local, remote);
        let names: Vec<_> = branches.iter().map(|b| (b.name.as_str(), b.current, b.remote)).collect();
        assert_eq!(
            names,
            [("gpui", true, false), ("main", false, false), ("origin/feature/login", false, true)]
        );
    }

    #[test]
    fn formats_times() {
        assert_eq!(format_took(Duration::from_millis(74)), "0.074s");
        assert_eq!(format_took(Duration::from_millis(12_345)), "12.3s");
        assert_eq!(format_took(Duration::from_secs(245)), "4m 05s");
    }
}
