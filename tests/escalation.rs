//! Replay pins for the recorded escalation corpus of Round 3.
//!
//! Each recorded case of tests/fixtures/escalation is rebuilt
//! from its construction script in corpus.json and judged again
//! offline, twice. The first replay runs through the subprocess
//! transport against a scripted helper that hands back the
//! recorded reply body, which pins the parsing of the recorded
//! body and the writing of its record, and the payload the
//! transport builds is compared byte for byte against the
//! recorded request. The second replay runs the same reply
//! through the fixture transport in typed form, and the two
//! replays must land the same record and the same state bytes.
//! The record of each replay is also compared against the
//! recorded outcome in manifest.json, and its stage against an
//! independent walk of the combination rule of DM-15 stated in
//! this file. The tests assert nothing about whether the model
//! was right, because a recorded reply is a fact and its quality
//! is the business of the landing evaluation, not of the gate.
//! No test in this file performs a network call.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use deadman::{
    Consultation, FixtureTransport, Instant, Record, SubprocessTransport, Switch, Value, Verdict,
    DE_ESCALATION_BAR, ESCALATION_BAR, STAGES,
};

// ----------------------------------------------------------------
// A small JSON reader for the corpus and manifest shapes, in the
// same form the recording harness example carries. The crate
// carries no JSON library for its tests.
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

    fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(value) => Some(*value as f64),
            Json::Float(value) => Some(*value),
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
// Corpus and manifest loading.
// ----------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("escalation")
}

fn read_json(path: &Path) -> Json {
    let text = std::fs::read_to_string(path).expect("a fixture file reads");
    Parser::parse(&text).expect("a fixture file parses")
}

struct Case {
    id: String,
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
    missed: i64,
    seat: String,
    expected_record: Recorded,
    stage_after: String,
}

enum Op {
    Prove { at: i64, note: String },
    Spend { at: i64, amount: i64 },
    Stage { at: i64 },
}

struct Recorded {
    model_name: Option<String>,
    model_verdict: Option<String>,
    model_confidence: Option<f64>,
    model_score: Option<f64>,
    fallback_taken: Option<String>,
}

fn opt_str(json: &Json, key: &str) -> Option<String> {
    json.get(key).and_then(Json::as_str).map(str::to_string)
}

fn load_case(id: &str) -> Case {
    let corpus = read_json(&fixtures_dir().join("corpus.json"));
    let manifest = read_json(&fixtures_dir().join("manifest.json"));
    let item = corpus
        .get("cases")
        .and_then(Json::as_array)
        .expect("the corpus carries cases")
        .iter()
        .find(|case| case.get("id").and_then(Json::as_str) == Some(id))
        .unwrap_or_else(|| panic!("case {id} stands in the corpus"));
    let entry = manifest
        .get("cases")
        .and_then(Json::as_array)
        .expect("the manifest carries cases")
        .iter()
        .find(|case| case.get("id").and_then(Json::as_str) == Some(id))
        .unwrap_or_else(|| panic!("case {id} stands in the manifest"));
    let register = item.get("register").expect("a case carries its register");
    let facts = item.get("facts").expect("a case carries its facts");
    let judge = item.get("judge").expect("a case carries its judge");
    let recorded = entry.get("recorded").expect("an entry carries its record");
    let mut ops = Vec::new();
    let mut spend_total = 0i64;
    for op in item
        .get("ops")
        .and_then(Json::as_array)
        .expect("a case carries ops")
    {
        let at = op
            .get("at")
            .and_then(Json::as_i64)
            .expect("an op carries its instant");
        match op
            .get("op")
            .and_then(Json::as_str)
            .expect("an op carries its kind")
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
                    .expect("a spend carries its amount");
                spend_total += amount;
                ops.push(Op::Spend { at, amount });
            }
            "stage" => ops.push(Op::Stage { at }),
            other => panic!("unknown op {other}"),
        }
    }
    Case {
        id: id.to_string(),
        run_id: register
            .get("run_id")
            .and_then(Json::as_str)
            .expect("a register carries its run id")
            .to_string(),
        interval: register
            .get("proof_interval_seconds")
            .and_then(Json::as_i64)
            .expect("interval"),
        limit: register
            .get("missed_proof_limit")
            .and_then(Json::as_i64)
            .expect("limit"),
        soft: register.get("soft_ceiling").and_then(Json::as_i64),
        hard: register.get("hard_ceiling").and_then(Json::as_i64),
        spend_total,
        ops,
        judge_at: judge
            .get("at")
            .and_then(Json::as_i64)
            .expect("judge instant"),
        context: judge
            .get("context")
            .and_then(Json::as_str)
            .expect("judge context")
            .to_string(),
        stage_before: facts
            .get("stage_before")
            .and_then(Json::as_str)
            .expect("stage before")
            .to_string(),
        missed: facts.get("missed").and_then(Json::as_i64).expect("missed"),
        seat: facts
            .get("seat")
            .and_then(Json::as_str)
            .expect("seat")
            .to_string(),
        expected_record: Recorded {
            model_name: opt_str(recorded, "model_name"),
            model_verdict: opt_str(recorded, "model_verdict"),
            model_confidence: recorded.get("model_confidence").and_then(Json::as_f64),
            model_score: recorded.get("model_score").and_then(Json::as_f64),
            fallback_taken: opt_str(recorded, "fallback_taken"),
        },
        stage_after: entry
            .get("stage_after")
            .and_then(Json::as_str)
            .expect("stage after")
            .to_string(),
    }
}

