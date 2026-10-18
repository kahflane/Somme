//! Which GitHub repository a folder belongs to, from its git `origin`. The
//! folder may be on a server, so this reads git's files through [`Disk`]
//! instead of running git.

use std::path::{Path, PathBuf};

use crate::terminal::disk::Disk;

/// Git config files are small; anything bigger is not one.
const CONFIG_LIMIT: u64 = 256 * 1024;
/// How far up from the folder to look for `.git`.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

/// The repository `origin` points at, for the folder `dir` or one above it.
/// Slow: run it in [`Disk::run`].
pub fn find(disk: &Disk, dir: &Path) -> Option<Repo> {
    let config = git_dir(disk, dir).and_then(|git| disk.read(&git.join("config"), CONFIG_LIMIT).ok())?;
    origin(&String::from_utf8_lossy(&config)).and_then(|url| parse_remote(&url))
}

/// The git folder that holds the config for `dir`. A worktree's `.git` is a
/// file pointing at its own folder, whose `commondir` leads to the shared one.
fn git_dir(disk: &Disk, dir: &Path) -> Option<PathBuf> {
    for dir in dir.ancestors().take(MAX_DEPTH) {
        let dot_git = dir.join(".git");
        match disk.is_dir(&dot_git) {
            Some(true) => return Some(dot_git),
            Some(false) => {
                let link = disk.read(&dot_git, CONFIG_LIMIT).ok()?;
                let link = String::from_utf8_lossy(&link);
                let target = link.trim().strip_prefix("gitdir:")?.trim();
                let own = dir.join(target);
                let common = disk
                    .read(&own.join("commondir"), CONFIG_LIMIT)
                    .ok()
                    .map(|common| own.join(String::from_utf8_lossy(&common).trim()));
                return Some(common.unwrap_or(own));
            }
            None => {}
        }
    }
    None
}

/// The `url` of `[remote "origin"]` in a git config.
fn origin(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            let section: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            in_origin = section == "[remote\"origin\"]";
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// `owner/name` from a github.com remote, in any of the forms git takes.
pub fn parse_remote(url: &str) -> Option<Repo> {
    let url = url.trim();
    let path = ["https://", "http://", "ssh://", "git://", "git+ssh://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
        .map(|rest| {
            // Drop a user and a port: `git@github.com:22/owner/name`.
            let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
            let (host, path) = rest.split_once('/')?;
            let host = host.split(':').next()?;
            host.eq_ignore_ascii_case("github.com").then_some(path)
        })
        .unwrap_or_else(|| {
            // scp style: `git@github.com:owner/name.git`.
            let rest = url.rsplit_once('@').map_or(url, |(_, host)| host);
            let (host, path) = rest.split_once(':')?;
            host.eq_ignore_ascii_case("github.com").then_some(path)
        })?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(name) && name != "." && name != "..").then(|| Repo {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(owner: &str, name: &str) -> Option<Repo> {
        Some(Repo {
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn reads_every_remote_form() {
        assert_eq!(parse_remote("https://github.com/rust-lang/rust.git"), repo("rust-lang", "rust"));
        assert_eq!(parse_remote("https://github.com/rust-lang/rust"), repo("rust-lang", "rust"));
        assert_eq!(parse_remote("https://user:token@github.com/o/r.git/"), repo("o", "r"));
        assert_eq!(parse_remote("git@github.com:owner/my.repo.git"), repo("owner", "my.repo"));
        assert_eq!(parse_remote("ssh://git@github.com/owner/name"), repo("owner", "name"));
        assert_eq!(parse_remote("ssh://git@github.com:22/owner/name.git"), repo("owner", "name"));
        assert_eq!(parse_remote("git@gitlab.com:owner/name.git"), None);
        assert_eq!(parse_remote("https://github.com.evil.example/o/r"), None);
        assert_eq!(parse_remote("https://github.com/only-owner"), None);
        assert_eq!(parse_remote("git@github.com:o/r;rm -rf"), None);
    }

    #[test]
    fn finds_origin_from_a_folder_inside_the_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(
            root.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:up/stream.git\n[remote \"origin\"]\n\turl = https://github.com/me/app.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        )
        .unwrap();
        assert_eq!(find(&Disk::Local, &root.join("src/deep")), repo("me", "app"));

        // A worktree points back at the main repository.
        std::fs::create_dir_all(root.join(".git/worktrees/wt")).unwrap();
        std::fs::write(root.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        let tree = root.join("wt");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join(".git"), format!("gitdir: {}\n", root.join(".git/worktrees/wt").display())).unwrap();
        assert_eq!(find(&Disk::Local, &tree), repo("me", "app"));

        let outside = tempfile::tempdir().unwrap();
        assert_eq!(find(&Disk::Local, outside.path()), None);
    }

    #[test]
    fn finds_origin_on_a_server() {
        use crate::terminal::disk::fake;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("srv/site/.git")).unwrap();
        std::fs::write(dir.path().join("srv/site/.git/config"), "[remote \"origin\"]\nurl = git@github.com:acme/site.git\n")
            .unwrap();
        std::fs::create_dir_all(dir.path().join("srv/site/public")).unwrap();
        assert_eq!(find(&fake::disk(dir.path()), Path::new("/srv/site/public")), repo("acme", "site"));
    }
}
