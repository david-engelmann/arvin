//! Core logic for deadman, the staged kill switch for agent runs.
//!
//! One state file holds every run as one JSON document under the
//! format name deadman-state/1, and the document holds each run
//! with its signals and its decision log, so a stage change and
//! its record publish in the same rename and can never disagree.
//! Every mutating operation holds an exclusive flock on the lock
//! file beside the state file across its whole read and evaluate
//! and modify and write cycle, and publishes by renaming a
//! complete temporary file over the state file. Reads take a
//! shared lock and open the state file fresh each time, and a
//! reader that changes nothing never writes.
//!
//! Time is an input. Every operation but the log read takes the
//! current instant as an argument, and this module reads no wall
//! clock and calls no model. The one random draw in the module
//! names a temporary file at publication, and it reaches no
//! stage, no reason, no record, and no state byte. The stage an
//! evaluation reports is a deterministic function of the recorded
//! state and the instant.
//!
//! The stage vocabulary is program vocabulary and stands in one
//! exact order in STAGES. The trigger names of DM-3 stand in
//! TRIGGERS.
//!
//! This crate is the Rust port of the Python implementation of
//! Round 1. The port amendment in CONTRACT.md records the
//! equivalence standard, the reshapings, and the two deliberate
//! divergences the port carries, the lock identity check and the
//! exclusive publish temporary.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use fs2::FileExt;

pub const FORMAT_NAME: &str = "deadman-state/1";
const LOCK_SUFFIX: &str = ".lock";
pub const NAME_MAX: usize = 64;
pub const NOTE_MAX: usize = 200;

pub const STAGES: [&str; 6] = [
    "none",
    "warn",
    "throttle",
    "cut tools",
    "cut budget",
    "kill",
];
pub const TRIGGERS: [&str; 4] = [
    "operator pull",
    "hard ceiling",
    "soft ceiling",
    "missed proofs",
];
const RECORD_KINDS: [&str; 5] = ["registered", "proof", "spend", "pull", "stage"];
const RESERVED_FIELDS: [&str; 3] = ["model_name", "model_confidence", "fallback_taken"];

pub const DEFAULT_PROOF_INTERVAL_SECONDS: i64 = 60;
pub const DEFAULT_MISSED_PROOF_LIMIT: i64 = 5;

pub const PROOF_INTERVAL_MAX_SECONDS: i64 = 86_399_999_999_999;
pub const SPEND_MAX: i64 = 1_000_000_000_000_000_000;
pub const MISSED_PROOF_LIMIT_MAX: i64 = 1_000_000;

/// The stage a missed proof count below the limit warrants under
/// DM-3, keyed by the count itself, with every higher count below
/// the limit standing at the last entry.
const MISSED_COUNT_STAGES: [&str; 4] = ["warn", "throttle", "cut tools", "cut budget"];

const DOCUMENT_KEYS: [&str; 2] = ["format", "runs"];
const RUN_KEYS: [&str; 12] = [
    "id",
    "registered_at",
    "proof_interval_seconds",
    "missed_proof_limit",
    "soft_ceiling",
    "hard_ceiling",
    "spend_total",
    "last_proof_at",
    "stage",
    "clear_streak",
    "killed",
    "log",
];
const RECORD_KEYS: [&str; 8] = [
    "seq",
    "kind",
    "at",
    "stage",
    "reasons",
    "model_name",
    "model_confidence",
    "fallback_taken",
];

/// The digit limit the Python int conversion and the Python json
/// reader apply to an integer literal, probed against the Python
/// runtime before the port was written.
const INT_DIGIT_LIMIT: usize = 4300;

/// The nesting depth the strict reader will read. The Python
/// reader refuses deeper documents at its own recursion limit,
/// and the port amendment records the bound.
const MAX_NESTING: usize = 256;

/// An error the CLI reports with a nonzero exit and no traceback.
/// The variants mirror the Python error classes of Round 1, with
/// Failure standing for the system failures the Python original
/// let escape as tracebacks, the publish failure among them.
#[derive(Debug)]
pub enum Error {
    /// A run id is not in the shape the switch accepts.
    InvalidName(String),
    /// A request value, such as an instant or a threshold, is not usable.
    InvalidRequest(String),
    /// The named run does not stand in the state file.
    NotFound(String),
    /// The standing state of the switch refuses the operation.
    Refusal(String),
    /// The state document cannot be read as it was recorded.
    Corrupt(String),
    /// A system failure the operation cannot complete through.
    Failure(String),
}

