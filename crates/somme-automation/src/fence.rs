//! The fence a step the AI wrote runs inside: the scope the user approved,
//! kept while it runs. Stand-ins for programs that change things go first
//! on its PATH, like for a what-if, but these act: inside the scope they do
//! the work, outside it they refuse and the step stops. Deletes go to the
//! Somme bin for 7 days unless the step says they can be made again, and
//! files are copied there before they change. On this Mac, macOS's sandbox
//! walls off every write outside the scope as well, whatever program makes
//! it.
//!
//! Shell can do anything, so on a server the fence only knows the programs
//! below. It is there for the mistakes an AI makes, not for someone working
//! around it on purpose.

use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};

use crate::nodes::quote;
use crate::readonly;
use crate::schema::{Mode, Node, NodeKind, Scope};
use crate::shell;

/// Programs the stand-in answers for.
pub const FENCED: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "srm", "trash", "mv", "cp", "ln", "install", "rsync", "scp", "sftp", "tee", "chmod", "chown", "chgrp", "touch",
    "mkdir", "truncate", "dd", "sed", "find", "xattr", "sudo", "doas", "git", "curl", "wget", "ssh", "nc", "ncat", "netcat", "systemctl", "service",
    "launchctl", "kill", "killall", "pkill", "docker", "podman", "kubectl", "reboot", "shutdown", "halt", "poweroff", "diskutil", "tmutil", "mount",
    "umount", "useradd", "usermod", "userdel", "passwd", "chpasswd", "iptables", "ip6tables", "ufw", "nft", "firewall-cmd", "crontab", "sysctl",
    "defaults", "mkfs", "mkfs.ext4", "mkfs.xfs", "mkfs.vfat",
];

/// Days the Somme bin keeps what it holds.
pub const BIN_DAYS: u32 = 7;

/// What one step may do while it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fence {
    pub scope: Scope,
    /// The step's id, in the name of its folder in the bin.
    pub step: String,
    /// Deletes go to the bin: the step doesn't say they can be made again.
    pub bin: bool,
    /// Files it says it writes; each is copied to the bin before it runs.
    pub backups: Vec<String>,
    /// Also run inside macOS's sandbox: only for steps on this Mac.
    pub sandbox: bool,
    /// The bin's folder; `~/.somme/bin` when not set.
    pub bin_root: Option<String>,
    /// The name of this run's folder in the bin, set by who runs it so it
    /// knows where to put things back from; one made from the time when
    /// not set.
    pub name: Option<String>,
}

impl Fence {
    /// The fence for `node`, when it needs one: the AI wrote its command, it
    /// does its work rather than only report, and it does more than look.
    pub fn for_step(node: &Node, scope: &Scope) -> Option<Self> {
        // An agent always works in a fence. One the user OKs change by change
        // may go anywhere, and keeps what it deletes; one on its own stays in
        // the automation's scope, and an empty scope lets it change nothing
        // without asking.
        if let NodeKind::Agent { ask_first, .. } = node.kind {
            return Some(if ask_first {
                Self::keep(&node.id, true)
            } else {
                Self { scope: scope.clone(), step: node.id.clone(), bin: true, sandbox: can_sandbox(), ..Self::default() }
            });
        }
        let command = node.kind.command()?;
        if !node.by_ai || node.mode != Mode::Do || readonly::check(command).is_ok() {
            return None;
        }
        Some(Self {
            scope: scope.clone(),
            step: node.id.clone(),
            bin: !node.effects.rebuildable,
            backups: node.effects.writes.iter().filter(|path| !path.contains(['*', '?', '['])).cloned().collect(),
            sandbox: false,
            bin_root: None,
            name: None,
        })
    }

    /// The fence for a change the user OKs by hand: it may change any path,
    /// and keeps what it deletes and copies what it changes in the bin, so
    /// they can be put back. With `bin` false, deletes are for good.
    pub fn keep(step: &str, bin: bool) -> Self {
        Self {
            scope: Scope {
                paths: vec!["/**".into()],
                delete: true,
                sends: vec!["*".into()],
                services: vec!["*".into()],
                sudo: true,
            },
            step: step.to_owned(),
            bin,
            backups: Vec::new(),
            sandbox: false,
            bin_root: None,
            name: None,
        }
    }
}

/// Whether `script` can run inside a fence. The error says why not.
pub fn can_fence(script: &str) -> Result<(), String> {
    let read = shell::parse(script);
    if read.broken {
        return Err("it can't be read as shell code".into());
    }
    for command in &read.commands {
        if command.dynamic {
            return Err(format!("`{}` is only known when it runs", command.program));
        }
        if command.program.contains('/') && FENCED.contains(&command.name()) {
            return Err(format!("it runs {} by its full path, past the fence; write `{}`", command.program, command.name()));
        }
        if command.name() == "command" && command.args.first().is_some_and(|arg| arg.starts_with('-') && arg.contains('p')) {
            return Err("`command -p` looks programs up past the fence".into());
        }
        if command.name() == "env" && command.args.iter().any(|arg| arg == "-i" || arg.starts_with("PATH=") || arg == "--ignore-environment") {
            return Err("it runs `env` with a new PATH, past the fence".into());
        }
    }
    if mentions(script, "PATH=") || mentions(script, "hash -p") {
        return Err("it changes PATH, which takes it past the fence; drop that".into());
    }
    for word in script.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | ';' | '|' | '&' | '=' | '{' | '}')) {
        if word.starts_with('/') && word.contains("bin/") && FENCED.contains(&word.rsplit('/').next().unwrap_or_default()) {
            return Err(format!("it names {word} by its full path, past the fence; write `{}`", word.rsplit('/').next().unwrap_or_default()));
        }
    }
    Ok(())
}

