//! The typed document and its strict validation.

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::evaluate::stage_index;
use crate::instant::{parse_stored_instant_text, Instant};
use crate::validate::{json_int_as_i64, validate_stored_name};
use crate::value::{corrupt, py_printable, py_repr_str, py_repr_value, JsonInt, Result, Value};
use crate::writer::json_int_value;
use crate::{
    ANCESTRY_BELOW_BAR, DE_ESCALATION_BAR, DOCUMENT_KEYS, ESCALATION_BAR, FALLBACK_CLASSES,
    FORMAT_NAME, MISSED_COUNT_STAGES, MISSED_PROOF_LIMIT_MAX, MODEL_NAME_MAX, RECORD_KEYS,
    RECORD_KEYS_ANCESTRY, RECORD_KEYS_EXTENDED, RECORD_KINDS, RESERVED_FIELDS, RUN_KEYS,
    RUN_KEYS_WITH_CARRY, SPEND_MAX, STAGES, TRIGGERS, VERDICT_LABELS,
};

/// One decision log record. The three reserved model fields
/// stand in every record, and the two sibling fields of DM-17
/// stand only in the extended shape of a stage record that
/// carries a consultation, so the typed form holds all five
/// and the writer chooses the shape by the record it writes.
/// The ancestry field of DM-22 stands beside the five in the
/// eleven key shape, on a stage record that carries a
/// consultation or an ancestry pattern, and stands null by
/// absence in every other shape. The instant stands beside
/// the text it was parsed from, parsed once where the record
/// is validated or appended, so the join and the evaluation
/// judge the instant itself and never parse the text again.
/// The text stays, because the writer renders the stored
/// spelling.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub seq: u64,
    pub kind: String,
    pub at: String,
    pub instant: Instant,
    pub stage: String,
    pub reasons: Vec<String>,
    pub model_name: Option<String>,
    pub model_verdict: Option<String>,
    pub model_confidence: Option<f64>,
    pub model_score: Option<f64>,
    pub fallback_taken: Option<String>,
    pub model_ancestry: Option<String>,
}

/// Which of the three patterns of DM-17 the model fields of a
/// record stand in. The reader refuses any other population
/// before a record of that shape can stand in a typed log, so
/// the classification of a validated record is total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelPattern {
    /// All five model fields stand null or absent.
    Null,
    /// A verdict stands recorded with its confidence and score.
    Verdict,
    /// A fallback class stands recorded and no verdict stands.
    Fallback,
}

fn model_pattern(record: &Record) -> ModelPattern {
    if record.fallback_taken.is_some() {
        ModelPattern::Fallback
    } else if record.model_verdict.is_some() {
        ModelPattern::Verdict
    } else {
        ModelPattern::Null
    }
}

/// Whether a model name stands in the shape DM-17 records, a
/// nonempty string of at most MODEL_NAME_MAX characters.
pub(crate) fn model_name_shape_ok(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= MODEL_NAME_MAX
}

/// A JSON number standing in the unit interval of DM-17, read
/// as the f64 the typed record carries. An integer stands as
/// its value, and a float stands where the range holds it, and
/// NaN stands nowhere because no comparison admits it.
pub(crate) fn model_number_unit(value: &Value) -> Option<f64> {
    match value {
        Value::Int(number) if (0..=1).contains(number) => Some(*number as f64),
        Value::BigInt(text) if text == "0" => Some(0.0),
        Value::BigInt(text) if text == "1" => Some(1.0),
        Value::Float(number) if (0.0..=1.0).contains(number) => Some(*number),
        _ => None,
    }
}

pub(crate) struct Run {
    pub(crate) id: String,
    pub(crate) registered_at: String,
    pub(crate) registered_instant: Instant,
    pub(crate) proof_interval_seconds: JsonInt,
    pub(crate) missed_proof_limit: i64,
    pub(crate) soft_ceiling: Option<i64>,
    pub(crate) hard_ceiling: Option<i64>,
    pub(crate) spend_total: i64,
    pub(crate) last_proof_at: Option<String>,
    pub(crate) last_proof_instant: Option<Instant>,
    pub(crate) stage: String,
    pub(crate) clear_streak: JsonInt,
    pub(crate) killed: bool,
    /// The standing certification of DM-23, set by a fall
    /// seat consultation that answered de-escalate below the
    /// bar and standing only until the step down it certifies
    /// consumes it or an intervening act clears it. It stands
    /// in no record and no reader re-derives it, in the
    /// manner of the clear streak itself.
    pub(crate) streak_ancestry: bool,
    pub(crate) log: Vec<Record>,
}

