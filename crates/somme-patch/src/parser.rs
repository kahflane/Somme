// Changed by Somme from OpenAI Codex (codex-rs/apply-patch). Apache-2.0;
// see LICENSE and NOTICE in this crate.

//! Parses and checks a patch into a list of "hunks". It does not check that
//! the patch can be applied to the files.
//!
//! The grammar of the patch format:
//!
//! start: begin_patch hunk+ end_patch
//! begin_patch: "*** Begin Patch" LF
//! end_patch: "*** End Patch" LF?
//!
//! hunk: add_hunk | delete_hunk | update_hunk
//! add_hunk: "*** Add File: " filename LF add_line+
//! delete_hunk: "*** Delete File: " filename LF
//! update_hunk: "*** Update File: " filename LF change_move? change?
//! filename: /(.+)/
//! add_line: "+" /(.+)/ LF -> line
//!
//! change_move: "*** Move to: " filename LF
//! change: (change_context | change_line)+ eof_line?
//! change_context: ("@@" | "@@ " /(.+)/) LF
//! change_line: ("+" | "-" | " ") /(.+)/ LF
//! eof_line: "*** End of File" LF
//!
//! The parser is a little more lenient than the grammar: it allows white
//! space around the markers, and a patch wrapped in a `<<EOF` heredoc.

use thiserror::Error;

use crate::line_parser::LineParser;

pub(crate) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub(crate) const END_PATCH_MARKER: &str = "*** End Patch";
pub(crate) const ADD_FILE_MARKER: &str = "*** Add File: ";
pub(crate) const DELETE_FILE_MARKER: &str = "*** Delete File: ";
pub(crate) const UPDATE_FILE_MARKER: &str = "*** Update File: ";
pub(crate) const MOVE_TO_MARKER: &str = "*** Move to: ";
pub(crate) const EOF_MARKER: &str = "*** End of File";
pub(crate) const CHANGE_CONTEXT_MARKER: &str = "@@ ";
pub(crate) const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

#[derive(Debug, PartialEq, Error, Clone)]
pub enum ParseError {
    #[error("invalid patch: {0}")]
    InvalidPatchError(String),
    #[error("invalid hunk at line {line_number}, {message}")]
    InvalidHunkError { message: String, line_number: usize },
}
use ParseError::*;

#[derive(Debug, PartialEq, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum Hunk {
    AddFile {
        path: String,
        contents: String,
    },
    DeleteFile {
        path: String,
    },
    UpdateFile {
        path: String,
        move_path: Option<String>,

        /// Chunks are in order: the `change_context` of one chunk comes later
        /// in the file than the chunk before it.
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    /// The path this hunk changes, using the move destination for a rename.
    pub fn path(&self) -> &str {
        match self {
            Hunk::AddFile { path, .. } => path,
            Hunk::DeleteFile { path } => path,
            Hunk::UpdateFile { move_path: Some(path), .. } => path,
            Hunk::UpdateFile { path, move_path: None, .. } => path,
        }
    }
}

#[cfg(test)]
use Hunk::*;

#[derive(Debug, Default, PartialEq, Clone)]
pub struct UpdateFileChunk {
    /// A single line of context used to narrow down the position of the chunk
    /// (this is usually a class, method, or function definition.)
    pub change_context: Option<String>,

    /// A contiguous block of lines that should be replaced with `new_lines`.
    /// `old_lines` must occur strictly after `change_context`.
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,

    /// Pairs of indices into `old_lines` and `new_lines` that identify lines
    /// parsed as context rather than inferred to be equal by their contents.
    pub context_line_indices: Vec<(usize, usize)>,

    /// If set to true, `old_lines` must occur at the end of the source file.
    /// (Tolerance around trailing newlines should be encouraged.)
    pub is_end_of_file: bool,
}

impl UpdateFileChunk {
    /// Adds a context line to both sides while recording its corresponding
    /// indices so it remains distinguishable from identical changed lines.
    pub(crate) fn push_context_line(&mut self, line: String) {
        self.context_line_indices.push((self.old_lines.len(), self.new_lines.len()));
        self.old_lines.push(line.clone());
        self.new_lines.push(line);
    }
}

/// Parses the patch. A patch wrapped in a `<<EOF` heredoc is unwrapped first,
/// because models sometimes send it that way.
pub fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    parse_patch_text(patch, ParseMode::Lenient)
}

