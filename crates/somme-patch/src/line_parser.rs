// Changed by Somme from OpenAI Codex (codex-rs/apply-patch). Apache-2.0;
// see LICENSE and NOTICE in this crate.

//! Reads a patch line by line into hunks.

use crate::parser::ADD_FILE_MARKER;
use crate::parser::BEGIN_PATCH_MARKER;
use crate::parser::CHANGE_CONTEXT_MARKER;
use crate::parser::DELETE_FILE_MARKER;
use crate::parser::EMPTY_CHANGE_CONTEXT_MARKER;
use crate::parser::END_PATCH_MARKER;
use crate::parser::EOF_MARKER;
use crate::parser::Hunk;
use crate::parser::MOVE_TO_MARKER;
use crate::parser::ParseError;
use crate::parser::UPDATE_FILE_MARKER;
use crate::parser::UpdateFileChunk;

use Hunk::*;
use ParseError::*;

const NOT_A_HUNK_HEADER: &str = "is not a valid hunk header. Valid hunk headers: '*** Add File: {path}', '*** Delete File: {path}', '*** Update File: {path}'";

#[derive(Debug, Default, Clone)]
pub(crate) struct LineParser {
    line_buffer: String,
    mode: Mode,
    hunks: Vec<Hunk>,
    line_number: usize,
}

#[derive(Debug, Default, Clone, Copy)]
enum Mode {
    #[default]
    NotStarted,
    StartedPatch,
    AddFile,
    DeleteFile,
    UpdateFile {
        hunk_line_number: usize,
    },
    EndedPatch,
}

