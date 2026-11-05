//! The recording harness for the escalation slice of Round 3.
//!
//! This example is run by hand by the operator and never by the
//! gate. It builds the corpus documents of
//! tests/fixtures/escalation/corpus.json by driving the landed
//! operations, judges each constructed state once through a live
//! transport pointed at an operator supplied helper command, and
//! either records the fixtures of the round (mode record) or runs
//! the landing live evaluation (mode evaluate) or drives one case
//! end to end for the shim verification (mode verify).
//!
//! Every judgment is cross checked as it lands. The facts the
//! corpus states for a case are read back from the state file the
//! operations wrote and from the vignette the transport was shown,
//! and the record the judgment appends is compared against an
//! independent walk of the combination rule of DM-15 in this file.
//! A case whose facts disagree is a harness failure and stops the
//! run, because a recording taken on a state other than the one
//! the corpus names is worth nothing.
//!
//! A consultation that ends in a transport class (call failed or
//! silence or malformed reply) earns one retry on a fresh
//! document, because no judgment stood in it. A verdict and an
//! abstention stand as recorded and are never retried. Every
//! attempt is counted and printed as it is spent.
//!
//! Usage, one of the following, where the program and any args
//! after it name the helper command the transport drives.
//!
//!   escalation_recording record CORPUS FIXTURES_DIR WORK_DIR
//!       CAPTURE_DIR PROGRAM [ARGS...]
//!   escalation_recording evaluate CORPUS REPORT_PATH WORK_DIR
//!       CAPTURE_DIR PROGRAM [ARGS...]
//!   escalation_recording verify CORPUS CASE_ID WORK_DIR
//!       CAPTURE_DIR PROGRAM [ARGS...]
//!
//! The helper is expected to leave two captures per attempt in
//! CAPTURE_DIR, named CASE.STEP.library-payload.json and
//! CASE.STEP.reply.json, where STEP is record-a1, record-a2,
//! evaluate-a1, evaluate-a2, or verify for the mode and attempt.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use deadman::{
    Instant, Record, SubprocessTransport, Switch, Value, DE_ESCALATION_BAR, ESCALATION_BAR,
    MODEL_NAME, STAGES,
};