pub(crate) struct Document {
    pub(crate) runs: Vec<Run>,
}

pub(crate) fn empty_document() -> Document {
    Document { runs: Vec::new() }
}

// The pairs of an object value, read where they stand. The
// validators walk these pairs directly, in place, where they
// once built a hash map of the same pairs on every object
// they validated, one map per record of every state read.
// The reader refuses a repeated key at parse time, so a
// first match lookup over the pairs reads the one value the
// map would have carried, and a shape check over the pairs
// counts the same keys the map counted.
pub(crate) fn object_pairs(value: &Value) -> Option<&[(String, Value)]> {
    match value {
        Value::Object(pairs) => Some(pairs),
        _ => None,
    }
}

fn keys_match(pairs: &[(String, Value)], expected: &[&str]) -> bool {
    pairs.len() == expected.len()
        && expected
            .iter()
            .all(|key| pairs.iter().any(|(name, _)| name.as_str() == *key))
}

// The value of a field the shape check has already passed,
// so the key stands present exactly once and the first match
// is the only match.
pub(crate) fn field<'a>(pairs: &'a [(String, Value)], key: &str) -> &'a Value {
    pairs
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, item)| item)
        .expect("a field the shape check has passed")
}

pub(crate) fn field_opt<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    pairs
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, item)| item)
}

/// A stored model unit field of a stage record, null or a
/// number in the unit interval of DM-17, read for the two
/// fields that carry one, the confidence and the score, so
/// the rule and its refusal stand in one place (DM-A43).
fn stored_model_unit(
    pairs: &[(String, Value)],
    field_name: &str,
    noun: &str,
) -> Result<Option<f64>> {
    match field(pairs, field_name) {
        Value::Null => Ok(None),
        value => match model_number_unit(value) {
            Some(number) => Ok(Some(number)),
            None => Err(corrupt(format!(
                "a stage record carries a model {noun} outside the range from 0 to 1"
            ))),
        },
    }
}
fn validate_record(value: &Value, position: usize) -> Result<Record> {
    let pairs = match object_pairs(value) {
        Some(pairs) => pairs,
        None => {
            return Err(corrupt(
                "a log record does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    // A record stands in the landed eight key shape or, for a
    // stage record carrying a consultation under DM-17, in the
    // extended ten key shape, or, under the Round 7 amendment,
    // in the eleven key shape that adds the ancestry field of
    // DM-22, and in no other shape. The kind is read before
    // the shape is judged because the extended shapes stand
    // open to stage records alone.
    let with_ancestry = keys_match(pairs, &RECORD_KEYS_ANCESTRY);
    let extended = with_ancestry || keys_match(pairs, &RECORD_KEYS_EXTENDED);
    if !extended && !keys_match(pairs, &RECORD_KEYS) {
        return Err(corrupt(
            "a log record does not carry exactly the recorded fields".to_string(),
        ));
    }
    let seq = field(pairs, "seq");
    match seq.as_json_int() {
        Some(number) if number.cmp_i128(position as i128 + 1) == Ordering::Equal => {}
        _ => {
            return Err(corrupt(
                "the log sequence does not grow by 1 from 1 without gaps".to_string(),
            ))
        }
    }
    let kind = match field(pairs, "kind") {
        Value::Str(text) if RECORD_KINDS.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "a log record holds an unknown kind {}",
                py_repr_value(other)
            )))
        }
    };
    let (instant, at) = parse_stored_instant_text(field(pairs, "at"))?;
    let stage = match field(pairs, "stage") {
        Value::Str(text) if STAGES.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "a log record holds an unknown stage {}",
                py_repr_value(other)
            )))
        }
    };
    let reasons: Vec<String> = match field(pairs, "reasons") {
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
    let (model_name, model_verdict, model_confidence, model_score, fallback_taken, model_ancestry) =
        if extended {
            if kind != "stage" {
                return Err(corrupt(format!(
                    "a {kind} record carries the extended model fields, which \
                     stand only on a stage record"
                )));
            }
            let model_name = match field(pairs, "model_name") {
                Value::Null => None,
                Value::Str(text) if model_name_shape_ok(text) => Some(text.clone()),
                _ => {
                    return Err(corrupt(
                        "a stage record carries a model name outside the recorded \
                         shape"
                            .to_string(),
                    ))
                }
            };
            let model_verdict = match field(pairs, "model_verdict") {
                Value::Null => None,
                Value::Str(text) if VERDICT_LABELS.contains(&text.as_str()) => Some(text.clone()),
                _ => {
                    return Err(corrupt(
                        "a stage record carries a model verdict outside the \
                         recorded names"
                            .to_string(),
                    ))
                }
            };
            let model_confidence = stored_model_unit(pairs, "model_confidence", "confidence")?;
            let model_score = stored_model_unit(pairs, "model_score", "score")?;
            let fallback_taken = match field(pairs, "fallback_taken") {
                Value::Null => None,
                Value::Str(text) if FALLBACK_CLASSES.contains(&text.as_str()) => Some(text.clone()),
                _ => {
                    return Err(corrupt(
                        "a stage record carries a fallback outside the recorded \
                         classes"
                            .to_string(),
                    ))
                }
            };
            let model_ancestry = if with_ancestry {
                match field(pairs, "model_ancestry") {
                    Value::Null => None,
                    Value::Str(text) if text == ANCESTRY_BELOW_BAR => Some(text.clone()),
                    _ => {
                        return Err(corrupt(
                            "a stage record carries a model ancestry outside the \
                             recorded values"
                                .to_string(),
                        ))
                    }
                }
            } else {
                None
            };
            let verdict_pattern = model_name.is_some()
                && model_verdict.is_some()
                && model_confidence.is_some()
                && model_score.is_some()
                && fallback_taken.is_none();
            let fallback_pattern = model_name.is_some()
                && fallback_taken.is_some()
                && model_verdict.is_none()
                && model_confidence.is_none()
                && model_score.is_none();
            let null_pattern = model_name.is_none()
                && model_verdict.is_none()
                && model_confidence.is_none()
                && model_score.is_none()
                && fallback_taken.is_none();
            if !verdict_pattern && !fallback_pattern && !null_pattern {
                return Err(corrupt(
                    "a stage record carries its model fields in no recorded pattern".to_string(),
                ));
            }
            // The ancestry field of DM-22 stands at its literal
            // only beside the null pattern, because a step down is
            // either the consultation's own endorsed act or a
            // later deterministic act the consultation stands
            // behind, and never both.
            if model_ancestry.is_some() && !null_pattern {
                return Err(corrupt(
                    "a stage record carries its model fields in no recorded pattern".to_string(),
                ));
            }
            (
                model_name,
                model_verdict,
                model_confidence,
                model_score,
                fallback_taken,
                model_ancestry,
            )
        } else {
            for name in RESERVED_FIELDS {
                if !matches!(field(pairs, name), Value::Null) {
                    return Err(corrupt(format!(
                        "a log record carries a value in the reserved field {name}"
                    )));
                }
            }
            (None, None, None, None, None, None)
        };
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
        instant,
        stage,
        reasons,
        model_name,
        model_verdict,
        model_confidence,
        model_score,
        fallback_taken,
        model_ancestry,
    })
}