impl Error {
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::InvalidName(_) | Error::InvalidRequest(_) => 2,
            _ => 1,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Error::InvalidName(message)
            | Error::InvalidRequest(message)
            | Error::NotFound(message)
            | Error::Refusal(message)
            | Error::Corrupt(message)
            | Error::Failure(message) => message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn corrupt(message: String) -> Error {
    Error::Corrupt(format!("the state is corrupt, {message}"))
}

/// A value of the JSON data model, as the strict reader produces
/// it and the value level validators judge it. An integer wider
/// than 128 bits stands as its canonical decimal text, so the
/// bounds in the contract judge the same value the Python reader
/// judged instead of a clamped one.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i128),
    BigInt(String),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value as an integer of any width, when it is one. A
    /// boolean is not an integer under the type test the Python
    /// validators use, and it never converts here.
    fn as_json_int(&self) -> Option<JsonInt> {
        match self {
            Value::Int(value) => Some(JsonInt::Small(*value)),
            Value::BigInt(text) => Some(JsonInt::Big(text.clone())),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }
}

/// An integer of any width. The small form holds a value that
/// fits in 128 bits. The big form holds canonical decimal text,
/// an optional minus sign and digits with no leading zeros, for a
/// value that does not fit.
#[derive(Clone, Debug, PartialEq, Eq)]
enum JsonInt {
    Small(i128),
    Big(String),
}

impl JsonInt {
    fn cmp_i128(&self, bound: i128) -> Ordering {
        match self {
            JsonInt::Small(value) => value.cmp(&bound),
            JsonInt::Big(text) => {
                if text.starts_with('-') {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
        }
    }

    fn is_zero(&self) -> bool {
        matches!(self, JsonInt::Small(0))
    }

    /// The value one higher. A stored clear streak can stand at
    /// any width, and the step down evaluation adds one to it
    /// before it judges the result, so the addition is exact at
    /// every width the reader admits.
    fn add_one(&self) -> JsonInt {
        match self {
            JsonInt::Small(value) => match value.checked_add(1) {
                Some(next) => JsonInt::Small(next),
                None => JsonInt::Big("170141183460469231731687303715884105728".to_string()),
            },
            JsonInt::Big(text) => {
                if let Some(magnitude) = text.strip_prefix('-') {
                    // A negative value of any width plus one stays
                    // well inside the small form only when the
                    // magnitude is small, which a big form never
                    // is, so the result is the magnitude less one
                    // with the sign kept, unless the magnitude is
                    // exactly one higher than the small range.
                    let decremented = decrement_digits(magnitude);
                    if decremented == "170141183460469231731687303715884105728" {
                        JsonInt::Small(i128::MIN)
                    } else {
                        JsonInt::Big(format!("-{decremented}"))
                    }
                } else {
                    JsonInt::Big(increment_digits(text))
                }
            }
        }
    }
}

fn increment_digits(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    let mut index = bytes.len();
    loop {
        if index == 0 {
            bytes.insert(0, b'1');
            break;
        }
        index -= 1;
        if bytes[index] == b'9' {
            bytes[index] = b'0';
        } else {
            bytes[index] += 1;
            break;
        }
    }
    String::from_utf8(bytes).expect("digit text stays ascii")
}

fn decrement_digits(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    let mut index = bytes.len();
    loop {
        index -= 1;
        if bytes[index] == b'0' {
            bytes[index] = b'9';
        } else {
            bytes[index] -= 1;
            break;
        }
    }
    let text = String::from_utf8(bytes).expect("digit text stays ascii");
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The repr of a text value, following the Python repr rules for
/// the shapes the grammar admits. Python chooses the quote that
/// needs fewer escapes, escapes the backslash and the chosen
/// quote and the named control characters, and escapes any
/// character its tables call not printable with a hex escape of
/// the width the codepoint needs.
pub fn py_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch == quote => {
                out.push('\\');
                out.push(ch);
            }
            ch if py_printable_char(ch) => out.push(ch),
            ch => {
                let code = ch as u32;
                if code < 0x100 {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code < 0x10000 {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

fn py_printable_char(ch: char) -> bool {
    let code = ch as u32;
    if code == 0x20 {
        return true;
    }
    if code < 0x20 || (0x7f..0xa0).contains(&code) {
        return false;
    }
    // The separator and format and private use ranges the Python
    // printable test refuses. Unassigned codepoints also fail the
    // Python test and cannot be enumerated here, so a value
    // carrying one renders raw where Python escapes it, a
    // difference confined to the rendering inside a message.
    !matches!(code,
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x0890..=0x0891
        | 0x08e2 | 0x1680 | 0x180e | 0x2000..=0x200a | 0x200b..=0x200f
        | 0x2028 | 0x2029 | 0x202a..=0x202e | 0x202f | 0x205f | 0x2060..=0x2064
        | 0x2066..=0x206f | 0x3000 | 0xfeff | 0xfff9..=0xfffb
        | 0xe000..=0xf8ff | 0x110bd | 0x110cd | 0x13430..=0x1343f
        | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f
        | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

/// The repr of any value, as the Python messages render the
/// values they name.
pub fn py_repr_value(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Int(value) => value.to_string(),
        Value::BigInt(text) => text.clone(),
        Value::Float(value) => py_float_repr(*value),
        Value::Str(text) => py_repr_str(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(py_repr_value).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(key, item)| format!("{}: {}", py_repr_str(key), py_repr_value(item)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Repr a request value for an error message, however large it
/// stands. An integer whose decimal text runs past the digit
/// limit cannot be reprised by the Python runtime either, and
/// both implementations fall back to the same standing phrase.
pub fn py_printable(value: &Value) -> String {
    match value {
        Value::Int(number) => {
            let text = number.to_string();
            if text.trim_start_matches('-').len() > INT_DIGIT_LIMIT {
                "a value too large to print".to_string()
            } else {
                text
            }
        }
        Value::BigInt(text) => {
            if text.trim_start_matches('-').len() > INT_DIGIT_LIMIT {
                "a value too large to print".to_string()
            } else {
                text.clone()
            }
        }
        _ => py_repr_value(value),
    }
}

/// The repr of a float, following the Python float repr rules.
/// The shortest digit string that round trips is taken from the
/// Rust exponent form, and the notation follows the Python rule,
/// fixed notation while the decimal exponent stands between -4
/// and 16 and scientific notation outside it.
pub fn py_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_string();
    }
    let raw = format!("{:e}", value);
    let (mantissa, exponent_text) = raw.split_once('e').expect("exponent form carries e");
    let exponent: i32 = exponent_text.parse().expect("exponent is an integer");
    let digits: String = mantissa
        .trim_start_matches('-')
        .chars()
        .filter(|ch| *ch != '.')
        .collect();
    let sign = if value < 0.0 { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        // Fixed notation. The point stands after the first
        // exponent plus one digits.
        let point = exponent + 1;
        let mut out = String::from(sign);
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..(-point) {
                out.push('0');
            }
            out.push_str(&digits);
        } else if (point as usize) >= digits.len() {
            out.push_str(&digits);
            for _ in 0..(point as usize - digits.len()) {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..point as usize]);
            out.push('.');
            out.push_str(&digits[point as usize..]);
        }
        out
    } else {
        let mut out = String::from(sign);
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exponent.abs()));
        out
    }
}

// ----------------------------------------------------------------
// The strict document reader.
//
// A hand written reader over the document bytes in place of the
// Python json module, shaped by three contract facts. A repeated
// key is refused as the key arrives, because a general reader
// that keeps the last value and says nothing cannot carry DM-8.
// An integer wider than the machine forms stands as its decimal
// text, so the DM-9 bounds judge the value Python judged. And
// the byte decoding detects the same encodings the Python reader
// detects, by the same first bytes rule.

enum ParseFailure {
    /// The document does not parse, with detail text from this
    /// reader standing after the shared message head.
    Syntax(String),
    /// A key stands twice in one object, named by the key.
    DuplicateKey(String),
}

fn parse_document(bytes: &[u8]) -> std::result::Result<Value, ParseFailure> {
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
        if self.chars[self.pos..].starts_with(&text.chars().collect::<Vec<char>>()[..]) {
            self.pos += text.len();
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

// ----------------------------------------------------------------
// Instants.
//
// A hand written reader and writer in place of the Python
// datetime machinery. The reader accepts the family the Python
// fromisoformat accepted, as probed against the Python runtime
// before the port was written, extended and basic and week
// calendar dates, any single character as the separator between
// the date and the time, a fraction written with a dot or a
// comma and truncated past six digits, and offsets whose hour
// and minute and second components normalize inside a day. A
// fraction is a fraction of a second wherever it stands, after
// the hour or the minute or the second of the time or of the
// offset, the Python parser's own reading. An offset fraction
// counts only when some whole component of the offset is
// nonzero and is dropped when they are all zero, again the
// Python parser's own reading, probed case by case.

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days since the epoch day 1970-01-01 for a civil date, the
/// standard era based conversion.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let month_prime = (month as i64 + 9).rem_euclid(12);
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// The civil date standing at a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719468;
    let era = shifted.div_euclid(146097);
    let day_of_era = shifted - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// The ISO weekday of a day count, Monday 1 through Sunday 7.
/// The epoch day was a Thursday.
fn iso_weekday(days: i64) -> u32 {
    ((days + 3).rem_euclid(7) + 1) as u32
}

fn weeks_in_iso_year(year: i64) -> u32 {
    let jan1 = iso_weekday(days_from_civil(year, 1, 1));
    if jan1 == 4 || (jan1 == 3 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

/// An instant, a wall reading with the offset it was read under.
/// Equality and order judge the UTC instant itself, as the Python
/// datetime comparison does, so two spellings of one instant
/// compare equal.
#[derive(Clone, Copy, Debug)]
pub struct Instant {
    days: i64,
    secs: u32,
    micros: u32,
    offset_micros: Option<i64>,
}

impl Instant {
    /// Build an instant from civil parts, refusing any part
    /// outside the ranges the parser itself accepts, so a caller
    /// handing absurd parts gets a typed refusal instead of a
    /// computation that panics or wraps. The ranges are the
    /// years 1 through 9999, the month and day of a real date,
    /// an hour under 24, a minute and a second under 60,
    /// microseconds under a second, and an offset under a day
    /// in either direction. The offset is the offset from UTC
    /// in microseconds, or None for a naive reading. The eight
    /// parts are the fields of the reading itself, so they stand
    /// as parameters rather than a parts structure.
    #[allow(clippy::too_many_arguments)]
    pub fn from_civil(
        year: i64,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        micros: u32,
        offset_micros: Option<i64>,
    ) -> Result<Instant> {
        let offset_in_range = match offset_micros {
            None => true,
            Some(offset) => offset > -86_400_000_000 && offset < 86_400_000_000,
        };
        let parts_in_range = (1..=9999).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month)
            && hour <= 23
            && minute <= 59
            && second <= 59
            && micros <= 999_999
            && offset_in_range;
        if !parts_in_range {
            return Err(Error::InvalidRequest(
                "the civil parts do not form an instant the switch can record".to_string(),
            ));
        }
        Ok(Instant {
            days: days_from_civil(year, month, day),
            secs: hour * 3600 + minute * 60 + second,
            micros,
            offset_micros,
        })
    }

    /// The current instant in UTC, the one clock read the command
    /// line performs. The library itself reads no wall clock.
    pub fn now_utc() -> Instant {
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as i128)
            .unwrap_or(0);
        Instant::from_epoch_micros(micros, Some(0))
    }

    fn from_epoch_micros(epoch_micros: i128, offset_micros: Option<i64>) -> Instant {
        let local = epoch_micros + offset_micros.unwrap_or(0) as i128;
        let days = local.div_euclid(86_400_000_000);
        let rest = local.rem_euclid(86_400_000_000);
        Instant {
            days: days as i64,
            secs: (rest / 1_000_000) as u32,
            micros: (rest % 1_000_000) as u32,
            offset_micros,
        }
    }

    /// Microseconds since the epoch at the UTC instant itself.
    pub fn epoch_micros(&self) -> i128 {
        ((self.days as i128) * 86_400 + self.secs as i128) * 1_000_000 + self.micros as i128
            - self.offset_micros.unwrap_or(0) as i128
    }

    pub fn is_aware(&self) -> bool {
        self.offset_micros.is_some()
    }

    /// The instant a span of seconds later, the offset kept.
    pub fn plus_seconds(&self, seconds: i64) -> Instant {
        Instant::from_epoch_micros(
            self.epoch_micros() + seconds as i128 * 1_000_000,
            self.offset_micros,
        )
    }
}

impl PartialEq for Instant {
    fn eq(&self, other: &Self) -> bool {
        self.epoch_micros() == other.epoch_micros()
    }
}

impl Eq for Instant {}

impl PartialOrd for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Instant {
    fn cmp(&self, other: &Self) -> Ordering {
        self.epoch_micros().cmp(&other.epoch_micros())
    }
}

fn validate_now(now: &Instant) -> Result<()> {
    if now.offset_micros.is_none() {
        return Err(Error::InvalidRequest(
            "the current instant must be a timezone aware datetime".to_string(),
        ));
    }
    Ok(())
}

/// Format an instant in UTC, refusing one outside the recorded
/// range, the years 1 through 9999 the format holds.
pub fn format_instant(moment: &Instant) -> Result<String> {
    let utc = moment.epoch_micros();
    let minimum = (days_from_civil(1, 1, 1) as i128) * 86_400 * 1_000_000;
    let maximum = ((days_from_civil(9999, 12, 31) as i128) * 86_400 + 86_399) * 1_000_000 + 999_999;
    if utc < minimum || utc > maximum {
        return Err(Error::InvalidRequest(
            "the instant stands outside the range the switch can record".to_string(),
        ));
    }
    let days = utc.div_euclid(86_400_000_000);
    let rest = utc.rem_euclid(86_400_000_000);
    let (year, month, day) = civil_from_days(days as i64);
    let secs = rest / 1_000_000;
    let micros = rest % 1_000_000;
    let mut out = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    );
    if micros > 0 {
        out.push_str(&format!(".{micros:06}"));
    }
    out.push_str("+00:00");
    Ok(out)
}

fn digit2(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    let first = *bytes.get(pos)?;
    let second = *bytes.get(pos + 1)?;
    if !first.is_ascii_digit() || !second.is_ascii_digit() {
        return None;
    }
    Some(((first - b'0') as u32 * 10 + (second - b'0') as u32, pos + 2))
}

fn parse_fraction(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    // The caller has seen the dot or comma. At least one digit
    // must follow, and the digits past the sixth are truncated.
    let mut index = pos + 1;
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == start {
        return None;
    }
    let mut micros: u32 = 0;
    let mut place: u32 = 100_000;
    for digit in &bytes[start..index] {
        if place == 0 {
            break;
        }
        micros += (*digit - b'0') as u32 * place;
        place /= 10;
    }
    Some((micros, index))
}

fn week_date_days(year: i64, week: u32, weekday: u32) -> Option<i64> {
    if week < 1 || week > weeks_in_iso_year(year) || !(1..=7).contains(&weekday) {
        return None;
    }
    let jan4 = days_from_civil(year, 1, 4);
    let week_one_monday = jan4 - (iso_weekday(jan4) as i64 - 1);
    let days = week_one_monday + (week as i64 - 1) * 7 + (weekday as i64 - 1);
    let (result_year, _, _) = civil_from_days(days);
    if !(1..=9999).contains(&result_year) {
        return None;
    }
    Some(days)
}

fn parse_date(bytes: &[u8], start: usize) -> Option<(i64, usize)> {
    if start + 4 > bytes.len() {
        return None;
    }
    let mut year: i64 = 0;
    for ch in bytes.iter().skip(start).take(4) {
        if !ch.is_ascii_digit() {
            return None;
        }
        year = year * 10 + (ch - b'0') as i64;
    }
    if year == 0 {
        return None;
    }
    let mut pos = start + 4;
    if pos < bytes.len() && bytes[pos] == b'-' {
        pos += 1;
        if pos < bytes.len() && bytes[pos] == b'W' {
            pos += 1;
            let (week, next) = digit2(bytes, pos)?;
            pos = next;
            let mut weekday = 1;
            if pos < bytes.len() && bytes[pos] == b'-' {
                pos += 1;
                let ch = *bytes.get(pos)?;
                if !ch.is_ascii_digit() {
                    return None;
                }
                weekday = (ch - b'0') as u32;
                pos += 1;
            }
            return week_date_days(year, week, weekday).map(|days| (days, pos));
        }
        let (month, next) = digit2(bytes, pos)?;
        pos = next;
        if pos < bytes.len() && bytes[pos] == b'-' {
            pos += 1;
            let (day, next) = digit2(bytes, pos)?;
            pos = next;
            if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
                return None;
            }
            return Some((days_from_civil(year, month, day), pos));
        }
        return None;
    }
    if pos < bytes.len() && bytes[pos] == b'W' {
        pos += 1;
        let (week, next) = digit2(bytes, pos)?;
        pos = next;
        let mut weekday = 1;
        if pos < bytes.len() && bytes[pos].is_ascii_digit() {
            weekday = (bytes[pos] - b'0') as u32;
            pos += 1;
        }
        return week_date_days(year, week, weekday).map(|days| (days, pos));
    }
    let (month, next) = digit2(bytes, pos)?;
    pos = next;
    let (day, next) = digit2(bytes, pos)?;
    pos = next;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some((days_from_civil(year, month, day), pos))
}

fn parse_offset(bytes: &[u8], start: usize) -> Option<(i64, usize)> {
    let sign: i64 = if bytes[start] == b'-' { -1 } else { 1 };
    let mut pos = start + 1;
    let (hours, next) = digit2(bytes, pos)?;
    pos = next;
    let mut minutes = 0u32;
    let mut seconds = 0u32;
    let mut micros = 0u32;
    if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
        let (parsed, next) = parse_fraction(bytes, pos)?;
        micros = parsed;
        pos = next;
    } else if pos < bytes.len() && bytes[pos] == b':' {
        pos += 1;
        let (parsed, next) = digit2(bytes, pos)?;
        minutes = parsed;
        pos = next;
        if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
            let (parsed, next) = parse_fraction(bytes, pos)?;
            micros = parsed;
            pos = next;
        } else if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
            let (parsed, next) = digit2(bytes, pos)?;
            seconds = parsed;
            pos = next;
            if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                let (parsed, next) = parse_fraction(bytes, pos)?;
                micros = parsed;
                pos = next;
            }
        }
    } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
        let (parsed, next) = digit2(bytes, pos)?;
        minutes = parsed;
        pos = next;
        if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
            let (parsed, next) = parse_fraction(bytes, pos)?;
            micros = parsed;
            pos = next;
        } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
            let (parsed, next) = digit2(bytes, pos)?;
            seconds = parsed;
            pos = next;
            if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                let (parsed, next) = parse_fraction(bytes, pos)?;
                micros = parsed;
                pos = next;
            }
        }
    }
    let whole = hours * 3600 + minutes * 60 + seconds;
    if whole >= 86_400 {
        return None;
    }
    if whole == 0 {
        // The Python parser drops the fraction of an offset whose
        // whole components are all zero, probed at +00:00:00.5 and
        // at -00:00:00.25, both reading as a zero offset.
        micros = 0;
    }
    Some((sign * (whole as i64 * 1_000_000 + micros as i64), pos))
}

