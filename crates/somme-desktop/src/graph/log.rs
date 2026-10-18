//! The commits of a repo, newest first, and the lines that join each one to
//! its parents, laid out in columns the way a commit graph draws them.

use std::path::{Path, PathBuf};

use crate::changes::diff::is_sha;
use crate::terminal::blocks::git_raw;

/// How many commits are read at a time.
pub const PAGE: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// The branch checked out.
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Seconds since 1970.
    pub time: i64,
    pub subject: String,
    pub refs: Vec<Ref>,
    /// The work folder is at this commit.
    pub head: bool,
}

pub struct Log {
    /// The repo's top folder.
    pub root: PathBuf,
    pub commits: Vec<Commit>,
    /// There are older commits than these.
    pub more: bool,
}

/// Reads the newest `count` commits of every branch and tag in the repo that
/// holds `dir`. Slow: run it off the UI thread.
pub fn load(dir: &Path, count: usize) -> Result<Log, String> {
    let root = git_raw(dir, &["rev-parse", "--show-toplevel"])
        .map(|out| PathBuf::from(out.trim_end_matches('\n')))
        .ok_or("This folder is not in a git repo.")?;
    let max = format!("--max-count={}", count + 1);
    let out = git_raw(
        &root,
        &[
            "log",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--date-order",
            "--decorate=full",
            &max,
            "--format=%H%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s%x1e",
        ],
    );
    // A repo without commits has nothing to show.
    let mut commits = out.as_deref().map(parse).unwrap_or_default();
    let more = commits.len() > count;
    commits.truncate(count);
    Ok(Log { root, commits, more })
}

fn parse(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').split('\x1f');
            let sha = fields.next().filter(|sha| is_sha(sha))?.to_owned();
            let parents = fields.next()?.split_whitespace().filter(|sha| is_sha(sha)).map(str::to_owned).collect();
            let author = fields.next()?.to_owned();
            let time = fields.next()?.trim().parse().unwrap_or_default();
            let (refs, head) = refs(fields.next()?);
            let subject = fields.next().unwrap_or_default().to_owned();
            Some(Commit {
                sha,
                parents,
                author,
                time,
                subject,
                refs,
                head,
            })
        })
        .collect()
}

/// Reads `%D` with full names: `HEAD -> refs/heads/main, tag: refs/tags/v1`.
fn refs(decorations: &str) -> (Vec<Ref>, bool) {
    let mut head = false;
    let mut refs = Vec::new();
    for item in decorations.split(", ").map(str::trim).filter(|item| !item.is_empty()) {
        let (current, name) = match item.strip_prefix("HEAD -> ") {
            Some(name) => (true, name),
            None if item == "HEAD" => {
                head = true;
                continue;
            }
            None => (false, item),
        };
        head |= current;
        let name = name.strip_prefix("tag: ").unwrap_or(name);
        let found = if let Some(branch) = name.strip_prefix("refs/heads/") {
            Ref {
                name: branch.to_owned(),
                kind: if current { RefKind::Head } else { RefKind::Branch },
            }
        } else if let Some(remote) = name.strip_prefix("refs/remotes/") {
            // Where a remote's default branch points says nothing new.
            if remote.ends_with("/HEAD") {
                continue;
            }
            Ref {
                name: remote.to_owned(),
                kind: RefKind::Remote,
            }
        } else if let Some(tag) = name.strip_prefix("refs/tags/") {
            Ref {
                name: tag.to_owned(),
                kind: RefKind::Tag,
            }
        } else {
            continue;
        };
        refs.push(found);
    }
    (refs, head)
}

/// How one row of the graph is drawn. Columns count from the left, and each
/// line has the colour of its branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub column: usize,
    pub color: usize,
    /// Lines that pass the commit by: column, colour.
    pub through: Vec<(usize, usize)>,
    /// Lines from the row above into the commit: the column they come from, colour.
    pub into: Vec<(usize, usize)>,
    /// Lines from the commit to the row below: the column they go to, colour.
    pub out: Vec<(usize, usize)>,
}