/// Whole proof intervals missed between two instants at a
/// whole second interval, counted under DM-3, deadline exact
/// counts zero. The count is exact at every width the reader
/// admits, because an interval wider than every span the
/// recorded years hold misses nothing at any recorded instant.
/// This is the one home of the DM-3 missed proof arithmetic:
/// the history join and the evaluation both count through it,
/// so the two readers of the rule cannot drift apart.
pub(crate) fn missed_count(base: &Instant, at: &Instant, interval_seconds: i128) -> i128 {
    if interval_seconds >= 400_000_000_000 {
        return 0;
    }
    let interval_micros = interval_seconds * 1_000_000;
    let delta_micros = at.epoch_micros() - base.epoch_micros();
    if delta_micros <= interval_micros {
        0
    } else {
        (delta_micros - 1) / interval_micros
    }
}

/// Whole proof intervals missed at an instant, counted under
/// DM-3 from a base instant, for the history join, whose
/// interval stands as the reader read it. An interval too wide
/// to stand as a whole second count misses nothing, the same
/// answer the count itself gives at that width.
pub(crate) fn missed_count_between(base: &Instant, at: &Instant, interval: &JsonInt) -> i128 {
    match interval {
        JsonInt::Small(value) => missed_count(base, at, *value),
        JsonInt::Big(_) => 0,
    }
}

/// The stage index the missed proof count warrants under DM-3.
pub(crate) fn missed_warrant_index(missed: i128, missed_proof_limit: i64) -> usize {
    if missed < 1 {
        0
    } else if missed >= missed_proof_limit as i128 {
        stage_index("kill")
    } else {
        stage_index(MISSED_COUNT_STAGES[(missed.min(4) - 1) as usize])
    }
}