/// Parse an instant in the family the Python fromisoformat
/// accepted, probed case by case before the port was written.
/// None stands for every refusal the Python parser raises.
pub fn parse_instant(text: &str) -> Option<Instant> {
    let bytes = text.as_bytes();
    let (days, mut pos) = parse_date(bytes, 0)?;
    let (mut secs, mut micros) = (0u32, 0u32);
    let mut offset_micros: Option<i64> = None;
    if pos == bytes.len() {
        return Some(Instant {
            days,
            secs,
            micros,
            offset_micros,
        });
    }
    // The separator between the date and the time is any single
    // character, the Python parser's own reading, so the advance
    // is by the character's own length. Every other position in
    // the grammar accepts ASCII only, so a non-ASCII byte anywhere
    // else still refuses.
    pos += text[pos..].chars().next()?.len_utf8();
    let (hours, next) = digit2(bytes, pos)?;
    pos = next;
    let mut minutes = 0u32;
    let mut seconds = 0u32;
    if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
        let (parsed, next) = parse_fraction(bytes, pos)?;
        micros = parsed;
        pos = next;
    } else if pos < bytes.len() && bytes[pos] == b':' {
        pos += 1;
        let (parsed, next) = digit2(bytes, pos)?;
        minutes = parsed;
        pos = next;
        if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
            let (parsed, next) = parse_fraction(bytes, pos)?;
            micros = parsed;
            pos = next;
        } else if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
            let (parsed, next) = digit2(bytes, pos)?;
            seconds = parsed;
            pos = next;
            if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                let (parsed, next) = parse_fraction(bytes, pos)?;
                micros = parsed;
                pos = next;
            }
        }
    } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
        let (parsed, next) = digit2(bytes, pos)?;
        minutes = parsed;
        pos = next;
        if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
            let (parsed, next) = parse_fraction(bytes, pos)?;
            micros = parsed;
            pos = next;
        } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
            let (parsed, next) = digit2(bytes, pos)?;
            seconds = parsed;
            pos = next;
            if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                let (parsed, next) = parse_fraction(bytes, pos)?;
                micros = parsed;
                pos = next;
            }
        }
    }
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    secs = hours * 3600 + minutes * 60 + seconds;
    if pos == bytes.len() {
        return Some(Instant {
            days,
            secs,
            micros,
            offset_micros,
        });
    }
    if bytes[pos] == b'Z' {
        pos += 1;
        offset_micros = Some(0);
    } else if bytes[pos] == b'+' || bytes[pos] == b'-' {
        let (parsed, next) = parse_offset(bytes, pos)?;
        offset_micros = Some(parsed);
        pos = next;
    } else {
        return None;
    }
    if pos != bytes.len() {
        return None;
    }
    Some(Instant {
        days,
        secs,
        micros,
        offset_micros,
    })
}

