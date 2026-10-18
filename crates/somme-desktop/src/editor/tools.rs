//! Text tools for the editor: tidy JSON, and Base64 and URL encoding. Each
//! works on the selection, or on the whole file when nothing is selected.

use base64::Engine as _;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, general_purpose};
use gpui_kit::{Action, actions};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};

actions!(
    editor_tools,
    [FormatDocument, FormatJson, MinifyJson, Base64Encode, Base64Decode, UrlEncode, UrlDecode]
);

/// What JSON is indented with.
const JSON_INDENT: &str = "  ";

/// Characters URL encoding leaves alone, as JavaScript's `encodeURIComponent` does.
const URL_KEEP: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~').remove(b'!').remove(b'*').remove(b'\'').remove(b'(').remove(b')');

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Tidies the file in its own language: JSON here, others by their language server.
    FormatDocument,
    FormatJson,
    MinifyJson,
    Base64Encode,
    Base64Decode,
    UrlEncode,
    UrlDecode,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::FormatDocument,
        Tool::FormatJson,
        Tool::MinifyJson,
        Tool::Base64Encode,
        Tool::Base64Decode,
        Tool::UrlEncode,
        Tool::UrlDecode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::FormatDocument => "Format Document",
            Tool::FormatJson => "Format JSON",
            Tool::MinifyJson => "Minify JSON",
            Tool::Base64Encode => "Base64 Encode",
            Tool::Base64Decode => "Base64 Decode",
            Tool::UrlEncode => "URL Encode",
            Tool::UrlDecode => "URL Decode",
        }
    }

    pub fn action(self) -> Box<dyn Action> {
        match self {
            Tool::FormatDocument => Box::new(FormatDocument),
            Tool::FormatJson => Box::new(FormatJson),
            Tool::MinifyJson => Box::new(MinifyJson),
            Tool::Base64Encode => Box::new(Base64Encode),
            Tool::Base64Decode => Box::new(Base64Decode),
            Tool::UrlEncode => Box::new(UrlEncode),
            Tool::UrlDecode => Box::new(UrlDecode),
        }
    }

    /// The new text, or why the tool can't change it. Format Document is
    /// Format JSON here; the editor sends other languages to their server.
    pub fn apply(self, text: &str) -> Result<String, String> {
        match self {
            Tool::FormatDocument | Tool::FormatJson => json(text, true),
            Tool::MinifyJson => json(text, false),
            Tool::Base64Encode => Ok(general_purpose::STANDARD.encode(text)),
            Tool::Base64Decode => base64_decode(text),
            Tool::UrlEncode => Ok(percent_encoding::utf8_percent_encode(text, URL_KEEP).to_string()),
            Tool::UrlDecode => percent_encoding::percent_decode_str(text.trim())
                .decode_utf8()
                .map(|text| text.into_owned())
                .map_err(|_| "The decoded text is not UTF-8 text.".to_owned()),
        }
    }
}

/// Takes the standard and the URL alphabet, with or without padding, and
/// skips line breaks and spaces, as encoded text is often wrapped.
fn base64_decode(text: &str) -> Result<String, String> {
    let packed: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    let config = GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent);
    let standard = GeneralPurpose::new(&base64::alphabet::STANDARD, config);
    let url = GeneralPurpose::new(&base64::alphabet::URL_SAFE, config);
    let bytes = standard
        .decode(&packed)
        .or_else(|_| url.decode(&packed))
        .map_err(|_| "It is not Base64.".to_owned())?;
    String::from_utf8(bytes).map_err(|_| "The decoded data is not text.".to_owned())
}

/// Lays JSON out one value per line (`pretty`), or on one line with no
/// spaces. Keys keep their order and numbers their spelling.
fn json(text: &str, pretty: bool) -> Result<String, String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text).map_err(|err| format!("It is not valid JSON: {err}"))?;
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut depth = 0;
    let newline = |out: &mut String, depth: usize| {
        if pretty {
            out.push('\n');
            for _ in 0..depth {
                out.push_str(JSON_INDENT);
            }
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                out.push('"');
                while let Some(ch) = chars.next() {
                    out.push(ch);
                    match ch {
                        '\\' => out.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                out.push(ch);
                while chars.next_if(|ch| ch.is_whitespace()).is_some() {}
                // Empty ones stay as `{}` and `[]`.
                if let Some(close) = chars.next_if(|next| matches!((ch, next), ('{', '}') | ('[', ']'))) {
                    out.push(close);
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth -= 1;
                newline(&mut out, depth);
                out.push(ch);
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => out.push_str(if pretty { ": " } else { ":" }),
            ch if ch.is_whitespace() => {}
            ch => out.push(ch),
        }
    }
    if pretty && text.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_json_keeping_key_order() {
        let text = r#"{"b":1,"a":[1, 2,{}],"s":"x,{\"y\":[]}","e":[ ],"n":1.50}"#;
        let pretty = Tool::FormatJson.apply(text).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2,\n    {}\n  ],\n  \"s\": \"x,{\\\"y\\\":[]}\",\n  \"e\": [],\n  \"n\": 1.50\n}"
        );
        assert_eq!(Tool::MinifyJson.apply(&pretty).unwrap(), r#"{"b":1,"a":[1,2,{}],"s":"x,{\"y\":[]}","e":[],"n":1.50}"#);
        assert!(Tool::FormatJson.apply("{\"a\":}").unwrap_err().starts_with("It is not valid JSON"));
    }

    #[test]
    fn encodes_and_decodes() {
        let text = "héllo wörld/?&=+";
        let base64 = Tool::Base64Encode.apply(text).unwrap();
        assert_eq!(base64, "aMOpbGxvIHfDtnJsZC8/Jj0r");
        assert_eq!(Tool::Base64Decode.apply(&base64).unwrap(), text);
        // Wrapped, URL alphabet, no padding.
        assert_eq!(Tool::Base64Decode.apply("aMOpbGxvIHfDtnJs\nZC8_Jj0r").unwrap(), text);
        assert_eq!(Tool::Base64Decode.apply("aGk").unwrap(), "hi");
        assert!(Tool::Base64Decode.apply("not base64!").is_err());
        let url = Tool::UrlEncode.apply(text).unwrap();
        assert_eq!(url, "h%C3%A9llo%20w%C3%B6rld%2F%3F%26%3D%2B");
        assert_eq!(Tool::UrlDecode.apply(&url).unwrap(), text);
    }
}
