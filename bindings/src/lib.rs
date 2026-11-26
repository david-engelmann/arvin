//! The thin Python binding over the arvin crate.
//!
//! Every rule lives in the library this crate wraps. The
//! functions here translate Python values into the library
//! arguments, call the one library function each one names, and
//! translate the result or the refusal back, and nothing else.
//! Each call runs detached from the interpreter, so the
//! filesystem and lock work inside a call never stalls the
//! other threads of the calling process while it waits. A
//! library refusal becomes a Python exception carrying the
//! same message text the command line prints, under a subclass
//! of ArvinError picked by the error class of the refusal, so
//! a Python caller can tell the classes apart the way a shell
//! caller tells them apart by exit code.
//!
//! The surface the command line does not have is the
//! instant. The command line reads the wall clock and offers no
//! override, and the library takes the current instant as an
//! argument to every operation but the log read. The binding
//! mirrors the library exactly and takes the instant as ISO
//! 8601 text on every such operation, parsed by the library
//! parser, and it never invents a clock of its own. The log
//! read judges no instant and takes none, here as in the
//! library.
//!
//! The register call returns the stored facts of the run it
//! opens as a plain dictionary, the stage call returns its
//! report as a plain dictionary carrying the stage name,
//! the reasons the command line prints under it, and the
//! record the evaluation appended, and the stage_with_model
//! call returns the same report for an evaluation that
//! consulted a model under DM-20, the spend
//! call returns the accumulated spend, the pull call returns
//! the stage name, and the log call returns the decision
//! records as plain dictionaries carrying every field the
//! records stand in the state file with, the model fields
//! among them. Those fields stand in the dictionaries exactly
//! as they stand in the file, null in every record the
//! deterministic calls write and carrying the consultation
//! pattern of DM-17 in a record the model call appends.

use std::path::PathBuf;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

use arvin_core::{
    consultation_from_reply_body, judge_payload_value, validate_model_id, Consultation,
    DecisionProvider, Error, FixtureTransport, Instant, ModelTransport, Record, StageReport,
    SubprocessTransport, Switch, Value, Verdict, DEFAULT_JUDGE_TIMEOUT_SECONDS,
    DEFAULT_MISSED_PROOF_LIMIT, DEFAULT_PROOF_INTERVAL_SECONDS, JUDGE_MODELS, LEGACY_MODEL_NAME,
    MODEL_NAME,
};

create_exception!(arvin, ArvinError, pyo3::exceptions::PyException);
create_exception!(arvin, InvalidNameError, ArvinError);
create_exception!(arvin, InvalidRequestError, ArvinError);
create_exception!(arvin, NotFoundError, ArvinError);
create_exception!(arvin, RefusalError, ArvinError);
create_exception!(arvin, CorruptError, ArvinError);
create_exception!(arvin, FailureError, ArvinError);

/// Translate a library refusal into the Python exception of its
/// error class, carrying the message the command line prints.
fn to_py_error(error: Error) -> PyErr {
    let message = error.message().to_string();
    match error {
        Error::InvalidName(_) => InvalidNameError::new_err(message),
        Error::InvalidRequest(_) => InvalidRequestError::new_err(message),
        Error::NotFound(_) => NotFoundError::new_err(message),
        Error::Refusal(_) => RefusalError::new_err(message),
        Error::Corrupt(_) => CorruptError::new_err(message),
        Error::Failure(_) => FailureError::new_err(message),
    }
}

/// Refuse a boolean where an integer belongs, with a TypeError
/// naming the argument, because Python spells True and False as
/// the integers 1 and 0 and no registration value or spend
/// amount is ever meant as a truth value.
fn refuse_boolean(value: &Bound<'_, PyAny>, name: &str) -> PyResult<()> {
    if value.is_instance_of::<PyBool>() {
        Err(PyTypeError::new_err(format!(
            "{name} must be an integer, not a boolean"
        )))
    } else {
        Ok(())
    }
}

/// The canonical decimal text of an integer too wide for the
/// 128 bit width, taken from the value arithmetically.
///
/// The digits come from the integer operations bit_length and
/// to_bytes and a base conversion here, never from a text
/// conversion of the value, because the interpreter caps the
/// digits of an integer to text conversion and a refusal past
/// that cap must still print the number whole, and because a
/// text conversion would print whatever an overridden __str__
/// says rather than the digits of the value. The two operations
/// run on the int type itself with the value passed to them,
/// never on the value, so an override of either method on a
/// subclass of int cannot stand in for the arithmetic either.
fn decimal_text(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let int_type = value.py().get_type::<PyInt>();
    let bits: usize = int_type.call_method1("bit_length", (value,))?.extract()?;
    let byte_count = bits / 8 + 2;
    let signed = PyDict::new(value.py());
    signed.set_item("signed", true)?;
    let bytes: Vec<u8> = int_type
        .call_method("to_bytes", (value, byte_count, "big"), Some(&signed))?
        .extract()?;
    let negative = bytes.first().is_some_and(|byte| byte & 0x80 != 0);
    let mut magnitude = bytes;
    if negative {
        for byte in magnitude.iter_mut() {
            *byte = !*byte;
        }
        let mut carry = 1u16;
        for byte in magnitude.iter_mut().rev() {
            let widened = *byte as u16 + carry;
            *byte = widened as u8;
            carry = widened >> 8;
        }
    }
    // A magnitude with no nonzero byte is the value zero, and
    // its digits are 0. Naming that case outright keeps the
    // scan for the first nonzero byte from ever subtracting
    // from the length of an empty magnitude, whatever a
    // conversion hands back.
    let first = match magnitude.iter().position(|byte| *byte != 0) {
        Some(index) => index,
        None => return Ok("0".to_string()),
    };
    let mut current: Vec<u8> = magnitude[first..].to_vec();
    let mut digits: Vec<u8> = Vec::new();
    while !current.is_empty() {
        let mut remainder = 0u32;
        let mut next: Vec<u8> = Vec::with_capacity(current.len());
        for byte in &current {
            let acc = (remainder << 8) | *byte as u32;
            let quotient = acc / 10;
            remainder = acc % 10;
            if !next.is_empty() || quotient != 0 {
                next.push(quotient as u8);
            }
        }
        digits.push(b'0' + remainder as u8);
        current = next;
    }
    if digits.is_empty() {
        digits.push(b'0');
    }
    digits.reverse();
    let mut text: String = digits.iter().map(|byte| *byte as char).collect();
    if negative && text != "0" {
        text.insert(0, '-');
    }
    Ok(text)
}

