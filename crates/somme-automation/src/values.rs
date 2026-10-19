//! `{{…}}` values in a step's fields: `{{nodes.check.output}}`,
//! `{{input.branch}}`, `{{nodes.api.json.status}}`.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

static VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*([A-Za-z0-9_.\-]+)\s*\}\}").expect("value pattern"));

/// Commands that read their words, or what goes into them, as code again,
/// where even a value in a variable can run.
static READ_AS_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        \b(?:eval|ssh|watch|powershell|pwsh)\b
        | \b(?:sh|bash|zsh|dash|ksh|ash|fish|su|runuser)\b[^\n;|&]*?\s-[A-Za-z]*c\b
        | \b(?:python[0-9.]*|perl|ruby|node|php|osascript|mysql|psql)\b[^\n;|&]*?\s-[A-Za-z]*[cer]\b
        | \|\s*(?:sudo\s+)?(?:sh|bash|zsh|dash|ksh|ash|fish|at|batch)\b
        | \b(?:sh|bash|zsh|dash|ksh|ash|fish)\b[^\n;|&]*<<
        | \bsudo\b[^\n;|&]*?\s-[A-Za-z]*[si]\b
        | (?i:\bcmd(?:\.exe)?\s+/[ck]\b)",
    )
    .expect("read as code pattern")
});

/// Characters a shell acts on.
const SHELL_CHARS: &[char] = &['$', '`', ';', '|', '&', '<', '>', '(', ')', '\\', '\'', '"', '\n', '\r', '%', '^'];

/// Fills each `{{path}}` from `context`. A path that leads nowhere becomes
/// empty text.
pub fn render(text: &str, context: &Value) -> String {
    if !text.contains("{{") {
        return text.to_owned();
    }
    VALUE
        .replace_all(text, |caps: &Captures| lookup(context, &caps[1]).map(as_text).unwrap_or_default())
        .into_owned()
}

/// The shell a command is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// sh, bash, zsh and fish.
    Posix,
    /// Windows `cmd`, run with delayed expansion on.
    Cmd,
}

/// A command with its `{{…}}` values kept out of the code. Each value is a
/// variable, so `; rm -rf ~` or `$(…)` in a step's output or an input an
/// agent sent stays text and never runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub script: String,
    /// Each variable's name and value.
    pub vars: Vec<(String, String)>,
    /// A value holding characters a shell acts on goes into a command that
    /// reads it as code again, like `ssh` or `sh -c`, so the user must
    /// allow it first.
    pub read_as_code: bool,
}

/// Where a `{{…}}` sits in a command, which decides how its variable goes in.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Spot {
    Bare,
    Single,
    Double,
    /// In a here-document's text. Nothing expands in it when its end word
    /// was quoted.
    Heredoc { quoted: bool, end: String, tabs: bool },
}

/// Fills a command's `{{path}}`s from `context` as variables.
pub fn render_shell(text: &str, context: &Value, shell: Shell) -> ShellCommand {
    if !text.contains("{{") {
        return ShellCommand { script: text.to_owned(), vars: Vec::new(), read_as_code: false };
    }
    let masked = VALUE.replace_all(text, "\0");
    let spots = spots(&masked);
    let (mut vars, mut paths) = (Vec::<(String, String)>::new(), Vec::<String>::new());
    let (mut at, mut shell_chars) = (0, false);
    let script = VALUE
        .replace_all(text, |caps: &Captures| {
            let spot = spots.get(at).cloned().unwrap_or(Spot::Bare);
            at += 1;
            let value = lookup(context, &caps[1]).map(as_text).unwrap_or_default();
            shell_chars |= value.contains(SHELL_CHARS);
            if let Spot::Heredoc { quoted: true, end, tabs } = &spot
                && !value.lines().any(|line| ends_heredoc(line, end, *tabs))
            {
                // Nothing expands here, so the value goes in as written.
                return value;
            }
            let name = match paths.iter().position(|path| *path == caps[1]) {
                Some(ix) => vars[ix].0.clone(),
                None => {
                    let name = format!("SOMME_VALUE_{}", vars.len() + 1);
                    paths.push(caps[1].to_owned());
                    vars.push((name.clone(), value));
                    name
                }
            };
            match (shell, spot) {
                (Shell::Cmd, _) => format!("!{name}!"),
                (Shell::Posix, Spot::Bare) => format!("\"${name}\""),
                // Closes the quotes, puts the value in, opens them again.
                (Shell::Posix, Spot::Single) => format!("'\"${name}\"'"),
                (Shell::Posix, Spot::Double) => format!("\"\"${name}\"\""),
                (Shell::Posix, Spot::Heredoc { .. }) => format!("${{{name}}}"),
            }
        })
        .into_owned();
    ShellCommand { script, vars, read_as_code: shell_chars && READ_AS_CODE.is_match(&masked) }
}

