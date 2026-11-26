//! Value level validation.
//!
//! Names and caller integers are validated through the validators
//! here, which the typed operations and the command line
//! converters and the strict reader all share. The Python
//! implementation validated the values its callers handed it at
//! runtime, and the Rust type system makes most wrong shapes
//! unrepresentable at the typed surface, so these validators are
//! where the refusal lives for the shapes that remain.

use std::cmp::Ordering;

use crate::value::{corrupt, py_printable, py_repr_value, Error, JsonInt, Result, Value};
use crate::{MISSED_PROOF_LIMIT_MAX, NAME_MAX, NOTE_MAX, PROOF_INTERVAL_MAX_SECONDS, SPEND_MAX};

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

/// The text of a run id validate_name has already judged,
/// cloned out of its value. Every entry point performs this
/// extraction after its validation, so the extraction and
/// the ground of its unreachable arm stand in one place.
pub(crate) fn run_id_text(run_id: &Value) -> String {
    match run_id {
        Value::Str(text) => text.clone(),
        _ => unreachable!("a validated name is text"),
    }
}

pub(crate) fn validate_stored_name(value: &Value) -> Result<String> {
    match value {
        Value::Str(text) if name_shape_ok(text) => Ok(text.clone()),
        _ => Err(corrupt(format!(
            "a run holds an invalid run id {}",
            py_repr_value(value)
        ))),
    }
}

pub(crate) fn validate_positive_int(value: &Value, field: &str) -> Result<JsonInt> {
    match value.as_json_int() {
        Some(number) if number.cmp_i128(1) != Ordering::Less => Ok(number),
        _ => Err(Error::InvalidRequest(format!(
            "invalid {field} {}, use a positive integer",
            py_printable(value)
        ))),
    }
}

pub(crate) fn json_int_as_i64(number: &JsonInt) -> i64 {
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