/// Keep an integer argument whole, at 128 bit width when it
/// fits and as decimal text when it does not, the way the
/// command line converter keeps it, so a later library
/// validation prints the number the caller typed rather than a
/// conversion failure at this layer.
///
/// The integer shape is judged on the value itself before any
/// extraction, because the extraction reaches an object that
/// is no integer partway through its index protocol and
/// refuses it with a message from its own machinery. A value
/// that is no integer fails here with the interpreter's own
/// wording for a value that cannot be read as an integer.
fn integer_value(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    if !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(format!(
            "'{}' object cannot be interpreted as an integer",
            value.get_type().name()?
        )));
    }
    match value.extract::<i128>() {
        Ok(number) => Ok(Value::Int(number)),
        Err(_) => Ok(Value::BigInt(wide_text(value)?)),
    }
}

/// The decimal text of an integer too wide for the 128 bit
/// width.
///
/// At or below the digit cap the library prints, the text is
/// the value's own digits from decimal_text. Past the cap the
/// digits stand in nothing the library decides or prints. A
/// big integer is judged at every bound by its sign alone,
/// its printed form past the cap is the standing phrase a
/// value too large to print, and no validator admits one into
/// the state, so the text stands as a one followed by enough
/// zeros to carry the digit count past the cap, signed as the
/// value is signed, and the digit conversion is skipped
/// before its cost can grow with the square of the width.
/// The width and the sign are read through the int type
/// itself, as decimal_text reads them, so an override on a
/// subclass cannot stand in for either.
fn wide_text(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let int_type = value.py().get_type::<PyInt>();
    let bits: usize = int_type.call_method1("bit_length", (value,))?.extract()?;
    // A value of 14287 or more bits holds at least 4301
    // digits, past the 4300 digit cap py_printable applies,
    // with two bits of margin under the exact bound.
    if bits > 14_286 {
        let negative: bool = int_type.call_method1("__lt__", (value, 0))?.extract()?;
        let mut text = String::with_capacity(4303);
        if negative {
            text.push('-');
        }
        text.push('1');
        text.push_str(&"0".repeat(4301));
        return Ok(text);
    }
    decimal_text(value)
}

/// Resolve an integer argument of a call, refusing a boolean
/// under the name the caller used and keeping the value whole
/// for the library validation to judge.
fn integer_argument(value: &Bound<'_, PyAny>, name: &str) -> PyResult<Value> {
    refuse_boolean(value, name)?;
    integer_value(value)
}

/// Parse the instant argument with the library parser.
///
/// The text is ISO 8601 in the family the library accepts, and
/// the parse is the library's own, so a spelling the library
/// takes is a spelling this surface takes. Text the parser
/// refuses draws the invalid request error the stored instant
/// refusal uses, in a wording that names the current instant,
/// and a parsed instant with no offset is refused downstream
/// by the library itself as a naive current instant.
fn instant_argument(text: &str) -> PyResult<Instant> {
    arvin_core::parse_instant(text).ok_or_else(|| {
        to_py_error(Error::InvalidRequest(format!(
            "the current instant holds unparseable text {}",
            arvin_core::py_repr_str(text)
        )))
    })
}

/// Translate one decision record into a plain dictionary
/// carrying every field the record stands in the state file
/// with. The model fields are carried exactly as they stand
/// in the file, null in every record the deterministic
/// operations write and populated on the stage record of a
/// consultation under DM-17. The ancestry field of DM-22
/// stands at its literal on a certified streak step down
/// and at null on every other record, a record whose shape
/// does not carry it included.
fn record_dict(py: Python<'_>, record: &Record) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("seq", record.seq)?;
    dict.set_item("kind", &record.kind)?;
    dict.set_item("at", &record.at)?;
    dict.set_item("stage", &record.stage)?;
    dict.set_item("reasons", &record.reasons)?;
    dict.set_item("model_name", &record.model_name)?;
    dict.set_item("model_confidence", record.model_confidence)?;
    dict.set_item("fallback_taken", &record.fallback_taken)?;
    dict.set_item("model_verdict", &record.model_verdict)?;
    dict.set_item("model_score", record.model_score)?;
    dict.set_item("model_ancestry", &record.model_ancestry)?;
    Ok(dict.unbind())
}