/// The command with its variables set first, as one sh script: what runs on
/// a server, and what the user and the guard are shown.
pub fn with_values(script: &str, vars: &[(String, String)]) -> String {
    let mut text: String = vars.iter().map(|(name, value)| format!("{name}={}\n", crate::nodes::quote(value))).collect();
    text.push_str(script);
    text
}

fn ends_heredoc(line: &str, end: &str, tabs: bool) -> bool {
    (if tabs { line.trim_start_matches('\t') } else { line }) == end
}

/// The spot of each `\0` in a command whose values were taken out. A rough
/// read of sh quoting: a wrong guess can split a value into words, but never
/// runs it.
fn spots(masked: &str) -> Vec<Spot> {
    let chars: Vec<char> = masked.chars().collect();
    let mut spots = Vec::new();
    let mut quote = Spot::Bare;
    let mut waiting = std::collections::VecDeque::new();
    let mut heredoc: Option<Spot> = None;
    let mut line = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        i += 1;
        if let Some(Spot::Heredoc { end, tabs, .. }) = &heredoc {
            if c == '\0' {
                spots.push(heredoc.clone().unwrap_or(Spot::Bare));
            }
            if c == '\n' {
                if ends_heredoc(&line, end, *tabs) {
                    heredoc = waiting.pop_front();
                }
                line.clear();
            } else {
                line.push(c);
            }
            continue;
        }
        let word_start = i < 2 || chars[i - 2].is_whitespace() || ";|&(".contains(chars[i - 2]);
        match (&quote, c) {
            (_, '\0') => spots.push(quote.clone()),
            (Spot::Single, '\'') | (Spot::Double, '"') => quote = Spot::Bare,
            (Spot::Double, '\\') | (Spot::Bare, '\\') if chars.get(i) != Some(&'\0') => i += 1,
            (Spot::Single | Spot::Double, _) => {}
            (_, '\'') => quote = Spot::Single,
            (_, '"') => quote = Spot::Double,
            (_, '\n') => heredoc = waiting.pop_front(),
            (_, '#') if word_start => {
                while let Some(&c) = chars.get(i).filter(|&&c| c != '\n') {
                    if c == '\0' {
                        spots.push(Spot::Bare);
                    }
                    i += 1;
                }
            }
            (_, '<') if chars.get(i) == Some(&'<') && chars.get(i + 1) != Some(&'<') => {
                i += 1;
                let tabs = chars.get(i) == Some(&'-');
                if tabs {
                    i += 1;
                }
                while matches!(chars.get(i), Some(' ' | '\t')) {
                    i += 1;
                }
                let (mut end, mut quoted) = (String::new(), false);
                while let Some(&c) = chars.get(i).filter(|c| !c.is_whitespace() && !";|&<>()\0".contains(**c)) {
                    if matches!(c, '\'' | '"' | '\\') {
                        quoted = true;
                    } else {
                        end.push(c);
                    }
                    i += 1;
                }
                if !end.is_empty() {
                    waiting.push_back(Spot::Heredoc { quoted, end, tabs });
                }
            }
            _ => {}
        }
    }
    spots
}

/// Fills each `{{path}}` with what `value` gives for it; a path it gives
/// nothing for becomes empty text.
pub fn render_with(text: &str, value: impl Fn(&str) -> Option<String>) -> String {
    if !text.contains("{{") {
        return text.to_owned();
    }
    VALUE.replace_all(text, |caps: &Captures| value(&caps[1]).unwrap_or_default()).into_owned()
}

/// The value at a dotted path. Numbers pick from lists.
pub fn lookup<'a>(context: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(context, |value, part| match value {
        Value::Object(fields) => fields.get(part),
        Value::Array(items) => part.parse::<usize>().ok().and_then(|ix| items.get(ix)),
        _ => None,
    })
}

/// The paths a text uses, to check they lead somewhere.
pub fn paths(text: &str) -> Vec<String> {
    VALUE.captures_iter(text).map(|caps| caps[1].to_owned()).collect()
}