/// Whether `text` holds `needle` at the start of a word.
fn mentions(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(at, _)| text[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_')))
}

/// The stand-in: one sh script, linked under each program's name.
const STAND_IN: &str = r##"#!/bin/sh
# A Somme fence stand-in: it does what it was asked when that stays inside
# the automation's scope, and refuses when it doesn't.
n=${0##*/}
F=$SOMME_FENCE
L=$F/log
shim_path=$PATH
PATH=$SOMME_REAL_PATH
export PATH
real=$(command -v "$n" 2>/dev/null)
line="$n $*"

stop() {
  printf 'Somme stopped `%s`: %s.\n' "$line" "$1" >&2
  : > "$L/stopped"
  exit 126
}
pass() {
  [ -n "$real" ] || { printf '%s: command not found\n' "$n" >&2; exit 127; }
  PATH=$shim_path
  export PATH
  exec "$real" "$@"
}
has() { grep -qx "$1" "$F/flags" 2>/dev/null; }
inlist() {
  [ -f "$2" ] || return 1
  while IFS= read -r pat; do
    [ -n "$pat" ] || continue
    case "$1" in $pat) return 0 ;; esac
  done < "$2"
  return 1
}
# The full path of $1: . and .. read, and the links in its folders followed.
abs() {
  p=$1
  [ -n "$p" ] || return 1
  case "$p" in /*) ;; *) p="$PWD/$p" ;; esac
  while :; do case "$p" in /) break ;; */) p=${p%/} ;; *) break ;; esac; done
  tail=""
  while [ "$p" != / ] && [ ! -e "$p" ] && [ ! -L "$p" ]; do
    b=${p##*/}
    case "$b" in .|..) return 1 ;; esac
    tail="/$b$tail"
    p=${p%/*}
    [ -n "$p" ] || p=/
  done
  if [ -d "$p" ] && [ ! -L "$p" ]; then
    d=$(cd "$p" 2>/dev/null && pwd -P) || return 1
    out="${d%/}$tail"
  else
    b=${p##*/}
    d=${p%/*}
    [ -n "$d" ] || d=/
    d=$(cd "$d" 2>/dev/null && pwd -P) || return 1
    out="${d%/}/$b$tail"
  fi
  printf '%s\n' "${out:-/}"
}
# The last part of $1 is . or .., which rm won't take.
dots() { case "${1%/}" in .|..|*/.|*/..) return 0 ;; esac; return 1; }
may_change() {
  [ -n "$1" ] || stop "an empty path"
  full=$(abs "$1") || stop "it can't tell where $1 is"
  case "$full" in /dev/null|/dev/stdout|/dev/stderr|/dev/tty|/dev/fd/*) return 0 ;; esac
  case "$full" in "$F"|"$F"/*) stop "$full is Somme's own fence" ;; esac
  bin=$(cat "$F/bin")
  case "$full" in "${bin%/*}"|"${bin%/*}"/*) stop "$full is the Somme bin, which keeps what steps deleted" ;; esac
  inlist "$full" "$F/temp" && return 0
  inlist "$full" "$F/paths" || stop "$full is outside what this automation may change (its scope)"
  return 0
}
# Writing through a link changes what it points at.
may_write() {
  may_change "$1"
  if [ -L "$full" ]; then
    to=$(readlink "$full")
    case "$to" in /*) ;; *) to="${full%/*}/$to" ;; esac
    may_change "$to"
  fi
}
may_send() {
  h=${1#*://}
  h=${h%%/*}
  h=${h##*@}
  case "$h" in \[*) h=${h#\[}; h=${h%%\]*} ;; *) h=${h%%:*} ;; esac
  h=$(printf '%s' "$h" | tr 'A-Z' 'a-z')
  [ -n "$h" ] || return 0
  case "$h" in localhost|127.*|::1) return 0 ;; esac
  inlist "$h" "$F/sends" || stop "it would send to $h, which this automation's scope doesn't list"
}
may_service() {
  s=${1%.service}
  inlist "$s" "$F/services" || stop "it would act on $s, which this automation's scope doesn't list"
}
some_service() { [ -s "$F/services" ] || stop "it would $1, and this automation's scope names no service it may"; }
may_delete() { has delete || stop "this automation's scope doesn't let it delete anything"; }
# Moves $1, a full path, to the bin.
tobin() {
  dest="$(cat "$F/bin")$1"
  mkdir -p "${dest%/*}" || return 1
  if [ -e "$dest" ] || [ -L "$dest" ]; then dest="$dest.$$"; fi
  mv -f "$1" "$dest" || return 1
  printf '%s\t%s\n' "$1" "$dest" >> "$L/binned"
}
# Copies $1, a file about to change, to the bin, once a run.
keep() {
  full=$(abs "$1") || return 0
  [ -f "$full" ] || return 0
  grep -qxF "$full" "$L/kept" 2>/dev/null && return 0
  if [ -n "$(find "$full" -prune -size +102400k 2>/dev/null)" ]; then
    printf 'Somme: %s is over 100 MB, so no copy was kept before it changed.\n' "$full" >&2
    return 0
  fi
  dest="$(cat "$F/bin")$full"
  mkdir -p "${dest%/*}" && cp -p "$full" "$dest" && printf '%s\n' "$full" >> "$L/kept"
}
# The last argument that isn't an option.
last_arg() {
  last=""
  for a in "$@"; do case "$a" in -*) ;; *) last=$a ;; esac; done
}
remote() {
  case "$1" in /*|./*|../*|"~"*) return 1 ;; esac
  pre=${1%%:*}
  [ "$pre" != "$1" ] && case "$pre" in */*|"") return 1 ;; *) return 0 ;; esac
  return 1
}