/// Translate the stored facts of a freshly registered run
/// into a plain dictionary. The facts are the validated
/// registration values the library call stored, judged again
/// here through the same library validators the call itself
/// applied, with the registration standing in the log.
fn register_dict(
    py: Python<'_>,
    run: &str,
    now: &Instant,
    proof_interval: &Value,
    missed_limit: &Value,
    soft_ceiling: &Value,
    hard_ceiling: &Value,
) -> PyResult<Py<PyDict>> {
    let registered_at = arvin_core::format_instant(now).map_err(to_py_error)?;
    let proof_interval_seconds =
        arvin_core::validate_proof_interval(proof_interval).map_err(to_py_error)?;
    let missed_proof_limit =
        arvin_core::validate_missed_proof_limit(missed_limit).map_err(to_py_error)?;
    let soft_ceiling =
        arvin_core::validate_ceiling(soft_ceiling, "soft ceiling").map_err(to_py_error)?;
    let hard_ceiling =
        arvin_core::validate_ceiling(hard_ceiling, "hard ceiling").map_err(to_py_error)?;
    let record = Record {
        seq: 1,
        kind: "registered".to_string(),
        at: registered_at.clone(),
        instant: *now,
        stage: "none".to_string(),
        reasons: Vec::new(),
        model_name: None,
        model_verdict: None,
        model_confidence: None,
        model_score: None,
        fallback_taken: None,
        model_ancestry: None,
    };
    let dict = PyDict::new(py);
    dict.set_item("id", run)?;
    dict.set_item("registered_at", &registered_at)?;
    dict.set_item("proof_interval_seconds", proof_interval_seconds)?;
    dict.set_item("missed_proof_limit", missed_proof_limit)?;
    dict.set_item("soft_ceiling", soft_ceiling)?;
    dict.set_item("hard_ceiling", hard_ceiling)?;
    dict.set_item("spend_total", 0)?;
    dict.set_item("last_proof_at", py.None())?;
    dict.set_item("stage", "none")?;
    dict.set_item("clear_streak", 0)?;
    dict.set_item("killed", false)?;
    dict.set_item("log", vec![record_dict(py, &record)?])?;
    Ok(dict.unbind())
}

/// Translate one stage report into a plain dictionary carrying
/// the stage name and the reasons the command line prints
/// under it, and the record the evaluation appended, rendered
/// as the log call renders a record, or null where the
/// evaluation appended none.
fn stage_dict(py: Python<'_>, report: &StageReport) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("run", &report.run)?;
    dict.set_item("stage", &report.stage)?;
    dict.set_item("reasons", &report.reasons)?;
    match &report.appended {
        Some(record) => dict.set_item("appended", record_dict(py, record)?)?,
        None => dict.set_item("appended", py.None())?,
    }
    Ok(dict.unbind())
}

/// Open a run record and return its stored facts as a plain
/// dictionary.
///
/// The proof_interval argument is the proof interval in
/// seconds and the missed_limit argument is the missed proof
/// limit that warrants kill, and each resolves from the
/// default the library publishes when the argument is not
/// given. The soft_ceiling and hard_ceiling arguments are the
/// spends at or above which cut budget and kill are warranted,
/// and each stands unset when the argument is not given. The
/// now argument is the current instant as ISO 8601 text.
///
/// The argument count is deliberate, because this signature
/// mirrors the library register call one for one, the run and
/// the instant and the four registration values in the order
/// the library takes them, and the binding translates that
/// call rather than restructuring it, so gathering the
/// arguments into a parameter struct of the binding's own
/// invention would add a shape the library does not have, and
/// the too many arguments allow below carries this reason.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (state, run, now, proof_interval = None, missed_limit = None, soft_ceiling = None, hard_ceiling = None))]
fn register(
    py: Python<'_>,
    state: PathBuf,
    run: &str,
    now: &str,
    proof_interval: Option<&Bound<'_, PyAny>>,
    missed_limit: Option<&Bound<'_, PyAny>>,
    soft_ceiling: Option<&Bound<'_, PyAny>>,
    hard_ceiling: Option<&Bound<'_, PyAny>>,
) -> PyResult<Py<PyDict>> {
    let proof_interval = match proof_interval {
        Some(value) => integer_argument(value, "proof_interval")?,
        None => Value::Int(DEFAULT_PROOF_INTERVAL_SECONDS as i128),
    };
    let missed_limit = match missed_limit {
        Some(value) => integer_argument(value, "missed_limit")?,
        None => Value::Int(DEFAULT_MISSED_PROOF_LIMIT as i128),
    };
    let soft_ceiling = match soft_ceiling {
        Some(value) => integer_argument(value, "soft_ceiling")?,
        None => Value::Null,
    };
    let hard_ceiling = match hard_ceiling {
        Some(value) => integer_argument(value, "hard_ceiling")?,
        None => Value::Null,
    };
    let now = instant_argument(now)?;
    check_not_reentrant(&state)?;
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    py.detach(|| {
        switch.register(
            &run_value,
            &now,
            &proof_interval,
            &missed_limit,
            &soft_ceiling,
            &hard_ceiling,
        )
    })
    .map_err(to_py_error)?;
    register_dict(
        py,
        run,
        &now,
        &proof_interval,
        &missed_limit,
        &soft_ceiling,
        &hard_ceiling,
    )
}

