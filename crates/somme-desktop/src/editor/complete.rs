//! Suggestions narrowed to the word being typed, best first. A language server
//! sends every name that fits the spot, often in its own order; editors pick
//! out the ones matching what was typed, as more letters come.

use gpui_kit::component::input::{Rope, RopeExt as _};
use lsp_types::{CompletionItem, CompletionTextEdit, Range, TextEdit};

/// Where the word ending at `offset` starts.
pub fn word_start(text: &Rope, offset: usize) -> usize {
    let len: usize = text
        .chars_at(offset)
        .reversed()
        .take_while(|ch| is_word(*ch))
        .map(char::len_utf8)
        .sum();
    offset - len
}

pub fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// The text of the line before `offset`.
pub fn line_before(text: &Rope, offset: usize) -> String {
    let line = text.offset_to_position(offset).line as usize;
    text.slice(text.line_start_offset(line)..offset).to_string()
}

/// The suggestions matching `word`, best first, each set to replace `typed`:
/// the word, from its start to the cursor.
pub fn narrow(items: &[CompletionItem], word: &str, typed: Range) -> Vec<CompletionItem> {
    let mut found: Vec<(u8, &CompletionItem)> = items
        .iter()
        .filter_map(|item| {
            let names = [item.filter_text.as_deref(), Some(item.label.as_str())];
            names.into_iter().flatten().filter_map(|name| rank(name, word)).min().map(|rank| (rank, item))
        })
        .collect();
    found.sort_by(|(a_rank, a), (b_rank, b)| {
        let order = |item: &CompletionItem| item.sort_text.clone().unwrap_or_else(|| item.label.clone());
        a_rank.cmp(b_rank).then_with(|| order(a).cmp(&order(b))).then_with(|| a.label.cmp(&b.label))
    });
    found.into_iter().map(|(_, item)| fit(item, word, typed)).collect()
}

/// How well `name` matches `word`, lower being better: the same name, a name
/// starting with it, the same in other case, its letters at the name's word
/// starts ("fe" for "forEach"), then its letters anywhere in order.
fn rank(name: &str, word: &str) -> Option<u8> {
    if word.is_empty() {
        return Some(1);
    }
    if name == word {
        return Some(0);
    }
    if name.starts_with(word) {
        return Some(1);
    }
    let lower = word.to_lowercase();
    if name.to_lowercase().starts_with(&lower) {
        return Some(2);
    }
    // Only from the start of the name, past marks like `_`.
    let name = name.trim_start_matches(|ch: char| !ch.is_alphanumeric());
    let first = lower.chars().next()?;
    if !name.chars().next().is_some_and(|ch| ch.to_lowercase().eq(first.to_lowercase())) {
        return None;
    }
    if in_order(&starts(name), &lower) {
        return Some(3);
    }
    in_order(&name.to_lowercase(), &lower).then_some(4)
}

/// The first letter of each word in a name: "fE" for "forEach", "gu" for "get_url".
fn starts(name: &str) -> String {
    let mut previous: Option<char> = None;
    let mut letters = String::new();
    for ch in name.chars() {
        let starts = match previous {
            None => true,
            Some(before) => {
                (ch.is_uppercase() && !before.is_uppercase()) || (!before.is_alphanumeric() && ch.is_alphanumeric())
            }
        };
        if starts {
            letters.extend(ch.to_lowercase());
        }
        previous = Some(ch);
    }
    letters
}

/// Whether `word`'s letters are all in `name`, in order.
fn in_order(name: &str, word: &str) -> bool {
    let mut letters = name.chars();
    word.chars().all(|ch| letters.any(|letter| letter == ch))
}

/// The item, set to replace what was typed of the word, and to color as much
/// of its name as matches.
fn fit(item: &CompletionItem, word: &str, typed: Range) -> CompletionItem {
    let mut item = item.clone();
    item.text_edit = Some(match item.text_edit.take() {
        // The server's edit ends where the cursor was when it was asked.
        Some(CompletionTextEdit::Edit(mut edit)) => {
            edit.range.end = typed.end;
            CompletionTextEdit::Edit(edit)
        }
        Some(CompletionTextEdit::InsertAndReplace(mut edit)) => {
            edit.insert.end = typed.end;
            edit.replace.end = typed.end;
            CompletionTextEdit::InsertAndReplace(edit)
        }
        None => {
            let new_text = item.insert_text.clone().unwrap_or_else(|| item.label.clone());
            CompletionTextEdit::Edit(TextEdit { range: typed, new_text })
        }
    });
    // The menu colors as many letters of the name as the filter text has.
    let starts = item.label.get(..word.len()).is_some_and(|start| start.eq_ignore_ascii_case(word));
    item.filter_text = Some(if starts { word.to_owned() } else { String::new() });
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_owned(),
            ..Default::default()
        }
    }

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    fn typed(start: u32, end: u32) -> Range {
        Range::new(Position::new(0, start), Position::new(0, end))
    }

    #[test]
    fn the_typed_name_comes_first_and_others_go() {
        let items: Vec<_> = ["at", "concat", "entries", "find", "findIndex", "forEach", "Foreign"].map(item).into();
        assert_eq!(labels(&narrow(&items, "forEach", typed(3, 10))), ["forEach"]);
        assert_eq!(labels(&narrow(&items, "f", typed(3, 4))), ["find", "findIndex", "forEach", "Foreign"]);
        assert_eq!(labels(&narrow(&items, "fo", typed(3, 5))), ["forEach", "Foreign"]);
        // Letters at word starts first: for-Each, then those anywhere in the name.
        assert_eq!(labels(&narrow(&items, "fe", typed(3, 5))), ["forEach", "Foreign", "findIndex"]);
        assert_eq!(labels(&narrow(&items, "ach", typed(3, 6))), Vec::<&str>::new());
        assert_eq!(labels(&narrow(&items, "", typed(3, 3))).len(), items.len());
    }

    #[test]
    fn the_servers_order_breaks_ties() {
        let mut first = item("zebra");
        first.sort_text = Some("0".into());
        let mut second = item("zap");
        second.sort_text = Some("1".into());
        assert_eq!(labels(&narrow(&[second, first], "z", typed(0, 1))), ["zebra", "zap"]);
    }

    #[test]
    fn a_pick_replaces_the_typed_letters_only() {
        let mut plain = item("forEach");
        plain.insert_text = Some("forEach".into());
        let [picked] = narrow(&[plain], "forE", typed(3, 7)).try_into().expect("one");
        let Some(CompletionTextEdit::Edit(edit)) = picked.text_edit else { panic!("an edit") };
        assert_eq!((edit.range, edit.new_text.as_str()), (typed(3, 7), "forEach"));
        assert_eq!(picked.filter_text.as_deref(), Some("forE"));

        // A server's edit from when fewer letters were typed reaches the cursor.
        let mut served = item("length");
        served.text_edit = Some(CompletionTextEdit::Edit(TextEdit { range: typed(3, 3), new_text: "length".into() }));
        let [picked] = narrow(&[served], "le", typed(3, 5)).try_into().expect("one");
        let Some(CompletionTextEdit::Edit(edit)) = picked.text_edit else { panic!("an edit") };
        assert_eq!(edit.range, typed(3, 5));
    }

    #[test]
    fn words_start_after_dots_and_spaces() {
        let text = Rope::from("  [].forE");
        assert_eq!(word_start(&text, text.len()), 5);
        assert_eq!(line_before(&text, 5), "  [].");
        let text = Rope::from("a\nlet $el");
        assert_eq!(word_start(&text, text.len()), 6);
        assert_eq!(line_before(&text, 6), "let ");
    }
}