// ----------------------------------------------------------------
// Construction, in the same shape the recording harness drives.
// ----------------------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> TestDir {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "deadman-escalation-{}-{name}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("test dir is created");
        TestDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn t0() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).expect("the test instant is in range")
}

fn text(value: &str) -> Value {
    Value::Str(value.to_string())
}

fn int(value: i64) -> Value {
    Value::Int(value as i128)
}

fn build_document(case: &Case, dir: &TestDir, name: &str) -> (Switch, PathBuf) {
    let state_path = dir.path().join(name);
    let switch = Switch::new(state_path.clone());
    let base = t0();
    switch
        .register(
            &text(&case.run_id),
            &base,
            &int(case.interval),
            &int(case.limit),
            &case.soft.map_or(Value::Null, int),
            &case.hard.map_or(Value::Null, int),
        )
        .expect("registration succeeds");
    for op in &case.ops {
        match op {
            Op::Prove { at, note } => switch
                .prove(&text(&case.run_id), &text(note), &base.plus_seconds(*at))
                .expect("the proof posts"),
            Op::Spend { at, amount } => {
                switch
                    .spend(&text(&case.run_id), &int(*amount), &base.plus_seconds(*at))
                    .expect("the spend posts");
            }
            Op::Stage { at } => {
                switch
                    .stage(&text(&case.run_id), &base.plus_seconds(*at))
                    .expect("the evaluation runs");
            }
        }
    }
    (switch, state_path)
}

fn write_helper(dir: &TestDir, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("the helper is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the helper is executable");
    path
}