pub fn as_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn values_are_filled_in() {
        let context = json!({
            "input": { "branch": "main" },
            "nodes": { "check": { "output": "ok\n", "exit_code": 0, "json": { "items": [ { "name": "a" } ] } } }
        });
        assert_eq!(render("git pull origin {{ input.branch }}", &context), "git pull origin main");
        assert_eq!(render("{{nodes.check.exit_code}}", &context), "0");
        assert_eq!(render("{{nodes.check.json.items.0.name}}", &context), "a");
        assert_eq!(render("[{{nodes.gone.output}}]", &context), "[]");
        assert_eq!(render("echo ${HOME} {not}", &context), "echo ${HOME} {not}");
        assert_eq!(paths("{{a.b}} and {{ c }}"), ["a.b", "c"]);
    }

    #[test]
    fn values_in_commands_are_variables() {
        let context = json!({ "input": { "branch": "main; touch pwned", "dir": "a b" } });
        let shell = render_shell("git pull origin {{input.branch}} && cd {{ input.dir }} # {{input.branch}}", &context, Shell::Posix);
        assert_eq!(shell.script, r#"git pull origin "$SOMME_VALUE_1" && cd "$SOMME_VALUE_2" # "$SOMME_VALUE_1""#);
        assert_eq!(shell.vars, [("SOMME_VALUE_1".into(), "main; touch pwned".into()), ("SOMME_VALUE_2".into(), "a b".into())]);
        assert_eq!(with_values("echo hi", &[("V".into(), "it's".into())]), "V='it'\\''s'\necho hi");
        assert_eq!(render_shell("echo hi", &context, Shell::Posix), ShellCommand { script: "echo hi".into(), vars: Vec::new(), read_as_code: false });
        assert_eq!(render_shell("echo {{input.dir}}", &context, Shell::Cmd).script, "echo !SOMME_VALUE_1!");
    }

    #[test]
    fn variables_fit_the_quotes_around_them() {
        let context = json!({ "v": "x" });
        let script = |text: &str| render_shell(text, &context, Shell::Posix).script;
        assert_eq!(script(r#"echo "a {{v}} b" '{{v}}' x{{v}}"#), r#"echo "a ""$SOMME_VALUE_1"" b" ''"$SOMME_VALUE_1"'' x"$SOMME_VALUE_1""#);
        // An apostrophe in a comment opens no quote.
        assert_eq!(script("# the server's disk\ndf {{v}}"), "# the server's disk\ndf \"$SOMME_VALUE_1\"");
        assert_eq!(script(r#"echo \"{{v}}"#), r#"echo \""$SOMME_VALUE_1""#);
        assert_eq!(script("cat <<-EOF > f\n\tv={{v}}\n\tEOF\necho {{v}}"), "cat <<-EOF > f\n\tv=${SOMME_VALUE_1}\n\tEOF\necho \"$SOMME_VALUE_1\"");
        // Nothing expands in a quoted here-document, so the value goes in
        // as written, unless a line of it would end the text early.
        assert_eq!(script("cat <<'EOF'\nv={{v}}\nEOF"), "cat <<'EOF'\nv=x\nEOF");
        let early = json!({ "v": "a\nEOF\nrm -rf ~" });
        assert_eq!(render_shell("cat <<'EOF'\n{{v}}\nEOF", &early, Shell::Posix).script, "cat <<'EOF'\n${SOMME_VALUE_1}\nEOF");
    }

    #[test]
    fn values_read_as_code_again_need_an_ok() {
        let risky = json!({ "v": "x; rm -rf ~" });
        let plain = json!({ "v": "web-1.example.test" });
        let read = |text: &str, context: &Value| render_shell(text, context, Shell::Posix).read_as_code;
        for text in [
            "ssh web 'cd {{v}}'",
            "sh -c 'echo {{v}}'",
            "/bin/bash -lc \"echo {{v}}\"",
            "eval {{v}}",
            "echo {{v}} | sudo bash",
            "bash <<'EOF'\n{{v}}\nEOF",
            "python3 -c 'print(\"{{v}}\")'",
            "sudo -i echo {{v}}",
            "CMD /C echo {{v}}",
        ] {
            assert!(read(text, &risky), "{text}");
            assert!(!read(text, &plain), "{text}");
        }
        for text in ["echo {{v}}", "git pull origin {{v}}", "curl -d '{\"a\":\"{{v}}\"}' https://example.test", "cat <<EOF\n{{v}}\nEOF"] {
            assert!(!read(text, &risky), "{text}");
        }
    }
}