/// Parse a stored instant field under the corruption messages
/// the Python reader raised for it.
fn parse_stored_instant(value: &Value) -> Result<Instant> {
    match value {
        Value::Str(text) => match parse_instant(text) {
            None => Err(corrupt(format!(
                "an instant field holds unparseable text {}",
                py_repr_str(text)
            ))),
            Some(instant) if !instant.is_aware() => Err(corrupt(format!(
                "an instant field holds a naive instant {}",
                py_repr_str(text)
            ))),
            Some(instant) => Ok(instant),
        },
        other => Err(corrupt(format!(
            "an instant field holds {} instead of text",
            py_repr_value(other)
        ))),
    }
}

// ----------------------------------------------------------------
// Value level validation.
//
// Names and caller integers are validated through the validators
// here, which the typed operations and the command line
// converters and the strict reader all share. The Python
// implementation validated the values its callers handed it at
// runtime, and the Rust type system makes most wrong shapes
// unrepresentable at the typed surface, so these validators are
// where the refusal lives for the shapes that remain.

fn name_shape_ok(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() || chars.len() > NAME_MAX {
        return false;
    }
    let edge = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    if !edge(chars[0]) || !edge(chars[chars.len() - 1]) {
        return false;
    }
    chars.iter().all(|ch| edge(*ch) || *ch == '-' || *ch == '_')
}

/// Refuse any run id outside the name rule of DM-9.
pub fn validate_name(value: &Value) -> Result<()> {
    let ok = matches!(value, Value::Str(text) if name_shape_ok(text));
    if !ok {
        return Err(Error::InvalidName(format!(
            "invalid run id {}, use one component of ASCII lowercase letters and \
             ASCII digits and hyphens and underscores, starting and ending with an \
             ASCII lowercase letter or an ASCII digit, at most {NAME_MAX} characters long",
            py_repr_value(value)
        )));
    }
    Ok(())
}

fn validate_stored_name(value: &Value) -> Result<String> {
    match value {
        Value::Str(text) if name_shape_ok(text) => Ok(text.clone()),
        _ => Err(corrupt(format!(
            "a run holds an invalid run id {}",
            py_repr_value(value)
        ))),
    }
}

fn validate_positive_int(value: &Value, field: &str) -> Result<JsonInt> {
    match value.as_json_int() {
        Some(number) if number.cmp_i128(1) != Ordering::Less => Ok(number),
        _ => Err(Error::InvalidRequest(format!(
            "invalid {field} {}, use a positive integer",
            py_printable(value)
        ))),
    }
}

fn json_int_as_i64(number: &JsonInt) -> i64 {
    match number {
        JsonInt::Small(value) => *value as i64,
        JsonInt::Big(_) => unreachable!("a bounded value is never big"),
    }
}

/// The proof interval as a registration value, positive and at
/// most the DM-9 bound, returned in seconds.
pub fn validate_proof_interval(value: &Value) -> Result<i64> {
    let number = validate_positive_int(value, "proof interval")?;
    if number.cmp_i128(PROOF_INTERVAL_MAX_SECONDS as i128) == Ordering::Greater {
        return Err(Error::InvalidRequest(format!(
            "invalid proof interval {}, use a positive integer of at most \
             {PROOF_INTERVAL_MAX_SECONDS} seconds",
            py_printable(value)
        )));
    }
    Ok(json_int_as_i64(&number))
}

/// The missed proof limit as a registration value, positive and
/// at most the DM-9 bound.
pub fn validate_missed_proof_limit(value: &Value) -> Result<i64> {
    let number = validate_positive_int(value, "missed proof limit")?;
    if number.cmp_i128(MISSED_PROOF_LIMIT_MAX as i128) == Ordering::Greater {
        return Err(Error::InvalidRequest(format!(
            "invalid missed proof limit {}, use a positive integer of at most \
             {MISSED_PROOF_LIMIT_MAX}",
            py_printable(value)
        )));
    }
    Ok(json_int_as_i64(&number))
}

/// A ceiling as a registration value, unset or a nonnegative
/// integer of at most the spend bound.
pub fn validate_ceiling(value: &Value, field: &str) -> Result<Option<i64>> {
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    match value.as_json_int() {
        Some(number)
            if number.cmp_i128(0) != Ordering::Less
                && number.cmp_i128(SPEND_MAX as i128) != Ordering::Greater =>
        {
            Ok(Some(json_int_as_i64(&number)))
        }
        _ => Err(Error::InvalidRequest(format!(
            "invalid {field} {}, use a nonnegative integer of at most {SPEND_MAX} \
             or leave it unset",
            py_printable(value)
        ))),
    }
}

