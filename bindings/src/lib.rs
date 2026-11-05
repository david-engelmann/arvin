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
//! The one surface the command line does not have is the
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
//! report as a plain dictionary carrying the stage name and
//! the reasons the command line prints under it, the spend
//! call returns the accumulated spend, the pull call returns
//! the stage name, and the log call returns the decision
//! records as plain dictionaries carrying every field the
//! records stand in the state file with, the reserved model
//! fields among them. Those fields stand null in every record
//! this slice writes and the library reader refuses a record
//! carrying a value in one of them, so they stand in the
//! dictionaries as None exactly as they stand in the file.

use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyInt};

use arvin_core::{
    Error, Instant, Record, StageReport, Switch, Value, DEFAULT_MISSED_PROOF_LIMIT,
    DEFAULT_PROOF_INTERVAL_SECONDS,
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
/// with. The five model fields are carried exactly as they
/// stand in the file, null in every record the deterministic
/// operations write and populated on the stage record of a
/// consultation under DM-17.
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
        stage: "none".to_string(),
        reasons: Vec::new(),
        model_name: None,
        model_verdict: None,
        model_confidence: None,
        model_score: None,
        fallback_taken: None,
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
/// under it.
fn stage_dict(py: Python<'_>, report: &StageReport) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("run", &report.run)?;
    dict.set_item("stage", &report.stage)?;
    dict.set_item("reasons", &report.reasons)?;
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
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    py.detach(|| switch.pull(&run_value, &now))
        .map_err(to_py_error)
}

/// Evaluate a run at the instant and return its report as a
/// plain dictionary carrying the stage name and its reasons.
///
/// The now argument is the current instant as ISO 8601 text.
#[pyfunction]
fn stage(py: Python<'_>, state: PathBuf, run: &str, now: &str) -> PyResult<Py<PyDict>> {
    let now = instant_argument(now)?;
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    let report = py
        .detach(|| switch.stage(&run_value, &now))
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
    let switch = Switch::new(state);
    let run_value = Value::Str(run.to_string());
    let records = py.detach(|| switch.log(&run_value)).map_err(to_py_error)?;
    records
        .iter()
        .map(|record| record_dict(py, record))
        .collect()
}

#[pymodule]
fn _arvin(m: &Bound<'_, PyModule>) -> PyResult<()> {
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
    m.add_function(wrap_pyfunction!(log, m)?)?;
    Ok(())
}