// ----------------------------------------------------------------
// A small JSON reader and writer for the corpus and manifest
// shapes this harness moves. The crate carries no JSON library
// for its examples, and the shapes here are the harness's own.
// ----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(text) => Some(text),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(value) => Some(*value),
            Json::Float(value) if value.fract() == 0.0 => Some(*value as i64),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn parse(text: &'a str) -> Result<Json, String> {
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
                    // Copy the full UTF-8 sequence for this byte.
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

fn json_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn json_str(text: &str) -> String {
    format!("\"{}\"", json_escape(text))
}

fn json_f64(value: f64) -> String {
    format!("{value:?}")
}

fn json_opt_str(value: &Option<String>) -> String {
    match value {
        Some(text) => json_str(text),
        None => "null".to_string(),
    }
}

fn json_opt_f64(value: &Option<f64>) -> String {
    match value {
        Some(number) => json_f64(*number),
        None => "null".to_string(),
    }
}

// ----------------------------------------------------------------
// The corpus case, read out of the corpus document.
// ----------------------------------------------------------------

struct Case {
    id: String,
    expected: Option<String>,
    run_id: String,
    interval: i64,
    limit: i64,
    soft: Option<i64>,
    hard: Option<i64>,
    spend_total: i64,
    ops: Vec<Op>,
    judge_at: i64,
    context: String,
    stage_before: String,
    warranted: String,
    missed: i64,
    streak: i64,
    seat: String,
}

enum Op {
    Prove { at: i64, note: String },
    Spend { at: i64, amount: i64 },
    Stage { at: i64 },
}

fn load_cases(corpus: &Json) -> Result<Vec<Case>, String> {
    let mut cases = Vec::new();
    for item in corpus
        .get("cases")
        .and_then(Json::as_array)
        .ok_or("corpus has no cases")?
    {
        let register = item.get("register").ok_or("case has no register")?;
        let facts = item.get("facts").ok_or("case has no facts")?;
        let judge = item.get("judge").ok_or("case has no judge")?;
        let mut ops = Vec::new();
        let mut spend_total = 0i64;
        for op in item
            .get("ops")
            .and_then(Json::as_array)
            .ok_or("case has no ops")?
        {
            let at = op.get("at").and_then(Json::as_i64).ok_or("op has no at")?;
            match op
                .get("op")
                .and_then(Json::as_str)
                .ok_or("op has no kind")?
            {
                "prove" => ops.push(Op::Prove {
                    at,
                    note: op
                        .get("note")
                        .and_then(Json::as_str)
                        .unwrap_or("")
                        .to_string(),
                }),
                "spend" => {
                    let amount = op
                        .get("amount")
                        .and_then(Json::as_i64)
                        .ok_or("spend has no amount")?;
                    spend_total += amount;
                    ops.push(Op::Spend { at, amount });
                }
                "stage" => ops.push(Op::Stage { at }),
                other => return Err(format!("unknown op {other}")),
            }
        }
        cases.push(Case {
            id: item
                .get("id")
                .and_then(Json::as_str)
                .ok_or("case has no id")?
                .to_string(),
            expected: item
                .get("expected")
                .and_then(Json::as_str)
                .map(str::to_string),
            run_id: register
                .get("run_id")
                .and_then(Json::as_str)
                .ok_or("no run id")?
                .to_string(),
            interval: register
                .get("proof_interval_seconds")
                .and_then(Json::as_i64)
                .ok_or("no interval")?,
            limit: register
                .get("missed_proof_limit")
                .and_then(Json::as_i64)
                .ok_or("no limit")?,
            soft: register.get("soft_ceiling").and_then(Json::as_i64),
            hard: register.get("hard_ceiling").and_then(Json::as_i64),
            spend_total,
            ops,
            judge_at: judge
                .get("at")
                .and_then(Json::as_i64)
                .ok_or("judge has no at")?,
            context: judge
                .get("context")
                .and_then(Json::as_str)
                .ok_or("judge has no context")?
                .to_string(),
            stage_before: facts
                .get("stage_before")
                .and_then(Json::as_str)
                .ok_or("no stage_before")?
                .to_string(),
            warranted: facts
                .get("warranted")
                .and_then(Json::as_str)
                .ok_or("no warranted")?
                .to_string(),
            missed: facts
                .get("missed")
                .and_then(Json::as_i64)
                .ok_or("no missed")?,
            streak: facts
                .get("streak")
                .and_then(Json::as_i64)
                .ok_or("no streak")?,
            seat: facts
                .get("seat")
                .and_then(Json::as_str)
                .ok_or("no seat")?
                .to_string(),
        });
    }
    Ok(cases)
}

// ----------------------------------------------------------------
// Construction and judgment.
// ----------------------------------------------------------------

fn base_instant() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).expect("the base instant is in range")
}

fn text(value: &str) -> Value {
    Value::Str(value.to_string())
}

fn int(value: i64) -> Value {
    Value::Int(value as i128)
}

fn build_document(case: &Case, state_path: &Path) -> Result<Switch, String> {
    let switch = Switch::new(state_path.to_path_buf());
    let base = base_instant();
    switch
        .register(
            &text(&case.run_id),
            &base,
            &int(case.interval),
            &int(case.limit),
            &case.soft.map_or(Value::Null, int),
            &case.hard.map_or(Value::Null, int),
        )
        .map_err(|error| format!("register failed: {}", error.message()))?;
    for op in &case.ops {
        match op {
            Op::Prove { at, note } => switch
                .prove(&text(&case.run_id), &text(note), &base.plus_seconds(*at))
                .map_err(|error| format!("prove failed: {}", error.message()))?,
            Op::Spend { at, amount } => switch
                .spend(&text(&case.run_id), &int(*amount), &base.plus_seconds(*at))
                .map(|_| ())
                .map_err(|error| format!("spend failed: {}", error.message()))?,
            Op::Stage { at } => switch
                .stage(&text(&case.run_id), &base.plus_seconds(*at))
                .map(|_| ())
                .map_err(|error| format!("stage failed: {}", error.message()))?,
        }
    }
    Ok(switch)
}