/// Record a progress proof for a run. The content of a proof
/// is never judged in this slice.
///
/// The note argument is a short progress note and defaults to
/// the empty note the command line records when no note is
/// given. The now argument is the current instant as ISO 8601
/// text.
#[pyfunction]
#[pyo3(signature = (state, run, now, note = None))]
fn prove(py: Python<'_>, state: PathBuf, run: &str, now: &str, note: Option<&str>) -> PyResult<()> {
    let now = instant_argument(now)?;
    check_not_reentrant(&state)?;
    let note = note.unwrap_or_default();
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    let note_value = Value::Str(note.to_string());
    py.detach(|| switch.prove(&run_value, &note_value, &now))
        .map_err(to_py_error)
}

/// Record a spend report for a run and return the accumulated
/// spend.
///
/// The amount argument is the spend to add, in host units,
/// and the now argument is the current instant as ISO 8601
/// text.
#[pyfunction]
fn spend(
    py: Python<'_>,
    state: PathBuf,
    run: &str,
    amount: &Bound<'_, PyAny>,
    now: &str,
) -> PyResult<i64> {
    let amount = integer_argument(amount, "amount")?;
    let now = instant_argument(now)?;
    check_not_reentrant(&state)?;
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    py.detach(|| switch.spend(&run_value, &amount, &now))
        .map_err(to_py_error)
}

/// Pull the switch for a run and return the stage name. A pull
/// goes straight to kill and latches it.
///
/// The now argument is the current instant as ISO 8601 text.
#[pyfunction]
fn pull(py: Python<'_>, state: PathBuf, run: &str, now: &str) -> PyResult<String> {
    let now = instant_argument(now)?;
    check_not_reentrant(&state)?;
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    py.detach(|| switch.pull(&run_value, &now))
        .map_err(to_py_error)
}

/// Evaluate a run at the instant and return its report as a
/// plain dictionary carrying the stage name and its reasons
/// and the record the evaluation appended.
///
/// The now argument is the current instant as ISO 8601 text.
#[pyfunction]
fn stage(py: Python<'_>, state: PathBuf, run: &str, now: &str) -> PyResult<Py<PyDict>> {
    let now = instant_argument(now)?;
    check_not_reentrant(&state)?;
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    let report = py
        .detach(|| switch.stage(&run_value, &now))
        .map_err(to_py_error)?;
    stage_dict(py, &report)
}

/// Refuse a fixture dictionary shape in the invalid request
/// class, with the message in the library voice, because a
/// malformed fixture dictionary is a caller error of the same
/// kind the library refuses, and the typed exception carries
/// the refusal the way every library refusal crosses.
fn fixture_refusal(message: String) -> PyErr {
    to_py_error(Error::InvalidRequest(message))
}

/// Convert the judge_args argument of stage_with_model into
/// the argument vector the subprocess transport carries. The
/// argument stands documented as a list or tuple of strings,
/// and it is converted element by element here, in the
/// surface's own refusal discipline, so a value that is no
/// list or tuple and an element that is no string each refuse
/// as an invalid request in the library voice. A plain
/// extraction would raise the binding framework's own error
/// in the framework's wording instead, the one caller error
/// of the model surface that escaped the discipline every
/// neighbouring conversion crosses in.
fn judge_arguments(value: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let items: Vec<Bound<'_, PyAny>> = if let Ok(list) = value.cast::<PyList>() {
        list.iter().collect()
    } else if let Ok(tuple) = value.cast::<PyTuple>() {
        tuple.iter().collect()
    } else {
        return Err(fixture_refusal(
            "the judge arguments stand as no list or tuple".to_string(),
        ));
    };
    let mut arguments = Vec::new();
    for item in items {
        let argument: String = item
            .extract()
            .map_err(|_| fixture_refusal("a judge argument stands as no string".to_string()))?;
        arguments.push(argument);
    }
    Ok(arguments)
}

