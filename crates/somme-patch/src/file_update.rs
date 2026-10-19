// Changed by Somme from OpenAI Codex (codex-rs/apply-patch). Apache-2.0;
// see LICENSE and NOTICE in this crate.

use crate::UpdateFileChunk;
use crate::seek_sequence;
use crate::text_file::Replacement;
use crate::text_file::SourceFile;

/// The new contents of a file after applying the chunks to `original`.
pub(crate) fn new_contents(original: &str, path: &str, chunks: &[UpdateFileChunk]) -> Result<String, String> {
    let mut source_file = SourceFile::parse(original);
    let original_lines = source_file.line_texts();
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    source_file.apply_replacements(&replacements);
    Ok(source_file.into_contents())
}

/// Compute a list of replacements needed to transform `original_lines` into the
/// new lines, given the patch `chunks`. Each replacement is returned as
/// `(start_index, old_len, new_lines)`.
fn compute_replacements(original_lines: &[String], path: &str, chunks: &[UpdateFileChunk]) -> Result<Vec<Replacement>, String> {
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // If a chunk has a `change_context`, we use seek_sequence to find it, then
        // adjust our `line_index` to continue from there.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(original_lines, std::slice::from_ref(ctx_line), line_index, /*eof*/ false) {
                line_index = idx + 1;
            } else {
                return Err(format!("Failed to find context '{ctx_line}' in {path}"));
            }
        }

        if chunk.old_lines.is_empty() {
            replacements.push((original_lines.len(), 0, chunk.new_lines.clone()));
            continue;
        }

        // Otherwise, try to match the existing lines in the file with the old lines
        // from the chunk. If found, schedule that region for replacement.
        // In many real-world diffs the last element of `old_lines` is an *empty*
        // string representing the terminating newline of the region being
        // replaced. This sentinel is not present in `original_lines` because
        // `SourceFile` stores the terminator on the preceding line. If a direct
        // search fails and the pattern ends with an empty string, retry without
        // that final element so changes at the end of the file are found.
        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        let Some(start_idx) = found else {
            return Err(format!("Failed to find expected lines in {}:\n{}", path, chunk.old_lines.join("\n")));
        };
        // Context lines occur in both sides of a patch chunk. Keep those
        // original lines in place so their exact contents and terminators
        // survive, especially when the file has mixed line endings.
        let mut old_start = 0;
        let mut new_start = 0;
        for &(old_context, new_context) in &chunk.context_line_indices {
            // A trailing empty context line can be removed from `pattern`
            // and `new_slice` above when it represents the final newline.
            if old_context >= pattern.len() || new_context >= new_slice.len() {
                break;
            }
            if old_start != old_context || new_start != new_context {
                replacements.push((start_idx + old_start, old_context - old_start, new_slice[new_start..new_context].to_vec()));
            }
            old_start = old_context + 1;
            new_start = new_context + 1;
        }
        if old_start != pattern.len() || new_start != new_slice.len() {
            replacements.push((start_idx + old_start, pattern.len() - old_start, new_slice[new_start..].to_vec()));
        }
        line_index = start_idx + pattern.len();
    }

    replacements.sort_by_key(|(index, _, _)| *index);

    Ok(replacements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_patch;
    use crate::parser::Hunk;

    fn chunks(patch: &str) -> Vec<UpdateFileChunk> {
        match parse_patch(patch).unwrap().remove(0) {
            Hunk::UpdateFile { chunks, .. } => chunks,
            other => panic!("not an update: {other:?}"),
        }
    }

    #[test]
    fn context_lines_keep_their_line_endings() {
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n one\n-two\n+TWO\n three\n*** End Patch";
        assert_eq!(new_contents("one\r\ntwo\nthree\r\n", "a.txt", &chunks(patch)).unwrap(), "one\r\nTWO\r\nthree\r\n");
    }

    #[test]
    fn missing_lines_are_an_error() {
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-nope\n+yes\n*** End Patch";
        assert_eq!(new_contents("one\n", "a.txt", &chunks(patch)), Err("Failed to find expected lines in a.txt:\nnope".to_string()));
    }

    #[test]
    fn typographic_dashes_still_match() {
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-a - b\n+a + b\n*** End Patch";
        assert_eq!(new_contents("a \u{2013} b\n", "a.txt", &chunks(patch)).unwrap(), "a + b\n");
    }
}
