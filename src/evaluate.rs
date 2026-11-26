//! The switch.

use std::cmp::Ordering;

use crate::instant::Instant;
use crate::join::{missed_count, missed_warrant_index, model_name_shape_ok, Record, Run};
use crate::value::{Error, JsonInt, Result};
use crate::{PROOF_INTERVAL_MAX_SECONDS, STAGES};

/// The outcome of one evaluation of one run. The appended
/// field carries the record the evaluation appended, or
/// stands empty where it appended none, so a caller
/// reporting the consultation of a model evaluation reads
/// that record from the report itself and never reads the
/// log back to find it.
#[derive(Clone, Debug, PartialEq)]
pub struct StageReport {
    pub run: String,
    pub stage: String,
    pub reasons: Vec<String>,
    pub appended: Option<Record>,
}

pub(crate) fn stage_index(stage: &str) -> usize {
    STAGES
        .iter()
        .position(|candidate| *candidate == stage)
        .expect("a stored stage stands in the ladder")
}

/// Whole proof intervals missed at the instant, deadline exact
/// counts zero.
pub(crate) fn missed_proofs(run: &Run, now: &Instant) -> Result<i128> {
    let base = run.last_proof_instant.unwrap_or(run.registered_instant);
    // The interval as a duration overflows the representable
    // range past the largest whole second count a duration of
    // 999999999 days holds, which is the check the Python
    // timedelta construction raises as its overflow error.
    if run
        .proof_interval_seconds
        .cmp_i128(PROOF_INTERVAL_MAX_SECONDS as i128)
        == Ordering::Greater
    {
        return Err(Error::InvalidRequest(
            "the missed proof count at this instant stands outside the range the \
             switch can represent"
                .to_string(),
        ));
    }
    let interval_seconds = match &run.proof_interval_seconds {
        JsonInt::Small(value) => *value,
        JsonInt::Big(_) => unreachable!("the bound check above refuses a big interval"),
    };
    Ok(missed_count(&base, now, interval_seconds))
}

/// The warranted stage index and the live trigger names at the
/// instant, the triggers in the order DM-3 names them.
pub(crate) fn warrant(run: &Run, now: &Instant) -> Result<(usize, Vec<&'static str>)> {
    let mut index = 0usize;
    let mut reasons: Vec<&'static str> = Vec::new();
    if let Some(hard) = run.hard_ceiling {
        if run.spend_total >= hard {
            index = stage_index("kill");
            reasons.push("hard ceiling");
        }
    }
    if let Some(soft) = run.soft_ceiling {
        if run.spend_total >= soft {
            index = index.max(stage_index("cut budget"));
            reasons.push("soft ceiling");
        }
    }
    let missed = missed_proofs(run, now)?;
    if missed >= 1 {
        reasons.push("missed proofs");
    }
    index = index.max(missed_warrant_index(missed, run.missed_proof_limit));
    Ok((index, reasons))
}

/// Which seat of DM-13 an evaluation stands at, where a seat
/// stands open at all.
pub(crate) enum Seat {
    /// The warranted stage stands equal to the recorded stage.
    Held,
    /// The warranted stage stands below the recorded stage and
    /// the standing clear streak stands at 1.
    Fall,
}

/// The reference instant of DM-19, the latest of the
/// registration instant and the standing last proof instant
/// where one stands and the instant of the latest stage
/// record in sequence order where one stands, each taken from
/// the run as recorded.
pub(crate) fn reference_instant(run: &Run) -> Instant {
    let mut reference = run.registered_instant;
    if let Some(proof) = run.last_proof_instant {
        if proof > reference {
            reference = proof;
        }
    }
    for record in run.log.iter().rev() {
        if record.kind == "stage" {
            if record.instant > reference {
                reference = record.instant;
            }
            break;
        }
    }
    reference
}

/// A verdict of the decision model, one of the three labels
/// of DM-17.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Hold,
    Escalate,
    DeEscalate,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Hold => "hold",
            Verdict::Escalate => "escalate",
            Verdict::DeEscalate => "de-escalate",
        }
    }

    pub(crate) fn from_label(text: &str) -> Option<Verdict> {
        match text {
            "hold" => Some(Verdict::Hold),
            "escalate" => Some(Verdict::Escalate),
            "de-escalate" => Some(Verdict::DeEscalate),
            _ => None,
        }
    }
}