case "$n" in
  somme-fence-paths)
    # Patterns with their folders' links followed, as targets are.
    for pat in "$@"; do
      pre=${pat%%[*?[]*}
      if [ "$pre" = "$pat" ]; then dir=$pat; else dir=${pre%/*}; fi
      rest=${pat#"$dir"}
      [ -n "$dir" ] || dir=/
      c=$(abs "$dir") || continue
      [ "$c" = / ] && c=""
      printf '%s%s\n' "$c" "$rest"
    done
    exit 0 ;;
  somme-fence-check)
    for a in "$@"; do may_write "$a"; done
    exit 0 ;;
  somme-fence-keep)
    for a in "$@"; do keep "$a"; done
    exit 0 ;;
esac

[ -e "$L/stopped" ] && stop "an earlier change in this step was stopped, so it does nothing more"

case "$n" in
  rm|rmdir|unlink|shred|srm|trash)
    may_delete
    rec=0; dirok=0; force=0; ends=0
    [ "$n" = rmdir ] && dirok=1
    for a in "$@"; do
      if [ $ends = 0 ]; then
        case "$a" in
          --) ends=1; continue ;;
          --recursive) rec=1; continue ;;
          --force) force=1; continue ;;
          --dir) dirok=1; continue ;;
          --*) continue ;;
          -?*)
            case "$a" in *[rR]*) rec=1 ;; esac
            case "$a" in *f*) force=1 ;; esac
            case "$a" in *d*) dirok=1 ;; esac
            continue ;;
        esac
      fi
      [ -n "$a" ] || continue
      dots "$a" && continue
      may_change "$a"
    done
    has bin || pass "$@"
    case "$n" in
      trash) pass "$@" ;;
      shred|srm) stop "it destroys files for good, and the step doesn't say they can be made again" ;;
    esac
    code=0; ends=0
    for a in "$@"; do
      if [ $ends = 0 ]; then case "$a" in --) ends=1; continue ;; -?*) continue ;; esac; fi
      if [ -z "$a" ] || dots "$a"; then
        printf '%s: "%s": may not be removed\n' "$n" "$a" >&2; code=1; continue
      fi
      full=$(abs "$a")
      if [ ! -e "$full" ] && [ ! -L "$full" ]; then
        [ $force = 1 ] || { printf '%s: %s: No such file or directory\n' "$n" "$a" >&2; code=1; }
        continue
      fi
      if [ -d "$full" ] && [ ! -L "$full" ] && [ $rec = 0 ]; then
        if [ $dirok = 0 ]; then printf '%s: %s: is a directory\n' "$n" "$a" >&2; code=1; continue; fi
        if [ -n "$(ls -A "$full" 2>/dev/null)" ]; then printf '%s: %s: Directory not empty\n' "$n" "$a" >&2; code=1; continue; fi
        # Empty: nothing to keep.
        rmdir "$full" || code=1
        continue
      fi
      tobin "$full" || code=1
    done
    exit $code ;;
  mv)
    skip=0
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; continue; fi
      case "$a" in -S|--suffix) skip=1 ;; -t) ;; -*) ;; *) may_change "$a" ;; esac
    done
    last_arg "$@"
    if [ -d "$last" ]; then
      for a in "$@"; do case "$a" in -*|"$last") ;; *) keep "$last/${a##*/}" ;; esac; done
    else
      keep "$last"
    fi
    pass "$@" ;;
  cp|install|ln)
    case " $* " in
      *" -d "*) [ "$n" = install ] && { for a in "$@"; do case "$a" in -*) ;; *) may_write "$a" ;; esac; done; pass "$@"; } ;;
    esac
    last_arg "$@"
    count=0; for a in "$@"; do case "$a" in -*) ;; *) count=$((count + 1)) ;; esac; done
    [ "$n" = ln ] && [ $count = 1 ] && last="$PWD/${last##*/}"
    may_write "$last"
    if [ -d "$last" ]; then
      for a in "$@"; do case "$a" in -*|"$last") ;; *) keep "$last/${a##*/}" ;; esac; done
    else
      keep "$last"
    fi
    pass "$@" ;;
  rsync|scp|sftp)
    skip=0; deletes=0
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; continue; fi
      case "$n:$a" in
        scp:-[PiFoclSJ]|sftp:-[PiFoclSJ]|*:-e) skip=1 ;;
        *:--delete*|*:--remove-source-files) deletes=1 ;;
        *:-*) ;;
        *) remote "$a" && may_send "$a" ;;
      esac
    done
    last_arg "$@"
    [ $deletes = 1 ] && may_delete
    if [ -n "$last" ] && ! remote "$last"; then
      may_write "$last"
      [ -d "$last" ] || keep "$last"
      if [ "$n" = rsync ] && has bin; then
        set -- --backup "--backup-dir=$(cat "$F/bin")$(abs "$last")" "$@"
      fi
    fi
    pass "$@" ;;
  tee)
    append=0
    for a in "$@"; do case "$a" in -a|--append) append=1 ;; esac; done
    for a in "$@"; do case "$a" in -*) ;; *) may_write "$a"; [ $append = 1 ] || keep "$a" ;; esac; done
    pass "$@" ;;
  dd)
    for a in "$@"; do case "$a" in of=*) may_write "${a#of=}"; keep "${a#of=}" ;; esac; done
    pass "$@" ;;
  truncate)
    skip=0
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; continue; fi
      case "$a" in -s|-r|--size|--reference) skip=1 ;; -*) ;; *) may_write "$a"; keep "$a" ;; esac
    done
    pass "$@" ;;
  touch|mkdir)
    skip=0
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; continue; fi
      case "$a" in -t|-d|-r|-m|-A) skip=1 ;; -*) ;; *) may_write "$a" ;; esac
    done
    pass "$@" ;;
  chmod|chown|chgrp)
    first=1
    for a in "$@"; do
      case "$a" in
        --reference=*) first=0 ;;
        -*) ;;
        *) if [ $first = 1 ]; then first=0; else may_change "$a"; fi ;;
      esac
    done
    pass "$@" ;;
  sed)
    edits=0; given=0; skip=0; bsd=0
    case "$(uname 2>/dev/null)" in Darwin|*BSD) bsd=1 ;; esac
    for a in "$@"; do
      case "$a" in -i|-i*|-I*|--in-place*) edits=1 ;; -e|-f|--expression|--file|--expression=*|--file=*) given=1 ;; esac
    done
    [ $edits = 1 ] || pass "$@"
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; continue; fi
      case "$a" in
        -i) [ $bsd = 1 ] && skip=1 ;;
        -e|-f|--expression|--file) skip=1 ;;
        -*) ;;
        "") ;;
        *) if [ $given = 0 ]; then given=1; else may_write "$a"; keep "$a"; fi ;;
      esac
    done
    pass "$@" ;;
  find)
    deletes=0; skip=0
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; may_write "$a"; continue; fi
      case "$a" in
        -delete) deletes=1 ;;
        -fprint|-fprint0|-fprintf|-fls) skip=1 ;;
      esac
    done
    [ $deletes = 1 ] || pass "$@"
    may_delete
    # Through the rm stand-in, deepest first, as -delete goes.
    argc=$#
    for a in "$@"; do
      case "$a" in -delete) set -- "$@" -depth -exec rm -d {} + ;; *) set -- "$@" "$a" ;; esac
    done
    shift $argc
    pass "$@" ;;
  xattr)
    changes=0
    for a in "$@"; do case "$a" in -w|-d|-c|-*[wdc]*) changes=1 ;; esac; done
    [ $changes = 1 ] || pass "$@"
    last_arg "$@"
    may_change "$last"
    pass "$@" ;;
  sudo|doas)
    has sudo || stop "it uses $n, and this automation's scope doesn't allow that"
    # What runs as root keeps the fence.
    argc=$#; opts=1; value=0
    for a in "$@"; do
      if [ $opts = 1 ]; then
        if [ $value = 1 ]; then value=0; set -- "$@" "$a"; continue; fi
        case "$a" in
          -u|-g|-C|-p|-U|-r|-t|-D|-h|-T) value=1; set -- "$@" "$a"; continue ;;
          --) set -- "$@" "$a"; continue ;;
          -*) set -- "$@" "$a"; continue ;;
          *) opts=0; set -- "$@" /usr/bin/env "PATH=$shim_path" "SOMME_FENCE=$F" "SOMME_REAL_PATH=$SOMME_REAL_PATH" ;;
        esac
      fi
      set -- "$@" "$a"
    done
    shift $argc
    [ -n "$real" ] || exit 127
    exec "$real" "$@" ;;
  git)
    dir=$PWD; skip=0; sub=""
    for a in "$@"; do
      if [ $skip = 1 ]; then skip=0; continue; fi
      if [ $skip = 2 ]; then skip=0; dir=$a; continue; fi
      case "$a" in -C) skip=2 ;; -c) skip=1 ;; -*) ;; *) sub=$a; break ;; esac
    done
    case "$sub" in
      ""|status|log|diff|show|ls-files|check-ignore|rev-parse|describe|blame|ls-tree|cat-file|count-objects|shortlog|grep|help|version|remote|fetch|config) pass "$@" ;;
      clone)
        for a in "$@"; do case "$a" in *://*|*@*:*) may_send "$a" ;; esac; done
        last_arg "$@"; may_write "$last"; pass "$@" ;;
      clean)
        may_delete
        has bin && stop "git clean deletes for good, and the step doesn't say what it deletes can be made again"
        may_change "$dir"; pass "$@" ;;
      reset|checkout|restore|stash)
        case " $* " in
          *" --hard "*|*" -- "*|*" drop "*|*" clear "*|*" -f "*|*" --force "*)
            has bin && stop "it throws away changes for good, and the step doesn't say they can be made again" ;;
        esac
        may_change "$dir"; pass "$@" ;;
      *) may_change "$dir"; pass "$@" ;;
    esac ;;
  curl|wget)
    value=0; out=0
    for a in "$@"; do
      if [ $out = 1 ]; then out=0; [ "$a" = - ] || may_write "$a"; continue; fi
      if [ $value = 1 ]; then value=0; case "$a" in *://*) may_send "$a" ;; esac; continue; fi
      case "$n:$a" in
        *://*) may_send "$a" ;;
        curl:-o|curl:--output|curl:-D|curl:--dump-header|curl:-c|curl:--cookie-jar|wget:-O|wget:--output-document|wget:-o|wget:--output-file|wget:-P|wget:--directory-prefix) out=1 ;;
        curl:--output=*|curl:--dump-header=*|curl:--cookie-jar=*|wget:--output-document=*|wget:--output-file=*|wget:--directory-prefix=*) may_write "${a#*=}" ;;
        curl:-O|curl:--remote-name|curl:-J|curl:--remote-name-all) may_write "$PWD/download" ;;
        *:--*=*) ;;
        curl:-[sSfLkviIGNnqjJR0123456]*|curl:--silent|curl:--show-error|curl:--fail|curl:--fail-with-body|curl:--location|curl:--insecure|curl:--verbose|curl:--include|curl:--head|curl:--compressed|curl:--globoff|curl:--get|curl:--http1.1|curl:--http2|curl:--no-progress-meter|curl:--progress-bar) ;;
        wget:-q|wget:--quiet|wget:-nv|wget:-v|wget:-c|wget:--continue|wget:-N|wget:--no-check-certificate|wget:-S|wget:--spider) ;;
        *:-*) value=1 ;;
        *) may_send "$a" ;;
      esac
    done
    pass "$@" ;;
  ssh|nc|ncat|netcat)
    value=0
    for a in "$@"; do
      if [ $value = 1 ]; then value=0; [ "$prev" = -J ] && may_send "$a"; continue; fi
      case "$a" in
        -[bcDEeFIiJLlmOopQRSWwBsxXqTVg]) value=1; prev=$a ;;
        -*) ;;
        *) may_send "$a"; break ;;
      esac
    done
    pass "$@" ;;
  systemctl)
    verb=""; for a in "$@"; do case "$a" in -*) ;; *) verb=$a; break ;; esac; done
    case "$verb" in
      ""|status|is-active|is-enabled|is-failed|show|list-units|list-timers|list-unit-files|list-sockets|list-dependencies|cat|help|get-default) pass "$@" ;;
    esac
    some_service "$verb a service"
    seen=0
    for a in "$@"; do case "$a" in -*) ;; *) if [ $seen = 1 ]; then may_service "$a"; else seen=1; fi ;; esac; done
    pass "$@" ;;
  service)
    case "$2" in status|"") pass "$@" ;; esac
    may_service "$1"; pass "$@" ;;
  launchctl)
    case "$1" in list|print|print-cache|print-disabled|blame|help|version|managername|manageruid|procinfo|hostinfo|examine|dumpstate) pass "$@" ;; esac
    some_service "change a launchd job"
    shift_one=1
    for a in "$@"; do
      if [ $shift_one = 1 ]; then shift_one=0; continue; fi
      case "$a" in -*) ;; *) b=${a##*/}; may_service "${b%.plist}" ;; esac
    done
    pass "$@" ;;
  kill)
    some_service "stop processes"; pass "$@" ;;
  killall|pkill)
    some_service "stop processes"
    value=0
    for a in "$@"; do
      if [ $value = 1 ]; then value=0; continue; fi
      case "$a" in -[sutUGgPFn]) value=1 ;; -*) ;; *) may_service "$a" ;; esac
    done
    pass "$@" ;;
  docker|podman)
    case "$1" in ""|ps|images|inspect|info|version|logs|stats|top|port|diff|history|search|events) pass "$@" ;; esac
    some_service "change containers"
    case "$1" in
      stop|start|restart|kill|rm|pause|unpause)
        [ "$1" = rm ] && may_delete
        seen=0; for a in "$@"; do case "$a" in -*) ;; *) if [ $seen = 1 ]; then may_service "$a"; else seen=1; fi ;; esac; done ;;
      rmi) may_delete ;;
      system|volume|image|container|network|builder)
        case "$2" in prune|rm|remove) may_delete ;; esac ;;
    esac
    pass "$@" ;;
  kubectl)
    case "$1" in get|describe|top|version|logs|explain|api-resources|config|cluster-info) pass "$@" ;; esac
    some_service "change the cluster"
    [ "$1" = delete ] && may_delete
    pass "$@" ;;
  reboot|shutdown|halt|poweroff)
    may_service "$n"; pass "$@" ;;
  diskutil)
    case "$1" in list|info|activity|listFilesystems) pass "$@" ;; esac
    stop "it changes disks, which only a step you write yourself may do" ;;
  tmutil)
    case "$1" in listbackups|latestbackup|destinationinfo|isexcluded|status|machinedirectory|listlocalsnapshots|listlocalsnapshotdates|calculatedrift|compare|uniquesize|version) pass "$@" ;; esac
    stop "it changes Time Machine, which only a step you write yourself may do" ;;
  mount|umount)
    [ $# = 0 ] && pass
    stop "it mounts disks, which only a step you write yourself may do" ;;
  iptables|ip6tables|nft|ufw|firewall-cmd)
    for a in "$@"; do case "$a" in -L|-S|--list*|list|status|--state|--get*) pass "$@" ;; esac; done
    stop "it changes the firewall, which only a step you write yourself may do" ;;
  crontab)
    [ "$1" = -l ] && pass "$@"
    stop "it changes scheduled jobs, which only a step you write yourself may do" ;;
  sysctl)
    for a in "$@"; do case "$a" in -w|*=*) stop "it changes the kernel's settings, which only a step you write yourself may do" ;; esac; done
    pass "$@" ;;
  defaults)
    case "$1" in write|delete|import|rename)
      domain=$2
      case "$domain" in -g|NSGlobalDomain) domain=.GlobalPreferences ;; -currentHost) domain=$3 ;; esac
      case "$domain" in /*) may_write "$domain.plist" ;; *) may_write "$HOME/Library/Preferences/$domain.plist" ;; esac ;;
    esac
    pass "$@" ;;
  *)
    stop "it changes the system, which only a step you write yourself may do" ;;
esac
"##;

/// `script` wrapped to run inside `fence`, for `/bin/sh`. What the fence
/// did is said on standard error, so a step's output stays its own.
pub fn wrap(script: &str, fence: &Fence) -> String {
    // A folder named from where it runs could be anywhere.
    if fence.bin_root.as_deref().is_some_and(|root| !root.starts_with('/')) {
        return "echo 'Somme: the bin needs a full path, so nothing ran.' >&2; exit 125".into();
    }
    let names = FENCED.iter().chain(&["somme-fence-paths", "somme-fence-check", "somme-fence-keep"]).copied().collect::<Vec<_>>().join(" ");
    let paths: Vec<String> = fence.scope.paths.iter().flat_map(|path| case_patterns(path)).collect();
    let lines = |items: &[String], tidy: fn(&str) -> String| -> String {
        let items: Vec<String> = items.iter().map(|item| tidy(item)).filter(|item| !item.is_empty()).map(|item| quote(&item)).collect();
        if items.is_empty() { ":".to_owned() } else { format!("printf '%s\\n' {}", items.join(" ")) }
    };
    let sends = lines(&fence.scope.sends, host_of);
    let services = lines(&fence.scope.services, |name| name.trim().trim_end_matches(".service").to_owned());
    let mut flags = Vec::new();
    if fence.scope.delete {
        flags.push("delete");
    }
    if fence.scope.sudo {
        flags.push("sudo");
    }
    if fence.bin {
        flags.push("bin");
    }
    let flags = if flags.is_empty() { ":".to_owned() } else { format!("printf '%s\\n' {}", flags.join(" ")) };
    let bin_root = match &fence.bin_root {
        Some(root) => quote(root),
        None => "\"$HOME/.somme/bin\"".to_owned(),
    };
    let step: String = fence.step.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    // Letters, digits, - and _ only, so it is safe inside the quotes below.
    let folder = match &fence.name {
        Some(name) => safe_name(name),
        None => format!("$(date +%Y-%m-%d-%H%M%S)-{step}"),
    };
    // Files it writes with `>`: checked, and kept, before anything runs.
    let read = shell::parse(script);
    let written: Vec<String> = read.writes.iter().filter_map(|write| literal_path(&write.target)).collect();
    let replaced: Vec<String> = read.writes.iter().filter(|write| !write.append).filter_map(|write| literal_path(&write.target)).collect();
    let kept: Vec<String> = fence.backups.iter().filter_map(|path| literal_path(path)).chain(replaced).collect();
    let paths = if paths.is_empty() { ":".to_owned() } else { format!("\"$__somme_f/somme-fence-paths\" {}", paths.join(" ")) };
    let check = if written.is_empty() { String::new() } else { format!("\"$__somme_f/somme-fence-check\" {} || __somme_code=126\n", written.join(" ")) };
    let keep = if kept.is_empty() { String::new() } else { format!("\"$__somme_f/somme-fence-keep\" {}\n", kept.join(" ")) };
    format!(
        r#"__somme_f=$(mktemp -d "${{TMPDIR:-/tmp}}/somme-fence.XXXXXX" 2>/dev/null || mktemp -d) || exit 125
__somme_f=$(cd "$__somme_f" && pwd -P) || exit 125
mkdir "$__somme_f/log" || exit 125
cat > "$__somme_f/.stand-in" <<'SOMME_FENCE_END'
{STAND_IN}SOMME_FENCE_END
chmod +x "$__somme_f/.stand-in"
for __somme_n in {names}; do ln -s "$__somme_f/.stand-in" "$__somme_f/$__somme_n"; done
SOMME_FENCE=$__somme_f
SOMME_REAL_PATH=$PATH
export SOMME_FENCE SOMME_REAL_PATH
__somme_root={bin_root}
# What the bin held for more than {BIN_DAYS} days goes. The bin is made
# when something goes in.
[ -d "$__somme_root" ] && find "$__somme_root" -mindepth 1 -maxdepth 1 -mtime +{BIN_DAYS} -exec rm -rf {{}} + 2>/dev/null
__somme_root=$(cd "$__somme_root" 2>/dev/null && pwd -P || printf '%s' "$__somme_root")
printf '%s' "$__somme_root/{folder}" > "$__somme_f/bin"
{flags} > "$__somme_f/flags"
{sends} | tr 'A-Z' 'a-z' > "$__somme_f/sends"
{services} > "$__somme_f/services"
{paths} > "$__somme_f/paths"
for __somme_t in "${{TMPDIR:-/tmp}}" /tmp /var/tmp; do
  __somme_c=$("$__somme_f/somme-fence-paths" "$__somme_t")
  [ -n "$__somme_c" ] && [ "$__somme_c" != / ] && printf '%s\n%s/*\n' "$__somme_c" "$__somme_c"
done > "$__somme_f/temp"
chmod 444 "$__somme_f/bin" "$__somme_f/flags" "$__somme_f/sends" "$__somme_f/services" "$__somme_f/paths" "$__somme_f/temp"
chmod 555 "$__somme_f"
__somme_code=0
{check}if [ $__somme_code = 0 ]; then
{keep}PATH="$__somme_f:$PATH"
export PATH
(
{script}
)
__somme_code=$?
PATH=$SOMME_REAL_PATH
fi
[ -e "$__somme_f/log/stopped" ] && __somme_code=126
__somme_bin=$(cat "$__somme_f/bin")
if [ -s "$__somme_f/log/binned" ]; then
  printf 'Somme bin: %s deleted item(s) are kept in %s for {BIN_DAYS} days.\n' "$(wc -l < "$__somme_f/log/binned" | tr -d ' ')" "$__somme_bin" >&2
fi
if [ -s "$__somme_f/log/kept" ]; then
  printf 'Somme bin: copies of %s file(s), from before they changed, are in %s for {BIN_DAYS} days.\n' "$(wc -l < "$__somme_f/log/kept" | tr -d ' ')" "$__somme_bin" >&2
fi
# What Put back reads.
[ -s "$__somme_f/log/binned" ] && cat "$__somme_f/log/binned" >> "$__somme_bin/{BINNED}"
[ -s "$__somme_f/log/kept" ] && cat "$__somme_f/log/kept" >> "$__somme_bin/{KEPT}"
chmod 755 "$__somme_f"
rm -rf "$__somme_f"
exit $__somme_code
"#
    )
}

/// A path written in a command with no values in it, as sh words: `~` is
/// the home folder where it runs.
fn literal_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.contains(['$', '`', '*', '?', '[']) || path.starts_with("&") || path.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("\"$HOME\"{}", quote(rest)),
        _ => quote(path),
    })
}

/// A scope path as sh words that print `case` patterns: the path, and what
/// is in it. In `case`, `*` also crosses folders, so a pattern is a little
/// wider than the glob; macOS's sandbox reads the glob exactly.
fn case_patterns(path: &str) -> Vec<String> {
    let path = path.trim().trim_end_matches('/');
    if path.is_empty() || !(path.starts_with('/') || path == "~" || path.starts_with("~/")) {
        return Vec::new();
    }
    let mut forms = vec![path.replace("**/", "*/").replace("**", "*")];
    if path.contains("**/") {
        forms.push(path.replace("**/", "").replace("**", "*"));
    }
    forms
        .into_iter()
        .flat_map(|form| [form.clone(), format!("{form}/*")])
        .map(|form| match form.strip_prefix('~') {
            Some(rest) => format!("\"$HOME\"{}", quote(rest)),
            None => quote(&form),
        })
        .collect()
}