/// A spend amount, a nonnegative integer of at most the spend
/// bound.
pub fn validate_spend_amount(value: &Value) -> Result<i64> {
    match value.as_json_int() {
        Some(number)
            if number.cmp_i128(0) != Ordering::Less
                && number.cmp_i128(SPEND_MAX as i128) != Ordering::Greater =>
        {
            Ok(json_int_as_i64(&number))
        }
        _ => Err(Error::InvalidRequest(format!(
            "invalid spend amount {}, use a nonnegative integer of at most {SPEND_MAX}",
            py_printable(value)
        ))),
    }
}

/// A proof note, a string of at most the note bound. The content
/// is never judged in this slice.
pub fn validate_note(value: &Value) -> Result<()> {
    match value {
        Value::Str(text) if text.chars().count() <= NOTE_MAX => Ok(()),
        _ => Err(Error::InvalidRequest(format!(
            "invalid note, use a string of at most {NOTE_MAX} characters"
        ))),
    }
}

// ----------------------------------------------------------------
// The typed document and its strict validation.

/// One decision log record. The reserved model fields stand null
/// in every record of this slice and are carried by the writer,
/// so the typed form holds only the fields that can stand.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub seq: u64,
    pub kind: String,
    pub at: String,
    pub stage: String,
    pub reasons: Vec<String>,
}

struct Run {
    id: String,
    registered_at: String,
    proof_interval_seconds: JsonInt,
    missed_proof_limit: i64,
    soft_ceiling: Option<i64>,
    hard_ceiling: Option<i64>,
    spend_total: i64,
    last_proof_at: Option<String>,
    stage: String,
    clear_streak: JsonInt,
    killed: bool,
    log: Vec<Record>,
}

struct Document {
    runs: Vec<Run>,
}

fn empty_document() -> Document {
    Document { runs: Vec::new() }
}

fn object_map(value: &Value) -> Option<std::collections::HashMap<&str, &Value>> {
    match value {
        Value::Object(pairs) => Some(
            pairs
                .iter()
                .map(|(key, item)| (key.as_str(), item))
                .collect(),
        ),
        _ => None,
    }
}

fn keys_match(map: &std::collections::HashMap<&str, &Value>, expected: &[&str]) -> bool {
    map.len() == expected.len() && expected.iter().all(|key| map.contains_key(key))
}

fn validate_record(value: &Value, position: usize) -> Result<Record> {
    let map = match object_map(value) {
        Some(map) if keys_match(&map, &RECORD_KEYS) => map,
        _ => {
            return Err(corrupt(
                "a log record does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let seq = map["seq"];
    match seq.as_json_int() {
        Some(number) if number.cmp_i128(position as i128 + 1) == Ordering::Equal => {}
        _ => {
            return Err(corrupt(
                "the log sequence does not grow by 1 from 1 without gaps".to_string(),
            ))
        }
    }
    let kind = match map["kind"] {
        Value::Str(text) if RECORD_KINDS.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "a log record holds an unknown kind {}",
                py_repr_value(other)
            )))
        }
    };
    parse_stored_instant(map["at"])?;
    let at = map["at"]
        .as_str()
        .expect("a parsed instant is text")
        .to_string();
    let stage = match map["stage"] {
        Value::Str(text) if STAGES.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "a log record holds an unknown stage {}",
                py_repr_value(other)
            )))
        }
    };
    let reasons: Vec<String> = match map["reasons"] {
        Value::Array(items) => {
            let mut reasons = Vec::new();
            for item in items {
                match item {
                    Value::Str(text) if TRIGGERS.contains(&text.as_str()) => {
                        reasons.push(text.clone())
                    }
                    _ => {
                        return Err(corrupt(
                            "a log record holds a reason outside the recorded names".to_string(),
                        ))
                    }
                }
            }
            reasons
        }
        _ => {
            return Err(corrupt(
                "a log record holds a reason outside the recorded names".to_string(),
            ))
        }
    };
    let mut seen_reasons: HashSet<&str> = HashSet::new();
    for reason in &reasons {
        if !seen_reasons.insert(reason.as_str()) {
            return Err(corrupt("a log record names one reason twice".to_string()));
        }
    }
    for field in RESERVED_FIELDS {
        if !matches!(map[field], Value::Null) {
            return Err(corrupt(format!(
                "a log record carries a value in the reserved field {field}"
            )));
        }
    }
    if matches!(kind.as_str(), "registered" | "proof" | "spend") && !reasons.is_empty() {
        return Err(corrupt(format!("a {kind} record names reasons")));
    }
    if kind == "registered" && stage != "none" {
        return Err(corrupt(
            "a registered record stands at a stage other than none".to_string(),
        ));
    }
    if kind == "pull" && (stage != "kill" || reasons != ["operator pull"]) {
        return Err(corrupt(
            "a pull record does not record the operator pull at kill".to_string(),
        ));
    }
    Ok(Record {
        seq: position as u64 + 1,
        kind,
        at,
        stage,
        reasons,
    })
}

fn validate_stored_ceiling(
    map: &std::collections::HashMap<&str, &Value>,
    field: &str,
    id: &str,
) -> Result<Option<i64>> {
    match map[field] {
        Value::Null => Ok(None),
        value => match value.as_json_int() {
            Some(number)
                if number.cmp_i128(0) != Ordering::Less
                    && number.cmp_i128(SPEND_MAX as i128) != Ordering::Greater =>
            {
                Ok(Some(json_int_as_i64(&number)))
            }
            _ => Err(corrupt(format!(
                "run {} holds an invalid {field}",
                py_repr_str(id)
            ))),
        },
    }
}

fn validate_run(value: &Value) -> Result<Run> {
    let map = match object_map(value) {
        Some(map) if keys_match(&map, &RUN_KEYS) => map,
        _ => {
            return Err(corrupt(
                "a run does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let id = validate_stored_name(map["id"])?;
    let id_repr = py_repr_str(&id);
    parse_stored_instant(map["registered_at"])?;
    let registered_at = map["registered_at"]
        .as_str()
        .expect("a parsed instant is text")
        .to_string();
    let proof_interval_seconds = match map["proof_interval_seconds"].as_json_int() {
        Some(number) if number.cmp_i128(1) != Ordering::Less => number,
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds an invalid proof interval"
            )))
        }
    };
    let missed_proof_limit = match map["missed_proof_limit"].as_json_int() {
        Some(number)
            if number.cmp_i128(1) != Ordering::Less
                && number.cmp_i128(MISSED_PROOF_LIMIT_MAX as i128) != Ordering::Greater =>
        {
            json_int_as_i64(&number)
        }
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds an invalid missed proof limit"
            )))
        }
    };
    let soft_ceiling = validate_stored_ceiling(&map, "soft_ceiling", &id)?;
    let hard_ceiling = validate_stored_ceiling(&map, "hard_ceiling", &id)?;
    if let (Some(soft), Some(hard)) = (soft_ceiling, hard_ceiling) {
        if soft > hard {
            return Err(corrupt(format!(
                "run {id_repr} holds a soft ceiling above its hard ceiling"
            )));
        }
    }
    let spend_total = match map["spend_total"].as_json_int() {
        Some(number)
            if number.cmp_i128(0) != Ordering::Less
                && number.cmp_i128(SPEND_MAX as i128) != Ordering::Greater =>
        {
            json_int_as_i64(&number)
        }
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds an invalid spend total"
            )))
        }
    };
    let last_proof_at = match map["last_proof_at"] {
        Value::Null => None,
        value => {
            parse_stored_instant(value)?;
            Some(
                value
                    .as_str()
                    .expect("a parsed instant is text")
                    .to_string(),
            )
        }
    };
    let stage = match map["stage"] {
        Value::Str(text) if STAGES.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "run {id_repr} holds an unknown stage {}",
                py_repr_value(other)
            )))
        }
    };
    let clear_streak = match map["clear_streak"].as_json_int() {
        Some(number) if number.cmp_i128(0) != Ordering::Less => number,
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds an invalid clear streak"
            )))
        }
    };
    let killed = match map["killed"] {
        Value::Bool(flag) => *flag,
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds a killed flag that is not a boolean"
            )))
        }
    };
    if killed != (stage == "kill") {
        return Err(corrupt(format!(
            "run {id_repr} holds a killed flag that disagrees with its stage"
        )));
    }
    let log: Vec<Record> = match map["log"] {
        Value::Array(items) if !items.is_empty() => {
            let mut records = Vec::new();
            for (position, item) in items.iter().enumerate() {
                records.push(validate_record(item, position)?);
            }
            records
        }
        _ => return Err(corrupt(format!("run {id_repr} holds no log"))),
    };
    let first = &log[0];
    if first.kind != "registered" || first.at != registered_at {
        return Err(corrupt(format!(
            "run {id_repr} does not open with its registration record"
        )));
    }
    Ok(Run {
        id,
        registered_at,
        proof_interval_seconds,
        missed_proof_limit,
        soft_ceiling,
        hard_ceiling,
        spend_total,
        last_proof_at,
        stage,
        clear_streak,
        killed,
        log,
    })
}

