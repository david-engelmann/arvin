//! The strict document reader.
//!
//! A hand written reader over the document bytes in place of the
//! Python json module, shaped by three contract facts. A repeated
//! key is refused as the key arrives, because a general reader
//! that keeps the last value and says nothing cannot carry DM-8.
//! An integer wider than the machine forms stands as its decimal
//! text, so the DM-9 bounds judge the value Python judged. And
//! the byte decoding detects the same encodings the Python reader
//! detects, by the same first bytes rule.

use std::collections::HashSet;

use crate::value::Value;
use crate::{INT_DIGIT_LIMIT, MAX_NESTING};

pub(crate) enum ParseFailure {
    /// The document does not parse, with detail text from this
    /// reader standing after the shared message head.
    Syntax(String),
    /// A key stands twice in one object, named by the key.
    DuplicateKey(String),
}

pub(crate) fn parse_document(bytes: &[u8]) -> std::result::Result<Value, ParseFailure> {
    let text = decode_document_bytes(bytes).map_err(ParseFailure::Syntax)?;
    let mut parser = Parser {
        chars: text.chars().collect(),
        pos: 0,
        depth: 0,
    };
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos != parser.chars.len() {
        return Err(ParseFailure::Syntax(format!(
            "trailing text stands after the document at character {}",
            parser.pos
        )));
    }
    Ok(value)
}

/// Decode document bytes to text the way the Python json module
/// decoded them, detecting a byte order mark and the null byte
/// patterns of mark free UTF-16 and UTF-32, and refusing what
/// does not decode.
fn decode_document_bytes(bytes: &[u8]) -> std::result::Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
        decode_utf32(&bytes[4..], true)
    } else if bytes.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
        decode_utf32(&bytes[4..], false)
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        decode_utf16(&bytes[2..], true)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        decode_utf16(&bytes[2..], false)
    } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        decode_utf8(&bytes[3..])
    } else if bytes.len() >= 4 {
        if bytes[0] == 0 {
            if bytes[1] == 0 {
                decode_utf32(bytes, false)
            } else {
                decode_utf16(bytes, false)
            }
        } else if bytes[1] == 0 {
            if bytes[2] == 0 && bytes[3] == 0 {
                decode_utf32(bytes, true)
            } else {
                decode_utf16(bytes, true)
            }
        } else {
            decode_utf8(bytes)
        }
    } else if bytes.len() >= 2 {
        if bytes[0] == 0 {
            decode_utf16(bytes, false)
        } else if bytes[1] == 0 {
            decode_utf16(bytes, true)
        } else {
            decode_utf8(bytes)
        }
    } else {
        decode_utf8(bytes)
    }
}

