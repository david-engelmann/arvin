//! Pure vignette renderer for the decision model seat of DM-14.

use crate::evaluate::Seat;
use crate::instant::Instant;
use crate::join::Run;
use crate::value::JsonInt;
use crate::STAGES;

/// The doctrine preamble of the vignette, a constant of the
/// library. It states the fall rules exactly, in so many
/// words, because the window evaluation behind this slice
/// found that a judge applies a doctrine the prompt states
/// and supplies none that is left implicit. The confidence
/// bars of DM-15 stand nowhere in it, because a model told
/// its own bar can aim at it.
const DOCTRINE_PREAMBLE: &str = "You are judging one evaluation of a staged kill switch for an agent run. The switch stands at one stage of a ladder, none and warn and throttle and cut tools and cut budget and kill, and it moves a run up the ladder to protect the systems around it and down the ladder as danger passes. Deterministic rules own the spend ceilings and the kill and the first move into warn, and none of that is before you. You judge only a run standing at warn or throttle or cut tools, and you answer with one verdict among hold and escalate and de-escalate, with a confidence, and with a score for how likely the run is to be running away.\n\nThe fall rules of the switch bind your verdict. A stage falls only after two consecutive clean evaluations, never after one, however clean the one looks. A stage entered for structural reasons, a planned spend profile that is proceeding to plan among them, is not relaxed because the current picture is tidy. Recommend de-escalate only where the signals that raised the stage have actually cleared and the fall has two consecutive clean evaluations behind it, because where fewer stand, hold is the truthful verdict even when nothing currently looks wrong. Recommend escalate only where the run should stand one stage higher than it stands, never more, because an escalation takes effect one step at a time. Where the evidence cuts both ways, hold. Where you cannot judge the picture before you, abstain rather than guess.";

/// Whole seconds from one instant to another, floored, the
/// age the vignette states for a recorded fact.
fn age_seconds(from: &Instant, to: &Instant) -> i128 {
    (to.epoch_micros() - from.epoch_micros()).div_euclid(1_000_000)
}

fn int_text(number: &JsonInt) -> String {
    match number {
        JsonInt::Small(value) => value.to_string(),
        JsonInt::Big(text) => text.clone(),
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// Render the vignette of DM-14, the picture the model
/// receives. The rendering is a pure function of the run and
/// the evaluation instant and the seat the evaluation stands
/// at and the optional host context, and the suite pins it
/// byte for byte. Where the seat is the fall decision seat,
/// the facts gain the completion statement of DM-14 as
/// amended in Round 4, that this evaluation is the second of
/// the two consecutive clean evaluations the fall requires,
/// rendered immediately after the streak line and nowhere
/// else, because the seat is decided before the rendering
/// and the statement is true only where that seat stands.
/// What never crosses stands in DM-14, the run id and the
/// state path and any part of any other run and the
/// confidence bars and any credential or transport detail.
pub(crate) fn render_vignette(
    run: &Run,
    now: &Instant,
    warranted: usize,
    reasons: &[&str],
    missed: i128,
    seat: &Seat,
    host_context: Option<&str>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("The run stands at stage {}.", run.stage));
    lines.push(format!(
        "The triggers at this instant warrant stage {}.",
        STAGES[warranted]
    ));
    if reasons.is_empty() {
        lines.push("No trigger stands live at this instant.".to_string());
    } else {
        lines.push(format!("The live reasons are {}.", reasons.join(", ")));
    }
    lines.push(format!(
        "The missed proof count stands at {missed} and the missed proof limit stands at {} \
         and the proof interval stands at {} seconds.",
        run.missed_proof_limit,
        int_text(&run.proof_interval_seconds)
    ));
    let registered = run.registered_instant;
    match run.last_proof_instant {
        Some(proof) => {
            lines.push(format!(
                "The last proof stands at an age of {} whole seconds.",
                age_seconds(&proof, now)
            ));
        }
        None => {
            lines.push(format!(
                "No proof stands, and the registration stands at an age of {} whole \
                 seconds.",
                age_seconds(&registered, now)
            ));
        }
    }
    lines.push(format!(
        "The accumulated spend stands at {}.",
        run.spend_total
    ));
    for (name, ceiling) in [("soft", run.soft_ceiling), ("hard", run.hard_ceiling)] {
        match ceiling {
            None => lines.push(format!("No {name} ceiling stands.")),
            Some(ceiling) if run.spend_total < ceiling => lines.push(format!(
                "The {name} ceiling stands at {ceiling} and the spend stands below it \
                 by {}.",
                ceiling - run.spend_total
            )),
            Some(ceiling) => lines.push(format!(
                "The {name} ceiling stands at {ceiling} and the spend stands at or \
                 above it."
            )),
        }
    }
    lines.push(format!(
        "The standing clear streak stands at {}.",
        int_text(&run.clear_streak)
    ));
    if matches!(seat, Seat::Fall) {
        lines.push("This evaluation is the second of the two consecutive clean evaluations the fall requires.".to_string());
    }
    // The picture carries the last five stage records and no
    // more, so the rendering counts them and walks from the
    // skip point instead of collecting every stage record the
    // log has accumulated into a vector it reads five of.
    let stage_count = run
        .log
        .iter()
        .filter(|record| record.kind == "stage")
        .count();
    if stage_count == 0 {
        lines.push("No stage record stands in the log.".to_string());
    } else {
        lines.push("The last stage records in sequence order stand as follows.".to_string());
        let skip = stage_count.saturating_sub(5);
        for record in run
            .log
            .iter()
            .filter(|record| record.kind == "stage")
            .skip(skip)
        {
            let at = record.instant;
            let naming = if record.reasons.is_empty() {
                "naming no reasons".to_string()
            } else {
                format!("naming {}", record.reasons.join(", "))
            };
            lines.push(format!(
                "A stage record at stage {} {} stands at an age of {} whole seconds.",
                record.stage,
                naming,
                age_seconds(&at, now)
            ));
        }
    }
    // The record that entered the standing stage opens the
    // trailing run of stage records at that stage, so the
    // traffic since the stage was entered is the traffic
    // after that record.
    let mut entry: Option<usize> = None;
    for (position, record) in run.log.iter().enumerate() {
        if record.kind == "stage" {
            if record.stage != run.stage {
                entry = None;
            } else if entry.is_none() {
                entry = Some(position);
            }
        }
    }
    let start = entry.map_or(0, |position| position + 1);
    let proofs = run.log[start..]
        .iter()
        .filter(|record| record.kind == "proof")
        .count();
    let spends = run.log[start..]
        .iter()
        .filter(|record| record.kind == "spend")
        .count();
    lines.push(format!(
        "Since the run entered stage {} it has recorded {} and {}.",
        run.stage,
        counted(proofs, "proof record", "proof records"),
        counted(spends, "spend record", "spend records")
    ));
    lines.push("No proof note stands recorded in the decision log.".to_string());
    let mut text = String::from(DOCTRINE_PREAMBLE);
    text.push_str("\n\nThe state of the run at this evaluation stands as follows.\n");
    text.push_str(&lines.join("\n"));
    if let Some(context) = host_context {
        text.push_str(
            "\n\nHost supplied context follows. It is marked as host supplied and it is \
             recorded nowhere.\n",
        );
        text.push_str(context);
    }
    text
}
