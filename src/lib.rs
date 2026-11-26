//! Core logic for arvin, the staged kill switch for agent runs.
//!
//! One state file holds every run as one JSON document under the
//! format name deadman-state/1, and the document holds each run
//! with its signals and its decision log, so a stage change and
//! its record publish in the same rename and can never disagree.
//! Every mutating operation holds an exclusive flock on the lock
//! file beside the state file across its whole read and evaluate
//! and modify and write cycle, and publishes by renaming a
//! complete temporary file over the state file. Reads take a
//! shared lock and open the state file fresh each time, and a
//! reader that changes nothing never writes.
//!
//! Time is an input. Every operation but the log read takes the
//! current instant as an argument, and this module reads no wall
//! clock. The deterministic operations call no model, and the one
//! model entry point calls only the transport the caller supplies.
//! The random draws in the module
//! name temporary files, the file a publication writes before
//! its rename and the payload file a subprocess consultation
//! writes before its helper runs, and no draw reaches a
//! stage, a reason, a record, or a state byte. The stage an
//! evaluation reports is a deterministic function of the recorded
//! state and the instant.
//!
//! The stage vocabulary is program vocabulary and stands in one
//! exact order in STAGES. The trigger names of DM-3 stand in
//! TRIGGERS.
//!
//! This crate is the Rust port of the Python implementation of
//! Round 1. The port amendment in CONTRACT.md records the
//! equivalence standard, the reshapings, and the two deliberate
//! divergences the port carries, the lock identity check and the
//! exclusive publish temporary.

mod evaluate;
mod instant;
mod join;
mod reader;
mod switch;
mod transport;
mod validate;
mod value;
mod vignette;
mod writer;

pub const FORMAT_NAME: &str = "deadman-state/1";
pub(crate) const LOCK_SUFFIX: &str = ".lock";
pub const NAME_MAX: usize = 64;
pub const NOTE_MAX: usize = 200;

pub const STAGES: [&str; 6] = [
    "none",
    "warn",
    "throttle",
    "cut tools",
    "cut budget",
    "kill",
];
pub const TRIGGERS: [&str; 4] = [
    "operator pull",
    "hard ceiling",
    "soft ceiling",
    "missed proofs",
];
pub(crate) const RECORD_KINDS: [&str; 5] = ["registered", "proof", "spend", "pull", "stage"];
pub(crate) const RESERVED_FIELDS: [&str; 3] = ["model_name", "model_confidence", "fallback_taken"];

pub const DEFAULT_PROOF_INTERVAL_SECONDS: i64 = 60;
pub const DEFAULT_MISSED_PROOF_LIMIT: i64 = 5;

pub const PROOF_INTERVAL_MAX_SECONDS: i64 = 86_399_999_999_999;
pub const SPEND_MAX: i64 = 1_000_000_000_000_000_000;
pub const MISSED_PROOF_LIMIT_MAX: i64 = 1_000_000;

/// The stage a missed proof count below the limit warrants under
/// DM-3, keyed by the count itself, with every higher count below
/// the limit standing at the last entry.
pub(crate) const MISSED_COUNT_STAGES: [&str; 4] = ["warn", "throttle", "cut tools", "cut budget"];

