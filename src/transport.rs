//! Subprocess judge transport and reply grammar of DM-20 and DM-21.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::evaluate::{Consultation, ModelTransport, Verdict};
use crate::join::{field_opt, model_number_unit, object_pairs};
use crate::reader::parse_document;
use crate::value::{Error, Result, Value};
use crate::writer::dump_value;
use crate::MODEL_NAME;

/// The levels of the score question, first to tenth, the ten
/// levels standing for the values from 0 to 1 in ninths.
const SCORE_LEVELS: [&str; 10] = [
    "no sign that the run is running away",
    "a trace of concern and nothing corroborating it",
    "one signal out of place and the rest steady",
    "signals beginning to lean the wrong way",
    "a mixed picture with real weight on the wrong side",
    "several signals corroborating each other",
    "the picture of a run losing its shape",
    "strong evidence the run is running away",
    "a runaway in all but the final proof",
    "a runaway in progress",
];

/// Build the structured DM-21 call payload document holding the
/// pinned model name, the vignette state text, and the choice
/// and score questions in construction order.
pub fn judge_payload_value(model_name: &str, vignette: &str) -> Value {
    let labels: Vec<Value> = [
        (
            "hold",
            "The run should stand at its stage, where the picture is mixed, or the \
             fall has not been earned by two consecutive clean evaluations, or the \
             elevation is structural and proceeding to plan.",
        ),
        (
            "escalate",
            "The run should stand one stage higher, where corroborating signals \
             stand behind the rise, among them rising spend velocity and \
             compounding missed proofs and new classes of action.",
        ),
        (
            "de-escalate",
            "The run should stand one stage lower, where the signals that raised \
             the stage have actually cleared and two consecutive clean evaluations \
             stand behind the fall.",
        ),
    ]
    .iter()
    .map(|(label, criterion)| {
        Value::Object(vec![
            ("label".to_string(), Value::Str(label.to_string())),
            ("criterion".to_string(), Value::Str(criterion.to_string())),
        ])
    })
    .collect();
    let levels: Vec<Value> = SCORE_LEVELS
        .iter()
        .map(|level| Value::Str(level.to_string()))
        .collect();
    Value::Object(vec![
        ("model".to_string(), Value::Str(model_name.to_string())),
        ("state".to_string(), Value::Str(vignette.to_string())),
        (
            "questions".to_string(),
            Value::Array(vec![
                Value::Object(vec![
                    ("kind".to_string(), Value::Str("choice".to_string())),
                    ("labels".to_string(), Value::Array(labels)),
                ]),
                Value::Object(vec![
                    ("kind".to_string(), Value::Str("score".to_string())),
                    ("levels".to_string(), Value::Array(levels)),
                ]),
            ]),
        ),
    ])
}

/// Render the canonical indented DM-21 call payload JSON text.
fn judge_payload(model_name: &str, vignette: &str) -> String {
    let value = judge_payload_value(model_name, vignette);
    let mut out = String::new();
    dump_value(&value, 0, &mut out);
    out
}