/// Read the standing stage and streak of the run out of the state
/// file the operations wrote, as a cross check on the corpus facts.
fn standing_facts(state_path: &Path, run_id: &str) -> Result<(String, i64), String> {
    let bytes = std::fs::read(state_path).map_err(|error| format!("state read failed: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "state is not utf8")?;
    let doc = Parser::parse(&text)?;
    for run in doc
        .get("runs")
        .and_then(Json::as_array)
        .ok_or("state has no runs")?
    {
        if run.get("id").and_then(Json::as_str) == Some(run_id) {
            let stage = run
                .get("stage")
                .and_then(Json::as_str)
                .ok_or("run has no stage")?
                .to_string();
            let streak = run
                .get("clear_streak")
                .and_then(Json::as_i64)
                .ok_or("run has no streak")?;
            return Ok((stage, streak));
        }
    }
    Err(format!("run {run_id} not in the state file"))
}

/// Read the facts the vignette states out of the payload the
/// transport wrote, as a second cross check on the corpus facts.
fn payload_facts(capture_path: &Path) -> Result<(String, String, i64, i64), String> {
    let bytes = std::fs::read(capture_path)
        .map_err(|error| format!("payload capture read failed: {error}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "payload is not utf8")?;
    let doc = Parser::parse(&text)?;
    let state = doc
        .get("state")
        .and_then(Json::as_str)
        .ok_or("payload has no state")?;
    let mut stage = None;
    let mut warranted = None;
    let mut missed = None;
    let mut streak = None;
    for line in state.lines() {
        if let Some(rest) = line.strip_prefix("The run stands at stage ") {
            stage = Some(rest.trim_end_matches('.').to_string());
        }
        if let Some(rest) = line.strip_prefix("The triggers at this instant warrant stage ") {
            warranted = Some(rest.trim_end_matches('.').to_string());
        }
        if let Some(rest) = line.strip_prefix("The missed proof count stands at ") {
            let number: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                .collect();
            missed = number.parse::<i64>().ok();
        }
        if let Some(rest) = line.strip_prefix("The standing clear streak stands at ") {
            let number: String = rest
                .trim_end_matches('.')
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                .collect();
            streak = number.parse::<i64>().ok();
        }
    }
    Ok((
        stage.ok_or("vignette has no stage line")?,
        warranted.ok_or("vignette has no warrant line")?,
        missed.ok_or("vignette has no missed line")?,
        streak.ok_or("vignette has no streak line")?,
    ))
}

fn stage_index(stage: &str) -> usize {
    STAGES
        .iter()
        .position(|name| *name == stage)
        .expect("a corpus stage is in the ladder")
}

fn missed_warrant_index(missed: i64, limit: i64) -> usize {
    if missed <= 0 {
        0
    } else if missed >= limit {
        stage_index("kill")
    } else {
        stage_index(["warn", "throttle", "cut tools", "cut budget"][(missed.min(4) - 1) as usize])
    }
}

/// The combination rule of DM-15 walked independently of the
/// library, giving the stage a consultation outcome must leave
/// standing under the corpus facts of its case.
fn mirror_stage_after(case: &Case, record: &Record) -> String {
    let before = stage_index(&case.stage_before);
    let verdict = record.model_verdict.as_deref();
    let confidence = record.model_confidence;
    let after = match (case.seat.as_str(), verdict, confidence) {
        ("held", Some("escalate"), Some(value)) if value >= ESCALATION_BAR => before + 1,
        ("fall", Some("de-escalate"), Some(value))
            if value >= DE_ESCALATION_BAR
                && missed_warrant_index(case.missed, case.limit) < before
                && case.soft.is_none_or(|ceiling| case.spend_total < ceiling)
                && case.hard.is_none_or(|ceiling| case.spend_total < ceiling) =>
        {
            before - 1
        }
        _ => before,
    };
    STAGES[after].to_string()
}

struct Attempt {
    record: Record,
    stage_after: String,
    calls: usize,
}

/// Judge one case once against a fresh document, retrying one
/// transport class failure on a second fresh document. Every
/// consultation is counted in the returned total.
fn judge_case(
    case: &Case,
    work_dir: &Path,
    capture_dir: &Path,
    program: &str,
    args: &[String],
    step_prefix: &str,
) -> Result<Attempt, String> {
    for attempt in 1..=2usize {
        let step = format!("{step_prefix}-a{attempt}");
        let state_path = work_dir.join(format!("{}-{step}.json", case.id));
        let switch = build_document(case, &state_path)?;
        let (stage, streak) = standing_facts(&state_path, &case.run_id)?;
        if stage != case.stage_before || streak != case.streak {
            return Err(format!(
                "{}: standing facts disagree with the corpus, state file says stage {stage} streak {streak}, corpus says stage {} streak {}",
                case.id, case.stage_before, case.streak
            ));
        }
        std::env::set_var("DEADMAN_CASE_ID", &case.id);
        std::env::set_var("DEADMAN_STEP", &step);
        let transport = SubprocessTransport::new(
            PathBuf::from(program),
            args.to_vec(),
            std::time::Duration::from_secs(10),
        );
        let report = switch
            .stage_with_model(
                &text(&case.run_id),
                &base_instant().plus_seconds(case.judge_at),
                Some(&transport),
                Some(&case.context),
            )
            .map_err(|error| format!("{}: judgment failed: {}", case.id, error.message()))?;
        let payload_path = capture_dir.join(format!("{}.{}.library-payload.json", case.id, step));
        let (p_stage, p_warranted, p_missed, p_streak) = payload_facts(&payload_path)?;
        if p_stage != case.stage_before
            || p_warranted != case.warranted
            || p_missed != case.missed
            || p_streak != case.streak
        {
            return Err(format!(
                "{}: vignette facts disagree with the corpus, vignette says stage {p_stage} warranted {p_warranted} missed {p_missed} streak {p_streak}, corpus says stage {} warranted {} missed {} streak {}",
                case.id, case.stage_before, case.warranted, case.missed, case.streak
            ));
        }
        let log = switch
            .log(&text(&case.run_id))
            .map_err(|error| error.message().to_string())?;
        let record = log.last().cloned().ok_or("the log stands empty")?;
        if record.kind != "stage"
            || (record.model_verdict.is_none() && record.fallback_taken.is_none())
        {
            return Err(format!(
                "{}: the judgment appended no consultation record, last record kind {} stands",
                case.id, record.kind
            ));
        }
        let stage_after = mirror_stage_after(case, &record);
        if record.stage != stage_after || report.stage != stage_after {
            return Err(format!(
                "{}: the record disagrees with the combination rule mirror, record stage {} report stage {} mirror {stage_after}",
                case.id, record.stage, report.stage
            ));
        }
        let transport_class = matches!(
            record.fallback_taken.as_deref(),
            Some("call failed") | Some("silence") | Some("malformed reply")
        );
        if transport_class && attempt == 1 {
            println!(
                "{}: attempt 1 landed in the transport class {} and is discarded, retrying on a fresh document",
                case.id,
                record.fallback_taken.as_deref().unwrap_or("")
            );
            continue;
        }
        // Each pass through the loop performs exactly one
        // consultation, so the attempt number is the call count.
        return Ok(Attempt {
            record,
            stage_after,
            calls: attempt,
        });
    }
    unreachable!("the attempt loop returns")
}

fn record_json(record: &Record) -> String {
    format!(
        "{{\"model_name\": {}, \"model_verdict\": {}, \"model_confidence\": {}, \"model_score\": {}, \"fallback_taken\": {}}}",
        json_opt_str(&record.model_name),
        json_opt_str(&record.model_verdict),
        json_opt_f64(&record.model_confidence),
        json_opt_f64(&record.model_score),
        json_opt_str(&record.fallback_taken),
    )
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.len() < 6 {
        eprintln!("usage: escalation_recording MODE CORPUS ... PROGRAM [ARGS...]");
        return ExitCode::from(2);
    }
    let mode = argv[0].as_str();
    let corpus_text = match std::fs::read_to_string(&argv[1]) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("corpus read failed: {error}");
            return ExitCode::from(2);
        }
    };
    let corpus = match Parser::parse(&corpus_text) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("corpus parse failed: {error}");
            return ExitCode::from(2);
        }
    };
    let cases = match load_cases(&corpus) {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("corpus load failed: {error}");
            return ExitCode::from(2);
        }
    };
    let call_shape = corpus
        .get("call_shape")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    match mode {
        "record" | "evaluate" => {
            if argv.len() < 7 {
                eprintln!("usage: escalation_recording {mode} CORPUS OUT WORK_DIR CAPTURE_DIR PROGRAM [ARGS...]");
                return ExitCode::from(2);
            }
            let out_path = PathBuf::from(&argv[2]);
            let work_dir = PathBuf::from(&argv[3]);
            let capture_dir = PathBuf::from(&argv[4]);
            let program = argv[5].clone();
            let args: Vec<String> = argv[6..].to_vec();
            run_batch(
                mode,
                &cases,
                &call_shape,
                &out_path,
                &work_dir,
                &capture_dir,
                &program,
                &args,
            )
        }
        "verify" => {
            if argv.len() < 7 {
                eprintln!("usage: escalation_recording verify CORPUS CASE_ID WORK_DIR CAPTURE_DIR PROGRAM [ARGS...]");
                return ExitCode::from(2);
            }
            let case_id = argv[2].clone();
            let work_dir = PathBuf::from(&argv[3]);
            let capture_dir = PathBuf::from(&argv[4]);
            let program = argv[5].clone();
            let args: Vec<String> = argv[6..].to_vec();
            let case = match cases.iter().find(|case| case.id == case_id) {
                Some(case) => case,
                None => {
                    eprintln!("no case {case_id} in the corpus");
                    return ExitCode::from(2);
                }
            };
            std::env::set_var("DEADMAN_CAPTURE_DIR", &capture_dir);
            match judge_case(case, &work_dir, &capture_dir, &program, &args, "verify") {
                Ok(attempt) => {
                    println!(
                        "{}: verdict {:?} confidence {:?} score {:?} fallback {:?}, stage {} to {}, calls {}",
                        case.id,
                        attempt.record.model_verdict,
                        attempt.record.model_confidence,
                        attempt.record.model_score,
                        attempt.record.fallback_taken,
                        case.stage_before,
                        attempt.stage_after,
                        attempt.calls
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(1)
                }
            }
        }
        other => {
            eprintln!("unknown mode {other}");
            ExitCode::from(2)
        }
    }
}