fn stage_index(stage: &str) -> usize {
    STAGES
        .iter()
        .position(|name| *name == stage)
        .expect("a stage is in the ladder")
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
/// library, giving the stage a recorded outcome must leave
/// standing under the facts of its case.
fn mirror_stage_after(case: &Case) -> String {
    let before = stage_index(&case.stage_before);
    let recorded = &case.expected_record;
    let after = match (
        case.seat.as_str(),
        recorded.model_verdict.as_deref(),
        recorded.model_confidence,
    ) {
        ("held", Some("escalate"), Some(value)) if value >= ESCALATION_BAR => before + 1,
        ("fall", Some("de-escalate"), Some(value))
            if value >= DE_ESCALATION_BAR
                && missed_warrant_index(case.missed, case.limit) < before
                && case.soft.map_or(true, |ceiling| case.spend_total < ceiling)
                && case.hard.map_or(true, |ceiling| case.spend_total < ceiling) =>
        {
            before - 1
        }
        _ => before,
    };
    STAGES[after].to_string()
}

fn assert_record_matches(case: &Case, record: &Record) {
    let expected = &case.expected_record;
    assert_eq!(
        record.kind, "stage",
        "{}: the consultation record is a stage record",
        case.id
    );
    assert_eq!(
        record.model_name, expected.model_name,
        "{}: model name",
        case.id
    );
    assert_eq!(
        record.model_verdict, expected.model_verdict,
        "{}: verdict",
        case.id
    );
    assert_eq!(
        record.model_confidence, expected.model_confidence,
        "{}: confidence",
        case.id
    );
    assert_eq!(
        record.model_score, expected.model_score,
        "{}: score",
        case.id
    );
    assert_eq!(
        record.fallback_taken, expected.fallback_taken,
        "{}: fallback",
        case.id
    );
    assert_eq!(
        record.stage, case.stage_after,
        "{}: recorded stage",
        case.id
    );
    assert_eq!(
        mirror_stage_after(case),
        case.stage_after,
        "{}: the recorded stage after matches the combination rule mirror",
        case.id
    );
}

fn recorded_consultation(case: &Case) -> Consultation {
    let recorded = &case.expected_record;
    if let Some(class) = &recorded.fallback_taken {
        return match class.as_str() {
            "abstention" => Consultation::Abstention {
                model_name: recorded
                    .model_name
                    .clone()
                    .expect("a fallback names its model"),
            },
            "call failed" => Consultation::CallFailed,
            "silence" => Consultation::Silence,
            "malformed reply" => Consultation::MalformedReply,
            other => panic!("unknown fallback class {other}"),
        };
    }
    if let Some(verdict) = &recorded.model_verdict {
        let verdict = match verdict.as_str() {
            "hold" => Verdict::Hold,
            "escalate" => Verdict::Escalate,
            "de-escalate" => Verdict::DeEscalate,
            other => panic!("unknown verdict {other}"),
        };
        return Consultation::Verdict {
            model_name: recorded
                .model_name
                .clone()
                .expect("a verdict names its model"),
            verdict,
            confidence: recorded
                .model_confidence
                .expect("a verdict carries its confidence"),
            score: recorded.model_score.expect("a verdict carries its score"),
        };
    }
    Consultation::Abstention {
        model_name: recorded
            .model_name
            .clone()
            .expect("an abstention names its model"),
    }
}

fn replay(case_id: &str) {
    let case = load_case(case_id);
    let dir = TestDir::new("replay");
    // The wire replay. A scripted helper captures the payload it
    // is handed and prints the recorded reply body.
    let (switch, state_path) = build_document(&case, &dir, "wire.json");
    let capture = dir.path().join("payload.json");
    let reply_path = fixtures_dir()
        .join("replies")
        .join(format!("{case_id}.json"));
    let helper = write_helper(
        &dir,
        "reply.sh",
        &format!(
            "#!/bin/sh\ncp \"$2\" \"{}\"\ncat \"{}\"\n",
            capture.display(),
            reply_path.display()
        ),
    );
    let transport =
        SubprocessTransport::new(helper, Vec::new(), std::time::Duration::from_secs(10));
    let report = switch
        .stage_with_model(
            &text(&case.run_id),
            &t0().plus_seconds(case.judge_at),
            Some(&transport),
            Some(&case.context),
        )
        .expect("the judgment runs");
    assert_eq!(
        report.stage, case.stage_after,
        "{case_id}: the report stage"
    );
    let payload = std::fs::read(&capture).expect("the helper captured the payload");
    let recorded_request = std::fs::read(
        fixtures_dir()
            .join("requests")
            .join(format!("{case_id}.json")),
    )
    .expect("the recorded request reads");
    assert_eq!(
        payload, recorded_request,
        "{case_id}: the payload the library builds matches the recorded request byte for byte"
    );
    let log = switch.log(&text(&case.run_id)).expect("the log reads");
    let record = log.last().expect("the log carries the consultation record");
    assert_record_matches(&case, record);
    let wire_bytes = std::fs::read(&state_path).expect("the state file reads");
    // The typed replay. The same reply through the fixture
    // transport must land the same record and the same bytes.
    let (typed_switch, typed_path) = build_document(&case, &dir, "typed.json");
    let fixture = FixtureTransport::new(
        case.expected_record
            .model_name
            .as_deref()
            .expect("the record names its model"),
        vec![recorded_consultation(&case)],
    );
    let typed_report = typed_switch
        .stage_with_model(
            &text(&case.run_id),
            &t0().plus_seconds(case.judge_at),
            Some(&fixture),
            Some(&case.context),
        )
        .expect("the typed judgment runs");
    assert_eq!(
        typed_report.stage, case.stage_after,
        "{case_id}: the typed report stage"
    );
    let typed_log = typed_switch
        .log(&text(&case.run_id))
        .expect("the typed log reads");
    let typed_record = typed_log.last().expect("the typed log carries its record");
    assert_record_matches(&case, typed_record);
    assert_eq!(
        typed_record, record,
        "{case_id}: the two replays land the same record"
    );
    let typed_bytes = std::fs::read(&typed_path).expect("the typed state file reads");
    assert_eq!(
        wire_bytes, typed_bytes,
        "{case_id}: the two replays land the same state bytes"
    );
}

macro_rules! replay_case {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            replay($id);
        }
    };
}