fn decode_utf8(bytes: &[u8]) -> std::result::Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|err| format!("the document does not decode as UTF-8, {err}"))
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> std::result::Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("the document does not decode as UTF-16".to_string());
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|pair| {
            if little_endian {
                (pair[0] as u16) | ((pair[1] as u16) << 8)
            } else {
                ((pair[0] as u16) << 8) | pair[1] as u16
            }
        })
        .collect();
    // An unpaired surrogate decodes to the replacement character
    // here, where the Python decoder holds the surrogate itself.
    // Every field such a character can stand in is refused by the
    // reading validation of both implementations, the difference
    // the port amendment records for surrogate escapes.
    Ok(char::decode_utf16(units)
        .map(|decoded| decoded.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect())
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> std::result::Result<String, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err("the document does not decode as UTF-32".to_string());
    }
    let mut out = String::new();
    for group in bytes.chunks(4) {
        let code = if little_endian {
            (group[0] as u32)
                | ((group[1] as u32) << 8)
                | ((group[2] as u32) << 16)
                | ((group[3] as u32) << 24)
        } else {
            ((group[0] as u32) << 24)
                | ((group[1] as u32) << 16)
                | ((group[2] as u32) << 8)
                | group[3] as u32
        };
        match char::from_u32(code) {
            Some(ch) => out.push(ch),
            None => return Err("the document does not decode as UTF-32".to_string()),
        }
    }
    Ok(out)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn syntax(&self, detail: &str) -> ParseFailure {
        ParseFailure::Syntax(format!("{detail} at character {}", self.pos))
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> std::result::Result<Value, ParseFailure> {
        self.skip_whitespace();
        match self.peek() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => Ok(Value::Str(self.parse_string()?)),
            Some('t') => self.parse_literal("true", Value::Bool(true)),
            Some('f') => self.parse_literal("false", Value::Bool(false)),
            Some('n') => self.parse_literal("null", Value::Null),
            Some('N') => self.parse_literal("NaN", Value::Float(f64::NAN)),
            Some('I') => self.parse_literal("Infinity", Value::Float(f64::INFINITY)),
            Some('-') => {
                if self.chars.get(self.pos + 1) == Some(&'I') {
                    self.pos += 1;
                    self.parse_literal("Infinity", Value::Float(f64::NEG_INFINITY))
                } else {
                    self.parse_number()
                }
            }
            Some(ch) if ch.is_ascii_digit() => self.parse_number(),
            _ => Err(self.syntax("a value stands expected here")),
        }
    }

    fn parse_literal(
        &mut self,
        text: &str,
        value: Value,
    ) -> std::result::Result<Value, ParseFailure> {
        // The literal stands compared character by character
        // against the input ahead of the cursor, with no
        // collection built for the comparison. The literals of
        // this grammar are plain ASCII, so the character count
        // the comparison advances is the byte count the
        // position has always advanced by.
        let matched = text
            .chars()
            .zip(self.chars[self.pos..].iter())
            .take_while(|(want, have)| want == *have)
            .count();
        if matched == text.chars().count() {
            self.pos += matched;
            Ok(value)
        } else {
            Err(self.syntax("a value stands expected here"))
        }
    }

    fn parse_number(&mut self) -> std::result::Result<Value, ParseFailure> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        // The integer part is one zero or a nonzero digit and its
        // run, the grammar the Python reader accepts.
        match self.peek() {
            Some('0') => {
                self.pos += 1;
            }
            Some(ch) if ch.is_ascii_digit() => {
                while matches!(self.peek(), Some(ch) if ch.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.syntax("a number stands malformed here")),
        }
        let mut is_float = false;
        if self.peek() == Some('.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(ch) if ch.is_ascii_digit()) {
                return Err(self.syntax("a number stands malformed here"));
            }
            while matches!(self.peek(), Some(ch) if ch.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(ch) if ch.is_ascii_digit()) {
                return Err(self.syntax("a number stands malformed here"));
            }
            while matches!(self.peek(), Some(ch) if ch.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let literal: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            return literal
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| self.syntax("a number stands malformed here"));
        }
        let digits = literal.trim_start_matches('-');
        if digits.len() > INT_DIGIT_LIMIT {
            return Err(ParseFailure::Syntax(format!(
                "the integer literal of {} digits exceeds the parse limit of {INT_DIGIT_LIMIT} digits",
                digits.len()
            )));
        }
        if literal == "-0" {
            return Ok(Value::Int(0));
        }
        match literal.parse::<i128>() {
            Ok(value) => Ok(Value::Int(value)),
            Err(_) => Ok(Value::BigInt(literal)),
        }
    }

    fn parse_string(&mut self) -> std::result::Result<String, ParseFailure> {
        // The caller has seen the opening quote.
        self.pos += 1;
        let mut out = String::new();
        loop {
            let ch = match self.peek() {
                Some(ch) => ch,
                None => return Err(self.syntax("a string stands unterminated")),
            };
            self.pos += 1;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escape = match self.peek() {
                        Some(ch) => ch,
                        None => return Err(self.syntax("a string stands unterminated")),
                    };
                    self.pos += 1;
                    match escape {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let unit = self.parse_hex_unit()?;
                            if (0xd800..0xdc00).contains(&unit) {
                                // A high surrogate. A low surrogate
                                // escape following it combines, and
                                // anything else leaves the replacement
                                // character standing for the unpaired
                                // half, where the Python reader holds
                                // the surrogate itself. The port
                                // amendment records the difference.
                                if self.peek() == Some('\\')
                                    && self.chars.get(self.pos + 1) == Some(&'u')
                                {
                                    self.pos += 2;
                                    let low = self.parse_hex_unit()?;
                                    if (0xdc00..0xe000).contains(&low) {
                                        let combined = 0x10000
                                            + (((unit as u32 - 0xd800) << 10)
                                                + (low as u32 - 0xdc00));
                                        out.push(
                                            char::from_u32(combined)
                                                .unwrap_or(char::REPLACEMENT_CHARACTER),
                                        );
                                    } else {
                                        out.push(char::REPLACEMENT_CHARACTER);
                                        out.push(
                                            char::from_u32(low as u32)
                                                .unwrap_or(char::REPLACEMENT_CHARACTER),
                                        );
                                    }
                                } else {
                                    out.push(char::REPLACEMENT_CHARACTER);
                                }
                            } else {
                                out.push(
                                    char::from_u32(unit as u32)
                                        .unwrap_or(char::REPLACEMENT_CHARACTER),
                                );
                            }
                        }
                        _ => return Err(self.syntax("a string escape stands unknown")),
                    }
                }
                ch if (ch as u32) < 0x20 => {
                    return Err(self.syntax("a string holds a raw control character"));
                }
                ch => out.push(ch),
            }
        }
    }

    fn parse_hex_unit(&mut self) -> std::result::Result<u16, ParseFailure> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let ch = match self.peek() {
                Some(ch) => ch,
                None => return Err(self.syntax("a string escape stands short")),
            };
            let digit = match ch.to_digit(16) {
                Some(digit) => digit as u16,
                None => return Err(self.syntax("a string escape holds a non hex digit")),
            };
            value = value * 16 + digit;
            self.pos += 1;
        }
        Ok(value)
    }

    fn parse_array(&mut self) -> std::result::Result<Value, ParseFailure> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return Err(self.syntax("the document nests past the depth the reader reads"));
        }
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        loop {
            let item = self.parse_value()?;
            items.push(item);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some(']') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.syntax("an array stands unterminated")),
            }
        }
    }

    fn parse_object(&mut self) -> std::result::Result<Value, ParseFailure> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return Err(self.syntax("the document nests past the depth the reader reads"));
        }
        self.pos += 1;
        let mut pairs: Vec<(String, Value)> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Value::Object(pairs));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some('"') {
                return Err(self.syntax("an object key stands expected here"));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.peek() != Some(':') {
                return Err(self.syntax("an object colon stands expected here"));
            }
            self.pos += 1;
            let value = self.parse_value()?;
            pairs.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.pos += 1;
                }
                Some('}') => {
                    self.pos += 1;
                    self.depth -= 1;
                    // The duplicate check runs as the object
                    // completes, the moment the Python reader's
                    // pairs hook runs, so the key named is the
                    // first repeated key of the innermost object
                    // that completes with one.
                    let mut seen: HashSet<&str> = HashSet::new();
                    for (key, _) in &pairs {
                        if !seen.insert(key.as_str()) {
                            return Err(ParseFailure::DuplicateKey(key.clone()));
                        }
                    }
                    return Ok(Value::Object(pairs));
                }
                _ => return Err(self.syntax("an object stands unterminated")),
            }
        }
    }
}