/// The host in a scope's `sends` entry: no scheme, user, port or path.
fn host_of(entry: &str) -> String {
    let entry = entry.trim();
    let entry = entry.split_once("://").map_or(entry, |(_, rest)| rest);
    let entry = entry.split('/').next().unwrap_or_default();
    let entry = entry.rsplit_once('@').map_or(entry, |(_, host)| host);
    let host = match entry.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => entry.split(':').next().unwrap_or_default(),
    };
    host.to_lowercase()
}

/// macOS's sandbox rules for a fenced step: it may write in its scope, the
/// temporary folders and the bin, and nowhere else.
pub fn profile(scope: &Scope, home: &str, tmp: &str, bin_root: &str) -> String {
    let mut allowed = vec![
        "(literal \"/dev/null\")".to_owned(),
        "(literal \"/dev/zero\")".to_owned(),
        "(literal \"/dev/tty\")".to_owned(),
        "(literal \"/dev/stdout\")".to_owned(),
        "(literal \"/dev/stderr\")".to_owned(),
        "(literal \"/dev/dtracehelper\")".to_owned(),
        "(regex #\"^/dev/fd/\")".to_owned(),
        "(subpath \"/private/tmp\")".to_owned(),
        "(subpath \"/private/var/tmp\")".to_owned(),
    ];
    for folder in [tmp, bin_root] {
        if !folder.is_empty() {
            allowed.push(format!("(subpath {})", sbpl_string(&real_path(folder))));
        }
    }
    for path in &scope.paths {
        let path = path.trim().trim_end_matches('/');
        let path = match path.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
            _ => path.to_owned(),
        };
        if !path.starts_with('/') || path == "/" || path.is_empty() {
            continue;
        }
        let glob_at = path.find(['*', '?', '[']).unwrap_or(path.len());
        let fixed = path[..glob_at].rsplit_once('/').map_or("", |(dir, _)| dir);
        let (fixed, rest) = if glob_at == path.len() { (path.as_str(), "") } else { (fixed, &path[fixed.len()..]) };
        let fixed = real_path(fixed);
        if rest.is_empty() {
            allowed.push(format!("(subpath {})", sbpl_string(&fixed)));
        } else {
            allowed.push(format!("(regex #\"^{}{}(/.*)?$\")", regex_text(&fixed).replace('"', "\\\""), glob_regex(rest).replace('"', "\\\"")));
        }
    }
    format!("(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write*\n  {})\n", allowed.join("\n  "))
}