/// Translate a fixture dictionary into the one consultation
/// it scripts. The dictionary stands in one of the two forms
/// of DM-20, the verdict form carrying a model name, a
/// verdict label, a confidence, and a score, and the fallback
/// form carrying a model name and a fallback class, and the
/// transport stands pinned to the name the dictionary carries.
/// A shape outside the two forms is refused here as an
/// invalid request in the library voice. The model name shape
/// itself is not judged here, because the library refuses a
/// transport pinned outside the recordable shape at entry,
/// and the confidence and score ranges are not judged here,
/// because the classification of DM-16 lands an out of shape
/// value in the malformed reply class, and a verdict value
/// judged at this layer could disagree with the record the
/// library would write for the same scripted consultation.
fn fixture_consultation(value: &Bound<'_, PyAny>) -> PyResult<(Consultation, String)> {
    let dict = value
        .cast::<PyDict>()
        .map_err(|_| fixture_refusal("a fixture stands as no dictionary".to_string()))?;
    let known = ["model_name", "verdict", "confidence", "score", "fallback"];
    for key in dict.keys() {
        match key.extract::<String>() {
            Ok(name) => {
                if !known.contains(&name.as_str()) {
                    return Err(fixture_refusal(format!(
                        "a fixture carries the key {} outside the two forms",
                        arvin_core::py_repr_str(&name)
                    )));
                }
            }
            Err(_) => {
                return Err(fixture_refusal(
                    "a fixture carries a key that stands as no name".to_string(),
                ));
            }
        }
    }
    let verdict_item = dict.get_item("verdict")?;
    let fallback_item = dict.get_item("fallback")?;
    match (&verdict_item, &fallback_item) {
        (Some(_), Some(_)) => {
            return Err(fixture_refusal(
                "a fixture carries the verdict and the fallback forms together".to_string(),
            ));
        }
        (None, None) => {
            return Err(fixture_refusal(
                "a fixture carries neither the verdict nor the fallback form".to_string(),
            ));
        }
        _ => {}
    }
    let name_item = dict.get_item("model_name")?.ok_or_else(|| {
        fixture_refusal("a fixture stands without the model name it stands pinned to".to_string())
    })?;
    let model_name: String = name_item
        .extract()
        .map_err(|_| fixture_refusal("a fixture model name stands as no name".to_string()))?;
    if let Some(verdict_value) = verdict_item {
        let label: String = verdict_value
            .extract()
            .map_err(|_| fixture_refusal("a fixture verdict stands as no verdict".to_string()))?;
        let verdict = match label.as_str() {
            "hold" => Verdict::Hold,
            "escalate" => Verdict::Escalate,
            "de-escalate" => Verdict::DeEscalate,
            _ => {
                return Err(fixture_refusal(format!(
                    "a fixture names the verdict {} outside the three verdicts",
                    arvin_core::py_repr_str(&label)
                )));
            }
        };
        let confidence_item = dict.get_item("confidence")?.ok_or_else(|| {
            fixture_refusal("a fixture verdict stands without its confidence".to_string())
        })?;
        let score_item = dict.get_item("score")?.ok_or_else(|| {
            fixture_refusal("a fixture verdict stands without its score".to_string())
        })?;
        if confidence_item.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err(
                "confidence must be a number, not a boolean",
            ));
        }
        if score_item.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err(
                "score must be a number, not a boolean",
            ));
        }
        let confidence: f64 = confidence_item
            .extract()
            .map_err(|_| fixture_refusal("a fixture confidence stands as no number".to_string()))?;
        let score: f64 = score_item
            .extract()
            .map_err(|_| fixture_refusal("a fixture score stands as no number".to_string()))?;
        Ok((
            Consultation::Verdict {
                model_name: model_name.clone(),
                verdict,
                confidence,
                score,
            },
            model_name,
        ))
    } else {
        for field in ["confidence", "score"] {
            if dict.get_item(field)?.is_some() {
                return Err(fixture_refusal(format!(
                    "a fixture fallback carries the {field} of the verdict form"
                )));
            }
        }
        let class_value = fallback_item.expect("one form stands present");
        let class: String = class_value
            .extract()
            .map_err(|_| fixture_refusal("a fixture fallback stands as no fallback".to_string()))?;
        match class.as_str() {
            "abstention" => Ok((
                Consultation::Abstention {
                    model_name: model_name.clone(),
                },
                model_name,
            )),
            "call failed" => Ok((Consultation::CallFailed, model_name)),
            "silence" => Ok((Consultation::Silence, model_name)),
            "malformed reply" => Ok((Consultation::MalformedReply, model_name)),
            _ => Err(fixture_refusal(format!(
                "a fixture names the fallback {} outside the four classes",
                arvin_core::py_repr_str(&class)
            ))),
        }
    }
}

/// Translate a judge timeout argument into the deadline the
/// subprocess transport holds. An integer is a whole number of
/// seconds and a float is a number of seconds with a fraction,
/// and each refuses under the wording the surfaces share where
/// the value does not stand as a positive number of seconds
/// the transport can hold. A boolean is refused as a type
/// error naming the argument, as the binding refuses a boolean
/// wherever a number belongs, and a value of any other kind
/// fails in translation.
fn timeout_argument(value: &Bound<'_, PyAny>) -> PyResult<Duration> {
    if value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(
            "judge_timeout must be a number, not a boolean",
        ));
    }
    let refusal = |rendered: &Value| {
        to_py_error(Error::InvalidRequest(format!(
            "judge timeout {} does not stand as a positive number of seconds the \
             transport can hold",
            arvin_core::timeout_printable(rendered)
        )))
    };
    if value.is_instance_of::<PyInt>() {
        let whole = integer_value(value)?;
        return match &whole {
            Value::Int(number) if *number >= 1 && *number <= u64::MAX as i128 => {
                Ok(Duration::from_secs(*number as u64))
            }
            _ => Err(refusal(&whole)),
        };
    }
    if value.is_instance_of::<PyFloat>() {
        let number: f64 = value.extract()?;
        // The conversion itself judges the range, because a
        // guard written in floats cannot see the boundary. The
        // float nearest above the largest whole second count
        // rounds to exactly 2 to the 64th, and every comparison
        // of that value against a float bound built from the
        // maximum compares equal, while the duration holds at
        // most one second less. The checked conversion refuses
        // exactly the values the duration cannot hold.
        if number > 0.0 {
            if let Ok(duration) = Duration::try_from_secs_f64(number) {
                return Ok(duration);
            }
        }
        return Err(refusal(&Value::Float(number)));
    }
    Err(PyTypeError::new_err("judge_timeout must be a number"))
}

