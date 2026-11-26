//! The document writer.
//!
//! Hand rolled over the fixed shape of the format, reproducing
//! the Python byte layout, its indentation of two spaces, its
//! construction key order, and its escaping of text outside the
//! printable ASCII run, because the stored bytes are part of the
//! recorded behaviour.

use crate::join::{Document, Record, Run};
use crate::value::{py_float_repr, JsonInt, Value};
use crate::FORMAT_NAME;

pub(crate) fn json_int_value(number: &JsonInt) -> Value {
    match number {
        JsonInt::Small(value) => Value::Int(*value),
        JsonInt::Big(text) => Value::BigInt(text.clone()),
    }
}

// The document stands dumped from its typed shape, field by
// field in the construction order the writer has always
// used, through the same leaf renderers the value dump
// uses, so a publish builds no second value tree of the
// whole document, cloning every string it carries, only to
// walk that tree back into text. The bytes stand identical
// to the tree form's bytes, laid out by the same rules the
// value dump lays a value out by, and the fixture
// production replay and the writer's golden pins compare
// them byte for byte.

fn dump_document(document: &Document, out: &mut String) {
    out.push_str("{\n");
    out.push_str("  ");
    dump_json_string("format", out);
    out.push_str(": ");
    dump_json_string(FORMAT_NAME, out);
    out.push_str(",\n");
    out.push_str("  ");
    dump_json_string("runs", out);
    out.push_str(": ");
    if document.runs.is_empty() {
        out.push_str("[]");
    } else {
        out.push_str("[\n");
        for (index, run) in document.runs.iter().enumerate() {
            out.push_str(&"  ".repeat(2));
            dump_run(run, 2, out);
            if index + 1 < document.runs.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("  ");
        out.push(']');
    }
    out.push('\n');
    out.push('}');
}

fn dump_run(run: &Run, level: usize, out: &mut String) {
    out.push_str("{\n");
    let inner = level + 1;
    out.push_str(&"  ".repeat(inner));
    dump_json_string("id", out);
    out.push_str(": ");
    dump_json_string(&run.id, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("registered_at", out);
    out.push_str(": ");
    dump_json_string(&run.registered_at, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("proof_interval_seconds", out);
    out.push_str(": ");
    dump_json_int(&run.proof_interval_seconds, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("missed_proof_limit", out);
    out.push_str(": ");
    out.push_str(&(run.missed_proof_limit as i128).to_string());
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("soft_ceiling", out);
    out.push_str(": ");
    dump_opt_int(run.soft_ceiling, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("hard_ceiling", out);
    out.push_str(": ");
    dump_opt_int(run.hard_ceiling, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("spend_total", out);
    out.push_str(": ");
    out.push_str(&(run.spend_total as i128).to_string());
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("last_proof_at", out);
    out.push_str(": ");
    dump_opt_str(&run.last_proof_at, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("stage", out);
    out.push_str(": ");
    dump_json_string(&run.stage, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("clear_streak", out);
    out.push_str(": ");
    dump_json_int(&run.clear_streak, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("killed", out);
    out.push_str(": ");
    out.push_str(if run.killed { "true" } else { "false" });
    // The standing certification of DM-23 renders
    // only while it stands, after killed and before
    // log, so a run carrying no standing certification
    // renders byte for byte as the landed writer
    // renders it.
    if run.streak_ancestry {
        out.push_str(",\n");
        out.push_str(&"  ".repeat(inner));
        dump_json_string("streak_ancestry", out);
        out.push_str(": ");
        out.push_str("true");
    }
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("log", out);
    out.push_str(": ");
    dump_records(&run.log, inner, out);
    out.push('\n');
    out.push_str(&"  ".repeat(level));
    out.push('}');
}

fn dump_records(records: &[Record], level: usize, out: &mut String) {
    if records.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (index, record) in records.iter().enumerate() {
        out.push_str(&"  ".repeat(level + 1));
        dump_record(record, level + 1, out);
        if index + 1 < records.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&"  ".repeat(level));
    out.push(']');
}

fn dump_record(record: &Record, level: usize, out: &mut String) {
    out.push_str("{\n");
    let inner = level + 1;
    out.push_str(&"  ".repeat(inner));
    dump_json_string("seq", out);
    out.push_str(": ");
    out.push_str(&(record.seq as i128).to_string());
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("kind", out);
    out.push_str(": ");
    dump_json_string(&record.kind, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("at", out);
    out.push_str(": ");
    dump_json_string(&record.at, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("stage", out);
    out.push_str(": ");
    dump_json_string(&record.stage, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("reasons", out);
    out.push_str(": ");
    dump_reasons(&record.reasons, inner, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("model_name", out);
    out.push_str(": ");
    dump_opt_str(&record.model_name, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("model_confidence", out);
    out.push_str(": ");
    dump_opt_float(&record.model_confidence, out);
    out.push_str(",\n");
    out.push_str(&"  ".repeat(inner));
    dump_json_string("fallback_taken", out);
    out.push_str(": ");
    dump_opt_str(&record.fallback_taken, out);
    // The extended shape stands written only on a stage
    // record that carries a consultation, judged by any of
    // its five model fields standing populated, because a
    // consultation record always carries its model name, or
    // that carries the ancestry pattern of DM-22. From the
    // Round 7 amendment the writer writes that shape in its
    // eleven key form uniformly, the ancestry field appended
    // last at null on a consultation record and at its
    // literal on an ancestry step down, and the ten key
    // shape for no record.
    if record.kind == "stage" && (record.model_name.is_some() || record.model_ancestry.is_some()) {
        out.push_str(",\n");
        out.push_str(&"  ".repeat(inner));
        dump_json_string("model_verdict", out);
        out.push_str(": ");
        dump_opt_str(&record.model_verdict, out);
        out.push_str(",\n");
        out.push_str(&"  ".repeat(inner));
        dump_json_string("model_score", out);
        out.push_str(": ");
        dump_opt_float(&record.model_score, out);
        out.push_str(",\n");
        out.push_str(&"  ".repeat(inner));
        dump_json_string("model_ancestry", out);
        out.push_str(": ");
        dump_opt_str(&record.model_ancestry, out);
    }
    out.push('\n');
    out.push_str(&"  ".repeat(level));
    out.push('}');
}

fn dump_reasons(reasons: &[String], level: usize, out: &mut String) {
    if reasons.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (index, reason) in reasons.iter().enumerate() {
        out.push_str(&"  ".repeat(level + 1));
        dump_json_string(reason, out);
        if index + 1 < reasons.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&"  ".repeat(level));
    out.push(']');
}

fn dump_json_int(number: &JsonInt, out: &mut String) {
    match number {
        JsonInt::Small(value) => out.push_str(&value.to_string()),
        JsonInt::Big(text) => out.push_str(text),
    }
}

fn dump_opt_str(value: &Option<String>, out: &mut String) {
    match value {
        Some(text) => dump_json_string(text, out),
        None => out.push_str("null"),
    }
}

fn dump_opt_float(value: &Option<f64>, out: &mut String) {
    match value {
        Some(number) => out.push_str(&py_float_repr(*number)),
        None => out.push_str("null"),
    }
}

fn dump_opt_int(value: Option<i64>, out: &mut String) {
    match value {
        Some(number) => out.push_str(&(number as i128).to_string()),
        None => out.push_str("null"),
    }
}

pub(crate) fn write_document(document: &Document) -> String {
    let mut out = String::new();
    dump_document(document, &mut out);
    out.push('\n');
    out
}

// The indent of one level stands pushed unit by unit, so a
// dump builds no throwaway string for every element and
// closing bracket it writes.
fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

pub(crate) fn dump_value(value: &Value, level: usize, out: &mut String) {
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
                push_indent(out, level + 1);
                dump_value(item, level + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, level);
            out.push(']');
        }
        Value::Object(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, item)) in pairs.iter().enumerate() {
                push_indent(out, level + 1);
                dump_json_string(key, out);
                out.push_str(": ");
                dump_value(item, level + 1, out);
                if index + 1 < pairs.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, level);
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