enum ParseMode {
    /// Parse the patch text as is.
    #[cfg_attr(not(test), allow(dead_code))]
    Strict,
    /// Also accept the patch inside `<<EOF` ... `EOF`, `<<'EOF'` or `<<"EOF"`.
    Lenient,
}

fn parse_patch_text(patch: &str, mode: ParseMode) -> Result<Vec<Hunk>, ParseError> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    let patch_lines = match mode {
        ParseMode::Strict => check_patch_boundaries_strict(&lines)?,
        ParseMode::Lenient => check_patch_boundaries_lenient(&lines)?,
    };

    let patch = patch_lines.join("\n");
    let mut parser = LineParser::default();
    parser.push(&patch)?;
    parser.finish()
}

/// Checks the start and end lines of the patch text, returning an error if
/// they do not match the expected markers.
fn check_patch_boundaries_strict<'a>(lines: &'a [&'a str]) -> Result<&'a [&'a str], ParseError> {
    let (first_line, last_line) = match lines {
        [] => (None, None),
        [first] => (Some(first), Some(first)),
        [first, .., last] => (Some(first), Some(last)),
    };
    check_start_and_end_lines_strict(first_line, last_line)?;
    Ok(lines)
}

/// In lenient mode, checks if the first line is `<<EOF` (possibly quoted) and
/// the last line ends with `EOF`. There must be at least 4 lines: the heredoc
/// markers take 2 lines and the patch text at least 2.
///
/// If so, returns the lines of the patch without the heredoc markers.
fn check_patch_boundaries_lenient<'a>(original_lines: &'a [&'a str]) -> Result<&'a [&'a str], ParseError> {
    let original_parse_error = match check_patch_boundaries_strict(original_lines) {
        Ok(lines) => return Ok(lines),
        Err(e) => e,
    };

    match original_lines {
        [first, .., last] => {
            if (first == &"<<EOF" || first == &"<<'EOF'" || first == &"<<\"EOF\"") && last.ends_with("EOF") && original_lines.len() >= 4 {
                let inner_lines = &original_lines[1..original_lines.len() - 1];
                check_patch_boundaries_strict(inner_lines)
            } else {
                Err(original_parse_error)
            }
        }
        _ => Err(original_parse_error),
    }
}

fn check_start_and_end_lines_strict(first_line: Option<&&str>, last_line: Option<&&str>) -> Result<(), ParseError> {
    let first_line = first_line.map(|line| line.trim());
    let last_line = last_line.map(|line| line.trim());

    match (first_line, last_line) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => Ok(()),
        (Some(first), _) if first != BEGIN_PATCH_MARKER => {
            Err(InvalidPatchError(String::from("The first line of the patch must be '*** Begin Patch'")))
        }
        _ => Err(InvalidPatchError(String::from("The last line of the patch must be '*** End Patch'"))),
    }
}

#[test]
fn test_parse_patch() {
    assert_eq!(
        parse_patch_text("bad", ParseMode::Strict),
        Err(InvalidPatchError("The first line of the patch must be '*** Begin Patch'".to_string()))
    );
    assert_eq!(
        parse_patch_text("*** Begin Patch\nbad", ParseMode::Strict),
        Err(InvalidPatchError("The last line of the patch must be '*** End Patch'".to_string()))
    );

    assert_eq!(
        parse_patch_text(concat!("*** Begin Patch", " ", "\n*** Add File: foo\n+hi\n", " ", "*** End Patch"), ParseMode::Strict).unwrap(),
        vec![AddFile { path: "foo".into(), contents: "hi\n".to_string() }]
    );
    assert_eq!(
        parse_patch_text(
            "*** Begin Patch\n\
             *** Update File: test.py\n\
             *** End Patch",
            ParseMode::Strict
        ),
        Err(InvalidHunkError { message: "Update file hunk for path 'test.py' is empty".to_string(), line_number: 2 })
    );
    assert_eq!(
        parse_patch_text(
            "*** Begin Patch\n\
             *** End Patch",
            ParseMode::Strict
        )
        .unwrap(),
        Vec::new()
    );
    assert_eq!(
        parse_patch_text(
            "*** Begin Patch\n\
             *** Add File: path/add.py\n\
             +abc\n\
             +def\n\
             *** Delete File: path/delete.py\n\
             *** Update File: path/update.py\n\
             *** Move to: path/update2.py\n\
             @@ def f():\n\
             -    pass\n\
             +    return 123\n\
             *** End Patch",
            ParseMode::Strict
        )
        .unwrap(),
        vec![
            AddFile { path: "path/add.py".into(), contents: "abc\ndef\n".to_string() },
            DeleteFile { path: "path/delete.py".into() },
            UpdateFile {
                path: "path/update.py".into(),
                move_path: Some("path/update2.py".into()),
                chunks: vec![UpdateFileChunk {
                    change_context: Some("def f():".to_string()),
                    old_lines: vec!["    pass".to_string()],
                    new_lines: vec!["    return 123".to_string()],
                    context_line_indices: vec![],
                    is_end_of_file: false
                }]
            }
        ]
    );
    // Update hunk followed by another hunk (Add File).
    assert_eq!(
        parse_patch_text(
            "*** Begin Patch\n\
             *** Update File: file.py\n\
             @@\n\
             +line\n\
             *** Add File: other.py\n\
             +content\n\
             *** End Patch",
            ParseMode::Strict
        )
        .unwrap(),
        vec![
            UpdateFile {
                path: "file.py".into(),
                move_path: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec![],
                    new_lines: vec!["line".to_string()],
                    context_line_indices: vec![],
                    is_end_of_file: false
                }],
            },
            AddFile { path: "other.py".into(), contents: "content\n".to_string() }
        ]
    );

    // Update hunk without an explicit @@ header for the first chunk should parse.
    // Use a raw string to preserve the leading space diff marker on the context line.
    assert_eq!(
        parse_patch_text(
            r#"*** Begin Patch
*** Update File: file2.py
 import foo
+bar
*** End Patch"#,
            ParseMode::Strict
        )
        .unwrap(),
        vec![UpdateFile {
            path: "file2.py".into(),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: vec!["import foo".to_string()],
                new_lines: vec!["import foo".to_string(), "bar".to_string()],
                context_line_indices: vec![(0, 0)],
                is_end_of_file: false,
            }],
        }]
    );
}