fn value_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    Ok(match value {
        Value::Null => py.None(),
        Value::Bool(flag) => PyBool::new(py, *flag).to_owned().into_any().unbind(),
        Value::Int(number) => number.into_pyobject(py)?.into_any().unbind(),
        Value::BigInt(text) => py.get_type::<PyInt>().call1((text,))?.unbind(),
        Value::Float(number) => PyFloat::new(py, *number).into_any().unbind(),
        Value::Str(text) => PyString::new(py, text).into_any().unbind(),
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(value_to_py(py, item)?);
            }
            PyList::new(py, converted)?.into_any().unbind()
        }
        Value::Object(pairs) => {
            let dict = PyDict::new(py);
            for (key, item) in pairs {
                dict.set_item(key, value_to_py(py, item)?)?;
            }
            dict.into_any().unbind()
        }
    })
}

#[derive(Clone, PartialEq, Eq)]
struct ActiveLockId {
    lock_path: PathBuf,
    #[cfg(unix)]
    dev_ino: Option<(u64, u64)>,
}

std::thread_local! {
    static ACTIVE_CONSULTATION_STATES: std::cell::RefCell<Vec<ActiveLockId>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn lock_path_for(state: &std::path::Path) -> PathBuf {
    let mut os = state.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

fn normalized_lock_path(state: &std::path::Path) -> PathBuf {
    let lock = lock_path_for(state);
    if let Ok(canon) = std::fs::canonicalize(&lock) {
        return canon;
    }
    if let (Some(parent), Some(name)) = (lock.parent(), lock.file_name()) {
        let base = if parent.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            parent
        };
        if let Ok(canon_parent) = std::fs::canonicalize(base) {
            return canon_parent.join(name);
        }
    }
    if lock.is_absolute() {
        lock
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(lock)
    } else {
        lock
    }
}

#[cfg(unix)]
fn lock_dev_ino(state: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(lock_path_for(state)).ok()?;
    Some((meta.dev(), meta.ino()))
}

fn snapshot_lock_id(state: &std::path::Path) -> ActiveLockId {
    ActiveLockId {
        lock_path: normalized_lock_path(state),
        #[cfg(unix)]
        dev_ino: lock_dev_ino(state),
    }
}

fn check_not_reentrant(state: &std::path::Path) -> PyResult<()> {
    let probe = snapshot_lock_id(state);
    let active = ACTIVE_CONSULTATION_STATES.with(|slot| {
        slot.borrow().iter().any(|held| {
            if held.lock_path == probe.lock_path {
                return true;
            }
            #[cfg(unix)]
            if let (Some(a), Some(b)) = (held.dev_ino, probe.dev_ino) {
                if a == b {
                    return true;
                }
            }
            false
        })
    });
    if active {
        return Err(to_py_error(Error::Failure(
            "a transport callback cannot re-enter the state file under consultation".to_string(),
        )));
    }
    Ok(())
}

struct ConsultationStateGuard {
    token: usize,
}

static GUARD_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

impl ConsultationStateGuard {
    fn enter(state: &std::path::Path) -> Self {
        let id = snapshot_lock_id(state);
        ACTIVE_CONSULTATION_STATES.with(|slot| slot.borrow_mut().push(id));
        Self {
            token: GUARD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}

impl Drop for ConsultationStateGuard {
    fn drop(&mut self) {
        let _ = self.token;
        ACTIVE_CONSULTATION_STATES.with(|slot| {
            slot.borrow_mut().pop();
        });
    }
}

struct PyModelTransport {
    callable: Py<PyAny>,
    model_name: String,
    state_path: PathBuf,
}

impl ModelTransport for PyModelTransport {
    fn consult(&self, vignette: &str) -> Consultation {
        let _inner_lock_guard = ConsultationStateGuard::enter(&self.state_path);
        Python::attach(|py| {
            let payload_val = judge_payload_value(&self.model_name, vignette);
            let Ok(py_payload) = value_to_py(py, &payload_val) else {
                return Consultation::CallFailed;
            };
            let Ok(result) = self.callable.bind(py).call1((py_payload,)) else {
                return Consultation::CallFailed;
            };
            if !result.is_instance_of::<PyString>() {
                return Consultation::CallFailed;
            }
            let Ok(body) = result.extract::<String>() else {
                return Consultation::MalformedReply;
            };
            if body.len() > 65_536 {
                return Consultation::MalformedReply;
            }
            let trimmed = body.trim();
            if trimmed.is_empty() {
                return Consultation::Silence;
            }
            consultation_from_reply_body(trimmed, &self.model_name)
        })
    }

    fn pinned_model_name(&self) -> &str {
        &self.model_name
    }
}

/// Evaluate a run at the instant with a model consulted under
/// DM-20 and DM-24, and return its report as a plain dictionary
/// carrying the stage name and its reasons and the record the
/// evaluation appended, the same report the stage call
/// returns.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (state, run, now, judge = None, judge_args = None, judge_timeout = None, fixture = None, host_context = None, judge_model = None, transport = None))]
fn stage_with_model(
    py: Python<'_>,
    state: PathBuf,
    run: &str,
    now: &str,
    judge: Option<String>,
    judge_args: Option<&Bound<'_, PyAny>>,
    judge_timeout: Option<&Bound<'_, PyAny>>,
    fixture: Option<&Bound<'_, PyAny>>,
    host_context: Option<&str>,
    judge_model: Option<&Bound<'_, PyAny>>,
    transport: Option<Py<PyAny>>,
) -> PyResult<Py<PyDict>> {
    let form_count =
        (judge.is_some() as u8) + (fixture.is_some() as u8) + (transport.is_some() as u8);
    if form_count > 1 {
        return Err(to_py_error(Error::InvalidRequest(
            "exactly one judge form may stand named on one call".to_string(),
        )));
    }
    arvin_core::validate_judge_form(
        judge.is_some(),
        fixture.is_some(),
        judge_args.is_some(),
        judge_timeout.is_some(),
    )
    .map_err(to_py_error)?;
    if let Some(ref callable) = transport {
        if !callable.bind(py).is_callable() {
            return Err(PyTypeError::new_err("transport must be callable"));
        }
    }
    if judge_model.is_some() && judge.is_none() && transport.is_none() {
        return Err(to_py_error(Error::InvalidRequest(
            "a judge model stands named with no judge or transport to carry it".to_string(),
        )));
    }
    let parsed_judge_model = match judge_model {
        None => None,
        Some(raw_jm) => {
            let py_str = raw_jm
                .cast::<PyString>()
                .map_err(|_| PyTypeError::new_err("judge_model must be a string"))?;
            let text = py_str.extract::<String>().map_err(|_| {
                to_py_error(Error::InvalidRequest(
                    "the judge model identifier must be 1 to 128 printable ASCII non-whitespace characters"
                        .to_string(),
                ))
            })?;
            validate_model_id(&text).map_err(to_py_error)?;
            Some(text)
        }
    };
    let translated = match fixture {
        Some(value) => Some(fixture_consultation(value)?),
        None => None,
    };
    let (consultation, pinned_name) = match translated {
        Some((consultation, name)) => (Some(consultation), Some(name)),
        None => (None, None),
    };
    let timeout = match judge_timeout {
        Some(value) => Some(timeout_argument(value)?),
        None => None,
    };
    let arguments: Vec<String> = match judge_args {
        Some(value) => judge_arguments(value)?,
        None => Vec::new(),
    };
    let now = instant_argument(now)?;
    check_not_reentrant(&state)?;
    let state_for_guard = state.clone();
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    let chosen_model = parsed_judge_model.unwrap_or_else(|| MODEL_NAME.to_string());
    let report = py
        .detach(|| -> arvin_core::Result<StageReport> {
            let _reentrancy_guard = if transport.is_some() {
                Some(ConsultationStateGuard::enter(&state_for_guard))
            } else {
                None
            };
            let subprocess_transport;
            let fixture_transport;
            let callable_transport;
            let active_transport: Option<&dyn ModelTransport> = if let Some(program) = judge {
                subprocess_transport = SubprocessTransport::with_model(
                    PathBuf::from(program),
                    arguments,
                    timeout.unwrap_or(Duration::from_secs(DEFAULT_JUDGE_TIMEOUT_SECONDS)),
                    chosen_model,
                )?;
                Some(&subprocess_transport)
            } else if let Some(consultation) = consultation {
                fixture_transport = FixtureTransport::new(
                    pinned_name
                        .as_deref()
                        .expect("a consultation stands only beside its name"),
                    vec![consultation],
                );
                Some(&fixture_transport)
            } else if let Some(callable) = transport {
                callable_transport = PyModelTransport {
                    callable,
                    model_name: chosen_model,
                    state_path: state_for_guard.clone(),
                };
                Some(&callable_transport)
            } else {
                None
            };
            switch.stage_with_model(&run_value, &now, active_transport, host_context)
        })
        .map_err(to_py_error)?;
    stage_dict(py, &report)
}

/// Return the recorded decision log of a run, in order, as
/// plain dictionaries.
///
/// The log read judges no instant and takes none, here as in
/// the library.
#[pyfunction]
fn log(py: Python<'_>, state: PathBuf, run: &str) -> PyResult<Vec<Py<PyDict>>> {
    check_not_reentrant(&state)?;
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    let records = py.detach(|| switch.log(&run_value)).map_err(to_py_error)?;
    records
        .iter()
        .map(|record| record_dict(py, record))
        .collect()
}

fn extract_utf8_arg(value: &Bound<'_, PyString>, label: &str) -> PyResult<String> {
    value.extract::<String>().map_err(|_| {
        to_py_error(Error::InvalidRequest(format!(
            "the {label} argument is not valid UTF-8 text"
        )))
    })
}

const MAX_PAYLOAD_DEPTH: usize = 256;

fn validate_dict_strings(root: &Bound<'_, PyDict>) -> PyResult<()> {
    let mut stack: Vec<(Bound<'_, PyAny>, usize)> = vec![(root.clone().into_any(), 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_PAYLOAD_DEPTH {
            return Err(to_py_error(Error::InvalidRequest(
                "the judge payload nests deeper than 256 levels".to_string(),
            )));
        }
        if let Ok(dict) = node.cast::<PyDict>() {
            for (k, v) in dict.iter() {
                let key_str = k.cast::<PyString>().map_err(|_| {
                    to_py_error(Error::InvalidRequest(
                        "a payload dictionary key is not a string".to_string(),
                    ))
                })?;
                extract_utf8_arg(key_str, "payload key")?;
                stack.push((v, depth + 1));
            }
        } else if let Ok(list) = node.cast::<PyList>() {
            for item in list.iter() {
                stack.push((item, depth + 1));
            }
        } else if let Ok(tuple) = node.cast::<pyo3::types::PyTuple>() {
            for item in tuple.iter() {
                stack.push((item, depth + 1));
            }
        } else if let Ok(s) = node.cast::<PyString>() {
            extract_utf8_arg(s, "payload string")?;
        }
    }
    Ok(())
}

fn parse_provider_arg(
    provider: Option<&Bound<'_, PyString>>,
) -> PyResult<Option<DecisionProvider>> {
    match provider {
        None => Ok(None),
        Some(raw_name) => {
            let name = extract_utf8_arg(raw_name, "provider")?;
            DecisionProvider::from_name(&name).map(Some).ok_or_else(|| {
                to_py_error(Error::InvalidRequest(format!(
                    "unknown decision provider {name}, expected typesafe, google, openai, perplexity, or cloudflare"
                )))
            })
        }
    }
}

/// Detect the canonical System One provider dialect name for a model identifier.
#[pyfunction]
fn detect_provider(model: &Bound<'_, PyString>) -> PyResult<&'static str> {
    let text = extract_utf8_arg(model, "model")?;
    Ok(arvin_core::detect_provider(&text).as_str())
}

/// Format a canonical DM-21 payload string or dict into provider wire JSON.
#[pyfunction]
#[pyo3(signature = (payload, provider = None))]
fn format_provider_request(
    py: Python<'_>,
    payload: &Bound<'_, PyAny>,
    provider: Option<&Bound<'_, PyString>>,
) -> PyResult<String> {
    let resolved_provider = parse_provider_arg(provider)?;
    let raw = if let Ok(text) = payload.cast::<PyString>() {
        extract_utf8_arg(text, "payload")?
    } else if let Ok(dict) = payload.cast::<PyDict>() {
        validate_dict_strings(dict)?;
        let json_mod = py.import("json")?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("allow_nan", false)?;
        json_mod
            .call_method("dumps", (dict,), Some(&kwargs))
            .map_err(|_| {
                to_py_error(Error::InvalidRequest(
                    "the judge payload carries a non-JSON or non-finite value".to_string(),
                ))
            })?
            .extract::<String>()?
    } else {
        return Err(PyTypeError::new_err(
            "payload must be a JSON string or dict",
        ));
    };
    arvin_core::format_provider_request(&raw, resolved_provider).map_err(to_py_error)
}

/// Normalize a raw System One provider response string into canonical DM-21 reply JSON.
#[pyfunction]
#[pyo3(signature = (raw_reply, provider = None, model = None))]
fn normalize_provider_reply(
    raw_reply: &Bound<'_, PyString>,
    provider: Option<&Bound<'_, PyString>>,
    model: Option<&Bound<'_, PyString>>,
) -> PyResult<String> {
    let reply_text = extract_utf8_arg(raw_reply, "raw_reply")?;
    let resolved_provider = parse_provider_arg(provider)?;
    let model_text = match model {
        Some(m) => Some(extract_utf8_arg(m, "model")?),
        None => None,
    };
    arvin_core::normalize_provider_reply(&reply_text, resolved_provider, model_text.as_deref())
        .map_err(to_py_error)
}

#[pymodule]
fn _arvin(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("MODEL_NAME", MODEL_NAME)?;
    m.add("LEGACY_MODEL_NAME", LEGACY_MODEL_NAME)?;
    m.add("JUDGE_MODELS", PyTuple::new(m.py(), JUDGE_MODELS)?)?;
    m.add("ArvinError", m.py().get_type::<ArvinError>())?;
    m.add("InvalidNameError", m.py().get_type::<InvalidNameError>())?;
    m.add(
        "InvalidRequestError",
        m.py().get_type::<InvalidRequestError>(),
    )?;
    m.add("NotFoundError", m.py().get_type::<NotFoundError>())?;
    m.add("RefusalError", m.py().get_type::<RefusalError>())?;
    m.add("CorruptError", m.py().get_type::<CorruptError>())?;
    m.add("FailureError", m.py().get_type::<FailureError>())?;
    m.add_function(wrap_pyfunction!(register, m)?)?;
    m.add_function(wrap_pyfunction!(prove, m)?)?;
    m.add_function(wrap_pyfunction!(spend, m)?)?;
    m.add_function(wrap_pyfunction!(pull, m)?)?;
    m.add_function(wrap_pyfunction!(stage, m)?)?;
    m.add_function(wrap_pyfunction!(stage_with_model, m)?)?;
    m.add_function(wrap_pyfunction!(log, m)?)?;
    m.add_function(wrap_pyfunction!(detect_provider, m)?)?;
    m.add_function(wrap_pyfunction!(format_provider_request, m)?)?;
    m.add_function(wrap_pyfunction!(normalize_provider_reply, m)?)?;
    Ok(())
}