/// The warrant terms of one run, the facts the history join
/// and its stage record judge both judge a stage record
/// against. They travel together through every call the two
/// judges make, so they stand in one structure the run
/// validator builds once, and a future term joins them here
/// instead of widening two call surfaces.
struct WarrantContext {
    spend_total: i64,
    soft_ceiling: Option<i64>,
    hard_ceiling: Option<i64>,
    proof_interval_seconds: JsonInt,
    missed_proof_limit: i64,
}

/// Note a live proof against the latest the history join
/// tracks, moving the instant and the text that carried it
/// together. The standing proof judgment at the join's end
/// compares instants and its refusal messages print the
/// text, so the two halves of the pair must never move
/// apart, and the update stands stated once here for both
/// seats that perform it.
fn note_live_proof(
    latest: &mut Option<Instant>,
    latest_text: &mut Option<String>,
    record: &Record,
) {
    let instant = record.instant;
    if latest.is_none_or(|standing| instant > standing) {
        *latest = Some(instant);
        *latest_text = Some(record.at.clone());
    }
}

/// Join the decision log of one run against the standing fields
/// of the run, as the Round 2 amendment under DM-7 states. The
/// per record and per field validators have already judged each
/// half on its own, and this pass replays the log once in
/// sequence order after one preparatory scan that fixes the
/// first record at kill and the live spend records, so its cost
/// grows with the size of the log and never with its square.
/// A proof record and a spend record stand live while no
/// earlier record of the run stands at kill.
#[allow(clippy::too_many_arguments)]
fn validate_history_against_standing(
    id_repr: &str,
    log: &[Record],
    standing_stage: &str,
    warrant: &WarrantContext,
    last_proof_at: &Option<String>,
    last_proof_instant: &Option<Instant>,
    clear_streak: &JsonInt,
    registered_instant: &Instant,
) -> Result<()> {
    // One preparatory scan fixes the first record at kill and
    // the first and last live spend positions. A record stands
    // live while it stands before the first record at kill, so
    // a spend counts towards the bounds only where no earlier
    // record stands at kill and the record itself does not.
    // Live spend positions stand in log order, so the first is
    // the smallest and the last is the largest, and those two
    // alone decide whether any live spend stands before or
    // after any position the replay later judges.
    let mut first_kill: Option<usize> = None;
    let mut live_spend_bounds: (Option<usize>, Option<usize>) = (None, None);
    for (position, record) in log.iter().enumerate() {
        if first_kill.is_none() && record.stage == "kill" {
            first_kill = Some(position);
        }
        if record.kind == "spend" && first_kill.is_none_or(|kill| position < kill) {
            if live_spend_bounds.0.is_none() {
                live_spend_bounds.0 = Some(position);
            }
            live_spend_bounds.1 = Some(position);
        }
    }
    let is_live = |position: usize| first_kill.is_none_or(|kill| position < kill);
    let mut live_proof_latest: Option<Instant> = None;
    let mut live_proof_latest_text: Option<String> = None;
    // The instant of the latest stage record in sequence
    // order, one term of the reference instant DM-19 defines,
    // tracked for the step down floor the join now imposes.
    let mut last_stage_instant: Option<Instant> = None;
    for (position, record) in log.iter().enumerate() {
        if position > 0 && record.kind == "registered" {
            return Err(corrupt(format!(
                "run {id_repr} carries a second registered record at sequence {}",
                record.seq
            )));
        }
        if let Some(kill) = first_kill {
            if position > kill && matches!(record.kind.as_str(), "stage" | "pull") {
                return Err(corrupt(format!(
                    "a {} record follows the kill record in the log of run \
                     {id_repr}, where a closed run records only proof and spend \
                     records",
                    record.kind
                )));
            }
        }
        if position == 0 {
            if record.kind == "proof" && is_live(position) {
                note_live_proof(&mut live_proof_latest, &mut live_proof_latest_text, record);
            }
            continue;
        }
        let previous = &log[position - 1];
        match record.kind.as_str() {
            "proof" | "spend" => {
                if record.stage != previous.stage {
                    return Err(corrupt(format!(
                        "a {} record in the log of run {id_repr} stands at stage \
                         {}, not the stage {} the log had reached",
                        record.kind, record.stage, previous.stage
                    )));
                }
            }
            "stage" => {
                // The reference instant the records before this
                // one establish, against which DM-19 floors a
                // step down record.
                let mut reference_floor = *registered_instant;
                if let Some(latest) = live_proof_latest {
                    if latest > reference_floor {
                        reference_floor = latest;
                    }
                }
                if let Some(latest) = last_stage_instant {
                    if latest > reference_floor {
                        reference_floor = latest;
                    }
                }
                validate_stage_record_against_warrant(
                    id_repr,
                    record,
                    &previous.stage,
                    &live_proof_latest,
                    registered_instant,
                    live_spend_bounds,
                    position,
                    warrant,
                    &reference_floor,
                )?;
                last_stage_instant = Some(record.instant);
            }
            _ => {}
        }
        if record.kind == "proof" && is_live(position) {
            note_live_proof(&mut live_proof_latest, &mut live_proof_latest_text, record);
        }
    }
    let last = log.last().expect("a validated log holds a record");
    if standing_stage != last.stage {
        return Err(corrupt(format!(
            "run {id_repr} stands at stage {standing_stage} where its log ends \
             at stage {}",
            last.stage
        )));
    }
    let standing_proof = *last_proof_instant;
    if standing_proof != live_proof_latest {
        return Err(match (last_proof_at, &live_proof_latest_text) {
            (Some(standing), None) => corrupt(format!(
                "run {id_repr} stands with a last proof at {} where its log \
                 holds no live proof record",
                py_repr_str(standing)
            )),
            (None, Some(expected)) => corrupt(format!(
                "run {id_repr} stands with no last proof where its log holds \
                 a live proof record at {}",
                py_repr_str(expected)
            )),
            (Some(standing), Some(expected)) => corrupt(format!(
                "run {id_repr} stands with a last proof at {} where its latest \
                 live proof record stands at {}",
                py_repr_str(standing),
                py_repr_str(expected)
            )),
            (None, None) => unreachable!("equal instants reach no refusal"),
        });
    }
    if live_spend_bounds.0.is_none() && warrant.spend_total != 0 {
        return Err(corrupt(format!(
            "run {id_repr} stands with spend total {} but its log \
             holds no live spend record",
            warrant.spend_total
        )));
    }
    if clear_streak.cmp_i128(1) == Ordering::Greater {
        return Err(corrupt(format!(
            "run {id_repr} holds a clear streak of {}, and no standing clear \
             streak stands above 1",
            py_printable(&json_int_value(clear_streak))
        )));
    }
    if !clear_streak.is_zero() && matches!(standing_stage, "none" | "kill") {
        return Err(corrupt(format!(
            "run {id_repr} stands at stage {standing_stage} with a clear streak \
             of {}, where a run at that stage holds a clear streak of 0",
            py_printable(&json_int_value(clear_streak))
        )));
    }
    Ok(())
}