fn validate_document(value: &Value) -> Result<Document> {
    let map = match object_map(value) {
        Some(map) if keys_match(&map, &DOCUMENT_KEYS) => map,
        _ => {
            return Err(corrupt(
                "the document does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    match map["format"] {
        Value::Str(text) if text == FORMAT_NAME => {}
        other => {
            return Err(corrupt(format!(
                "the format name is {}, not {}",
                py_repr_value(other),
                py_repr_str(FORMAT_NAME)
            )))
        }
    }
    let items = match map["runs"] {
        Value::Array(items) => items,
        _ => return Err(corrupt("the runs field is not a list".to_string())),
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut runs = Vec::new();
    for item in items {
        let run = validate_run(item)?;
        if !seen.insert(run.id.clone()) {
            return Err(corrupt(format!(
                "run {} stands in the state twice",
                py_repr_str(&run.id)
            )));
        }
        runs.push(run);
    }
    Ok(Document { runs })
}

// ----------------------------------------------------------------
// The document writer.
//
// Hand rolled over the fixed shape of the format, reproducing
// the Python byte layout, its indentation of two spaces, its
// construction key order, and its escaping of text outside the
// printable ASCII run, because the stored bytes are part of the
// recorded behaviour.

fn json_int_value(number: &JsonInt) -> Value {
    match number {
        JsonInt::Small(value) => Value::Int(*value),
        JsonInt::Big(text) => Value::BigInt(text.clone()),
    }
}

fn document_to_value(document: &Document) -> Value {
    let runs: Vec<Value> = document
        .runs
        .iter()
        .map(|run| {
            let records: Vec<Value> = run
                .log
                .iter()
                .map(|record| {
                    Value::Object(vec![
                        ("seq".to_string(), Value::Int(record.seq as i128)),
                        ("kind".to_string(), Value::Str(record.kind.clone())),
                        ("at".to_string(), Value::Str(record.at.clone())),
                        ("stage".to_string(), Value::Str(record.stage.clone())),
                        (
                            "reasons".to_string(),
                            Value::Array(
                                record
                                    .reasons
                                    .iter()
                                    .map(|reason| Value::Str(reason.clone()))
                                    .collect(),
                            ),
                        ),
                        ("model_name".to_string(), Value::Null),
                        ("model_confidence".to_string(), Value::Null),
                        ("fallback_taken".to_string(), Value::Null),
                    ])
                })
                .collect();
            Value::Object(vec![
                ("id".to_string(), Value::Str(run.id.clone())),
                (
                    "registered_at".to_string(),
                    Value::Str(run.registered_at.clone()),
                ),
                (
                    "proof_interval_seconds".to_string(),
                    json_int_value(&run.proof_interval_seconds),
                ),
                (
                    "missed_proof_limit".to_string(),
                    Value::Int(run.missed_proof_limit as i128),
                ),
                (
                    "soft_ceiling".to_string(),
                    run.soft_ceiling
                        .map_or(Value::Null, |v| Value::Int(v as i128)),
                ),
                (
                    "hard_ceiling".to_string(),
                    run.hard_ceiling
                        .map_or(Value::Null, |v| Value::Int(v as i128)),
                ),
                (
                    "spend_total".to_string(),
                    Value::Int(run.spend_total as i128),
                ),
                (
                    "last_proof_at".to_string(),
                    run.last_proof_at.clone().map_or(Value::Null, Value::Str),
                ),
                ("stage".to_string(), Value::Str(run.stage.clone())),
                (
                    "clear_streak".to_string(),
                    json_int_value(&run.clear_streak),
                ),
                ("killed".to_string(), Value::Bool(run.killed)),
                ("log".to_string(), Value::Array(records)),
            ])
        })
        .collect();
    Value::Object(vec![
        ("format".to_string(), Value::Str(FORMAT_NAME.to_string())),
        ("runs".to_string(), Value::Array(runs)),
    ])
}

fn write_document(document: &Document) -> String {
    let mut out = String::new();
    dump_value(&document_to_value(document), 0, &mut out);
    out.push('\n');
    out
}

fn dump_value(value: &Value, level: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::BigInt(text) => out.push_str(text),
        Value::Float(number) => out.push_str(&py_float_repr(*number)),
        Value::Str(text) => dump_json_string(text, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&"  ".repeat(level + 1));
                dump_value(item, level + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(level));
            out.push(']');
        }
        Value::Object(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, item)) in pairs.iter().enumerate() {
                out.push_str(&"  ".repeat(level + 1));
                dump_json_string(key, out);
                out.push_str(": ");
                dump_value(item, level + 1, out);
                if index + 1 < pairs.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(level));
            out.push('}');
        }
    }
}

fn dump_json_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            ch if ('\u{0020}'..='\u{007e}').contains(&ch) => out.push(ch),
            ch => {
                let code = ch as u32;
                if code < 0x10000 {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    let shifted = code - 0x10000;
                    out.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (shifted >> 10),
                        0xdc00 + (shifted & 0x3ff)
                    ));
                }
            }
        }
    }
    out.push('"');
}

// ----------------------------------------------------------------
// The switch.

/// The outcome of one evaluation of one run.
#[derive(Clone, Debug, PartialEq)]
pub struct StageReport {
    pub run: String,
    pub stage: String,
    pub reasons: Vec<String>,
}

fn stage_index(stage: &str) -> usize {
    STAGES
        .iter()
        .position(|candidate| *candidate == stage)
        .expect("a stored stage stands in the ladder")
}

/// Whole proof intervals missed at the instant, deadline exact
/// counts zero.
fn missed_proofs(run: &Run, now: &Instant) -> Result<i128> {
    let base_text = run.last_proof_at.as_deref().unwrap_or(&run.registered_at);
    let base = parse_instant(base_text).ok_or_else(|| {
        corrupt(format!(
            "an instant field holds unparseable text {}",
            py_repr_str(base_text)
        ))
    })?;
    // The interval as a duration overflows the representable
    // range past the largest whole second count a duration of
    // 999999999 days holds, which is the check the Python
    // timedelta construction raises as its overflow error.
    if run
        .proof_interval_seconds
        .cmp_i128(PROOF_INTERVAL_MAX_SECONDS as i128)
        == Ordering::Greater
    {
        return Err(Error::InvalidRequest(
            "the missed proof count at this instant stands outside the range the \
             switch can represent"
                .to_string(),
        ));
    }
    let interval_seconds = match &run.proof_interval_seconds {
        JsonInt::Small(value) => *value,
        JsonInt::Big(_) => unreachable!("the bound check above refuses a big interval"),
    };
    let interval_micros = interval_seconds * 1_000_000;
    let delta_micros = now.epoch_micros() - base.epoch_micros();
    if delta_micros <= interval_micros {
        Ok(0)
    } else {
        Ok((delta_micros - 1) / interval_micros)
    }
}

