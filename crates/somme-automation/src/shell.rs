//! Reads shell code into what it does: the commands it runs, the files it
//! writes with `>`, and the parts only known when it runs. It uses
//! tree-sitter's bash grammar, the one the editor uses, and sees through
//! wrappers like `sudo`, `env`, `xargs`, `timeout` and `sh -c '…'`, and the
//! commands `find -exec` runs.
//!
//! It reads text: it can't know what a script file or a variable holds, so
//! it says when a command's name is only known at run time.

use std::path::Path;

use tree_sitter::{Node, Parser};

/// One command as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    /// As written, with quotes taken off.
    pub program: String,
    pub args: Vec<String>,
    /// Its name comes from a variable or a substitution.
    pub dynamic: bool,
    /// The wrappers it was run through, like `sudo` or `xargs`.
    pub via: Vec<String>,
}

impl Command {
    /// The program's own name, without its folder.
    pub fn name(&self) -> &str {
        Path::new(&self.program).file_name().and_then(|name| name.to_str()).unwrap_or(&self.program)
    }

    /// Run through `sudo` or `doas`.
    pub fn privileged(&self) -> bool {
        self.via.iter().any(|wrapper| matches!(wrapper.as_str(), "sudo" | "doas"))
    }
}

/// A file a command writes with `>`, `>>` or `&>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub target: String,
    /// `>>`: adds to the end.
    pub append: bool,
}

/// What a piece of shell code does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    /// Every command, wrappers seen through, nested ones too.
    pub commands: Vec<Command>,
    pub writes: Vec<Write>,
    /// It defines functions, so a name may not mean the program.
    pub functions: bool,
    /// The grammar could not read some of it.
    pub broken: bool,
}

impl Script {
    /// Whether any command's name is only known when it runs.
    pub fn dynamic(&self) -> bool {
        self.commands.iter().any(|command| command.dynamic)
    }
}

/// Reads `text` as bash.
pub fn parse(text: &str) -> Script {
    let mut script = Script::default();
    read_into(text, &mut script, 0);
    script
}

/// How deep `sh -c '…'` may nest before it counts as unreadable.
const DEEPEST: usize = 4;

fn read_into(text: &str, script: &mut Script, depth: usize) {
    let mut parser = Parser::new();
    let tree = parser.set_language(&tree_sitter_bash::LANGUAGE.into()).ok().and_then(|()| parser.parse(text, None));
    let Some(tree) = tree else {
        script.broken = true;
        return;
    };
    let root = tree.root_node();
    if root.has_error() {
        script.broken = true;
    }
    let mut found = Vec::new();
    walk(root, text.as_bytes(), script, &mut found);
    for command in found {
        unwrap(command, script, depth);
    }
}

fn walk(node: Node, src: &[u8], script: &mut Script, found: &mut Vec<Command>) {
    match node.kind() {
        "command" => {
            let name = node.child_by_field_name("name");
            let mut cursor = node.walk();
            let args = node.children_by_field_name("argument", &mut cursor).map(|arg| word(arg, src)).collect();
            if let Some(name) = name {
                found.push(Command { program: word(name, src), args, dynamic: is_dynamic(name), via: Vec::new() });
            }
        }
        "file_redirect" => {
            if let Some(write) = redirect(node, src) {
                script.writes.push(write);
            }
        }
        "function_definition" => script.functions = true,
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, src, script, found);
    }
}

