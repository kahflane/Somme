//! What the assistant remembers about this user and their machines: short
//! facts it chose to keep with `remember`, like where their projects are.
//! Each chat starts with them. The user sees and deletes them in
//! Settings → AI.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Facts kept; older ones go.
const KEPT: usize = 60;
/// Facts a chat starts with.
pub const SHOWN: usize = 30;
/// Characters of one fact.
const FACT: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub at: i64,
    pub text: String,
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("memory.json")
}

pub fn load(data_dir: &Path) -> Vec<Fact> {
    std::fs::read(path(data_dir)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// The newest facts, for a chat.
pub fn newest(data_dir: &Path) -> Vec<String> {
    let facts = load(data_dir);
    facts.iter().skip(facts.len().saturating_sub(SHOWN)).map(|fact| fact.text.clone()).collect()
}

/// Keeps `text`. The same fact again moves to the end.
pub fn remember(data_dir: &Path, text: &str) -> Result<String, String> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(FACT).collect();
    if text.is_empty() {
        return Err("the fact is empty".into());
    }
    let mut facts = load(data_dir);
    facts.retain(|fact| !fact.text.eq_ignore_ascii_case(&text));
    facts.push(Fact { at: somme_automation::engine::now_ms(), text: text.clone() });
    let extra = facts.len().saturating_sub(KEPT);
    facts.drain(..extra);
    save(data_dir, &facts)?;
    Ok(text)
}

/// Forgets one fact.
pub fn forget(data_dir: &Path, text: &str) {
    let mut facts = load(data_dir);
    facts.retain(|fact| fact.text != text);
    if let Err(err) = save(data_dir, &facts) {
        tracing::warn!(%err, "could not forget a fact");
    }
}

/// Written whole, then moved over the old file.
fn save(data_dir: &Path, facts: &[Fact]) -> Result<(), String> {
    write(&path(data_dir), facts)
}

fn write(file: &Path, facts: &[Fact]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(facts).map_err(|err| err.to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("could not save it: {err}"))?;
    }
    let fresh = file.with_extension("json.new");
    std::fs::write(&fresh, bytes).and_then(|()| std::fs::rename(&fresh, file)).map_err(|err| format!("could not save it: {err}"))
}

/// Notes an automation's agent keeps for its next runs.
const NOTES: usize = 10;

fn notes_path(data_dir: &Path, automation: &str) -> PathBuf {
    let name: String = automation.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    data_dir.join("automations").join("notes").join(format!("{name}.json"))
}

/// What earlier runs of `automation` noted, oldest first.
pub fn notes(data_dir: &Path, automation: &str) -> Vec<String> {
    let facts: Vec<Fact> = std::fs::read(notes_path(data_dir, automation)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    facts.into_iter().map(|fact| fact.text).collect()
}

/// Keeps a note for `automation`'s next runs; the newest few stay.
pub fn note(data_dir: &Path, automation: &str, text: &str) -> Result<(), String> {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(FACT).collect();
    if text.is_empty() || automation.is_empty() {
        return Err("the note is empty".into());
    }
    let file = notes_path(data_dir, automation);
    let mut facts: Vec<Fact> = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    facts.retain(|fact| !fact.text.eq_ignore_ascii_case(&text));
    facts.push(Fact { at: somme_automation::engine::now_ms(), text });
    let extra = facts.len().saturating_sub(NOTES);
    facts.drain(..extra);
    write(&file, &facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_are_kept_newest_last_and_once() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..65 {
            remember(dir.path(), &format!("fact {n}")).unwrap();
        }
        remember(dir.path(), "  Fact   10 ").unwrap();
        let facts = load(dir.path());
        assert_eq!(facts.len(), KEPT);
        assert_eq!(facts.last().unwrap().text, "Fact 10");
        assert_eq!(newest(dir.path()).len(), SHOWN);
        forget(dir.path(), "Fact 10");
        assert!(!load(dir.path()).iter().any(|fact| fact.text == "Fact 10"));
        assert!(remember(dir.path(), " ").is_err());
        for n in 0..12 {
            note(dir.path(), "a1/../b", &format!("run {n}")).unwrap();
        }
        assert_eq!(notes(dir.path(), "a1/../b").len(), NOTES);
        assert!(dir.path().join("automations/notes/a1b.json").exists());
    }
}