pub(crate) const DOCUMENT_KEYS: [&str; 2] = ["format", "runs"];
pub(crate) const RUN_KEYS: [&str; 12] = [
    "id",
    "registered_at",
    "proof_interval_seconds",
    "missed_proof_limit",
    "soft_ceiling",
    "hard_ceiling",
    "spend_total",
    "last_proof_at",
    "stage",
    "clear_streak",
    "killed",
    "log",
];
pub(crate) const RECORD_KEYS: [&str; 8] = [
    "seq",
    "kind",
    "at",
    "stage",
    "reasons",
    "model_name",
    "model_confidence",
    "fallback_taken",
];
pub(crate) const RECORD_KEYS_EXTENDED: [&str; 10] = [
    "seq",
    "kind",
    "at",
    "stage",
    "reasons",
    "model_name",
    "model_confidence",
    "fallback_taken",
    "model_verdict",
    "model_score",
];
/// The eleven key shape of the Round 7 amendment, the
/// extended shape plus the ancestry field of DM-22.
pub(crate) const RECORD_KEYS_ANCESTRY: [&str; 11] = [
    "seq",
    "kind",
    "at",
    "stage",
    "reasons",
    "model_name",
    "model_confidence",
    "fallback_taken",
    "model_verdict",
    "model_score",
    "model_ancestry",
];
/// The one ancestry value the ancestry pattern of DM-22
/// carries, a fixed literal of the format naming the below
/// bar fall seat de-escalate that stands in a step down's
/// ancestry, and no other value stands in the field.
pub(crate) const ANCESTRY_BELOW_BAR: &str = "de-escalate below bar";
/// The run key set that carries the standing certification
/// of DM-23, the landed twelve keys plus streak_ancestry,
/// admitted only while a certification stands unconsumed.
pub(crate) const RUN_KEYS_WITH_CARRY: [&str; 13] = [
    "id",
    "registered_at",
    "proof_interval_seconds",
    "missed_proof_limit",
    "soft_ceiling",
    "hard_ceiling",
    "spend_total",
    "last_proof_at",
    "stage",
    "clear_streak",
    "killed",
    "streak_ancestry",
    "log",
];

/// The model the escalation slice stands pinned to, named by
/// the roadmap and reached only through a caller supplied
/// transport under DM-20.
pub const MODEL_NAME: &str = "jev-1.13.0";

/// The confidence bars of the combination rule in DM-15. The
/// bars are constants of the library, named in the contract,
/// and stand in no stored field and no caller input, and the
/// de-escalation bar stands above the escalation bar.
pub const ESCALATION_BAR: f64 = 0.60;
pub const DE_ESCALATION_BAR: f64 = 0.85;

/// The widest model name a record can carry under DM-17.
pub const MODEL_NAME_MAX: usize = 128;

/// The longest host context the model entry point accepts
/// under DM-14, counted in characters.
pub const HOST_CONTEXT_MAX: usize = 2000;

/// The default deadline of a judge consultation, in seconds,
/// standing published beside the landed defaults so neither
/// surface restates the number and the surfaces cannot drift
/// apart on it. Its value is the 10 seconds every in-repo
/// driver already constructs, and the Round 5 amendment names
/// it a contract visible fact in the manner of the bars.
pub const DEFAULT_JUDGE_TIMEOUT_SECONDS: u64 = 10;

/// The widest judge timeout value the surfaces hold whole
/// while an out of range value crosses to its refusal, one
/// more than the widest duration a transport can hold in
/// whole seconds. A value at or past this ceiling refuses
/// without being held, so no surface reads a number it
/// cannot hold to name the value it refuses.
pub const TIMEOUT_WHOLE_CEILING: u128 = 18446744073709551616;

/// The verdict labels of DM-17, in the order the doctrine
/// names them.
pub(crate) const VERDICT_LABELS: [&str; 3] = ["hold", "escalate", "de-escalate"];

/// The fallback classes of DM-16, a closed vocabulary the
/// record carries and the reader validates.
pub(crate) const FALLBACK_CLASSES: [&str; 4] =
    ["abstention", "call failed", "silence", "malformed reply"];

/// The digit limit the Python int conversion and the Python json
/// reader apply to an integer literal, probed against the Python
/// runtime before the port was written.
pub(crate) const INT_DIGIT_LIMIT: usize = 4300;

/// The nesting depth the strict reader will read. The Python
/// reader refuses deeper documents at its own recursion limit,
/// and the port amendment records the bound.
pub(crate) const MAX_NESTING: usize = 256;

pub use evaluate::{Consultation, FixtureTransport, ModelTransport, StageReport, Verdict};
pub use instant::{format_instant, parse_instant, Instant};
pub use join::Record;
pub use switch::Switch;
pub use transport::{consultation_from_reply_body, validate_judge_form, SubprocessTransport};
pub use validate::{
    validate_ceiling, validate_missed_proof_limit, validate_name, validate_note,
    validate_proof_interval, validate_spend_amount,
};
pub use value::{
    py_float_repr, py_printable, py_repr_str, py_repr_value, timeout_printable, Error, Result,
    Value,
};
