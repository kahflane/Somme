//! Finds files on a server, by name or by the text in them. Each search runs
//! `find` and `grep` on a channel of its own, so nothing needs installing.
//! Dropping a search closes its channel, and `timeout`, where the server has
//! it, ends one the server keeps running anyway.

use russh::{ChannelMsg, ChannelWriteHalf, client::Msg};
use somme_application::ports::{GatewayError, RemoteHit};

use crate::ssh::connect::Connection;
use crate::ssh::sudo::quote;
use crate::ssh::{connection_error, session_error};

/// `$1` is the folder, `$2` the `find` pattern, `$3` how many hits, and `$4`
/// is `-iname` or `-name`. Hidden things and the kernel's folders are
/// skipped. Each hit comes out as `d/` for a folder or `f/`, then the path.
const NAMES: &str = r#"t=; timeout 1 true >/dev/null 2>&1 && t='timeout 60'
$t find "$1" -mindepth 1 \( -name '.*' -o -path /proc -o -path /sys -o -path /dev \) -prune -o "$4" "$2" -print 2>/dev/null |
head -n "$3" |
while IFS= read -r f; do
  if [ -d "$f" ]; then printf 'd/%s\n' "$f"; else printf 'f/%s\n' "$f"; fi
done"#;

/// `$1` is the folder, `$2` the text, `$3` how many hits, and `$4` is `-i` or
/// empty. Files over 16 MB are skipped. Older greps without `-I`, `--null` or
/// `-m` get the plain options.
const TEXT: &str = r#"t=; timeout 1 true >/dev/null 2>&1 && t='timeout 60'
o='-nHI --null -m 5'
grep $o -e x -- /dev/null >/dev/null 2>&1
[ $? -le 1 ] || o='-nH'
$t find "$1" -mindepth 1 \( -name '.*' -o -path /proc -o -path /sys -o -path /dev \) -prune -o -type f -size -32768 -print0 2>/dev/null |
$t xargs -0 grep $o ${4:+"$4"} -F -e "$2" -- 2>/dev/null |
head -n "$3""#;

/// Output past this is cut off, for minified files with huge lines.
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
/// Characters kept of a line with a hit.
const LINE_CHARS: usize = 200;

pub async fn search(
    conn: &Connection,
    dir: &str,
    query: &str,
    text: bool,
    ignore_case: bool,
    limit: usize,
) -> Result<Vec<RemoteHit>, GatewayError> {
    let limit = limit.to_string();
    if text {
        let case = if ignore_case { "-i" } else { "" };
        let out = run(conn, TEXT, &[dir, query, &limit, case]).await?;
        Ok(text_hits(&out))
    } else {
        let pattern = format!("*{}*", escape_glob(query));
        let case = if ignore_case { "-iname" } else { "-name" };
        let out = run(conn, NAMES, &[dir, &pattern, &limit, case]).await?;
        Ok(name_hits(&out))
    }
}

/// Runs `script` with `args` as `$1`, `$2`, … and gives what it printed.
async fn run(conn: &Connection, script: &str, args: &[&str]) -> Result<Vec<u8>, GatewayError> {
    let channel = conn.target().channel_open_session().await.map_err(connection_error)?;
    let (mut reader, writer) = channel.split();
    let writer = CloseOnDrop(Some(writer));
    writer.get().exec(true, command(script, args)).await.map_err(session_error)?;
    writer.get().eof().await.map_err(session_error)?;
    let mut out = Vec::new();
    while let Some(msg) = reader.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                out.extend_from_slice(&data);
                if out.len() > MAX_OUTPUT {
                    break;
                }
            }
            ChannelMsg::Failure => return Err(GatewayError::Session("the server would not run the search".into())),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(out)
}

/// Closes the channel when the search ends or is dropped.
struct CloseOnDrop(Option<ChannelWriteHalf<Msg>>);

impl CloseOnDrop {
    fn get(&self) -> &ChannelWriteHalf<Msg> {
        self.0.as_ref().expect("open until dropped")
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        let (Some(writer), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current()) else { return };
        runtime.spawn(async move {
            let _ = writer.close().await;
        });
    }
}

