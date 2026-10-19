//! Takes secrets out of text before it goes to an AI service: the user's
//! own saved values, matched exactly, and anything shaped like a key, token
//! or password.

use std::sync::LazyLock;

use regex::{Captures, Regex};

pub const HIDDEN: &str = "[hidden]";

/// Saved values shorter than this are too likely to match ordinary words.
const SHORTEST_EXACT: usize = 6;

struct Shape {
    regex: Regex,
    /// What replaces a match. `$name` groups are kept.
    with: &'static str,
}

static SHAPES: LazyLock<Vec<Shape>> = LazyLock::new(|| {
    let shape = |pattern: &str, with: &'static str| Shape { regex: Regex::new(pattern).expect("secret pattern"), with };
    vec![
        // A whole private key, PEM or OpenSSH.
        shape(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)", "[hidden private key]"),
        // name=value and name: value, where the name says it is secret.
        shape(
            r#"(?i)(?P<name>\b[A-Z0-9_.\-]*(?:password|passwd|pwd|secret|token|api[_\-]?key|access[_\-]?key|private[_\-]?key|client[_\-]?secret|authorization|credential)[A-Z0-9_.\-]*["']?\s*(?:=|:)\s*)(?:(?:bearer|basic|token)\s+[^\s'"]+|"[^"\n]*"|'[^'\n]*'|[^\s,;&'"]+)"#,
            "${name}[hidden]",
        ),
        // --password value and --password=value.
        shape(
            r"(?i)(?P<name>--?(?:password|passwd|pass|token|secret|api-key|apikey|access-key|auth-token)(?:=|\s+))\S+",
            "${name}[hidden]",
        ),
        // mysql -pSECRET.
        shape(r"(?P<name>\bmysql(?:dump|admin)?\b[^\n]*?\s-p)[^\s-]\S*", "${name}[hidden]"),
        shape(r"(?i)(?P<name>\bbearer\s+)[A-Za-z0-9\-._~+/]+=*", "${name}[hidden]"),
        shape(r"(?i)(?P<name>\bbasic\s+)[A-Za-z0-9+/]{8,}=*", "${name}[hidden]"),
        // user:password@ in addresses.
        shape(r"(?P<name>\b[a-z][a-z0-9+.\-]*://[^/\s:@]+:)[^@\s/]+@", "${name}[hidden]@"),
        // Tokens known by their shape.
        shape(r"\bgh[pousr]_[A-Za-z0-9]{20,}", HIDDEN),
        shape(r"\bgithub_pat_[A-Za-z0-9_]{20,}", HIDDEN),
        shape(r"\bglpat-[A-Za-z0-9\-_]{20,}", HIDDEN),
        shape(r"\bxox[abprs]-[A-Za-z0-9\-]{10,}", HIDDEN),
        shape(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", HIDDEN),
        shape(r"\bsk-[A-Za-z0-9_\-]{20,}", HIDDEN),
        shape(r"\bAIza[0-9A-Za-z\-_]{35}", HIDDEN),
        shape(r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}", HIDDEN),
    ]
});

/// Cleans text. Holds the user's saved secrets to match them exactly.
#[derive(Clone, Debug, Default)]
pub struct Cleaner {
    exact: Vec<String>,
}

/// `value`, and how it reads once or twice inside sh single quotes, where
/// each `'` turns into `'\''`, as in a command's values or a sudo step.
pub fn with_quoted(value: &str) -> Vec<String> {
    let once = value.replace('\'', r"'\''");
    if once == value {
        return vec![value.to_owned()];
    }
    let twice = once.replace('\'', r"'\''");
    vec![value.to_owned(), once, twice]
}

impl Cleaner {
    pub fn new(exact: impl IntoIterator<Item = String>) -> Self {
        let mut exact: Vec<String> = exact
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| value.chars().count() >= SHORTEST_EXACT)
            .flat_map(|value| with_quoted(&value))
            .collect();
        // Longest first, so a secret that holds another goes whole.
        exact.sort_by_key(|value| std::cmp::Reverse(value.len()));
        exact.dedup();
        Self { exact }
    }

    pub fn clean(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for value in &self.exact {
            if text.contains(value.as_str()) {
                text = text.replace(value.as_str(), HIDDEN);
            }
        }
        for shape in SHAPES.iter() {
            if shape.regex.is_match(&text) {
                text = shape.regex.replace_all(&text, |caps: &Captures| {
                    let mut out = String::new();
                    caps.expand(shape.with, &mut out);
                    out
                }).into_owned();
            }
        }
        text
    }

    /// Cleans every string inside a JSON value.
    pub fn clean_json(&self, value: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::String(text) => Value::String(self.clean(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.clean_json(item)).collect()),
            Value::Object(fields) => {
                Value::Object(fields.iter().map(|(key, item)| (key.clone(), self.clean_json(item))).collect())
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(text: &str) -> String {
        Cleaner::default().clean(text)
    }

    #[test]
    fn secrets_in_shell_quotes_are_hidden() {
        let cleaner = Cleaner::new(["it's-s3cret".to_owned()]);
        let once = "SOMME_VALUE_1='it'\\''s-s3cret'\ndeploy";
        assert_eq!(cleaner.clean(once), "SOMME_VALUE_1='[hidden]'\ndeploy");
        let sudo = format!("sudo -n sh -c '{}'", once.replace('\'', r"'\''"));
        assert!(!cleaner.clean(&sudo).contains("s3cret"), "{}", cleaner.clean(&sudo));
    }

    #[test]
    fn saved_values_are_hidden_exactly() {
        let cleaner = Cleaner::new(["hunter2-long".to_owned(), "abc".to_owned()]);
        assert_eq!(cleaner.clean("echo hunter2-long | sudo -S ls"), "echo [hidden] | sudo -S ls");
        // Too short to match safely.
        assert_eq!(cleaner.clean("abc"), "abc");
    }

    #[test]
    fn secret_shapes_are_hidden() {
        assert_eq!(clean("DB_PASSWORD=s3cret!x psql"), "DB_PASSWORD=[hidden] psql");
        assert_eq!(clean(r#"api_key: "abc def""#), "api_key: [hidden]");
        assert_eq!(clean("mysql -u root -pS3cr3t db"), "mysql -u root -p[hidden] db");
        assert_eq!(clean("curl -H 'Authorization: Bearer abc.def-ghi'"), "curl -H 'Authorization: [hidden]'");
        assert_eq!(clean("tool --token abc123 --name x"), "tool --token [hidden] --name x");
        assert_eq!(clean("git clone https://me:pa55@example.test/r.git"), "git clone https://me:[hidden]@example.test/r.git");
        assert_eq!(clean("export GH=ghp_abcdefghijklmnopqrstuvwxyz0123"), "export GH=[hidden]");
        assert_eq!(clean("key AKIAABCDEFGHIJKLMNOP here"), "key [hidden] here");
        let pem = "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3Blbn\n-----END OPENSSH PRIVATE KEY-----\nz";
        assert_eq!(clean(pem), "a\n[hidden private key]\nz");
    }

    #[test]
    fn ordinary_text_stays() {
        let text = "SELECT * FROM users WHERE id = 3; ls -la /etc/nginx; author: Ada";
        assert_eq!(clean(text), text);
    }

    #[test]
    fn json_strings_are_cleaned() {
        let value = serde_json::json!({ "command": "TOKEN=abcdef123 run", "n": 3 });
        let cleaned = Cleaner::default().clean_json(&value);
        assert_eq!(cleaned["command"], "TOKEN=[hidden] run");
        assert_eq!(cleaned["n"], 3);
    }
}
