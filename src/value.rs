//! Document values, error classes, arbitrary-width JSON integers, and Python repr formatting.

use std::cmp::Ordering;
use std::fmt;

use crate::{INT_DIGIT_LIMIT, TIMEOUT_WHOLE_CEILING};

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

pub(crate) fn corrupt(message: String) -> Error {
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
    pub(crate) fn as_json_int(&self) -> Option<JsonInt> {
        match self {
            Value::Int(value) => Some(JsonInt::Small(*value)),
            Value::BigInt(text) => Some(JsonInt::Big(text.clone())),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
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
pub(crate) enum JsonInt {
    Small(i128),
    Big(String),
}

impl JsonInt {
    pub(crate) fn cmp_i128(&self, bound: i128) -> Ordering {
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

    pub(crate) fn is_zero(&self) -> bool {
        matches!(self, JsonInt::Small(0))
    }

    /// The value one higher. A stored clear streak can stand at
    /// any width, and the step down evaluation adds one to it
    /// before it judges the result, so the addition is exact at
    /// every width the reader admits.
    pub(crate) fn add_one(&self) -> JsonInt {
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

/// The digit limit rule of the two integer arms of py_printable,
/// stated once (DM-A27). An integer whose decimal text, trimmed
/// of its sign, runs past the digit limit falls back to the
/// standing phrase, because the Python runtime cannot repr it
/// either.
fn printable_int_text(text: String) -> String {
    if text.trim_start_matches('-').len() > INT_DIGIT_LIMIT {
        "a value too large to print".to_string()
    } else {
        text
    }
}

/// Repr a request value for an error message, however large it
/// stands. An integer whose decimal text runs past the digit
/// limit cannot be reprised by the Python runtime either, and
/// both implementations fall back to the same standing phrase.
pub fn py_printable(value: &Value) -> String {
    match value {
        Value::Int(number) => printable_int_text(number.to_string()),
        Value::BigInt(text) => printable_int_text(text.clone()),
        _ => py_repr_value(value),
    }
}

/// The rendering of a judge timeout value for its refusal.
/// A value standing at or past the whole ceiling renders as
/// the ceiling itself, the boundary the refusal names,
/// because the value crossed no surface whole. Every other
/// value renders as itself.
pub fn timeout_printable(value: &Value) -> String {
    let past_ceiling = match value {
        Value::Int(number) => *number >= 1 && *number as u128 >= TIMEOUT_WHOLE_CEILING,
        _ => false,
    };
    if past_ceiling {
        TIMEOUT_WHOLE_CEILING.to_string()
    } else {
        py_printable(value)
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
