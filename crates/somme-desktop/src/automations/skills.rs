//! Skills: know-how for kinds of jobs, kept out of the builder's prompt so
//! the prompt holds only ways of working. Each is a SKILL.md in the format
//! Hermes Agent, Claude and others share: a header with a name and a
//! description, then the text, and maybe more files beside it. The builder
//! sees every skill's name and description, and reads the ones that fit.
//! Somme comes with some; the user's own, in `<data dir>/skills`, come first
//! and replace Somme's of the same name.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Characters of a skill's text read at once.
const READ: usize = 12_000;
/// Characters of a description in the list.
const DESCRIBED: usize = 240;
/// Skills listed for the builder, at most.
const LISTED: usize = 60;
/// Bytes of one file read from the user's skills.
const FILE_LIMIT: u64 = 64 * 1024;
/// Other files read beside one skill, at most.
const FILES: usize = 12;
/// Folders gone into to find skills: a skill, or a group of them.
const DEPTH: usize = 3;

const BUILT_IN: &[&str] = &[
    include_str!("../../skills/freeing-disk-space/SKILL.md"),
    include_str!("../../skills/backups/SKILL.md"),
    include_str!("../../skills/databases/SKILL.md"),
    include_str!("../../skills/deploys/SKILL.md"),
    include_str!("../../skills/watching-and-alerts/SKILL.md"),
    include_str!("../../skills/certificates/SKILL.md"),
    include_str!("../../skills/reports/SKILL.md"),
    include_str!("../../skills/containers/SKILL.md"),
    include_str!("../../skills/git-repos/SKILL.md"),
    include_str!("../../skills/log-files/SKILL.md"),
    include_str!("../../skills/users-and-access/SKILL.md"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub text: String,
    /// Other files beside it, like references/setup.md: path and text.
    pub files: Vec<(String, String)>,
    /// The user's, not Somme's.
    pub own: bool,
}

/// The skills Somme comes with.
pub fn built_in() -> Vec<Skill> {
    BUILT_IN.iter().filter_map(|text| parse(text, "", false)).collect()
}

/// Where the user's skills go.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

/// The user's skills, then Somme's they don't replace.
pub fn all(data_dir: &Path) -> Vec<Skill> {
    let mut skills = own(data_dir);
    for skill in built_in() {
        if !skills.iter().any(|mine| mine.name.eq_ignore_ascii_case(&skill.name)) {
            skills.push(skill);
        }
    }
    skills
}

/// The user's skills: each folder with a SKILL.md, alone or in a group.
pub fn own(data_dir: &Path) -> Vec<Skill> {
    let mut found = Vec::new();
    gather(&folder(data_dir), DEPTH, &mut found);
    found.sort_by_key(|skill| skill.name.to_lowercase());
    found.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    found
}

fn gather(dir: &Path, depth: usize, found: &mut Vec<Skill>) {
    let skill_file = dir.join("SKILL.md");
    if is_file(&skill_file) {
        if let Some(text) = read_text(&skill_file) {
            let name = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(mut skill) = parse(&text, &name, true) {
                skill.files = beside(dir);
                found.push(skill);
            }
        }
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir())).map(|entry| entry.path()).collect();
    dirs.sort();
    for dir in dirs {
        gather(&dir, depth - 1, found);
    }
}

/// A plain file, not a link.
fn is_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > FILE_LIMIT {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

/// The text files beside a skill, a folder deep at most.
fn beside(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut add = |path: &Path| {
        let wanted = path.extension().is_some_and(|ext| matches!(ext.to_string_lossy().to_lowercase().as_str(), "md" | "txt" | "sh" | "json" | "yaml" | "yml"));
        if !wanted || path.file_name().is_some_and(|name| name == "SKILL.md") || files.len() >= FILES {
            return;
        }
        if let (Ok(short), Some(text)) = (path.strip_prefix(dir), read_text(path)) {
            files.push((short.to_string_lossy().replace('\\', "/"), text));
        }
    };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            let Ok(inner) = std::fs::read_dir(&path) else { continue };
            let mut inner: Vec<PathBuf> = inner.filter_map(Result::ok).map(|entry| entry.path()).collect();
            inner.sort();
            for path in inner.iter().filter(|path| is_file(path)) {
                add(path);
            }
        } else if is_file(&path) {
            add(&path);
        }
    }
    files
}

/// A SKILL.md: the header's name and description, then the text. Without
/// a header, the folder names it and its first line describes it.
fn parse(text: &str, folder: &str, own: bool) -> Option<Skill> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (header, body) = match text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---")) {
        Some((header, body)) => (header.to_owned(), body.split_once('\n').map_or("", |(_, body)| body).to_owned()),
        None => (String::new(), text.clone()),
    };
    let name = field(&header, "name").unwrap_or_else(|| folder.to_owned());
    let description = field(&header, "description").or_else(|| {
        body.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned)
    })?;
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 64 {
        return None;
    }
    Some(Skill { name, description: description.trim().to_owned(), text: body.trim().to_owned(), files: Vec::new(), own })
}

