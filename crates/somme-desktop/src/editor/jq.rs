//! jq filters over JSON and JSON Lines text, run with jaq. Each value in the
//! text is one input, as `jq` takes them, so both kinds of file work alike.

use std::sync::atomic::{AtomicBool, Ordering};

use jaq_all::data::{self, Runner};
use jaq_all::jaq_core::{Vars, unwrap_valr};
use jaq_all::json::{Val, read, write};
use jaq_all::load::FileReportsDisp;

/// Results past these are left out, so a filter like `repeat(1)` ends.
const MAX_VALUES: usize = 10_000;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// The results as pretty JSON, one after another.
    pub text: String,
    pub count: usize,
    /// More results were left out.
    pub cut: bool,
    /// The first error, as jq goes on after one with the next input.
    pub error: Option<String>,
    pub errors: usize,
}

/// Runs `filter` over every JSON value in `input`, or over all of them in one
/// array when `slurp` is on, as `jq -s` does. Err says why the filter or the
/// input is wrong. Stops early once `cancel` is set.
pub fn run(filter: &str, input: &str, slurp: bool, cancel: &AtomicBool) -> Result<Outcome, String> {
    let filter = jaq_all::compile_with(filter, jaq_all::defs(), data::base_funs(), &[]).map_err(|reports| {
        reports.iter().map(|report| FileReportsDisp::new(report).to_string()).collect::<String>().trim().to_owned()
    })?;
    let values = read::parse_many(input.as_bytes());
    let inputs: Box<dyn Iterator<Item = Result<Val, String>>> = if slurp {
        let all: Result<Val, _> = values.collect();
        Box::new(std::iter::once(all.map_err(|err| err.to_string())))
    } else {
        Box::new(values.map(|value| value.map_err(|err| err.to_string())))
    };
    let pretty = write::Pp {
        indent: Some("  ".to_owned()),
        sep_space: true,
        ..Default::default()
    };
    let mut out = Outcome::default();
    let mut bytes = Vec::new();
    let finished = data::run(&Runner::default(), &filter, Vars::new([]), inputs, Stop::Input, |value| {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        match unwrap_valr(value) {
            Ok(value) => {
                if out.count == MAX_VALUES || bytes.len() > MAX_BYTES {
                    out.cut = true;
                    return Err(Stop::Full);
                }
                write::write(&mut bytes, &pretty, 0, &value).map_err(|err| Stop::Input(err.to_string()))?;
                bytes.push(b'\n');
                out.count += 1;
            }
            Err(err) => {
                out.errors += 1;
                out.error.get_or_insert_with(|| err.to_string());
            }
        }
        Ok(())
    });
    match finished {
        Ok(()) | Err(Stop::Full | Stop::Cancelled) => {}
        // Bad JSON in the file: keep what came before it.
        Err(Stop::Input(err)) => {
            out.errors += 1;
            out.error.get_or_insert(format!("The file is not valid JSON here: {err}"));
        }
    }
    out.text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(out)
}

enum Stop {
    Input(String),
    Full,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jq(filter: &str, input: &str, slurp: bool) -> Result<Outcome, String> {
        run(filter, input, slurp, &AtomicBool::new(false))
    }

    #[test]
    fn picks_fields_from_json() {
        let out = jq(".users[] | select(.age > 30) | .name", r#"{"users":[{"name":"a","age":40},{"name":"b","age":20}]}"#, false).unwrap();
        assert_eq!(out.text, "\"a\"\n");
        assert_eq!(out.count, 1);
        let out = jq(".users[0]", r#"{"users":[{"name":"a","age":40}]}"#, false).unwrap();
        assert_eq!(out.text, "{\n  \"name\": \"a\",\n  \"age\": 40\n}\n");
    }

    #[test]
    fn json_lines_are_one_input_each_or_one_array() {
        let lines = "{\"level\":\"info\"}\n{\"level\":\"error\"}\n{\"level\":\"error\"}\n";
        assert_eq!(jq("select(.level == \"error\") | .level", lines, false).unwrap().count, 2);
        assert_eq!(jq("length", lines, true).unwrap().text, "3\n");
    }

    #[test]
    fn says_what_is_wrong() {
        assert!(jq(".a |", "{}", false).is_err());
        let out = jq(".a.b", "{\"a\":1}\n{\"a\":{\"b\":2}}", false).unwrap();
        assert_eq!((out.count, out.errors), (1, 1));
        assert_eq!(out.text, "2\n");
        let out = jq(".", "{\"a\":1}\n{oops", false).unwrap();
        assert_eq!(out.count, 1);
        assert!(out.error.unwrap().contains("not valid JSON"));
    }

    #[test]
    fn endless_filters_end() {
        let out = jq("repeat(1)", "null", false).unwrap();
        assert!(out.cut);
        assert_eq!(out.count, MAX_VALUES);
    }
}