/// Judge one stage record against the warrants at its instant,
/// as the Round 2 amendment under DM-7 states. The missed proof
/// warrant is exact, counted from the live proofs the replay
/// had reached. The spend standing at the record is a range,
/// standing at 0 where no live spend record stands before it
/// and at the standing spend total where no live spend record
/// stands after it, because a spend record carries no amount.
#[allow(clippy::too_many_arguments)]
fn validate_stage_record_against_warrant(
    id_repr: &str,
    record: &Record,
    previous_stage: &str,
    live_proof_latest: &Option<Instant>,
    registered_instant: &Instant,
    live_spend_bounds: (Option<usize>, Option<usize>),
    position: usize,
    warrant: &WarrantContext,
    reference_floor: &Instant,
) -> Result<()> {
    let previous_index = stage_index(previous_stage);
    let new_index = stage_index(&record.stage);
    let rises = new_index > previous_index;
    let holds = new_index == previous_index && new_index != stage_index("none");
    let steps_down = new_index + 1 == previous_index;
    if !rises && !holds && !steps_down {
        return Err(corrupt(format!(
            "a stage record in the log of run {id_repr} follows stage \
             {previous_stage} with stage {}, a succession no evaluation writes",
            record.stage
        )));
    }
    let warrant_order: [&str; 3] = ["hard ceiling", "soft ceiling", "missed proofs"];
    let mut highest_rank: Option<usize> = None;
    for reason in &record.reasons {
        if reason == "operator pull" {
            return Err(corrupt(format!(
                "a stage record in the log of run {id_repr} names operator \
                 pull, a reason no evaluation reports"
            )));
        }
        let rank = warrant_order
            .iter()
            .position(|name| *name == reason.as_str())
            .expect("a stage record reason stands among the warrant names");
        if highest_rank.is_some_and(|highest| rank <= highest) {
            return Err(corrupt(format!(
                "a stage record in the log of run {id_repr} names its reasons \
                 out of the order the warrant reports them"
            )));
        }
        highest_rank = Some(rank);
    }
    let record_instant = record.instant;
    let base = live_proof_latest.unwrap_or(*registered_instant);
    let missed = missed_count_between(&base, &record_instant, &warrant.proof_interval_seconds);
    let missed_named = record
        .reasons
        .iter()
        .any(|reason| reason == "missed proofs");
    if missed_named && missed == 0 {
        return Err(corrupt(format!(
            "a stage record in the log of run {id_repr} names missed proofs \
             where no whole proof interval stands missed at its instant"
        )));
    }
    if !missed_named && missed >= 1 {
        return Err(corrupt(format!(
            "a stage record in the log of run {id_repr} names no missed \
             proofs where the missed proof count at its instant stands \
             at {missed}"
        )));
    }
    let (spend_low, spend_high) = match live_spend_bounds {
        (None, None) => (0, 0),
        (Some(first), Some(last)) => {
            // The bounds stand in log order, so a live spend
            // stands before this record exactly where the first
            // one does and after it exactly where the last one
            // does. A spend at the record position itself counts
            // on neither side, as the strict comparisons state.
            let before = first < position;
            let after = last > position;
            (
                if after { 0 } else { warrant.spend_total },
                if before { warrant.spend_total } else { 0 },
            )
        }
        _ => unreachable!("the preparatory scan sets both bounds or neither"),
    };
    for (which, ceiling) in [
        ("hard", warrant.hard_ceiling),
        ("soft", warrant.soft_ceiling),
    ] {
        let name = format!("{which} ceiling");
        let named = record.reasons.iter().any(|reason| reason == &name);
        match ceiling {
            None => {
                if named {
                    return Err(corrupt(format!(
                        "a stage record in the log of run {id_repr} names the \
                         {which} ceiling where the run holds no {which} ceiling"
                    )));
                }
            }
            Some(ceiling) => {
                if spend_high < ceiling && named {
                    return Err(corrupt(format!(
                        "a stage record in the log of run {id_repr} names the \
                         {which} ceiling where the spend standing at the record \
                         cannot reach it"
                    )));
                }
                if spend_low >= ceiling && !named {
                    return Err(corrupt(format!(
                        "a stage record in the log of run {id_repr} names no \
                         {which} ceiling, and the spend at the record reaches it"
                    )));
                }
            }
        }
    }
    // The ceilings the record names and the ceilings it does
    // not name bound the spend at the record from both sides,
    // because the warrant names every live trigger, so the
    // warranted stage is judged against the range those bounds
    // leave.
    let mut bounded_low = spend_low;
    let mut bounded_high = spend_high;
    for (ceiling, name) in [
        (warrant.hard_ceiling, "hard ceiling"),
        (warrant.soft_ceiling, "soft ceiling"),
    ] {
        if let Some(ceiling) = ceiling {
            if record.reasons.iter().any(|reason| reason == name) {
                bounded_low = bounded_low.max(ceiling);
            } else {
                bounded_high = bounded_high.min(ceiling - 1);
            }
        }
    }
    if bounded_low > bounded_high {
        return Err(corrupt(format!(
            "a stage record in the log of run {id_repr} names ceiling reasons \
             no spend standing at the record can carry together"
        )));
    }
    let missed_index = missed_warrant_index(missed, warrant.missed_proof_limit);
    let zero_threshold = warrant.soft_ceiling.or(warrant.hard_ceiling);
    let mut spend_warrants: Vec<usize> = Vec::new();
    if zero_threshold.is_none_or(|threshold| bounded_low < threshold) {
        spend_warrants.push(0);
    }
    if let Some(soft) = warrant.soft_ceiling {
        let first_at_soft = bounded_low.max(soft);
        if first_at_soft <= bounded_high
            && warrant.hard_ceiling.is_none_or(|hard| first_at_soft < hard)
        {
            spend_warrants.push(stage_index("cut budget"));
        }
    }
    if let Some(hard) = warrant.hard_ceiling {
        if bounded_high >= hard {
            spend_warrants.push(stage_index("kill"));
        }
    }
    let possible: Vec<usize> = spend_warrants
        .iter()
        .map(|warrant| missed_index.max(*warrant))
        .collect();
    // The model judgments of DM-18 stand on the pattern the
    // record carries, and the floor of DM-19 stands on every
    // step down. A record with null model fields is judged
    // exactly as the join judged it before the Round 3
    // amendment in every particular but the floor, which the
    // amendment adds for step down records alone.
    let pattern = model_pattern(record);
    if steps_down {
        if record_instant < *reference_floor {
            return Err(corrupt(format!(
                "a stage record in the log of run {id_repr} steps down from \
                 {previous_stage} to {} at an instant below the reference \
                 instant its preceding records establish",
                record.stage
            )));
        }
        let lowest = possible.iter().min().copied().unwrap_or(missed_index);
        if lowest >= previous_index {
            return Err(corrupt(format!(
                "a stage record in the log of run {id_repr} steps down from \
                 {previous_stage} to {} where the triggers at its instant \
                 warrant no stage below {previous_stage}",
                record.stage
            )));
        }
        match pattern {
            // The null pattern stands alone, and it stands
            // with the ancestry pattern of DM-22 beside it,
            // the five fields null and the ancestry field at
            // its literal. The join judges that population
            // for well-formedness alone under DM-23. It does
            // not re-derive the ancestry from the records
            // before the step down, because the seat and the
            // bar and the freshness the pattern rests on
            // stand in no record, and the certification rests
            // on the writer that made it at the fall seat.
            // The ancestry pattern stands admitted only on a
            // step down from a middle stage, as DM-23 states,
            // because the fall seat it certifies stands only
            // at a middle stage and the certification is
            // consumed by the step down from that stage. A
            // step down from cut budget carrying the pattern
            // therefore stands behind no writer.
            ModelPattern::Null
                if record.model_ancestry.is_some() && !matches!(previous_index, 1..=3) =>
            {
                return Err(corrupt(format!(
                    "a stage record in the log of run {id_repr} steps down from \
                     {previous_stage} to {} carrying the ancestry pattern of \
                     DM-22 from a stage no fall seat of DM-13 stands at",
                    record.stage
                )))
            }
            ModelPattern::Null => {}
            ModelPattern::Verdict
                if record.model_ancestry.is_none()
                    && matches!(previous_index, 1..=3)
                    && record.model_verdict.as_deref() == Some("de-escalate")
                    && record
                        .model_confidence
                        .is_some_and(|confidence| confidence >= DE_ESCALATION_BAR) => {}
            _ => {
                return Err(corrupt(format!(
                    "a stage record in the log of run {id_repr} steps down from \
                     {previous_stage} to {} carrying model fields no endorsed \
                     step down of DM-15 carries",
                    record.stage
                )))
            }
        }
    } else {
        // The ancestry pattern of DM-22 stands open to a step
        // down record alone, so a hold or a rise carrying a
        // standing ancestry value stands behind no writer.
        if record.model_ancestry.is_some() {
            return Err(corrupt(format!(
                "a stage record in the log of run {id_repr} stands at stage {} \
                 carrying a model ancestry no step down carries",
                record.stage
            )));
        }
        match pattern {
            ModelPattern::Null => {
                if !possible.contains(&new_index) {
                    return Err(corrupt(format!(
                        "a stage record in the log of run {id_repr} stands at stage {}, \
                         a stage the triggers at its instant cannot warrant for this run",
                        record.stage
                    )));
                }
            }
            _ if holds => {
                // A hold record carrying a model pattern is
                // judged for field coherence and stage
                // succession only, as DM-18 states, and both
                // stand judged already.
            }
            _ => {
                // A rise carrying a model pattern stands only
                // as the model rise of DM-18. The standing stage
                // stands among the stages the triggers can
                // warrant, as the seat of DM-13 requires, the
                // rise is exactly one step from a middle stage,
                // it lands no higher than cut budget so no model
                // path reaches kill, and it carries an escalate
                // verdict at or above the escalation bar.
                let endorsed = pattern == ModelPattern::Verdict
                    && record.model_verdict.as_deref() == Some("escalate")
                    && record
                        .model_confidence
                        .is_some_and(|confidence| confidence >= ESCALATION_BAR)
                    && new_index == previous_index + 1
                    && possible.contains(&previous_index)
                    && matches!(previous_index, 1..=3)
                    && new_index <= stage_index("cut budget");
                if !endorsed {
                    return Err(corrupt(format!(
                        "a stage record in the log of run {id_repr} stands at \
                         stage {} carrying model fields no consultation of \
                         DM-13 stands behind",
                        record.stage
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_stored_ceiling(pairs: &[(String, Value)], name: &str, id: &str) -> Result<Option<i64>> {
    match field(pairs, name) {
        Value::Null => Ok(None),
        value => match value.as_json_int() {
            Some(number)
                if number.cmp_i128(0) != Ordering::Less
                    && number.cmp_i128(SPEND_MAX as i128) != Ordering::Greater =>
            {
                Ok(Some(json_int_as_i64(&number)))
            }
            _ => Err(corrupt(format!(
                "run {} holds an invalid {name}",
                py_repr_str(id)
            ))),
        },
    }
}

fn validate_run(value: &Value) -> Result<Run> {
    // A run carries exactly the landed twelve keys or, while
    // a certification of DM-23 stands unconsumed, the same
    // keys plus streak_ancestry, and no third key set stands.
    let pairs = match object_pairs(value) {
        Some(pairs) if keys_match(pairs, &RUN_KEYS) || keys_match(pairs, &RUN_KEYS_WITH_CARRY) => {
            pairs
        }
        _ => {
            return Err(corrupt(
                "a run does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    let id = validate_stored_name(field(pairs, "id"))?;
    let id_repr = py_repr_str(&id);
    let (registered_instant, registered_at) =
        parse_stored_instant_text(field(pairs, "registered_at"))?;
    let proof_interval_seconds = match field(pairs, "proof_interval_seconds").as_json_int() {
        Some(number) if number.cmp_i128(1) != Ordering::Less => number,
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds an invalid proof interval"
            )))
        }
    };
    let missed_proof_limit = match field(pairs, "missed_proof_limit").as_json_int() {
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
    let soft_ceiling = validate_stored_ceiling(pairs, "soft_ceiling", &id)?;
    let hard_ceiling = validate_stored_ceiling(pairs, "hard_ceiling", &id)?;
    if let (Some(soft), Some(hard)) = (soft_ceiling, hard_ceiling) {
        if soft > hard {
            return Err(corrupt(format!(
                "run {id_repr} holds a soft ceiling above its hard ceiling"
            )));
        }
    }
    let spend_total = match field(pairs, "spend_total").as_json_int() {
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
    let (last_proof_at, last_proof_instant) = match field(pairs, "last_proof_at") {
        Value::Null => (None, None),
        value => {
            let (instant, text) = parse_stored_instant_text(value)?;
            (Some(text), Some(instant))
        }
    };
    let stage = match field(pairs, "stage") {
        Value::Str(text) if STAGES.contains(&text.as_str()) => text.clone(),
        other => {
            return Err(corrupt(format!(
                "run {id_repr} holds an unknown stage {}",
                py_repr_value(other)
            )))
        }
    };
    let clear_streak = match field(pairs, "clear_streak").as_json_int() {
        Some(number) if number.cmp_i128(0) != Ordering::Less => number,
        _ => {
            return Err(corrupt(format!(
                "run {id_repr} holds an invalid clear streak"
            )))
        }
    };
    let killed = match field(pairs, "killed") {
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
    // The standing certification of DM-23 carries the
    // boolean true and no other value, and it stands only on
    // a run at a middle stage that stands not killed, because
    // the writer sets it only at a middle stage and clears it
    // on every act that ends or leaves the stage history it
    // belongs to.
    let streak_ancestry = match field_opt(pairs, "streak_ancestry") {
        None => false,
        Some(Value::Bool(true)) if matches!(stage.as_str(), "warn" | "throttle" | "cut tools") => {
            true
        }
        Some(Value::Bool(_)) => {
            return Err(corrupt(format!(
                "run {id_repr} holds a streak ancestry certification outside \
                 its recorded standing"
            )))
        }
        Some(_) => {
            return Err(corrupt(format!(
                "run {id_repr} holds a streak ancestry that is not the boolean true"
            )))
        }
    };
    let log: Vec<Record> = match field(pairs, "log") {
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
    let warrant = WarrantContext {
        spend_total,
        soft_ceiling,
        hard_ceiling,
        proof_interval_seconds: proof_interval_seconds.clone(),
        missed_proof_limit,
    };
    validate_history_against_standing(
        &id_repr,
        &log,
        &stage,
        &warrant,
        &last_proof_at,
        &last_proof_instant,
        &clear_streak,
        &registered_instant,
    )?;
    Ok(Run {
        id,
        registered_at,
        registered_instant,
        proof_interval_seconds,
        missed_proof_limit,
        soft_ceiling,
        hard_ceiling,
        spend_total,
        last_proof_at,
        last_proof_instant,
        stage,
        clear_streak,
        killed,
        streak_ancestry,
        log,
    })
}

pub(crate) fn validate_document(value: &Value) -> Result<Document> {
    let pairs = match object_pairs(value) {
        Some(pairs) if keys_match(pairs, &DOCUMENT_KEYS) => pairs,
        _ => {
            return Err(corrupt(
                "the document does not carry exactly the recorded fields".to_string(),
            ))
        }
    };
    match field(pairs, "format") {
        Value::Str(text) if text == FORMAT_NAME => {}
        other => {
            return Err(corrupt(format!(
                "the format name is {}, not {}",
                py_repr_value(other),
                py_repr_str(FORMAT_NAME)
            )))
        }
    }
    let items = match field(pairs, "runs") {
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