#[test]
fn test_parse_patch_preserves_end_of_file_marker() {
    let patch = "*** Begin Patch\n*** Update File: file.txt\n@@\n+quux\n*** End of File\n\n*** End Patch";
    assert_eq!(
        parse_patch(patch),
        Ok(vec![UpdateFile {
            path: "file.txt".into(),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: Vec::new(),
                new_lines: vec!["quux".to_string()],
                context_line_indices: vec![],
                is_end_of_file: true,
            }],
        }])
    );
}

#[test]
fn test_parse_patch_lenient() {
    let patch_text = r#"*** Begin Patch
*** Update File: file2.py
 import foo
+bar
*** End Patch"#;
    let expected_patch = vec![UpdateFile {
        path: "file2.py".into(),
        move_path: None,
        chunks: vec![UpdateFileChunk {
            change_context: None,
            old_lines: vec!["import foo".to_string()],
            new_lines: vec!["import foo".to_string(), "bar".to_string()],
            context_line_indices: vec![(0, 0)],
            is_end_of_file: false,
        }],
    }];
    let expected_error = InvalidPatchError("The first line of the patch must be '*** Begin Patch'".to_string());

    for wrapped in [format!("<<EOF\n{patch_text}\nEOF\n"), format!("<<'EOF'\n{patch_text}\nEOF\n"), format!("<<\"EOF\"\n{patch_text}\nEOF\n")] {
        assert_eq!(parse_patch_text(&wrapped, ParseMode::Strict), Err(expected_error.clone()));
        assert_eq!(parse_patch_text(&wrapped, ParseMode::Lenient), Ok(expected_patch.clone()));
    }

    let patch_text_in_mismatched_quotes_heredoc = format!("<<\"EOF'\n{patch_text}\nEOF\n");
    assert_eq!(parse_patch_text(&patch_text_in_mismatched_quotes_heredoc, ParseMode::Lenient), Err(expected_error.clone()));

    let patch_text_with_missing_closing_heredoc = "<<EOF\n*** Begin Patch\n*** Update File: file2.py\nEOF\n".to_string();
    assert_eq!(parse_patch_text(&patch_text_with_missing_closing_heredoc, ParseMode::Strict), Err(expected_error));
    assert_eq!(
        parse_patch_text(&patch_text_with_missing_closing_heredoc, ParseMode::Lenient),
        Err(InvalidPatchError("The last line of the patch must be '*** End Patch'".to_string()))
    );
}