/// `path` with links in it followed, as the kernel sees it: as far as it
/// is there, the rest as written.
fn real_path(path: &str) -> String {
    let mut there = Path::new(path);
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = there.canonicalize() {
            let mut real = real;
            for part in rest.iter().rev() {
                real.push(part);
            }
            return real.display().to_string();
        }
        match (there.parent(), there.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                there = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

fn sbpl_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn regex_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | ' ' | '~' | ',') { c.to_string() } else { format!("\\{c}") })
        .collect()
}

/// A glob as a regex: `**/` any folders or none, `**` anything, `*` and `?`
/// inside one folder.
fn glob_regex(glob: &str) -> String {
    let mut out = String::new();
    let mut rest = glob;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("**/") {
            out.push_str("(.*/)?");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("**") {
            out.push_str(".*");
            rest = after;
        } else {
            match c {
                '*' => out.push_str("[^/]*"),
                '?' => out.push_str("[^/]"),
                _ => out.push_str(&regex_text(&c.to_string())),
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// `command` run by `/bin/sh` inside the fence: on a server, or on this
/// computer. On a Mac, macOS's sandbox walls it in too.
pub fn command(script: &str, fence: &Fence) -> String {
    let wrapped = format!("/bin/sh -c {}", quote(&wrap(script, fence)));
    if !fence.sandbox {
        return wrapped;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let tmp = std::env::var("TMPDIR").unwrap_or_default();
    let bin_root = fence.bin_root.clone().unwrap_or_else(|| format!("{home}/.somme/bin"));
    format!("/usr/bin/sandbox-exec -p {} {wrapped}", quote(&profile(&fence.scope, &home, &tmp, &bin_root)))
}

/// The list of what a fenced command moved to the bin, in its bin folder:
/// each line the path, a tab, and where it went.
const BINNED: &str = ".somme-binned";
/// The list of files copied before they changed, in its bin folder.
const KEPT: &str = ".somme-kept";

/// A bin folder's name, with nothing in it that leaves the bin.
fn safe_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

/// A script that puts back what a fenced command left in the bin folder
/// `name`, under `root` (None: `~/.somme/bin` where it runs). What it
/// deleted goes back where it was, unless something new is there, and the
/// files it changed get their copies back. Only what is in that folder is
/// used.
pub fn put_back_script(root: Option<&str>, name: &str) -> String {
    let root = match root {
        Some(root) => quote(root),
        None => "\"$HOME/.somme/bin\"".to_owned(),
    };
    format!(
        r#"b={root}/{name}
[ -d "$b" ] || {{ echo "The Somme bin no longer holds these; it keeps things {BIN_DAYS} days." >&2; exit 1; }}
rb=$(cd -P "$b" && pwd) || exit 1
n=0
left=0
# Whether a path names its folders plainly, with no . or .. in it.
plain() {{ case "/$1/" in */./*|*/../*) return 1 ;; esac; }}
# Whether $1 is in the bin, with links on the way followed.
in_bin() {{ bd=$(cd -P "${{1%/*}}" 2>/dev/null && pwd) && case "$bd/" in "$rb"/*) ;; *) false ;; esac; }}
if [ -f "$b/{BINNED}" ]; then
  while IFS="$(printf '\t')" read -r p d; do
    [ -n "$p" ] && [ -n "$d" ] || continue
    case "$d" in "$b"/*) ;; *) continue ;; esac
    case "$p" in /*) ;; *) continue ;; esac
    plain "$p" && plain "${{d#"$b"}}" && in_bin "$d" || continue
    [ -e "$d" ] || [ -L "$d" ] || continue
    if [ -e "$p" ] || [ -L "$p" ]; then
      left=$((left + 1))
      echo "$p is there again, so the old one stays in the bin." >&2
      continue
    fi
    mkdir -p "${{p%/*}}" && mv -f "$d" "$p" && n=$((n + 1))
  done < "$b/{BINNED}"
fi
if [ -f "$b/{KEPT}" ]; then
  while IFS= read -r p; do
    case "$p" in /*) ;; *) continue ;; esac
    plain "$p" && in_bin "$b$p" || continue
    [ -f "$b$p" ] && [ ! -L "$b$p" ] || continue
    mkdir -p "${{p%/*}}" && cp -p "$b$p" "$p" && n=$((n + 1))
  done < "$b/{KEPT}"
fi
echo "Put back $n item(s)."
[ "$left" = 0 ]
"#,
        name = safe_name(name)
    )
}

/// A glob for a path the way people write it: `~` is the home folder, `*`
/// stays inside one folder and `**` goes through any number. A plain folder
/// covers what is in it too.
pub fn matcher(path: &str, home: &str) -> Option<GlobMatcher> {
    let path = path.trim().trim_end_matches('/');
    let path = match path.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None => path.to_owned(),
    };
    // globset reads an empty choice in `{,/**}` as no match at all.
    let pattern = if path.contains(['*', '?', '[']) { path } else { format!("{{{path},{path}/**}}") };
    GlobBuilder::new(&pattern).literal_separator(true).build().ok().map(|glob| glob.compile_matcher())
}

/// Whether `path` is inside `scope`'s paths: it, or a folder it is in,
/// matches one. `~` and relative paths start at `home`.
pub fn inside(scope: &Scope, path: &str, home: &str) -> bool {
    let path = path.trim();
    let full = match path.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None if path.starts_with('/') => path.to_owned(),
        None => format!("{home}/{path}"),
    };
    let matchers: Vec<_> = scope.paths.iter().filter_map(|glob| matcher(glob, home)).collect();
    let mut at = Path::new(&full);
    loop {
        if matchers.iter().any(|matcher| matcher.is_match(at)) {
            return true;
        }
        match at.parent() {
            Some(parent) if parent != at => at = parent,
            _ => return false,
        }
    }
}

/// What the scope would need for a step the fence stopped: a path, a host
/// to send to, a service, or deleting at all. As a kind and a value.
pub fn widen(stop: &str) -> Option<(&'static str, String)> {
    if let Some(at) = stop.find(" is outside what this automation may change") {
        let before = &stop[..at];
        let start = before.rfind("`: ").map_or(0, |found| found + 3);
        let path = before[start..].trim();
        return (!path.is_empty()).then(|| ("path", path.to_owned()));
    }
    let named = |mark: &str| stop.split_once(mark).and_then(|(_, rest)| rest.split_once(", which")).map(|(name, _)| name.trim().to_owned());
    if let Some(host) = named("it would send to ") {
        return Some(("send", host));
    }
    if let Some(service) = named("it would act on ") {
        return Some(("service", service));
    }
    if stop.contains("scope doesn't let it delete anything") {
        return Some(("delete", String::new()));
    }
    // macOS's sandbox names the path in its program's own words: the last
    // full path in the line.
    if stop.starts_with("macOS's sandbox stopped") {
        let path = stop.split_whitespace().rev().map(|word| word.trim_matches(|c: char| matches!(c, '\'' | '"' | '`' | ':' | ',' | '.' | '(' | ')'))).find(|word| word.starts_with('/'));
        return path.map(|path| ("path", path.to_owned()));
    }
    None
}

/// `scope` with the one thing `widen` named added.
pub fn widened(scope: &Scope, kind: &str, value: &str) -> Scope {
    let mut scope = scope.clone();
    let value = value.trim().to_owned();
    match kind {
        "path" if !value.is_empty() => scope.paths.push(value),
        "send" if !value.is_empty() => scope.sends.push(value),
        "service" if !value.is_empty() => scope.services.push(value),
        "delete" => scope.delete = true,
        _ => {}
    }
    scope
}

/// Put before a command that only looks: git settings in a repository
/// that would run a program (a file watcher, hooks, a pager) are turned off.
/// Git older than 2.31 skips the `GIT_CONFIG_*` list, so `git` itself is
/// called with them too.
const GIT_LOOKS: &str = "export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0= GIT_CONFIG_KEY_1=core.hooksPath GIT_CONFIG_VALUE_1=/dev/null GIT_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0\ngit() { command git -c core.fsmonitor= -c core.hooksPath=/dev/null \"$@\"; }\n";

/// A command that only looks, as `/bin/sh` runs it on a server: git can't
/// run programs from a repository's settings.
pub fn look_script(script: &str) -> String {
    format!("/bin/sh -c {}", quote(&format!("{GIT_LOOKS}{script}")))
}

/// Programs that only look, which run as root and so can't start in the
/// sandbox; none of them can write a file the way they may be called.
const AS_ROOT: &[&str] = &["ps", "top", "crontab"];

/// A command that only looks, as it runs on this Mac: inside macOS's
/// sandbox, it may write only in the temporary folders, and reach the
/// network only when it is a tool for that, like `dig`. None where there is
/// no sandbox.
pub fn look_command(script: &str) -> Option<String> {
    if !can_sandbox() || shell::parse(script).commands.iter().any(|command| AS_ROOT.contains(&command.name())) {
        return None;
    }
    let tmp = std::env::var("TMPDIR").unwrap_or_default();
    let mut rules = profile(&Scope::default(), "", &tmp, "");
    if !readonly::uses_network(script) {
        rules.push_str("(deny network-outbound)\n(allow network-outbound (remote unix-socket))\n");
    }
    Some(format!("/usr/bin/sandbox-exec -p {} {}", quote(&rules), look_script(script)))
}

/// Whether macOS's sandbox can wall in steps here.
pub fn can_sandbox() -> bool {
    cfg!(target_os = "macos") && Path::new("/usr/bin/sandbox-exec").exists()
}

/// Why a fenced step failed, when the fence stopped it.
pub fn stopped(output: &str, sandboxed: bool) -> Option<String> {
    if let Some(line) = output.lines().find(|line| line.starts_with("Somme stopped")) {
        return Some(line.trim().to_owned());
    }
    if sandboxed && let Some(line) = output.lines().find(|line| line.contains("Operation not permitted")) {
        return Some(format!("macOS's sandbox stopped a write outside what this automation may change (its scope): {}", line.trim()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_paths_and_new_paths_are_refused() {
        assert!(can_fence("find ~/p -name x -exec rm -rf {} +").is_ok());
        assert!(can_fence("/bin/rm -rf ~/x").unwrap_err().contains("full path"));
        assert!(can_fence("export PATH=/usr/bin:$PATH; rm x").unwrap_err().contains("PATH"));
        assert!(can_fence("env -i rm x").is_err());
        assert!(can_fence("command -p rm x").is_err());
        assert!(can_fence("RM=/bin/rm; $RM x").is_err());
        assert!(can_fence("echo $SOMME_PATH=1").is_ok());
    }

    #[test]
    fn scope_paths_become_patterns_and_sandbox_rules() {
        assert_eq!(case_patterns("~/Projects/**/node_modules"), [
            "\"$HOME\"'/Projects/*/node_modules'",
            "\"$HOME\"'/Projects/*/node_modules/*'",
            "\"$HOME\"'/Projects/node_modules'",
            "\"$HOME\"'/Projects/node_modules/*'",
        ]);
        assert!(case_patterns("/").is_empty() && case_patterns("relative/x").is_empty());
        assert_eq!(host_of("https://User@Hooks.Slack.com:443/x"), "hooks.slack.com");
        let rules = profile(&Scope { paths: vec!["~/Projects/**/node_modules".into(), "/nowhere/logs".into()], ..Scope::default() }, "/nowhere/home", "", "");
        assert!(rules.contains("(regex #\"^/nowhere/home/Projects/(.*/)?node_modules(/.*)?$\")"), "{rules}");
        assert!(rules.contains("(subpath \"/nowhere/logs\")"), "{rules}");
    }

    #[test]
    fn a_stop_says_what_the_scope_would_need() {
        assert_eq!(widen("Somme stopped `rm -rf /a b`: /a b is outside what this automation may change (its scope)."), Some(("path", "/a b".to_owned())));
        assert_eq!(widen("Somme stopped `curl x`: it would send to evil.test, which this automation's scope doesn't list."), Some(("send", "evil.test".to_owned())));
        assert_eq!(widen("Somme stopped `rm x`: this automation's scope doesn't let it delete anything."), Some(("delete", String::new())));
        assert_eq!(
            widen("macOS's sandbox stopped a write outside what this automation may change (its scope): Error: Operation not permitted @ rb_sysopen - /opt/homebrew/var/homebrew/locks/go.formula.lock"),
            Some(("path", "/opt/homebrew/var/homebrew/locks/go.formula.lock".to_owned()))
        );
        assert_eq!(widened(&Scope::default(), "send", "api.test").sends, ["api.test"]);
        let scope = Scope { paths: vec!["~/Projects/**/node_modules".into()], ..Scope::default() };
        assert!(inside(&scope, "Projects/a/node_modules/x/index.js", "/h") && inside(&scope, "/h/Projects/node_modules", "/h"));
        assert!(!inside(&scope, "~/Projects/a/src/main.js", "/h") && !inside(&scope, "/etc/hosts", "/h"));
    }

    #[test]
    fn the_fence_knows_who_stopped_a_step() {
        assert_eq!(stopped("x\nSomme stopped `rm /a`: no.\n", false).as_deref(), Some("Somme stopped `rm /a`: no."));
        assert!(stopped("touch: /a: Operation not permitted", true).unwrap().contains("sandbox"));
        assert_eq!(stopped("touch: /a: Operation not permitted", false), None);
    }

    /// Runs `script` in the fence with `/bin/sh`, home, bin and temporary
    /// files in `root`.
    #[cfg(unix)]
    fn run(root: &Path, scope: Scope, bin: bool, script: &str) -> (i32, String) {
        let fence = Fence { scope, step: "clean".into(), bin, bin_root: Some(root.join("bin").display().to_string()), ..Fence::default() };
        shell(root, &wrap(script, &fence))
    }

    /// Runs `script` with `/bin/sh`, home and temporary files in `root`.
    #[cfg(unix)]
    fn shell(root: &Path, script: &str) -> (i32, String) {
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .current_dir(root)
            .env("HOME", root.join("home"))
            .env("TMPDIR", root.join("tmp"))
            .output()
            .unwrap();
        (out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    #[cfg(unix)]
    fn home(root: &Path) -> std::path::PathBuf {
        let home = root.join("home");
        for dir in ["Projects/a/node_modules/pkg", "Projects/a/src", "Documents"] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        std::fs::write(home.join("Projects/a/node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(home.join("Projects/a/src/main.js"), "keep me").unwrap();
        std::fs::write(home.join("Documents/notes.txt"), "keep me").unwrap();
        home
    }

    /// A folder outside the system's temporary folders, which the fence
    /// lets any step write: under the build's own folder.
    #[cfg(unix)]
    fn test_root() -> tempfile::TempDir {
        let exe = std::env::current_exe().unwrap();
        let target = exe.ancestors().nth(2).unwrap().to_path_buf();
        tempfile::Builder::new().prefix("fence-test").tempdir_in(target).unwrap()
    }

    #[cfg(unix)]
    fn scope() -> Scope {
        Scope { paths: vec!["~/Projects/**/node_modules".into()], delete: true, ..Scope::default() }
    }

    #[cfg(unix)]
    #[test]
    fn deletes_inside_the_scope_go_to_the_bin() {
        let dir = test_root();
        let home = home(dir.path());
        let (code, said) = run(dir.path(), scope(), true, "rm -rf ~/Projects/a/node_modules && echo done");
        assert_eq!(code, 0, "{said}");
        assert!(said.contains("done") && said.contains("Somme bin: 1 deleted item(s)"), "{said}");
        assert!(!home.join("Projects/a/node_modules").exists());
        let kept: Vec<_> = walk(&dir.path().join("bin")).into_iter().filter(|path| path.ends_with("pkg/index.js")).collect();
        assert_eq!(kept.len(), 1, "{said}");
    }

    #[cfg(unix)]
    #[test]
    fn what_a_change_by_hand_deleted_or_changed_can_be_put_back() {
        let dir = test_root();
        let home = home(dir.path());
        let root = dir.path().join("bin").display().to_string();
        let fence = Fence { bin_root: Some(root.clone()), name: Some("t1-chat".into()), ..Fence::keep("chat", true) };
        let (code, said) = shell(dir.path(), &wrap("rm -rf ~/Documents/notes.txt && sed -i.bak 's/keep/kept/' ~/Projects/a/src/main.js", &fence));
        assert_eq!(code, 0, "{said}");
        assert!(!home.join("Documents/notes.txt").exists() && dir.path().join("bin/t1-chat").is_dir());
        assert_eq!(std::fs::read_to_string(home.join("Projects/a/src/main.js")).unwrap(), "kept me");
        let (code, said) = shell(dir.path(), &put_back_script(Some(&root), "t1-chat"));
        assert_eq!((code, said.trim()), (0, "Put back 2 item(s)."));
        assert_eq!(std::fs::read_to_string(home.join("Documents/notes.txt")).unwrap(), "keep me");
        assert_eq!(std::fs::read_to_string(home.join("Projects/a/src/main.js")).unwrap(), "keep me");
        // Deletes for good leave nothing to put back.
        let gone = Fence { bin_root: Some(root.clone()), name: Some("t2-chat".into()), ..Fence::keep("chat", false) };
        let (code, said) = shell(dir.path(), &wrap("rm -rf ~/Projects/a/node_modules", &gone));
        assert!(code == 0 && !said.contains("Somme bin") && !home.join("Projects/a/node_modules").exists(), "{said}");
        // A list that points outside its folder is not followed.
        let fake = dir.path().join("bin/t3");
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(fake.join(".somme-binned"), format!("{}\t/etc/hosts\n", home.join("planted").display())).unwrap();
        let (_, said) = shell(dir.path(), &put_back_script(Some(&root), "t3"));
        assert!(said.contains("Put back 0 item(s).") && !home.join("planted").exists(), "{said}");
        // Nor one that climbs out with `..` or a link.
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "mine").unwrap();
        std::os::unix::fs::symlink(dir.path(), fake.join("up")).unwrap();
        std::fs::write(
            fake.join(".somme-binned"),
            format!("{}\t{}/t3/../../outside.txt\n{}\t{}/t3/up/outside.txt\n", home.join("planted").display(), root, home.join("planted2").display(), root),
        )
        .unwrap();
        // A kept copy whose folder in the bin is a link to elsewhere.
        let copied = home.join("Documents/copied.txt");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("copied.txt"), "not from the bin").unwrap();
        let in_bin = fake.join(copied.parent().unwrap().strip_prefix("/").unwrap());
        std::fs::create_dir_all(in_bin.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &in_bin).unwrap();
        std::fs::write(fake.join(".somme-kept"), format!("{}\n", copied.display())).unwrap();
        let (_, said) = shell(dir.path(), &put_back_script(Some(&root), "t3"));
        assert!(said.contains("Put back 0 item(s).") && outside.exists() && !home.join("planted").exists() && !home.join("planted2").exists() && !copied.exists(), "{said}");
    }

    #[cfg(unix)]
    #[test]
    fn a_look_cannot_write_or_let_git_run_a_repository_s_program() {
        let dir = test_root();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git").args(args).current_dir(&repo).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap()
        };
        std::fs::write(repo.join("a"), "a").unwrap();
        git(&["init", "-q"]);
        git(&["add", "a"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "a"]);
        let ran = |name: &str| {
            let marker = dir.path().join(name);
            git(&["config", "core.fsmonitor", &format!("touch {}", marker.display())]);
            marker
        };
        // Plain, the repository's file watcher runs; as a look, it doesn't.
        let plain = ran("plain");
        git(&["status"]);
        assert!(plain.exists(), "git ran no file watcher here, so this test shows nothing");
        let looked = ran("looked");
        let status = format!("git -C {} status", quote(&repo.display().to_string()));
        std::process::Command::new("/bin/sh").arg("-c").arg(look_script(&status)).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        assert!(!looked.exists());
        // On this Mac, the sandbox stops a write the checks missed.
        if let Some(walled) = look_command(&format!("touch {}", quote(&dir.path().join("wrote").display().to_string()))) {
            std::process::Command::new("/bin/sh").arg("-c").arg(walled).output().unwrap();
            assert!(!dir.path().join("wrote").exists());
            assert!(look_command("ps -ax").is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn find_delete_and_exec_go_through_the_fence() {
        let dir = test_root();
        let home = home(dir.path());
        let (code, said) = run(dir.path(), scope(), true, "find ~/Projects -name node_modules -type d -prune -exec rm -rf {} +");
        assert_eq!(code, 0, "{said}");
        assert!(!home.join("Projects/a/node_modules").exists() && home.join("Projects/a/src/main.js").exists());
        std::fs::create_dir_all(home.join("Projects/a/node_modules/pkg")).unwrap();
        std::fs::write(home.join("Projects/a/node_modules/pkg/x.js"), "x").unwrap();
        let (code, said) = run(dir.path(), scope(), true, "find ~/Projects/a/node_modules -delete");
        assert_eq!(code, 0, "{said}");
        assert!(!home.join("Projects/a/node_modules/pkg/x.js").exists(), "{said}");
        assert!(walk(&dir.path().join("bin")).iter().any(|path| path.ends_with("pkg/x.js")), "{said}");
    }

    #[cfg(unix)]
    #[test]
    fn changes_outside_the_scope_are_stopped() {
        let dir = test_root();
        let home = home(dir.path());
        let (code, said) = run(dir.path(), scope(), true, "rm -rf ~/Projects/a/src; echo after; rm -rf ~/Projects/a/node_modules");
        assert_eq!(code, 126, "{said}");
        assert!(said.contains("Somme stopped `rm -rf") && said.contains("outside what this automation may change"), "{said}");
        // Once stopped, it changes nothing more.
        assert!(home.join("Projects/a/src/main.js").exists() && home.join("Projects/a/node_modules").exists(), "{said}");
        let (code, said) = run(dir.path(), scope(), true, "rm -rf ~/Projects/a/node_modules/../../../Documents");
        assert_eq!(code, 126, "{said}");
        assert!(home.join("Documents/notes.txt").exists());
        let (code, said) = run(dir.path(), scope(), true, "echo x > ~/Documents/notes.txt");
        assert_eq!(code, 126, "{said}");
        assert_eq!(std::fs::read_to_string(home.join("Documents/notes.txt")).unwrap(), "keep me");
        let (code, said) = run(dir.path(), Scope { delete: false, ..scope() }, true, "rm -rf ~/Projects/a/node_modules");
        assert!(code == 126 && said.contains("doesn't let it delete"), "{said}");
        let (code, said) = run(dir.path(), scope(), true, "cd ~/Projects/a/node_modules && rm -rf . ''; ls");
        assert!(code == 0 && said.contains("pkg") && said.contains("may not be removed"), "{said}");
        let (code, said) = run(dir.path(), scope(), true, "curl -s https://example.invalid/x");
        assert!(code == 126 && said.contains("send to example.invalid"), "{said}");
        let (code, said) = run(dir.path(), scope(), true, "systemctl restart nginx");
        assert!(code == 126 && said.contains("names no service"), "{said}");
    }

    #[cfg(unix)]
    #[test]
    fn rebuildable_deletes_are_gone_and_edits_are_kept_first() {
        let dir = test_root();
        let home = home(dir.path());
        let (code, said) = run(dir.path(), scope(), false, "rm -rf ~/Projects/a/node_modules");
        assert_eq!(code, 0, "{said}");
        assert!(!home.join("Projects/a/node_modules").exists() && !said.contains("Somme bin"), "{said}");
        let scope = Scope { paths: vec!["~/Documents".into()], ..Scope::default() };
        let edit = if cfg!(target_os = "macos") { "sed -i '' 's/keep/kept/' ~/Documents/notes.txt" } else { "sed -i 's/keep/kept/' ~/Documents/notes.txt" };
        let (code, said) = run(dir.path(), scope, true, edit);
        assert_eq!(code, 0, "{said}");
        assert_eq!(std::fs::read_to_string(home.join("Documents/notes.txt")).unwrap(), "kept me");
        assert!(said.contains("copies of 1 file(s)"), "{said}");
        let copy = walk(&dir.path().join("bin")).into_iter().find(|path| path.ends_with("Documents/notes.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(copy).unwrap(), "keep me");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_sandbox_walls_off_writes_the_stand_ins_cant_see() {
        let dir = test_root();
        let root = dir.path();
        let home = home(root);
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        let fence = Fence { scope: scope(), step: "clean".into(), bin: true, bin_root: Some(root.join("bin").display().to_string()), sandbox: true, ..Fence::default() };
        let rules = profile(&fence.scope, &home.display().to_string(), &root.join("tmp").display().to_string(), &root.join("bin").display().to_string());
        let script = "perl -e 'open(my $f, \">\", \"$ENV{HOME}/Projects/a/node_modules/new.txt\") or die \"in: $!\\n\"'; \
            perl -e 'open(my $f, \">\", \"$ENV{HOME}/Documents/new.txt\") or die \"out: $!\\n\"'; \
            rm -rf ~/Projects/a/node_modules";
        let out = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&rules)
            .arg("/bin/sh")
            .arg("-c")
            .arg(wrap(script, &fence))
            .current_dir(root)
            .env("HOME", &home)
            .env("TMPDIR", root.join("tmp"))
            .output()
            .unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(said.contains("out: Operation not permitted") && !said.lines().any(|line| line.starts_with("in: ")), "{said}");
        assert!(!home.join("Documents/new.txt").exists() && !home.join("Projects/a/node_modules").exists(), "{said}");
        assert!(walk(&root.join("bin")).iter().any(|path| path.ends_with("node_modules/new.txt")), "{said}");
        assert!(stopped(&said, true).is_some());
    }

    #[cfg(unix)]
    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
        found
    }
}