/// The warranted stage index and the live trigger names at the
/// instant, the triggers in the order DM-3 names them.
fn warrant(run: &Run, now: &Instant) -> Result<(usize, Vec<&'static str>)> {
    let mut index = 0usize;
    let mut reasons: Vec<&'static str> = Vec::new();
    if let Some(hard) = run.hard_ceiling {
        if run.spend_total >= hard {
            index = stage_index("kill");
            reasons.push("hard ceiling");
        }
    }
    if let Some(soft) = run.soft_ceiling {
        if run.spend_total >= soft {
            index = index.max(stage_index("cut budget"));
            reasons.push("soft ceiling");
        }
    }
    let missed = missed_proofs(run, now)?;
    if missed >= 1 {
        reasons.push("missed proofs");
        if missed >= run.missed_proof_limit as i128 {
            index = index.max(stage_index("kill"));
        } else {
            let warranted = MISSED_COUNT_STAGES[(missed.min(4) - 1) as usize];
            index = index.max(stage_index(warranted));
        }
    }
    Ok((index, reasons))
}

struct LockGuard {
    file: File,
    lock_path: PathBuf,
}

impl LockGuard {
    /// Whether the file this guard holds is the file standing
    /// at the lock path now.
    fn identity_holds(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        let held = match self.file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };
        let standing = match std::fs::metadata(&self.lock_path) {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };
        held.dev() == standing.dev() && held.ino() == standing.ino()
    }

    /// Refuse to go on when the held lock is not the file at
    /// the lock path, because anything this command published
    /// could then be overwritten by the command that locked the
    /// standing file, and a success of this command could be
    /// erased without a trace.
    fn verify(&self) -> Result<()> {
        if self.identity_holds() {
            Ok(())
        } else {
            Err(Error::Failure(format!(
                "the lock file at {} is not the file this command locked, \
                 so the state was left unchanged",
                self.lock_path.display()
            )))
        }
    }
}

/// One staged cutoff in one state file, opened fresh on every
/// call.
pub struct Switch {
    path: PathBuf,
    lock_path: PathBuf,
}

impl Switch {
    pub fn new(path: PathBuf) -> Switch {
        let lock_path = match path.file_name() {
            Some(name) => {
                let mut lock_name = name.to_os_string();
                lock_name.push(LOCK_SUFFIX);
                path.with_file_name(lock_name)
            }
            None => {
                let mut raw = path.clone().into_os_string();
                raw.push(LOCK_SUFFIX);
                PathBuf::from(raw)
            }
        };
        Switch { path, lock_path }
    }