/// A word's text, with quotes and simple escapes taken off.
fn word(node: Node, src: &[u8]) -> String {
    let text = node.utf8_text(src).unwrap_or_default();
    match node.kind() {
        "raw_string" => text.trim_start_matches('\'').trim_end_matches('\'').to_owned(),
        "string" => text.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')).unwrap_or(text).to_owned(),
        "ansi_c_string" => text.trim_start_matches("$'").trim_end_matches('\'').to_owned(),
        "concatenation" | "command_name" => {
            let mut cursor = node.walk();
            let parts: Vec<String> = node.named_children(&mut cursor).map(|part| word(part, src)).collect();
            if parts.is_empty() { unescape(text) } else { parts.concat() }
        }
        "word" => unescape(text),
        _ => text.to_owned(),
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a command's name holds a variable or a substitution.
fn is_dynamic(node: Node) -> bool {
    if matches!(node.kind(), "simple_expansion" | "expansion" | "command_substitution" | "process_substitution" | "arithmetic_expansion") {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(is_dynamic)
}

/// A redirect that writes a file. Copies to another descriptor and the
/// null device don't count.
fn redirect(node: Node, src: &[u8]) -> Option<Write> {
    let mut cursor = node.walk();
    let op = node
        .children(&mut cursor)
        .find(|child| !child.is_named())
        .and_then(|child| child.utf8_text(src).ok())
        .unwrap_or_default()
        .to_owned();
    if !op.contains('>') {
        return None;
    }
    let target = word(node.child_by_field_name("destination")?, src);
    let to_descriptor = op.ends_with('&') && (target == "-" || target.chars().all(|c| c.is_ascii_digit()));
    let harmless = matches!(target.as_str(), "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty");
    (!to_descriptor && !harmless).then(|| Write { target, append: op.contains(">>") })
}

/// Adds `command` and what it runs: a wrapper gives way to the command it
/// runs, `sh -c` is read as code, and `find -exec` adds the command it runs.
fn unwrap(command: Command, script: &mut Script, depth: usize) {
    let name = command.name().to_owned();
    let inner = |skip: usize| -> Option<Command> {
        let rest = &command.args[skip.min(command.args.len())..];
        let (program, args) = rest.split_first()?;
        let mut via = command.via.clone();
        via.push(name.clone());
        Some(Command { program: program.clone(), args: args.to_vec(), dynamic: command.dynamic || program.starts_with('$'), via })
    };
    let wrapped = match name.as_str() {
        "sudo" | "doas" => Some(options_end(&command.args, &["-u", "-g", "-C", "-p", "-U", "-r", "-t", "-D", "-h"])),
        "env" => Some(command.args.iter().position(|arg| !arg.starts_with('-') && !arg.contains('=')).unwrap_or(command.args.len())),
        "nice" => Some(options_end(&command.args, &["-n"])),
        "ionice" => Some(options_end(&command.args, &["-c", "-n", "-p"])),
        "nohup" | "time" | "stdbuf" | "command" | "exec" | "caffeinate" | "setsid" | "unbuffer" | "builtin" => {
            // `command -v x` only asks where x is.
            if name == "command" && command.args.first().is_some_and(|arg| arg == "-v" || arg == "-V") {
                None
            } else {
                Some(options_end(&command.args, &[]))
            }
        }
        "timeout" | "gtimeout" => Some(options_end(&command.args, &["-s", "-k", "--signal", "--kill-after"]) + 1),
        "watch" => Some(options_end(&command.args, &["-n", "-d", "--interval"])),
        "xargs" | "gxargs" => Some(options_end(&command.args, &["-I", "-i", "-n", "-P", "-L", "-l", "-d", "-E", "-e", "-s", "-a", "--max-args", "--max-procs"])),
        _ => None,
    };
    if let Some(skip) = wrapped {
        let runs = inner(skip);
        script.commands.push(command);
        // `xargs` alone runs `echo`.
        if let Some(runs) = runs {
            unwrap(runs, script, depth);
        }
        return;
    }
    if matches!(name.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "fish")
        && let Some(at) = command.args.iter().position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
    {
        match command.args.get(at + 1) {
            Some(code) if depth < DEEPEST => {
                let mut inner = Script::default();
                read_into(code, &mut inner, depth + 1);
                script.writes.extend(inner.writes);
                script.functions |= inner.functions;
                script.broken |= inner.broken;
                for mut found in inner.commands {
                    let mut via = command.via.clone();
                    via.push(name.clone());
                    via.append(&mut found.via);
                    found.via = via;
                    found.dynamic |= command.dynamic;
                    script.commands.push(found);
                }
            }
            _ => script.broken = true,
        }
        script.commands.push(command);
        return;
    }
    if name == "find" {
        let mut args = command.args.iter().enumerate();
        while let Some((at, arg)) = args.next() {
            if matches!(arg.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
                let end = command.args[at + 1..].iter().position(|arg| arg == ";" || arg == "+").map_or(command.args.len(), |end| at + 1 + end);
                if let Some((program, rest)) = command.args[at + 1..end].split_first() {
                    let mut via = command.via.clone();
                    via.push("find".into());
                    unwrap(Command { program: program.clone(), args: rest.to_vec(), dynamic: command.dynamic, via }, script, depth);
                }
                for _ in at + 1..end {
                    args.next();
                }
            }
        }
    }
    script.commands.push(command);
}

/// Where the options before a wrapped command end. `with_value` options take
/// the next word.
fn options_end(args: &[String], with_value: &[&str]) -> usize {
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if arg == "--" {
            return at + 1;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        at += if with_value.contains(&arg.as_str()) { 2 } else { 1 };
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        parse(text).commands.iter().map(|command| command.name().to_owned()).collect()
    }

    #[test]
    fn commands_are_found_in_pipes_lists_and_substitutions() {
        assert_eq!(names("df -h / | tail -1 && echo \"$(date +%F)\"; ls ~/x || true"), ["df", "tail", "echo", "date", "ls", "true"]);
        let script = parse("for d in ~/a ~/b; do [ -d \"$d\" ] && du -sh \"$d\"; done");
        assert!(script.commands.iter().any(|command| command.name() == "du" && command.args == ["-sh", "$d"]));
    }

    #[test]
    fn wrappers_give_way_to_what_they_run() {
        let script = parse("sudo -u root env A=1 timeout 10 rm -rf /tmp/x");
        let rm = script.commands.iter().find(|command| command.name() == "rm").unwrap();
        assert_eq!(rm.args, ["-rf", "/tmp/x"]);
        assert_eq!(rm.via, ["sudo", "env", "timeout"]);
        assert!(rm.privileged());
        assert!(names("find . -name x | xargs -0 -n1 rm -f").contains(&"rm".to_owned()));
        assert!(names("find ~/p -name node_modules -prune -exec rm -rf {} +").contains(&"rm".to_owned()));
        assert!(names("bash -lc 'cd /srv && make clean'").contains(&"make".to_owned()));
        assert!(!names("command -v npm").contains(&"npm".to_owned()));
    }

    #[test]
    fn writes_with_a_redirect_are_found() {
        let script = parse("echo hi > /etc/motd 2>/dev/null; date >> ~/log.txt 2>&1; cat < in.txt");
        assert_eq!(script.writes, [Write { target: "/etc/motd".into(), append: false }, Write { target: "~/log.txt".into(), append: true }]);
        assert!(parse("ls 2>/dev/null >&2").writes.is_empty());
    }

    #[test]
    fn names_known_only_at_run_time_are_flagged() {
        assert!(parse("$CMD --all").dynamic());
        assert!(parse("\"$(which rm)\" -rf x").dynamic());
        assert!(!parse("rm \"$x\"").dynamic());
        assert!(parse("f() { rm -rf /; }; f").functions);
    }
}