replay_case!(replay_dm_e01, "DM-E01");
replay_case!(replay_dm_e02, "DM-E02");
replay_case!(replay_dm_e03, "DM-E03");
replay_case!(replay_dm_e04, "DM-E04");
replay_case!(replay_dm_e05, "DM-E05");
replay_case!(replay_dm_e06, "DM-E06");
replay_case!(replay_dm_e07, "DM-E07");
replay_case!(replay_dm_e08, "DM-E08");
replay_case!(replay_dm_e09, "DM-E09");
replay_case!(replay_dm_e10, "DM-E10");
replay_case!(replay_dm_e11, "DM-E11");
replay_case!(replay_dm_e12, "DM-E12");
replay_case!(replay_dm_h01, "DM-H01");
replay_case!(replay_dm_h02, "DM-H02");
replay_case!(replay_dm_h03, "DM-H03");
replay_case!(replay_dm_h04, "DM-H04");
replay_case!(replay_dm_h05, "DM-H05");
replay_case!(replay_dm_h06, "DM-H06");
replay_case!(replay_dm_h07, "DM-H07");
replay_case!(replay_dm_h08, "DM-H08");
replay_case!(replay_dm_h09, "DM-H09");
replay_case!(replay_dm_h10, "DM-H10");
replay_case!(replay_dm_h11, "DM-H11");
replay_case!(replay_dm_h12, "DM-H12");
replay_case!(replay_dm_d01, "DM-D01");
replay_case!(replay_dm_d02, "DM-D02");
replay_case!(replay_dm_d03, "DM-D03");
replay_case!(replay_dm_d04, "DM-D04");
replay_case!(replay_dm_d05, "DM-D05");
replay_case!(replay_dm_d06, "DM-D06");
replay_case!(replay_dm_d07, "DM-D07");
replay_case!(replay_dm_d08, "DM-D08");
replay_case!(replay_dm_d09, "DM-D09");
replay_case!(replay_dm_d10, "DM-D10");
replay_case!(replay_dm_d11, "DM-D11");
replay_case!(replay_dm_d12, "DM-D12");
replay_case!(replay_dm_x01, "DM-X01");
replay_case!(replay_dm_x02, "DM-X02");
replay_case!(replay_dm_x03, "DM-X03");
replay_case!(replay_dm_x04, "DM-X04");
replay_case!(replay_dm_x05, "DM-X05");
replay_case!(replay_dm_x06, "DM-X06");
replay_case!(replay_dm_x07, "DM-X07");
replay_case!(replay_dm_x08, "DM-X08");
replay_case!(replay_dm_x09, "DM-X09");
replay_case!(replay_dm_x10, "DM-X10");
replay_case!(replay_dm_x11, "DM-X11");
replay_case!(replay_dm_x12, "DM-X12");

/// A recorded case replayed twice from its construction lands
/// byte identical state, the determinism claim of DM-20 read
/// against a recorded reply.
#[test]
fn a_recorded_case_replays_to_identical_state_bytes() {
    let case = load_case("DM-E01");
    let dir = TestDir::new("determinism");
    let mut states = Vec::new();
    for name in ["one.json", "two.json"] {
        let (switch, state_path) = build_document(&case, &dir, name);
        let fixture = FixtureTransport::new(
            case.expected_record
                .model_name
                .as_deref()
                .expect("the record names its model"),
            vec![recorded_consultation(&case)],
        );
        switch
            .stage_with_model(
                &text(&case.run_id),
                &t0().plus_seconds(case.judge_at),
                Some(&fixture),
                Some(&case.context),
            )
            .expect("the judgment runs");
        states.push(std::fs::read(&state_path).expect("the state file reads"));
    }
    assert_eq!(
        states[0], states[1],
        "two replays of one recorded case land identical bytes"
    );
}