impl Row {
    /// How many columns the row needs.
    pub fn width(&self) -> usize {
        let lines = self.through.iter().chain(&self.into).chain(&self.out).map(|(column, _)| *column);
        lines.chain([self.column]).max().unwrap_or_default() + 1
    }
}

/// Lays the commits out in columns. Each column holds the commit it waits
/// for next; a commit goes in the first column waiting for it, or a free one.
pub fn layout(commits: &[Commit]) -> Vec<Row> {
    // What each column waits for, and its colour.
    let mut lanes: Vec<Option<(&str, usize)>> = Vec::new();
    let mut colors = 0;
    let mut next_color = || {
        colors += 1;
        colors - 1
    };
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let mut row = Row::default();
        let mut own = None;
        for (column, lane) in lanes.iter_mut().enumerate() {
            match *lane {
                Some((sha, color)) if sha == commit.sha => {
                    row.into.push((column, color));
                    own.get_or_insert((column, color));
                    *lane = None;
                }
                Some((_, color)) => row.through.push((column, color)),
                None => {}
            }
        }
        let (column, color) = own.unwrap_or_else(|| (free(&mut lanes), next_color()));
        row.column = column;
        row.color = color;
        for (n, parent) in commit.parents.iter().enumerate() {
            // A parent another line already goes to: join it.
            if let Some((joined, (_, color))) =
                lanes.iter().enumerate().find_map(|(ix, lane)| lane.filter(|(sha, _)| *sha == parent.as_str()).map(|lane| (ix, lane)))
            {
                row.out.push((joined, color));
                continue;
            }
            let (lane, color) = if n == 0 { (column, color) } else { (free(&mut lanes), next_color()) };
            lanes[lane] = Some((parent.as_str(), color));
            row.out.push((lane, color));
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        rows.push(row);
    }
    rows
}

/// The first free column, made if there is none.
fn free(lanes: &mut Vec<Option<(&str, usize)>>) -> usize {
    lanes.iter().position(Option::is_none).unwrap_or_else(|| {
        lanes.push(None);
        lanes.len() - 1
    })
}

/// What a commit says and changed, for the box under the graph.
#[derive(Clone, Debug, Default)]
pub struct Details {
    pub message: String,
    pub email: String,
    /// Path, lines added and removed (None for binary files).
    pub files: Vec<(String, Option<u64>, Option<u64>)>,
}

