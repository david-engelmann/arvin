//! The one home of the small JSON reader and the stage ladder
//! helpers the test suites and the recording harness share
//! (DM-A10).
//!
//! tests/escalation.rs and tests/fixtures.rs include this file
//! as a module, and examples/escalation_recording.rs includes
//! the same file through a path attribute, so the recording
//! side and the replay side read the corpus through one text.
//! The reader is deliberately independent of the library's own
//! strict reader: the corpus and the manifest are harness
//! documents, not deadman-state/1 documents, and they do not
//! live under the format's rules. The accessor surface is the
//! union of what the three copies that stood in the consumers
//! carried; each consumer uses a subset of it.

use arvin::STAGES;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

#[allow(dead_code)]
impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(value) => Some(*value),
            Json::Float(value) if value.fract() == 0.0 => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(value) => Some(*value as f64),
            Json::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn keys(&self) -> Vec<&str> {
        match self {
            Json::Object(pairs) => pairs.iter().map(|(name, _)| name.as_str()).collect(),
            _ => Vec::new(),
        }
    }
}

pub struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    pub fn parse(text: &'a str) -> Result<Json, String> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        parser.skip_space();
        let value = parser.value()?;
        parser.skip_space();
        if parser.at != parser.bytes.len() {
            return Err(format!("trailing bytes at {}", parser.at));
        }
        Ok(value)
    }

    fn skip_space(&mut self) {
        while self.at < self.bytes.len()
            && matches!(self.bytes[self.at], b' ' | b'\n' | b'\r' | b'\t')
        {
            self.at += 1;
        }
    }

    fn peek(&self) -> Result<u8, String> {
        self.bytes
            .get(self.at)
            .copied()
            .ok_or_else(|| "unexpected end".to_string())
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek()? {
            b'{' => {
                self.at += 1;
                let mut pairs = Vec::new();
                self.skip_space();
                if self.peek()? == b'}' {
                    self.at += 1;
                    return Ok(Json::Object(pairs));
                }
                loop {
                    self.skip_space();
                    let key = match self.value()? {
                        Json::Str(text) => text,
                        _ => return Err("object key is not a string".to_string()),
                    };
                    self.skip_space();
                    if self.peek()? != b':' {
                        return Err("object pair without a colon".to_string());
                    }
                    self.at += 1;
                    self.skip_space();
                    let value = self.value()?;
                    pairs.push((key, value));
                    self.skip_space();
                    match self.peek()? {
                        b',' => self.at += 1,
                        b'}' => {
                            self.at += 1;
                            return Ok(Json::Object(pairs));
                        }
                        _ => return Err("object pair without a comma".to_string()),
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.peek()? == b']' {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    self.skip_space();
                    items.push(self.value()?);
                    self.skip_space();
                    match self.peek()? {
                        b',' => self.at += 1,
                        b']' => {
                            self.at += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err("array item without a comma".to_string()),
                    }
                }
            }
            b'"' => Ok(Json::Str(self.string()?)),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'n' => self.literal("null", Json::Null),
            _ => self.number(),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(format!("bad literal at {}", self.at))
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.peek()? != b'"' {
            return Err("string does not open".to_string());
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            let byte = self.peek()?;
            self.at += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = self.peek()?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.at..self.at + 4)
                                .ok_or("short unicode escape")?;
                            let text =
                                std::str::from_utf8(hex).map_err(|_| "bad unicode escape")?;
                            let code =
                                u32::from_str_radix(text, 16).map_err(|_| "bad unicode escape")?;
                            self.at += 4;
                            out.push(char::from_u32(code).ok_or("bad unicode code")?);
                        }
                        _ => return Err("bad escape".to_string()),
                    }
                }
                _ => {
                    let start = self.at - 1;
                    let width = if byte < 0x80 {
                        1
                    } else if byte >= 0xF0 {
                        4
                    } else if byte >= 0xE0 {
                        3
                    } else {
                        2
                    };
                    let end = start + width;
                    let chunk = self.bytes.get(start..end).ok_or("short utf8")?;
                    out.push_str(std::str::from_utf8(chunk).map_err(|_| "bad utf8")?);
                    self.at = end;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while self.at < self.bytes.len()
            && matches!(
                self.bytes[self.at],
                b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
            )
        {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| "bad number")?;
        if text.contains('.') || text.contains('e') || text.contains('E') {
            text.parse::<f64>()
                .map(Json::Float)
                .map_err(|_| format!("bad float {text}"))
        } else {
            text.parse::<i64>()
                .map(Json::Int)
                .map_err(|_| format!("bad int {text}"))
        }
    }
}

// ----------------------------------------------------------------
// The stage ladder helpers, over the library's own ladder.
// ----------------------------------------------------------------

#[allow(dead_code)]
pub fn stage_index(stage: &str) -> usize {
    STAGES
        .iter()
        .position(|name| *name == stage)
        .expect("a stage is in the ladder")
}

#[allow(dead_code)]
pub fn missed_warrant_index(missed: i64, limit: i64) -> usize {
    if missed <= 0 {
        0
    } else if missed >= limit {
        stage_index("kill")
    } else {
        stage_index(["warn", "throttle", "cut tools", "cut budget"][(missed.min(4) - 1) as usize])
    }
}