    fn locked(&self, exclusive: bool) -> Result<LockGuard> {
        use std::os::unix::fs::OpenOptionsExt;
        // The identity of the locked file is checked as soon as
        // the lock stands. A mismatch means the file at the path
        // was deleted and replaced under this command, so the
        // command locks the standing file on a bounded retry
        // instead of working under a file no other command can
        // see, and refuses when the path will not settle.
        for attempt in 1..=5 {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o644)
                .open(&self.lock_path)
                .map_err(|err| {
                    Error::Failure(format!(
                        "the lock file beside the state file cannot be opened, {err}"
                    ))
                })?;
            let result = if exclusive {
                file.lock_exclusive()
            } else {
                file.lock_shared()
            };
            result.map_err(|err| {
                Error::Failure(format!(
                    "the lock file beside the state file cannot be locked, {err}"
                ))
            })?;
            let guard = LockGuard {
                file,
                lock_path: self.lock_path.clone(),
            };
            if guard.identity_holds() {
                return Ok(guard);
            }
            if attempt == 5 {
                return Err(Error::Failure(format!(
                    "the lock file at {} is not the file this command locked, \
                     so the state was left unchanged",
                    self.lock_path.display()
                )));
            }
        }
        unreachable!()
    }

    /// Read and strictly validate the state, or start empty.
    ///
    /// A missing file reads as the empty document. A file that
    /// stands but cannot be read as recorded raises the corrupt
    /// error and is never reset and never written over by a read.
    fn read_document(&self) -> Result<Document> {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(empty_document()),
            Err(err) => return Err(corrupt(format!("the state file cannot be read, {err}"))),
        };
        let value = parse_document(&raw).map_err(|failure| match failure {
            ParseFailure::DuplicateKey(key) => {
                corrupt(format!("a key stands twice in the document, {key}"))
            }
            ParseFailure::Syntax(detail) => {
                corrupt(format!("the document does not parse, {detail}"))
            }
        })?;
        validate_document(&value)
    }

    fn publish(&self, document: &Document, guard: &LockGuard) -> Result<()> {
        guard.verify()?;
        let text = write_document(document);
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        name = format!(".{name}.tmp-{}-", std::process::id());
        let mut draw = [0u8; 4];
        getrandom::fill(&mut draw).map_err(|err| {
            Error::Failure(format!("the publication draw could not be made, {err}"))
        })?;
        for byte in draw {
            name.push_str(&format!("{byte:02x}"));
        }
        let temp = self.path.with_file_name(name);
        // The temporary file is opened exclusively, so a plant
        // standing at the name fails the publish instead of being
        // followed and written through, the deliberate divergence
        // the port amendment records.
        let written = (|| -> std::io::Result<()> {
            let mut handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            handle.write_all(text.as_bytes())?;
            handle.sync_all()?;
            Ok(())
        })();
        if let Err(err) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(Error::Failure(format!(
                "the state file cannot be published, {err}"
            )));
        }
        // The held lock's identity is verified once more with the
        // rename as the very next step, so the window between the
        // check and the publication stands as narrow as the
        // design can make it. A failure here leaves the state
        // unchanged under the identity error's own message.
        if let Err(err) = guard.verify() {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
        if let Err(err) = std::fs::rename(&temp, &self.path) {
            let _ = std::fs::remove_file(&temp);
            return Err(Error::Failure(format!(
                "the state file cannot be published, {err}"
            )));
        }
        // A best effort fsync of the holding directory, as the
        // Python publication performs it.
        let parent = self.path.parent().map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        });
        if let Some(parent) = parent {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }

    fn ensure_parent(&self) -> Result<()> {
        let parent = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => return Ok(()),
        };
        std::fs::create_dir_all(&parent).map_err(|err| {
            Error::Failure(format!(
                "the directory holding the state file cannot be created, {err}"
            ))
        })
    }

    fn mutate<T>(
        &self,
        now: &Instant,
        action: impl FnOnce(&mut Document) -> Result<T>,
    ) -> Result<T> {
        validate_now(now)?;
        self.ensure_parent()?;
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let result = action(&mut document)?;
        self.publish(&document, &guard)?;
        Ok(result)
    }

    fn read<T>(&self, action: impl FnOnce(&Document) -> Result<T>) -> Result<T> {
        if !self.path.exists() {
            return action(&empty_document());
        }
        let _guard = self.locked(false)?;
        action(&self.read_document()?)
    }

    fn append_record(
        run: &mut Run,
        kind: &str,
        now: &Instant,
        stage: &str,
        reasons: &[&str],
    ) -> Result<()> {
        let at = format_instant(now)?;
        run.log.push(Record {
            seq: run.log.len() as u64 + 1,
            kind: kind.to_string(),
            at,
            stage: stage.to_string(),
            reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
        });
        Ok(())
    }

    fn require_run(document: &Document, run_id: &str) -> Result<usize> {
        match document.runs.iter().position(|run| run.id == run_id) {
            Some(index) => Ok(index),
            None => Err(Error::NotFound(format!(
                "run {} does not stand in the state file",
                py_repr_str(run_id)
            ))),
        }
    }

    /// Open a run record and record the registration.
    pub fn register(
        &self,
        run_id: &Value,
        now: &Instant,
        proof_interval_seconds: &Value,
        missed_proof_limit: &Value,
        soft_ceiling: &Value,
        hard_ceiling: &Value,
    ) -> Result<()> {
        validate_name(run_id)?;
        let proof_interval_seconds = validate_proof_interval(proof_interval_seconds)?;
        let missed_proof_limit = validate_missed_proof_limit(missed_proof_limit)?;
        let soft_ceiling = validate_ceiling(soft_ceiling, "soft ceiling")?;
        let hard_ceiling = validate_ceiling(hard_ceiling, "hard ceiling")?;
        if let (Some(soft), Some(hard)) = (soft_ceiling, hard_ceiling) {
            if soft > hard {
                return Err(Error::InvalidRequest(
                    "the soft ceiling stands above the hard ceiling, declare the soft \
                     ceiling at or below the hard ceiling"
                        .to_string(),
                ));
            }
        }
        let run_id = match run_id {
            Value::Str(text) => text.clone(),
            _ => unreachable!("a validated name is text"),
        };
        self.mutate(now, |document| {
            if document.runs.iter().any(|run| run.id == run_id) {
                return Err(Error::Refusal(format!(
                    "run {} already stands in the state file",
                    py_repr_str(&run_id)
                )));
            }
            let mut run = Run {
                id: run_id.clone(),
                registered_at: format_instant(now)?,
                proof_interval_seconds: JsonInt::Small(proof_interval_seconds as i128),
                missed_proof_limit,
                soft_ceiling,
                hard_ceiling,
                spend_total: 0,
                last_proof_at: None,
                stage: "none".to_string(),
                clear_streak: JsonInt::Small(0),
                killed: false,
                log: Vec::new(),
            };
            Self::append_record(&mut run, "registered", now, "none", &[])?;
            document.runs.push(run);
            Ok(())
        })
    }

    /// Record a progress proof. Content is never judged in this
    /// slice.
    pub fn prove(&self, run_id: &Value, note: &Value, now: &Instant) -> Result<()> {
        validate_name(run_id)?;
        validate_note(note)?;
        let run_id = match run_id {
            Value::Str(text) => text.clone(),
            _ => unreachable!("a validated name is text"),
        };
        self.mutate(now, |document| {
            let index = Self::require_run(document, &run_id)?;
            let run = &mut document.runs[index];
            if !run.killed {
                let stands = match &run.last_proof_at {
                    None => true,
                    Some(text) => {
                        let standing = parse_instant(text).ok_or_else(|| {
                            corrupt(format!(
                                "an instant field holds unparseable text {}",
                                py_repr_str(text)
                            ))
                        })?;
                        *now > standing
                    }
                };
                if stands {
                    run.last_proof_at = Some(format_instant(now)?);
                }
            }
            let stage = run.stage.clone();
            Self::append_record(run, "proof", now, &stage, &[])
        })
    }

    /// Record a spend report and return the accumulated spend.
    pub fn spend(&self, run_id: &Value, amount: &Value, now: &Instant) -> Result<i64> {
        validate_name(run_id)?;
        let amount = validate_spend_amount(amount)?;
        let run_id = match run_id {
            Value::Str(text) => text.clone(),
            _ => unreachable!("a validated name is text"),
        };
        self.mutate(now, |document| {
            let index = Self::require_run(document, &run_id)?;
            let run = &mut document.runs[index];
            if !run.killed {
                if run.spend_total + amount > SPEND_MAX {
                    return Err(Error::InvalidRequest(format!(
                        "the spend report of {amount} would carry the accumulated \
                         spend of run {} past {SPEND_MAX}",
                        py_repr_str(&run_id)
                    )));
                }
                run.spend_total += amount;
            }
            let stage = run.stage.clone();
            Self::append_record(run, "spend", now, &stage, &[])?;
            Ok(run.spend_total)
        })
    }

    /// Pull the switch. A pull goes straight to kill and latches
    /// it.
    pub fn pull(&self, run_id: &Value, now: &Instant) -> Result<String> {
        validate_name(run_id)?;
        validate_now(now)?;
        let run_id = match run_id {
            Value::Str(text) => text.clone(),
            _ => unreachable!("a validated name is text"),
        };
        self.ensure_parent()?;
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let index = Self::require_run(&document, &run_id)?;
        if document.runs[index].killed {
            return Ok("kill".to_string());
        }
        let run = &mut document.runs[index];
        run.stage = "kill".to_string();
        run.killed = true;
        run.clear_streak = JsonInt::Small(0);
        Self::append_record(run, "pull", now, "kill", &["operator pull"])?;
        self.publish(&document, &guard)?;
        Ok("kill".to_string())
    }

    /// Evaluate one run at the instant and report its stage and
    /// reasons.
    pub fn stage(&self, run_id: &Value, now: &Instant) -> Result<StageReport> {
        validate_name(run_id)?;
        validate_now(now)?;
        let run_id = match run_id {
            Value::Str(text) => text.clone(),
            _ => unreachable!("a validated name is text"),
        };
        self.ensure_parent()?;
        let guard = self.locked(true)?;
        let mut document = self.read_document()?;
        let index = Self::require_run(&document, &run_id)?;
        if document.runs[index].killed {
            let (_, reasons) = warrant(&document.runs[index], now)?;
            return Ok(StageReport {
                run: run_id,
                stage: "kill".to_string(),
                reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
            });
        }
        let (warranted, reasons) = warrant(&document.runs[index], now)?;
        let recorded = stage_index(&document.runs[index].stage);
        let mut changed = false;
        if warranted > recorded {
            let run = &mut document.runs[index];
            run.stage = STAGES[warranted].to_string();
            run.clear_streak = JsonInt::Small(0);
            if run.stage == "kill" {
                run.killed = true;
            }
            let stage = run.stage.clone();
            Self::append_record(run, "stage", now, &stage, &reasons)?;
            changed = true;
        } else if warranted == recorded {
            let run = &mut document.runs[index];
            if !run.clear_streak.is_zero() {
                run.clear_streak = JsonInt::Small(0);
                changed = true;
            }
            if recorded != stage_index("none") {
                let stage = run.stage.clone();
                Self::append_record(run, "stage", now, &stage, &reasons)?;
                changed = true;
            }
        } else {
            let run = &mut document.runs[index];
            run.clear_streak = run.clear_streak.add_one();
            changed = true;
            if run.clear_streak.cmp_i128(2) != Ordering::Less {
                run.stage = STAGES[recorded - 1].to_string();
                run.clear_streak = JsonInt::Small(0);
                let stage = run.stage.clone();
                Self::append_record(run, "stage", now, &stage, &reasons)?;
            }
        }
        if changed {
            self.publish(&document, &guard)?;
        }
        Ok(StageReport {
            run: run_id,
            stage: document.runs[index].stage.clone(),
            reasons: reasons.iter().map(|reason| reason.to_string()).collect(),
        })
    }

    /// Return the recorded decision log of one run, in order.
    pub fn log(&self, run_id: &Value) -> Result<Vec<Record>> {
        validate_name(run_id)?;
        let run_id = match run_id {
            Value::Str(text) => text.clone(),
            _ => unreachable!("a validated name is text"),
        };
        self.read(|document| {
            let index = Self::require_run(document, &run_id)?;
            Ok(document.runs[index].log.clone())
        })
    }
}