/// Reads commit `sha` in the repo at `root`. Slow: run it off the UI thread.
pub fn details(root: &Path, sha: &str) -> Result<Details, String> {
    if !is_sha(sha) {
        return Err("That is not a commit.".to_owned());
    }
    let shown = git_raw(root, &["show", "-s", "--format=%ae%x1f%B", sha]).ok_or("Git could not show this commit.")?;
    let (email, message) = shown.split_once('\x1f').unwrap_or_default();
    let parent = git_raw(root, &["rev-parse", "--verify", "--quiet", &format!("{sha}^1")]);
    let numstat = match parent.as_deref().map(str::trim).filter(|parent| is_sha(parent)) {
        Some(parent) => git_raw(
            root,
            &["-c", "core.quotePath=false", "diff", "--numstat", "--no-ext-diff", "--no-textconv", "--find-renames", parent, sha],
        ),
        None => git_raw(
            root,
            &["-c", "core.quotePath=false", "diff-tree", "--numstat", "--no-ext-diff", "--no-textconv", "--root", "-r", "--find-renames", sha],
        ),
    }
    .unwrap_or_default();
    let files = numstat
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let (added, removed, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some((path.to_owned(), added.parse().ok(), removed.parse().ok()))
        })
        .collect();
    Ok(Details {
        message: message.trim_end().to_owned(),
        email: email.to_owned(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(sha: char, parents: &[char]) -> Commit {
        Commit {
            sha: sha.to_string().repeat(40),
            parents: parents.iter().map(|p| p.to_string().repeat(40)).collect(),
            author: String::new(),
            time: 0,
            subject: String::new(),
            refs: Vec::new(),
            head: false,
        }
    }

    #[test]
    fn reads_what_git_log_prints() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let out = format!(
            "{a}\x1f{b}\x1fAnn Lee\x1f1700000000\x1fHEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, tag: refs/tags/v1.0\x1fFix it\x1e\n{b}\x1f\x1fBob\x1f1690000000\x1f\x1fFirst\x1e\n"
        );
        let commits = parse(&out);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, std::slice::from_ref(&b));
        assert!(commits[0].head);
        let names: Vec<_> = commits[0].refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
        assert_eq!(names, [("main", RefKind::Head), ("origin/main", RefKind::Remote), ("v1.0", RefKind::Tag)]);
        assert_eq!(commits[1].subject, "First");
        assert!(commits[1].parents.is_empty() && !commits[1].head);
    }

    #[test]
    fn a_branch_and_its_merge_take_two_columns() {
        // m merges f into c; f and c both come from r.
        let rows = layout(&[commit('m', &['c', 'f']), commit('f', &['r']), commit('c', &['r']), commit('r', &[])]);
        assert_eq!(rows[0], Row { column: 0, color: 0, through: vec![], into: vec![], out: vec![(0, 0), (1, 1)] });
        assert_eq!(rows[1], Row { column: 1, color: 1, through: vec![(0, 0)], into: vec![(1, 1)], out: vec![(1, 1)] });
        // c's parent r is already waited for in column 1, so c's line joins it.
        assert_eq!(rows[2], Row { column: 0, color: 0, through: vec![(1, 1)], into: vec![(0, 0)], out: vec![(1, 1)] });
        assert_eq!(rows[3], Row { column: 1, color: 1, through: vec![], into: vec![(1, 1)], out: vec![] });
        assert_eq!(rows.iter().map(Row::width).max(), Some(2));
    }

    /// A repo's own config can ask git to check signatures with a program it
    /// names. Reading the log must not run it.
    #[cfg(unix)]
    #[test]
    fn a_repo_cannot_run_its_signature_checker() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::{Command, Stdio};

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let ran = dir.path().join("ran");
        let checker = dir.path().join("check.sh");
        std::fs::write(&checker, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).unwrap();
        std::fs::set_permissions(&checker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let git = |args: &[&str], input: Option<&str>| {
            let mut child = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            std::io::Write::write_all(&mut stdin, input.unwrap_or_default().as_bytes()).unwrap();
            drop(stdin);
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap().trim().to_owned()
        };
        std::fs::create_dir(&repo).unwrap();
        git(&["init", "-q"], None);
        let tree = git(&["mktree"], Some(""));
        let signed = format!(
            "tree {tree}\nauthor Ann <ann@example.test> 1700000000 +0000\ncommitter Ann <ann@example.test> 1700000000 +0000\n\
             gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQEzBAABCAAdFiEE\n -----END PGP SIGNATURE-----\n\nSigned\n"
        );
        let sha = git(&["hash-object", "-t", "commit", "-w", "--stdin"], Some(&signed));
        git(&["update-ref", "refs/heads/main", &sha], None);
        git(&["symbolic-ref", "HEAD", "refs/heads/main"], None);
        git(&["config", "log.showSignature", "true"], None);
        git(&["config", "gpg.program", checker.to_str().unwrap()], None);
        // Plain git does run it.
        git(&["log", "-1"], None);
        assert!(ran.exists(), "the test repo should trigger the checker");
        std::fs::remove_file(&ran).unwrap();

        let log = load(&repo, 10).unwrap();
        assert_eq!(log.commits.len(), 1);
        let details = details(&log.root, &sha).unwrap();
        assert_eq!(details.message, "Signed");
        assert!(!ran.exists(), "Somme ran the repo's signature checker");
    }

    #[test]
    fn a_new_branch_tip_takes_a_free_column() {
        let rows = layout(&[commit('x', &['r']), commit('y', &['r']), commit('r', &[])]);
        assert_eq!((rows[0].column, rows[1].column), (0, 1));
        assert_eq!(rows[1].out, [(0, 0)]);
        assert_eq!(rows[2].into, [(0, 0)]);
    }
}