// The eight parameters are the operator inputs of the batch as the
// command line hands them over, the mode, the corpus and its call
// shape, the three directory paths, and the helper program with its
// arguments, and no second caller would share a bundle of them.
#[allow(clippy::too_many_arguments)]
fn run_batch(
    mode: &str,
    cases: &[Case],
    call_shape: &str,
    out_path: &Path,
    work_dir: &Path,
    capture_dir: &Path,
    program: &str,
    args: &[String],
) -> ExitCode {
    std::env::set_var("DEADMAN_CAPTURE_DIR", capture_dir);
    if mode == "record" {
        if let Err(error) = std::fs::create_dir_all(out_path.join("requests")) {
            eprintln!("fixtures dir failed: {error}");
            return ExitCode::from(2);
        }
        if let Err(error) = std::fs::create_dir_all(out_path.join("replies")) {
            eprintln!("fixtures dir failed: {error}");
            return ExitCode::from(2);
        }
    }
    let mut total_calls = 0usize;
    let mut entries: Vec<String> = Vec::new();
    let mut scored = 0usize;
    let mut correct = 0usize;
    let mut misses: Vec<String> = Vec::new();
    for case in cases {
        let step_prefix = if mode == "record" {
            "record"
        } else {
            "evaluate"
        };
        let attempt = match judge_case(case, work_dir, capture_dir, program, args, step_prefix) {
            Ok(attempt) => attempt,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        };
        total_calls += attempt.calls;
        let attempts = attempt.calls;
        let step = format!("{step_prefix}-a{attempts}");
        if mode == "record" {
            let payload = capture_dir.join(format!("{}.{}.library-payload.json", case.id, step));
            let reply = capture_dir.join(format!("{}.{}.reply.json", case.id, step));
            if let Err(error) = std::fs::copy(
                &payload,
                out_path.join("requests").join(format!("{}.json", case.id)),
            ) {
                eprintln!("{}: request fixture copy failed: {error}", case.id);
                return ExitCode::from(1);
            }
            if let Err(error) = std::fs::copy(
                &reply,
                out_path.join("replies").join(format!("{}.json", case.id)),
            ) {
                eprintln!("{}: reply fixture copy failed: {error}", case.id);
                return ExitCode::from(1);
            }
        } else if let Some(expected) = &case.expected {
            scored += 1;
            let verdict = attempt.record.model_verdict.as_deref();
            if verdict == Some(expected.as_str()) {
                correct += 1;
            } else {
                let got = verdict
                    .map(str::to_string)
                    .or_else(|| attempt.record.fallback_taken.clone())
                    .unwrap_or_else(|| "no outcome".to_string());
                misses.push(format!("{}: expected {}, got {}", case.id, expected, got));
            }
        }
        println!(
            "{}: attempts {} (running call total {}), verdict {:?}, stage {} to {}",
            case.id,
            attempt.calls,
            total_calls,
            attempt.record.model_verdict,
            case.stage_before,
            attempt.stage_after
        );
        entries.push(format!(
            "    {{\"id\": {}, \"expected\": {}, \"attempts\": {}, \"stage_before\": {}, \"stage_after\": {}, \"record_seq\": {}, \"recorded\": {}}}",
            json_str(&case.id),
            case.expected.as_deref().map_or("null".to_string(), json_str),
            attempt.calls,
            json_str(&case.stage_before),
            json_str(&attempt.stage_after),
            attempt.record.seq,
            record_json(&attempt.record),
        ));
    }
    let document = if mode == "record" {
        format!(
            "{{\n  \"format\": \"deadman-escalation-fixtures/1\",\n  \"model\": {},\n  \"call_shape\": {},\n  \"cases\": [\n{}\n  ]\n}}\n",
            json_str(MODEL_NAME),
            json_str(call_shape),
            entries.join(",\n")
        )
    } else {
        format!(
            "{{\n  \"format\": \"deadman-escalation-evaluation/1\",\n  \"model\": {},\n  \"total_calls\": {},\n  \"scored_cases\": {},\n  \"correct\": {},\n  \"misses\": [{}],\n  \"cases\": [\n{}\n  ]\n}}\n",
            json_str(MODEL_NAME),
            total_calls,
            scored,
            correct,
            misses.iter().map(|miss| json_str(miss)).collect::<Vec<_>>().join(", "),
            entries.join(",\n")
        )
    };
    let target = if mode == "record" {
        out_path.join("manifest.json")
    } else {
        out_path.to_path_buf()
    };
    if let Err(error) = std::fs::write(&target, document) {
        eprintln!("output write failed: {error}");
        return ExitCode::from(1);
    }
    println!(
        "{mode} complete: {} cases, {} calls spent",
        cases.len(),
        total_calls
    );
    if mode == "evaluate" {
        println!("evaluation: {correct} of {scored} scored cases correct");
        for miss in &misses {
            println!("miss: {miss}");
        }
    }
    ExitCode::SUCCESS
}