/// Read a consultation out of a reply body, under the reply
/// shape this slice pins. The body is one JSON document
/// naming the model and carrying a choice object whose label
/// is one of the three verdict labels or null for an
/// abstention and, on a verdict, the confidence standing
/// against the chosen label, and a score object whose level
/// is an integer from 1 to 10. Anything off that shape is a
/// malformed reply, and a reported model name other than the
/// pinned name is a malformed reply.
///
/// The grammar stands public because it has two callers by
/// design. The subprocess transport reads the body its helper
/// printed through it, and the command line fixture route of
/// the Round 5 amendment reads the body its fixture file
/// holds through it, so the grammar stands in one
/// implementation and its behaviour over a body is the landed
/// behaviour for both. The silence class of DM-16 stands
/// outside this grammar, because silence names a completed
/// call whose body stands empty and the reading of a
/// completed call belongs to the transport, so an empty body
/// handed to this grammar directly parses as no document and
/// lands in the malformed reply class, which is the reading
/// the fixture route states for its scripted bodies.
pub fn consultation_from_reply_body(body: &str, pinned: &str) -> Consultation {
    let value = match parse_document(body.as_bytes()) {
        Ok(value) => value,
        Err(_) => return Consultation::MalformedReply,
    };
    let pairs = match object_pairs(&value) {
        Some(pairs) => pairs,
        None => return Consultation::MalformedReply,
    };
    let model_name = match field_opt(pairs, "model") {
        Some(Value::Str(text)) => (*text).clone(),
        _ => return Consultation::MalformedReply,
    };
    if model_name != pinned {
        return Consultation::MalformedReply;
    }
    let choice = match field_opt(pairs, "choice") {
        Some(value) => match object_pairs(value) {
            Some(pairs) => pairs,
            None => return Consultation::MalformedReply,
        },
        None => return Consultation::MalformedReply,
    };
    let verdict = match field_opt(choice, "label") {
        Some(Value::Null) => {
            // An abstention is a well formed reply that
            // reports no decision under DM-16, so a confidence
            // or a score standing outside its declared range
            // makes the reply malformed even here, where no
            // verdict stands to carry them.
            if let Some(value) = field_opt(choice, "confidence") {
                if model_number_unit(value).is_none() {
                    return Consultation::MalformedReply;
                }
            }
            if let Some(value) = field_opt(pairs, "score") {
                let level_ok = match object_pairs(value) {
                    Some(score_pairs) => matches!(
                        field_opt(score_pairs, "level"),
                        Some(Value::Int(level)) if (1..=10).contains(level)
                    ),
                    None => false,
                };
                if !level_ok {
                    return Consultation::MalformedReply;
                }
            }
            return Consultation::Abstention { model_name };
        }
        Some(Value::Str(text)) => match Verdict::from_label(text) {
            Some(verdict) => verdict,
            None => return Consultation::MalformedReply,
        },
        _ => return Consultation::MalformedReply,
    };
    let confidence = match field_opt(choice, "confidence") {
        Some(value) => match model_number_unit(value) {
            Some(number) => number,
            None => return Consultation::MalformedReply,
        },
        None => return Consultation::MalformedReply,
    };
    let score_pairs = match field_opt(pairs, "score") {
        Some(value) => match object_pairs(value) {
            Some(pairs) => pairs,
            None => return Consultation::MalformedReply,
        },
        None => return Consultation::MalformedReply,
    };
    let level = match field_opt(score_pairs, "level") {
        Some(Value::Int(level)) if (1..=10).contains(level) => *level,
        _ => return Consultation::MalformedReply,
    };
    Consultation::Verdict {
        model_name,
        verdict,
        confidence,
        score: (level - 1) as f64 / 9.0,
    }
}

/// The judge form discipline of the model surface, stated
/// once here because both surfaces enforce it, the command
/// line in its stage command and the binding in its
/// stage_with_model call. Exactly one of the judge and
/// fixture forms may stand named on one call, and a judge
/// argument or a judge timeout named with no judge program
/// to carry it refuses. Each surface computes its own named
/// predicates, the command line's repeatable judge argument
/// counting as named where its list stands nonempty and the
/// binding's where its argument stands present, and crosses
/// the refusal this function returns in its own discipline
/// (DM-A42).
pub fn validate_judge_form(
    judge_named: bool,
    fixture_named: bool,
    judge_argument_named: bool,
    judge_timeout_named: bool,
) -> Result<()> {
    if judge_named && fixture_named {
        return Err(Error::InvalidRequest(
            "exactly one judge form may stand named on one call".to_string(),
        ));
    }
    if !judge_named {
        if judge_argument_named {
            return Err(Error::InvalidRequest(
                "a judge argument stands named with no judge program to carry it".to_string(),
            ));
        }
        if judge_timeout_named {
            return Err(Error::InvalidRequest(
                "a judge timeout stands named with no judge program to carry it".to_string(),
            ));
        }
    }
    Ok(())
}

/// The widest reply body the subprocess transport will read.
/// A body past the bound is a malformed reply under DM-16.
const REPLY_MAX_BYTES: usize = 64 * 1024;

/// The grace the subprocess transport grants its reader
/// thread to deliver the drained reply after the helper
/// status resolves, before the call fails under DM-20.
const READER_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// A live transport that drives an operator supplied helper
/// command, the connector helper of the program being its
/// intended program. On a consultation it writes the call
/// payload to a temporary file it creates exclusively and
/// removes when the call ends, runs the program with the raw
/// payload flag the helper offers and that file as its
/// argument, and waits with the deadline it was constructed
/// with, killing and reaping the child past the deadline. An
/// exit but success with an empty body is silence, and any
/// other nonzero exit is a call failure whatever its code.
/// The payload carries the vignette and no credential, and
/// the transport inherits the environment of its process and
/// sets nothing and stores nothing, so the library holds no
/// key in any form.
pub struct SubprocessTransport {
    program: PathBuf,
    args: Vec<String>,
    timeout: std::time::Duration,
    model_name: String,
}

impl SubprocessTransport {
    pub fn new(
        program: PathBuf,
        args: Vec<String>,
        timeout: std::time::Duration,
    ) -> SubprocessTransport {
        SubprocessTransport {
            program,
            args,
            timeout,
            model_name: MODEL_NAME.to_string(),
        }
    }

