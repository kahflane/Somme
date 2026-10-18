//! Finds the marks Somme's shell hooks print (`ESC ] 6973 ; <secret> ; ... BEL`),
//! so a local terminal knows where each prompt and command starts. A mark
//! without this shell's secret came from program output and is dropped.

use std::path::PathBuf;

const PREFIX: &[u8] = b"\x1b]6973;";
/// A mark longer than this is not ours, so its bytes pass through untouched.
const MAX_MARK: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A prompt is about to show. Carries the last command's exit code and the shell's folder.
    Prompt { exit: i32, cwd: Option<PathBuf> },
    /// The user accepted a line. The shell may still ask for more of it.
    Accepted,
    /// The command starts running.
    Command(String),
    /// The shell's PATH, sent when it changes.
    Path(String),
    /// The user's home folder where the shell runs.
    Home(PathBuf),
}

pub enum Piece<'a> {
    Output(&'a [u8]),
    Mark(Mark),
}

/// Splits output into plain bytes and marks. A mark cut in half by a read is
/// kept until the rest arrives. With no secret, every mark is dropped.
#[derive(Default)]
pub struct MarkScanner {
    pending: Vec<u8>,
    token: String,
}

impl MarkScanner {
    pub fn new(token: &str) -> Self {
        Self {
            pending: Vec::new(),
            token: token.to_owned(),
        }
    }

    pub fn scan(&mut self, input: &[u8], mut each: impl FnMut(Piece<'_>)) {
        let joined;
        let data: &[u8] = if self.pending.is_empty() {
            input
        } else {
            let mut bytes = std::mem::take(&mut self.pending);
            bytes.extend_from_slice(input);
            joined = bytes;
            &joined
        };

        let mut emitted = 0;
        let mut at = 0;
        while let Some(offset) = data[at..].iter().position(|&b| b == 0x1b) {
            let start = at + offset;
            let rest = &data[start..];
            if rest.len() < PREFIX.len() {
                if PREFIX.starts_with(rest) {
                    self.keep(data, emitted, start, &mut each);
                    return;
                }
                at = start + 1;
                continue;
            }
            if !rest.starts_with(PREFIX) {
                at = start + 1;
                continue;
            }
            match find_end(&rest[PREFIX.len()..]) {
                End::Found { payload, len } => {
                    if emitted < start {
                        each(Piece::Output(&data[emitted..start]));
                    }
                    if let Some(mark) = parse(&rest[PREFIX.len()..PREFIX.len() + payload], &self.token) {
                        each(Piece::Mark(mark));
                    }
                    emitted = start + PREFIX.len() + payload + len;
                    at = emitted;
                }
                End::Missing if rest.len() <= MAX_MARK => {
                    self.keep(data, emitted, start, &mut each);
                    return;
                }
                End::Missing | End::Broken => at = start + 1,
            }
        }
        if emitted < data.len() {
            each(Piece::Output(&data[emitted..]));
        }
    }

    /// Hands out what came before `start` and holds the rest for the next read.
    fn keep(&mut self, data: &[u8], emitted: usize, start: usize, each: &mut impl FnMut(Piece<'_>)) {
        if emitted < start {
            each(Piece::Output(&data[emitted..start]));
        }
        self.pending = data[start..].to_vec();
    }
}

enum End {
    /// The payload length and the terminator length.
    Found { payload: usize, len: usize },
    Missing,
    /// An escape that does not end the mark: not one of ours.
    Broken,
}

fn find_end(body: &[u8]) -> End {
    let Some(i) = body.iter().position(|&b| b == 0x07 || b == 0x1b) else {
        return End::Missing;
    };
    if body[i] == 0x07 {
        return End::Found { payload: i, len: 1 };
    }
    match body.get(i + 1) {
        Some(b'\\') => End::Found { payload: i, len: 2 },
        Some(_) => End::Broken,
        None => End::Missing,
    }
}

fn parse(payload: &[u8], token: &str) -> Option<Mark> {
    let text = String::from_utf8_lossy(payload);
    let (secret, text) = text.split_once(';')?;
    if token.is_empty() || secret != token {
        return None;
    }
    let (kind, rest) = text.split_once(';').unwrap_or((text, ""));
    match kind {
        "A" => {
            let (exit, cwd) = rest.split_once(';').unwrap_or((rest, ""));
            let cwd = decode(cwd);
            Some(Mark::Prompt {
                exit: exit.trim().parse().unwrap_or(0),
                cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
            })
        }
        "B" => Some(Mark::Accepted),
        "C" => Some(Mark::Command(decode(rest))),
        "P" => Some(Mark::Path(decode(rest))),
        "H" => Some(Mark::Home(PathBuf::from(decode(rest)))),
        _ => None,
    }
}

/// Undoes the hooks' percent-encoding.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    enum Got {
        Out(Vec<u8>),
        Mark(Mark),
    }

    fn run(scanner: &mut MarkScanner, input: &[u8], got: &mut Vec<Got>) {
        scanner.scan(input, |piece| match piece {
            Piece::Output(bytes) => match got.last_mut() {
                Some(Got::Out(prev)) => prev.extend_from_slice(bytes),
                _ => got.push(Got::Out(bytes.to_vec())),
            },
            Piece::Mark(mark) => got.push(Got::Mark(mark)),
        });
    }

    const STREAM: &[u8] = b"prompt\x1b]6973;k;B\x07junk\x1b]6973;k;C;ls -l%0Aecho 50%25\x07out\x1b[1m\x1b]6973;k;A;1;/tmp/a%3Bb\x1b\\\x1b]2;title\x07";

    fn scanner() -> MarkScanner {
        MarkScanner::new("k")
    }

    fn expected() -> Vec<Got> {
        vec![
            Got::Out(b"prompt".to_vec()),
            Got::Mark(Mark::Accepted),
            Got::Out(b"junk".to_vec()),
            Got::Mark(Mark::Command("ls -l\necho 50%".into())),
            Got::Out(b"out\x1b[1m".to_vec()),
            Got::Mark(Mark::Prompt {
                exit: 1,
                cwd: Some(PathBuf::from("/tmp/a;b")),
            }),
            Got::Out(b"\x1b]2;title\x07".to_vec()),
        ]
    }

    #[test]
    fn splits_marks_from_output() {
        let mut got = Vec::new();
        run(&mut scanner(), STREAM, &mut got);
        assert_eq!(got, expected());
    }

    #[test]
    fn marks_cut_by_a_read_still_parse() {
        for cut in 0..=STREAM.len() {
            let mut scanner = scanner();
            let mut got = Vec::new();
            run(&mut scanner, &STREAM[..cut], &mut got);
            run(&mut scanner, &STREAM[cut..], &mut got);
            assert_eq!(got, expected(), "cut at {cut}");
        }
    }

    #[test]
    fn other_escapes_pass_through() {
        let mut got = Vec::new();
        let input = b"a\x1b]697x;nope\x07\x1b]6973\x1bXb";
        run(&mut scanner(), input, &mut got);
        assert_eq!(got, vec![Got::Out(input.to_vec())]);
    }

    #[test]
    fn marks_without_the_secret_are_dropped() {
        let mut got = Vec::new();
        run(&mut scanner(), b"a\x1b]6973;guess;C;rm -rf ~\x07\x1b]6973;C;x\x07b", &mut got);
        assert_eq!(got, vec![Got::Out(b"ab".to_vec())]);
        let mut got = Vec::new();
        run(&mut MarkScanner::default(), b"\x1b]6973;;B\x07", &mut got);
        assert_eq!(got, vec![]);
    }
}