/// The outcome of one consultation, a verdict or one of the
/// fallback classes of DM-16 in typed form. A transport
/// returns this and judges nothing beyond the parsing its own
/// shape states.
#[derive(Clone, Debug, PartialEq)]
pub enum Consultation {
    Verdict {
        model_name: String,
        verdict: Verdict,
        confidence: f64,
        score: f64,
    },
    Abstention {
        model_name: String,
    },
    CallFailed,
    Silence,
    MalformedReply,
}

/// A caller supplied model transport under DM-20. The one
/// operation takes the rendered vignette and returns the
/// consultation outcome. The pinned model name stands beside
/// it because the fallback pattern of DM-17 records the name
/// of the model the transport stands pinned to, and a
/// fallback outcome carries no reply a name could be read
/// from.
pub trait ModelTransport {
    fn consult(&self, vignette: &str) -> Consultation;
    fn pinned_model_name(&self) -> &str;
}

/// A fixture transport, shipped in the crate because the gate
/// and the recording harness and any rehearsing host need the
/// same shape, and a test only copy would fork it. It answers
/// canned consultations in the order supplied and records
/// every vignette it was shown, and it performs no network
/// access and spawns no process. A consultation asked past
/// the supplied replies is a call failure, the class a script
/// that has run out has honestly earned.
pub struct FixtureTransport {
    model_name: String,
    replies: std::cell::RefCell<std::collections::VecDeque<Consultation>>,
    shown: std::cell::RefCell<Vec<String>>,
}

impl FixtureTransport {
    pub fn new(model_name: &str, replies: Vec<Consultation>) -> FixtureTransport {
        FixtureTransport {
            model_name: model_name.to_string(),
            replies: std::cell::RefCell::new(replies.into()),
            shown: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Every vignette this transport was shown, in order.
    pub fn shown(&self) -> Vec<String> {
        self.shown.borrow().clone()
    }

    /// How many canned replies stand unspent.
    pub fn remaining(&self) -> usize {
        self.replies.borrow().len()
    }
}

impl ModelTransport for FixtureTransport {
    fn consult(&self, vignette: &str) -> Consultation {
        self.shown.borrow_mut().push(vignette.to_string());
        self.replies
            .borrow_mut()
            .pop_front()
            .unwrap_or(Consultation::CallFailed)
    }

    fn pinned_model_name(&self) -> &str {
        &self.model_name
    }
}

/// The model fields one consultation record carries, in one
/// of the patterns of DM-17.
pub(crate) struct ConsultationFields {
    pub(crate) model_name: Option<String>,
    pub(crate) model_verdict: Option<String>,
    pub(crate) model_confidence: Option<f64>,
    pub(crate) model_score: Option<f64>,
    pub(crate) fallback_taken: Option<String>,
}

/// Turn a consultation outcome into the fields its record
/// carries. An outcome whose values stand outside the shapes
/// DM-17 records lands in the malformed reply class on the
/// pinned name, because a record the reader would refuse must
/// never be written by the switch's own hand.
pub(crate) fn consultation_fields(outcome: Consultation, pinned: &str) -> ConsultationFields {
    fn fallback(class: &str, name: &str) -> ConsultationFields {
        ConsultationFields {
            model_name: Some(name.to_string()),
            model_verdict: None,
            model_confidence: None,
            model_score: None,
            fallback_taken: Some(class.to_string()),
        }
    }
    match outcome {
        Consultation::Verdict {
            model_name,
            verdict,
            confidence,
            score,
        } if model_name_shape_ok(&model_name)
            && model_name == pinned
            && (0.0..=1.0).contains(&confidence)
            && (0.0..=1.0).contains(&score) =>
        {
            ConsultationFields {
                model_name: Some(model_name),
                model_verdict: Some(verdict.label().to_string()),
                model_confidence: Some(confidence),
                model_score: Some(score),
                fallback_taken: None,
            }
        }
        Consultation::Abstention { model_name }
            if model_name_shape_ok(&model_name) && model_name == pinned =>
        {
            fallback("abstention", &model_name)
        }
        Consultation::CallFailed => fallback("call failed", pinned),
        Consultation::Silence => fallback("silence", pinned),
        _ => fallback("malformed reply", pinned),
    }
}