    /// Construct a subprocess transport pinned to a caller-selected model
    /// identifier under DM-24, refusing out-of-shape identifiers before
    /// any consultation runs.
    pub fn with_model(
        program: PathBuf,
        args: Vec<String>,
        timeout: std::time::Duration,
        model_name: impl Into<String>,
    ) -> Result<SubprocessTransport> {
        let model_name = model_name.into();
        crate::validate_model_id(&model_name)?;
        Ok(SubprocessTransport {
            program,
            args,
            timeout,
            model_name,
        })
    }

    fn run_helper(&self, payload_path: &PathBuf) -> Consultation {
        // The deadline is computed before any process or
        // thread stands, with a checked addition, so a timeout
        // too wide to add to the current instant fails the call
        // here and leaves nothing running and nothing to reap,
        // and the caller still removes the payload file.
        let Some(deadline) = std::time::Instant::now().checked_add(self.timeout) else {
            return Consultation::CallFailed;
        };
        let mut command = std::process::Command::new(&self.program);
        command
            .args(&self.args)
            .arg("--raw-payload")
            .arg(payload_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(err)
                    if err.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => return Consultation::CallFailed,
            }
        };
        let mut stdout = child.stdout.take().expect("stdout is piped");
        // The reply is drained on its own thread so a helper
        // that writes more than a pipe holds cannot wedge the
        // deadline loop, and only the bounded head is kept.
        // The drained reply reaches this thread over a channel,
        // because the wait for it must stand bounded below.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut stored: Vec<u8> = Vec::new();
            let mut total: usize = 0;
            let mut chunk = [0u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        total += read;
                        if stored.len() <= REPLY_MAX_BYTES {
                            let keep = (REPLY_MAX_BYTES + 1 - stored.len()).min(read);
                            stored.extend_from_slice(&chunk[..keep]);
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = sender.send((stored, total));
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break None,
            }
        };
        // The reader finishes at the end of the pipe, and the
        // pipe reaches its end only when every process holding
        // it closes it. A child of the helper can hold the pipe
        // open past the helper's own exit and past its kill, as
        // the subprocess lifecycle probe in CONTRACT.md
        // records, so an unbounded wait here would let the
        // consultation run past the deadline DM-20 bounds it
        // by, inside the exclusive lock of DM-10. The wait
        // stands bounded by a short grace, and a reader that
        // has not finished inside it fails the call, with its
        // thread left to end when the pipe finally closes.
        let (body, total) = match receiver.recv_timeout(READER_GRACE) {
            Ok(reply) => reply,
            Err(_) => return Consultation::CallFailed,
        };
        let status = match status {
            Some(status) => status,
            None => return Consultation::CallFailed,
        };
        if !status.success() {
            return Consultation::CallFailed;
        }
        if total > REPLY_MAX_BYTES {
            return Consultation::MalformedReply;
        }
        let text = match String::from_utf8(body) {
            Ok(text) => text,
            Err(_) => return Consultation::MalformedReply,
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Consultation::Silence;
        }
        consultation_from_reply_body(trimmed, &self.model_name)
    }
}

impl ModelTransport for SubprocessTransport {
    fn consult(&self, vignette: &str) -> Consultation {
        let payload = judge_payload(&self.model_name, vignette);
        let temp = match write_payload_temp(&payload) {
            Ok(path) => path,
            Err(_) => return Consultation::CallFailed,
        };
        let outcome = self.run_helper(&temp);
        let _ = std::fs::remove_file(&temp);
        outcome
    }

    fn pinned_model_name(&self) -> &str {
        &self.model_name
    }
}

static PAYLOAD_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The lowercase hex text of a draw of random bytes, the
/// suffix the temporary names of this module carry, stated
/// once for the publication's temporary name and the
/// payload writer's alike (DM-A44).
pub(crate) fn hex_text(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Write the call payload to a temporary file created
/// exclusively, in the directory the process environment
/// names for temporary files, under a name carrying the
/// process id and a counter and a random draw, so no other
/// call predicts it and a plant standing at the name fails
/// the write instead of being followed.
fn write_payload_temp(payload: &str) -> std::io::Result<PathBuf> {
    for _ in 0..16 {
        let mut draw = [0u8; 8];
        getrandom::fill(&mut draw).map_err(|err| std::io::Error::other(err.to_string()))?;
        let suffix = hex_text(&draw);
        let count = PAYLOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arvin-judge-{}-{count}-{suffix}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut handle) => {
                if let Err(err) = handle.write_all(payload.as_bytes()) {
                    drop(handle);
                    let _ = std::fs::remove_file(&path);
                    return Err(err);
                }
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::other(
        "no exclusive temporary name could be taken",
    ))
}