impl LineParser {
    fn ensure_update_hunk_is_not_empty(&self, line: &str) -> Result<(), ParseError> {
        if let Some(UpdateFile { path, chunks, .. }) = self.hunks.last() {
            if chunks.is_empty()
                && let Mode::UpdateFile { hunk_line_number } = self.mode
            {
                return Err(InvalidHunkError { message: format!("Update file hunk for path '{path}' is empty"), line_number: hunk_line_number });
            }
            if chunks.last().is_some_and(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty()) {
                if line == END_PATCH_MARKER {
                    return Err(InvalidHunkError { message: "Update hunk does not contain any lines".to_string(), line_number: self.line_number });
                }
                return Err(InvalidHunkError {
                    message: format!(
                        "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                    ),
                    line_number: self.line_number,
                });
            }
        }
        Ok(())
    }

    fn handle_hunk_headers_and_end_patch(&mut self, trimmed: &str) -> Result<bool, ParseError> {
        if trimmed == END_PATCH_MARKER {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.mode = Mode::EndedPatch;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(AddFile { path: path.to_owned(), contents: String::new() });
            self.mode = Mode::AddFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(DeleteFile { path: path.to_owned() });
            self.mode = Mode::DeleteFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(UpdateFile { path: path.to_owned(), move_path: None, chunks: Vec::new() });
            self.mode = Mode::UpdateFile { hunk_line_number: self.line_number };
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) fn push(&mut self, text: &str) -> Result<(), ParseError> {
        for ch in text.chars() {
            if ch == '\n' {
                let mut line = std::mem::take(&mut self.line_buffer);
                line.truncate(line.strip_suffix('\r').map_or(line.len(), str::len));
                self.line_number += 1;
                self.process_line(&line)?;
            } else {
                self.line_buffer.push(ch);
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<Hunk>, ParseError> {
        if !self.line_buffer.is_empty() {
            let line = std::mem::take(&mut self.line_buffer);
            self.line_number += 1;
            if line.trim() == END_PATCH_MARKER {
                self.ensure_update_hunk_is_not_empty(line.trim())?;
                self.mode = Mode::EndedPatch;
            } else {
                self.process_line(&line)?;
            }
        }

        if !matches!(self.mode, Mode::EndedPatch) {
            return Err(InvalidPatchError("The last line of the patch must be '*** End Patch'".to_string()));
        }

        Ok(std::mem::take(&mut self.hunks))
    }

    fn process_line(&mut self, line: &str) -> Result<(), ParseError> {
        let trimmed = line.trim();
        match self.mode {
            Mode::NotStarted => {
                if trimmed == BEGIN_PATCH_MARKER {
                    self.mode = Mode::StartedPatch;
                    return Ok(());
                }
                Err(InvalidPatchError("The first line of the patch must be '*** Begin Patch'".to_string()))
            }
            Mode::StartedPatch | Mode::DeleteFile => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                Err(InvalidHunkError { message: format!("'{trimmed}' {NOT_A_HUNK_HEADER}"), line_number: self.line_number })
            }
            Mode::AddFile => {
                if self.handle_hunk_headers_and_end_patch(trimmed)? {
                    return Ok(());
                }
                if let Some(line_to_add) = line.strip_prefix('+')
                    && let Some(AddFile { contents, .. }) = self.hunks.last_mut()
                {
                    contents.push_str(line_to_add);
                    contents.push('\n');
                    return Ok(());
                }
                Err(InvalidHunkError { message: format!("'{trimmed}' {NOT_A_HUNK_HEADER}"), line_number: self.line_number })
            }
            Mode::UpdateFile { .. } => self.process_update_line(line),
            Mode::EndedPatch => {
                if trimmed.is_empty() {
                    Ok(())
                } else {
                    Err(InvalidPatchError("The last line of the patch must be '*** End Patch'".to_string()))
                }
            }
        }
    }

    fn process_update_line(&mut self, line: &str) -> Result<(), ParseError> {
        let update_line = line.trim_end();
        if self.handle_hunk_headers_and_end_patch(update_line)? {
            return Ok(());
        }
        let line_number = self.line_number;
        let unexpected = || InvalidHunkError {
            message: format!("Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"),
            line_number,
        };
        let Some(UpdateFile { move_path, chunks, .. }) = self.hunks.last_mut() else {
            return Err(unexpected());
        };
        let empty_last = |chunks: &Vec<UpdateFileChunk>| chunks.last().is_some_and(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty());

        if chunks.last().is_some_and(|chunk| chunk.is_end_of_file) {
            if update_line.is_empty() {
                return Ok(());
            }
            if update_line != EMPTY_CHANGE_CONTEXT_MARKER && !update_line.starts_with(CHANGE_CONTEXT_MARKER) {
                return Err(InvalidHunkError { message: format!("Expected update hunk to start with a @@ context marker, got: '{line}'"), line_number });
            }
        }

        if chunks.is_empty()
            && move_path.is_none()
            && let Some(move_to_path) = update_line.strip_prefix(MOVE_TO_MARKER)
        {
            *move_path = Some(move_to_path.to_owned());
            return Ok(());
        }

        if (update_line == EMPTY_CHANGE_CONTEXT_MARKER || update_line.starts_with(CHANGE_CONTEXT_MARKER)) && empty_last(chunks) {
            return Err(unexpected());
        }

        if update_line == EMPTY_CHANGE_CONTEXT_MARKER {
            chunks.push(UpdateFileChunk::default());
            return Ok(());
        }

        if let Some(change_context) = update_line.strip_prefix(CHANGE_CONTEXT_MARKER) {
            chunks.push(UpdateFileChunk { change_context: Some(change_context.to_string()), ..UpdateFileChunk::default() });
            return Ok(());
        }

        if update_line == EOF_MARKER {
            if empty_last(chunks) {
                return Err(InvalidHunkError { message: "Update hunk does not contain any lines".to_string(), line_number });
            }
            if let Some(chunk) = chunks.last_mut() {
                chunk.is_end_of_file = true;
            }
            return Ok(());
        }

        // A bare empty line is an empty context line.
        let (context, added, removed) = if line.is_empty() {
            (Some(""), None, None)
        } else {
            (line.strip_prefix(' '), line.strip_prefix('+'), line.strip_prefix('-'))
        };
        if context.is_some() || added.is_some() || removed.is_some() {
            if chunks.is_empty() {
                chunks.push(UpdateFileChunk::default());
            }
            if let Some(chunk) = chunks.last_mut() {
                if let Some(text) = context {
                    chunk.push_context_line(text.to_string());
                } else if let Some(text) = added {
                    chunk.new_lines.push(text.to_string());
                } else if let Some(text) = removed {
                    chunk.old_lines.push(text.to_string());
                }
            }
            return Ok(());
        }

        if chunks.last().is_some_and(|chunk| !chunk.old_lines.is_empty() || !chunk.new_lines.is_empty()) {
            return Err(InvalidHunkError { message: format!("Expected update hunk to start with a @@ context marker, got: '{line}'"), line_number });
        }
        Err(unexpected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Vec<Hunk>, ParseError> {
        let mut parser = LineParser::default();
        parser.push(text)?;
        parser.finish()
    }

    #[test]
    fn indented_update_markers_are_context_lines() {
        assert_eq!(
            parse("*** Begin Patch\n*** Update File: a.txt\n@@\n-old a\n+new a\n *** Update File: b.txt\n@@\n-old b\n+new b\n*** End Patch\n"),
            Ok(vec![UpdateFile {
                path: "a.txt".into(),
                move_path: None,
                chunks: vec![
                    UpdateFileChunk {
                        change_context: None,
                        old_lines: vec!["old a".to_string(), "*** Update File: b.txt".to_string()],
                        new_lines: vec!["new a".to_string(), "*** Update File: b.txt".to_string()],
                        context_line_indices: vec![(1, 1)],
                        is_end_of_file: false,
                    },
                    UpdateFileChunk {
                        change_context: None,
                        old_lines: vec!["old b".to_string()],
                        new_lines: vec!["new b".to_string()],
                        context_line_indices: vec![],
                        is_end_of_file: false,
                    },
                ],
            }])
        );
    }

    #[test]
    fn bare_empty_update_lines_are_kept() {
        let lines = vec!["context before".to_string(), String::new(), "context after".to_string()];
        assert_eq!(
            parse("*** Begin Patch\n*** Update File: file.txt\n@@\n context before\n\n context after\n*** End Patch\n"),
            Ok(vec![UpdateFile {
                path: "file.txt".into(),
                move_path: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: lines.clone(),
                    new_lines: lines,
                    context_line_indices: vec![(0, 0), (1, 1), (2, 2)],
                    is_end_of_file: false,
                }],
            }])
        );
    }

    #[test]
    fn windows_line_endings_are_read() {
        assert_eq!(
            parse("*** Begin Patch\r\n*** Update File: file.txt\r\n@@\r\n-old\r\r\n+new\r\n*** End Patch\r\n"),
            Ok(vec![UpdateFile {
                path: "file.txt".into(),
                move_path: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec!["old\r".to_string()],
                    new_lines: vec!["new".to_string()],
                    context_line_indices: vec![],
                    is_end_of_file: false,
                }],
            }])
        );
    }

    #[test]
    fn bad_patches_say_why() {
        let header = |line: &str, line_number| Err(InvalidHunkError { message: format!("'{line}' {NOT_A_HUNK_HEADER}"), line_number });
        assert_eq!(parse("bad\n"), Err(InvalidPatchError("The first line of the patch must be '*** Begin Patch'".to_string())));
        assert_eq!(parse("*** Begin Patch\nbad\n"), header("bad", 2));
        assert_eq!(parse("*** Begin Patch\n*** Add File: file.txt\nbad\n"), header("bad", 3));
        assert_eq!(parse("*** Begin Patch\n*** Delete File: file.txt\nbad\n"), header("bad", 3));
        assert_eq!(
            parse("*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n*** Delete File: other.txt\n"),
            Err(InvalidHunkError { message: "Update file hunk for path 'old.txt' is empty".to_string(), line_number: 2 })
        );
        assert_eq!(
            parse("*** Begin Patch\n*** Update File: file.txt\n@@\n*** End of File\n"),
            Err(InvalidHunkError { message: "Update hunk does not contain any lines".to_string(), line_number: 4 })
        );
        assert_eq!(
            parse("*** Begin Patch\n*** Update File: file.txt\n@@\n-old\nbad\n"),
            Err(InvalidHunkError { message: "Expected update hunk to start with a @@ context marker, got: 'bad'".to_string(), line_number: 5 })
        );
        assert_eq!(
            parse("*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nextra\n"),
            Err(InvalidPatchError("The last line of the patch must be '*** End Patch'".to_string()))
        );
        assert_eq!(parse("*** Begin Patch\n*** Add File: file.txt\n+hello\n"), Err(InvalidPatchError("The last line of the patch must be '*** End Patch'".to_string())));
    }
}