/// A top-level `key: value` from a YAML header, quoted or folded.
fn field(header: &str, key: &str) -> Option<String> {
    let mut lines = header.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(value) = line.strip_prefix(key).and_then(|rest| rest.strip_prefix(':')) else { continue };
        let value = value.trim();
        if value.is_empty() || matches!(value, ">" | "|" | ">-" | "|-" | ">+" | "|+") {
            let mut folded = Vec::new();
            while let Some(next) = lines.next_if(|next| next.starts_with(' ') || next.starts_with('\t') || next.trim().is_empty()) {
                folded.push(next.trim());
            }
            let folded = folded.join(" ").trim().to_owned();
            return (!folded.is_empty()).then_some(folded);
        }
        let unquoted = value
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))
            .unwrap_or(value);
        return Some(unquoted.replace("\\\"", "\"").replace("''", "'"));
    }
    None
}

/// The list the builder sees: each skill's name and what it is for.
pub fn listed(skills: &[Skill]) -> String {
    skills
        .iter()
        .take(LISTED)
        .map(|skill| {
            let description: String = skill.description.chars().take(DESCRIBED).collect();
            format!("- {}: {description}", skill.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What `read_skill` gives back: a skill's text, or one file beside it.
pub fn read(skills: &[Skill], args: &Value) -> (Option<String>, String) {
    let wanted = args["name"].as_str().unwrap_or_default().trim();
    let Some(skill) = skills.iter().find(|skill| skill.name.eq_ignore_ascii_case(wanted)) else {
        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        return (None, format!("There is no skill called \u{201c}{wanted}\u{201d}. The skills are: {}.", names.join(", ")));
    };
    let file = args["file"].as_str().map(str::trim).filter(|file| !file.is_empty());
    let text = match file {
        Some(file) => match skill.files.iter().find(|(path, _)| path == file) {
            Some((_, text)) => cut(text),
            None => return (Some(skill.name.clone()), format!("{} has no file {file}.{}", skill.name, files_line(skill))),
        },
        None => format!("{}{}", cut(&skill.text), files_line(skill)),
    };
    (Some(skill.name.clone()), text)
}

fn files_line(skill: &Skill) -> String {
    if skill.files.is_empty() {
        return String::new();
    }
    let paths: Vec<&str> = skill.files.iter().map(|(path, _)| path.as_str()).collect();
    format!("\n\nFiles beside it, read with read_skill and `file`: {}.", paths.join(", "))
}

fn cut(text: &str) -> String {
    if text.chars().count() <= READ {
        return text.to_owned();
    }
    format!("{}\n(the rest was cut)", text.chars().take(READ).collect::<String>())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn somme_skills_all_read() {
        let skills = built_in();
        assert_eq!(skills.len(), BUILT_IN.len());
        for skill in &skills {
            assert!(!skill.description.is_empty() && skill.text.len() > 200, "{}", skill.name);
            assert!(skill.name.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "{}", skill.name);
        }
    }

    #[test]
    fn headers_are_read_quoted_folded_or_missing() {
        let skill = parse("---\nname: \"log-files\"\ndescription: >\n  Keep logs\n  small.\nversion: 1\n---\n# Logs\nBody.\n", "x", true).unwrap();
        assert_eq!((skill.name.as_str(), skill.description.as_str(), skill.text.as_str()), ("log-files", "Keep logs small.", "# Logs\nBody."));
        let bare = parse("# Title\n\nWhat it does.\n\nMore.", "my-skill", true).unwrap();
        assert_eq!((bare.name.as_str(), bare.description.as_str()), ("my-skill", "What it does."));
    }

    #[test]
    fn the_users_skills_come_first_and_replace_somme_s() {
        let dir = tempfile::tempdir().unwrap();
        let mine = folder(dir.path()).join("ops").join("backups");
        std::fs::create_dir_all(mine.join("references")).unwrap();
        std::fs::write(mine.join("SKILL.md"), "---\nname: backups\ndescription: Our backups go to the NAS.\n---\nUse rsync to nas-1.\n").unwrap();
        std::fs::write(mine.join("references").join("nas.md"), "nas-1 is at 10.0.0.5").unwrap();
        let skills = all(dir.path());
        let backups: Vec<&Skill> = skills.iter().filter(|skill| skill.name == "backups").collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].own && skills[0].name == "backups");
        assert_eq!(read(&skills, &json!({ "name": "Backups" })).1, "Use rsync to nas-1.\n\nFiles beside it, read with read_skill and `file`: references/nas.md.");
        assert_eq!(read(&skills, &json!({ "name": "backups", "file": "references/nas.md" })).1, "nas-1 is at 10.0.0.5");
        assert!(read(&skills, &json!({ "name": "nope" })).1.starts_with("There is no skill called"));
        assert!(listed(&skills).contains("- backups: Our backups go to the NAS."));
    }
}