/// The command line for the login shell: `script` run by `/bin/sh`.
fn command(script: &str, args: &[&str]) -> String {
    let mut line = format!("/bin/sh -c {} sh", quote(script));
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

/// `text` matched as itself by `find -name`.
fn escape_glob(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn name_hits(out: &[u8]) -> Vec<RemoteHit> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|line| {
            let (kind, path) = line.split_once('/')?;
            Some(RemoteHit {
                path: path.to_owned(),
                is_dir: kind == "d",
                line: None,
            })
        })
        .collect()
}

/// Reads `path NUL line:text`, or `path:line:text` from a grep without `--null`.
fn text_hits(out: &[u8]) -> Vec<RemoteHit> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|line| {
            let (path, rest) = match line.split_once('\0') {
                Some(split) => split,
                None => {
                    // The first `:digits:` ends the path.
                    let at = line.match_indices(':').map(|(at, _)| at).find(|&at| {
                        let rest = &line[at + 1..];
                        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                        digits > 0 && rest.as_bytes().get(digits) == Some(&b':')
                    })?;
                    (&line[..at], &line[at + 1..])
                }
            };
            let (number, text) = rest.split_once(':')?;
            let text: String = text.trim().chars().take(LINE_CHARS).collect();
            Some(RemoteHit {
                path: path.to_owned(),
                is_dir: false,
                line: Some((number.parse().ok()?, text)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    /// Runs a script the way the login shell would.
    fn run_here(script: &str, args: &[&str]) -> Vec<u8> {
        let out = Command::new("/bin/sh").arg("-c").arg(command(script, args)).output().unwrap();
        out.stdout
    }

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/Order Query")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/order.rs"), "fn main() {}\nlet order = 1; // Order\n").unwrap();
        std::fs::write(root.join("src/notes.txt"), "nothing\n").unwrap();
        std::fs::write(root.join("src/it's: odd.txt"), "an order: here\n").unwrap();
        std::fs::write(root.join(".git/order"), "order\n").unwrap();
        std::fs::write(root.join("src/bin.dat"), b"order\0\x01").unwrap();
        dir
    }

    #[test]
    fn finds_names_on_a_server() {
        let dir = tree();
        let root = dir.path().display().to_string();
        let mut hits = name_hits(&run_here(NAMES, &[&root, "*order*", "50", "-iname"]));
        hits.sort_by(|a, b| a.path.cmp(&b.path));
        let found: Vec<_> = hits.iter().map(|hit| (hit.path.strip_prefix(&root).unwrap(), hit.is_dir)).collect();
        assert_eq!(found, [("/src/Order Query", true), ("/src/order.rs", false)]);
        let hits = name_hits(&run_here(NAMES, &[&root, "*Order*", "50", "-name"]));
        assert_eq!(hits.len(), 1);
        let hits = name_hits(&run_here(NAMES, &[&root, "*", "1", "-iname"]));
        assert_eq!(hits.len(), 1, "stops at the limit");
        assert_eq!(escape_glob("a*b?[c]"), r"a\*b\?\[c\]");
    }

    #[test]
    fn finds_text_on_a_server() {
        let dir = tree();
        let root = dir.path().display().to_string();
        let mut hits = text_hits(&run_here(TEXT, &[&root, "order", "50", "-i"]));
        hits.sort_by(|a, b| (&a.path, a.line.clone()).cmp(&(&b.path, b.line.clone())));
        let found: Vec<_> = hits
            .iter()
            .map(|hit| (hit.path.strip_prefix(&root).unwrap().to_owned(), hit.line.clone().unwrap()))
            .collect();
        assert_eq!(
            found,
            [
                ("/src/it's: odd.txt".to_owned(), (1, "an order: here".to_owned())),
                ("/src/order.rs".to_owned(), (2, "let order = 1; // Order".to_owned())),
            ]
        );
        let hits = text_hits(&run_here(TEXT, &[&root, "Order", "50", ""]));
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn reads_grep_output_without_null() {
        let hits = text_hits(b"/srv/a:b.txt:12:hello: world\nBinary file /srv/x matches\n");
        assert_eq!(
            hits,
            [RemoteHit {
                path: "/srv/a:b.txt".into(),
                is_dir: false,
                line: Some((12, "hello: world".into())),
            }]
        );
    }
}
