//! Tests for the deadman staged cutoff, slice 1, translated pin
//! for pin from the Python protocol suite of Round 1 at the
//! counts that suite collects, 101 pins, with the three process
//! race pins restaged on operating system processes that reenter
//! this test binary through an environment gate, plus the worker
//! entry those processes run.
//!
//! Deterministic time comes only from injected instants. No test
//! sleeps to age a proof, and the only wall clock use is the
//! command line smoke path where no deadline is crossed.
//! Fixtures are synthetic and written for this repo.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use deadman::{
    format_instant, parse_instant, Consultation, Error, FixtureTransport, Instant, ModelTransport,
    Record, SubprocessTransport, Switch, Value, Verdict, DE_ESCALATION_BAR, ESCALATION_BAR,
    FORMAT_NAME, MODEL_NAME, STAGES, TRIGGERS,
};

fn t0() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).expect("the test instant is in range")
}

fn at(seconds: i64) -> Instant {
    t0().plus_seconds(seconds)
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> TestDir {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "deadman-test-{}-{name}-{unique}",
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

fn make_switch(dir: &TestDir) -> Switch {
    Switch::new(dir.path().join("state.json"))
}

fn state_path(dir: &TestDir) -> PathBuf {
    dir.path().join("state.json")
}

fn state_text(dir: &TestDir) -> String {
    std::fs::read_to_string(state_path(dir)).expect("the state file stands")
}

fn state_bytes(dir: &TestDir) -> Vec<u8> {
    std::fs::read(state_path(dir)).expect("the state file stands")
}

fn text(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn int(value: i64) -> Value {
    Value::Int(value as i128)
}

fn register(switch: &Switch, run_id: &str, now: &Instant) {
    switch
        .register(
            &text(run_id),
            now,
            &int(60),
            &int(5),
            &Value::Null,
            &Value::Null,
        )
        .expect("registration succeeds");
}

fn register_full(
    switch: &Switch,
    run_id: &str,
    now: &Instant,
    proof_interval_seconds: i64,
    missed_proof_limit: i64,
    soft_ceiling: Option<i64>,
    hard_ceiling: Option<i64>,
) {
    switch
        .register(
            &text(run_id),
            now,
            &int(proof_interval_seconds),
            &int(missed_proof_limit),
            &soft_ceiling.map_or(Value::Null, int),
            &hard_ceiling.map_or(Value::Null, int),
        )
        .expect("registration succeeds");
}

fn register_values(
    switch: &Switch,
    run_id: &Value,
    now: &Instant,
    proof_interval_seconds: &Value,
    missed_proof_limit: &Value,
    soft_ceiling: &Value,
    hard_ceiling: &Value,
) -> Result<(), Error> {
    switch.register(
        run_id,
        now,
        proof_interval_seconds,
        missed_proof_limit,
        soft_ceiling,
        hard_ceiling,
    )
}

fn kinds(records: &[Record]) -> Vec<&str> {
    records.iter().map(|record| record.kind.as_str()).collect()
}

fn assert_invalid_request<T: std::fmt::Debug>(result: Result<T, Error>) {
    assert!(
        matches!(result, Err(Error::InvalidRequest(_))),
        "expected an invalid request error, got {result:?}"
    );
}

fn assert_invalid_name<T: std::fmt::Debug>(result: Result<T, Error>) {
    assert!(
        matches!(result, Err(Error::InvalidName(_))),
        "expected an invalid name error, got {result:?}"
    );
}

fn assert_corrupt<T: std::fmt::Debug>(result: Result<T, Error>) {
    assert!(
        matches!(result, Err(Error::Corrupt(_))),
        "expected a corrupt state error, got {result:?}"
    );
}

fn assert_not_found<T: std::fmt::Debug>(result: Result<T, Error>) {
    assert!(
        matches!(result, Err(Error::NotFound(_))),
        "expected a not found error, got {result:?}"
    );
}

fn run_cli(args: &[&str], expect: i32) -> (String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_deadman"))
        .args(args)
        .output()
        .expect("the binary runs");
    assert_eq!(
        output.status.code(),
        Some(expect),
        "stderr was {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn vocabulary_stands_in_the_contract_order() {
    assert_eq!(
        STAGES,
        [
            "none",
            "warn",
            "throttle",
            "cut tools",
            "cut budget",
            "kill"
        ]
    );
    assert_eq!(
        TRIGGERS,
        [
            "operator pull",
            "hard ceiling",
            "soft ceiling",
            "missed proofs"
        ]
    );
    assert_eq!(FORMAT_NAME, "deadman-state/1");
}

#[test]
fn registration_defaults_and_declared_thresholds() {
    let dir = TestDir::new("registration-defaults");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let document = state_text(&dir);
    assert!(
        document.contains("\"proof_interval_seconds\": 60"),
        "{document}"
    );
    assert!(document.contains("\"missed_proof_limit\": 5"), "{document}");
    assert!(document.contains("\"soft_ceiling\": null"), "{document}");
    assert!(document.contains("\"hard_ceiling\": null"), "{document}");
    assert!(document.contains("\"spend_total\": 0"), "{document}");
    assert!(document.contains("\"last_proof_at\": null"), "{document}");
    assert!(document.contains("\"stage\": \"none\""), "{document}");
    assert!(document.contains("\"killed\": false"), "{document}");
    let other = Switch::new(dir.path().join("other.json"));
    register_full(&other, "run-b", &t0(), 30, 3, Some(40), Some(90));
    let declared = std::fs::read_to_string(dir.path().join("other.json")).unwrap();
    assert!(
        declared.contains("\"proof_interval_seconds\": 30"),
        "{declared}"
    );
    assert!(declared.contains("\"missed_proof_limit\": 3"), "{declared}");
    assert!(declared.contains("\"soft_ceiling\": 40"), "{declared}");
    assert!(declared.contains("\"hard_ceiling\": 90"), "{declared}");
}

#[test]
fn register_records_the_registration_first() {
    let dir = TestDir::new("registration-first");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let records = switch.log(&text("run-a")).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.seq, 1);
    assert_eq!(record.kind, "registered");
    assert_eq!(record.stage, "none");
    assert!(record.reasons.is_empty());
    assert_eq!(record.at, format_instant(&t0()).unwrap());
}

macro_rules! missed_case {
    ($name:ident, $elapsed:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("missed-matrix");
            let switch = make_switch(&dir);
            register(&switch, "run-a", &t0());
            let report = switch.stage(&text("run-a"), &at($elapsed)).unwrap();
            assert_eq!(report.stage, $expected);
            if $expected == "none" {
                assert!(report.reasons.is_empty());
            } else {
                assert_eq!(report.reasons, ["missed proofs"]);
            }
        }
    };
}

missed_case!(missed_proof_matrix_at_0_seconds, 0, "none");
missed_case!(missed_proof_matrix_at_60_seconds, 60, "none");
missed_case!(missed_proof_matrix_at_61_seconds, 61, "warn");
missed_case!(missed_proof_matrix_at_120_seconds, 120, "warn");
missed_case!(missed_proof_matrix_at_121_seconds, 121, "throttle");
missed_case!(missed_proof_matrix_at_181_seconds, 181, "cut tools");
missed_case!(missed_proof_matrix_at_241_seconds, 241, "cut budget");
missed_case!(missed_proof_matrix_at_299_seconds, 299, "cut budget");
missed_case!(missed_proof_matrix_at_301_seconds, 301, "kill");

#[test]
fn missed_counts_with_a_declared_limit_of_three() {
    let dir = TestDir::new("limit-of-three");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 3, None, None);
    assert_eq!(
        switch.stage(&text("run-a"), &at(121)).unwrap().stage,
        "throttle"
    );
    let other = Switch::new(dir.path().join("other.json"));
    register_full(&other, "run-a", &t0(), 60, 3, None, None);
    assert_eq!(other.stage(&text("run-a"), &at(181)).unwrap().stage, "kill");
}

#[test]
fn missed_counts_above_four_stay_at_cut_budget_below_the_limit() {
    let dir = TestDir::new("above-four");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 10, None, None);
    assert_eq!(
        switch.stage(&text("run-a"), &at(60 * 7 + 1)).unwrap().stage,
        "cut budget"
    );
    let other = Switch::new(dir.path().join("other.json"));
    register_full(&other, "run-a", &t0(), 60, 10, None, None);
    assert_eq!(
        other.stage(&text("run-a"), &at(60 * 10 + 1)).unwrap().stage,
        "kill"
    );
}

#[test]
fn a_limit_of_one_kills_on_the_first_missed_proof() {
    let dir = TestDir::new("limit-of-one");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 1, None, None);
    assert_eq!(switch.stage(&text("run-a"), &at(61)).unwrap().stage, "kill");
    assert!(state_text(&dir).contains("\"killed\": true"));
}

#[test]
fn spend_boundaries_at_the_exact_ceilings() {
    let dir = TestDir::new("spend-boundaries");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(100), Some(1000));
    switch.spend(&text("run-a"), &int(99), &at(1)).unwrap();
    assert_eq!(switch.stage(&text("run-a"), &at(2)).unwrap().stage, "none");
    switch.spend(&text("run-a"), &int(1), &at(3)).unwrap();
    let report = switch.stage(&text("run-a"), &at(4)).unwrap();
    assert_eq!(report.stage, "cut budget");
    assert_eq!(report.reasons, ["soft ceiling"]);
    let other = Switch::new(dir.path().join("other.json"));
    register_full(&other, "run-a", &t0(), 60, 5, Some(100), Some(1000));
    other.spend(&text("run-a"), &int(1000), &at(1)).unwrap();
    let report = other.stage(&text("run-a"), &at(2)).unwrap();
    assert_eq!(report.stage, "kill");
    assert_eq!(report.reasons, ["hard ceiling", "soft ceiling"]);
}

#[test]
fn spend_and_proofs_move_signals_only() {
    let dir = TestDir::new("signals-only");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, None, Some(10));
    let total = switch.spend(&text("run-a"), &int(50), &at(1)).unwrap();
    assert_eq!(total, 50);
    switch
        .prove(&text("run-a"), &text("working"), &at(2))
        .unwrap();
    let document = state_text(&dir);
    assert!(document.contains("\"stage\": \"none\""), "{document}");
    assert!(document.contains("\"spend_total\": 50"), "{document}");
    assert_eq!(switch.stage(&text("run-a"), &at(3)).unwrap().stage, "kill");
}

#[test]
fn reasons_name_every_live_trigger() {
    let dir = TestDir::new("every-trigger");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(10), Some(100));
    switch.spend(&text("run-a"), &int(100), &at(1)).unwrap();
    let report = switch.stage(&text("run-a"), &at(122)).unwrap();
    assert_eq!(report.stage, "kill");
    assert_eq!(
        report.reasons,
        ["hard ceiling", "soft ceiling", "missed proofs"]
    );
}

#[test]
fn a_proof_restores_freshness() {
    let dir = TestDir::new("proof-restores");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch
        .prove(&text("run-a"), &text("working"), &at(100))
        .unwrap();
    assert_eq!(
        switch.stage(&text("run-a"), &at(160)).unwrap().stage,
        "none"
    );
    assert_eq!(
        switch.stage(&text("run-a"), &at(161)).unwrap().stage,
        "warn"
    );
}

#[test]
fn a_late_proof_with_an_older_instant_moves_nothing() {
    let dir = TestDir::new("late-proof");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch
        .prove(&text("run-a"), &text("working"), &at(100))
        .unwrap();
    switch
        .prove(&text("run-a"), &text("late arrival"), &at(50))
        .unwrap();
    let standing = format!(
        "\"last_proof_at\": \"{}\"",
        format_instant(&at(100)).unwrap()
    );
    assert!(state_text(&dir).contains(&standing));
    assert_eq!(
        switch.stage(&text("run-a"), &at(160)).unwrap().stage,
        "none"
    );
}

#[test]
fn hysteresis_holds_once_then_steps_down_one_stage() {
    let dir = TestDir::new("hysteresis-holds");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    assert_eq!(switch.stage(&text("run-a"), &at(61)).unwrap().stage, "warn");
    switch
        .prove(&text("run-a"), &text("working"), &at(61))
        .unwrap();
    let held = switch.stage(&text("run-a"), &at(61)).unwrap();
    assert_eq!(held.stage, "warn");
    let stepped = switch.stage(&text("run-a"), &at(61)).unwrap();
    assert_eq!(stepped.stage, "none");
    let records = switch.log(&text("run-a")).unwrap();
    assert_eq!(kinds(&records), ["registered", "stage", "proof", "stage"]);
}

#[test]
fn hysteresis_steps_down_one_stage_at_a_time_from_high_stages() {
    let dir = TestDir::new("hysteresis-steps");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    assert_eq!(
        switch.stage(&text("run-a"), &at(241)).unwrap().stage,
        "cut budget"
    );
    switch
        .prove(&text("run-a"), &text("working"), &at(241))
        .unwrap();
    assert_eq!(
        switch.stage(&text("run-a"), &at(241)).unwrap().stage,
        "cut budget"
    );
    assert_eq!(
        switch.stage(&text("run-a"), &at(241)).unwrap().stage,
        "cut tools"
    );
    assert_eq!(
        switch.stage(&text("run-a"), &at(241)).unwrap().stage,
        "cut tools"
    );
    assert_eq!(
        switch.stage(&text("run-a"), &at(241)).unwrap().stage,
        "throttle"
    );
}

#[test]
fn a_fresh_warrant_resets_the_fall_count() {
    let dir = TestDir::new("fresh-warrant");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    assert_eq!(switch.stage(&text("run-a"), &at(61)).unwrap().stage, "warn");
    switch
        .prove(&text("run-a"), &text("working"), &at(61))
        .unwrap();
    assert_eq!(switch.stage(&text("run-a"), &at(61)).unwrap().stage, "warn");
    assert_eq!(
        switch.stage(&text("run-a"), &at(122)).unwrap().stage,
        "warn"
    );
    switch
        .prove(&text("run-a"), &text("working again"), &at(122))
        .unwrap();
    assert_eq!(
        switch.stage(&text("run-a"), &at(122)).unwrap().stage,
        "warn"
    );
    assert_eq!(
        switch.stage(&text("run-a"), &at(122)).unwrap().stage,
        "none"
    );
}

#[test]
fn a_rise_jumps_stages_when_a_hard_trigger_warrants_it() {
    let dir = TestDir::new("rise-jumps");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, None, Some(10));
    switch.spend(&text("run-a"), &int(10), &at(1)).unwrap();
    assert_eq!(switch.stage(&text("run-a"), &at(2)).unwrap().stage, "kill");
    let records = switch.log(&text("run-a")).unwrap();
    assert_eq!(kinds(&records), ["registered", "spend", "stage"]);
    assert_eq!(records[records.len() - 1].stage, "kill");
}

#[test]
fn pull_goes_straight_to_kill_and_latches() {
    let dir = TestDir::new("pull-latches");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    assert_eq!(switch.stage(&text("run-a"), &at(61)).unwrap().stage, "warn");
    assert_eq!(switch.pull(&text("run-a"), &at(62)).unwrap(), "kill");
    let records = switch.log(&text("run-a")).unwrap();
    let last = &records[records.len() - 1];
    assert_eq!(last.kind, "pull");
    assert_eq!(last.stage, "kill");
    assert_eq!(last.reasons, ["operator pull"]);
    let before = state_bytes(&dir);
    assert_eq!(switch.pull(&text("run-a"), &at(63)).unwrap(), "kill");
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn a_trigger_kill_latches_against_clear_evaluations_and_proofs() {
    let dir = TestDir::new("kill-latches");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, None, Some(10));
    switch.spend(&text("run-a"), &int(10), &at(1)).unwrap();
    assert_eq!(switch.stage(&text("run-a"), &at(2)).unwrap().stage, "kill");
    switch
        .prove(&text("run-a"), &text("still here"), &at(3))
        .unwrap();
    for step in 4..8 {
        assert_eq!(
            switch.stage(&text("run-a"), &at(step)).unwrap().stage,
            "kill"
        );
    }
    let document = state_text(&dir);
    assert!(document.contains("\"killed\": true"), "{document}");
    assert!(document.contains("\"stage\": \"kill\""), "{document}");
}

#[test]
fn a_pulled_run_stays_killed_when_no_trigger_stands() {
    let dir = TestDir::new("pulled-stays");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.pull(&text("run-a"), &at(1)).unwrap();
    assert_eq!(switch.stage(&text("run-a"), &at(30)).unwrap().stage, "kill");
    assert_eq!(switch.stage(&text("run-a"), &at(30)).unwrap().stage, "kill");
    assert!(state_text(&dir).contains("\"stage\": \"kill\""));
}

#[test]
fn a_closed_run_records_proofs_and_spend_as_no_ops() {
    let dir = TestDir::new("closed-no-ops");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(5), Some(10));
    switch.spend(&text("run-a"), &int(7), &at(1)).unwrap();
    switch.pull(&text("run-a"), &at(2)).unwrap();
    let total = switch.spend(&text("run-a"), &int(500), &at(3)).unwrap();
    assert_eq!(total, 7);
    switch
        .prove(&text("run-a"), &text("late proof"), &at(4))
        .unwrap();
    let document = state_text(&dir);
    assert!(document.contains("\"spend_total\": 7"), "{document}");
    assert!(document.contains("\"last_proof_at\": null"), "{document}");
    assert!(document.contains("\"stage\": \"kill\""), "{document}");
    let records = switch.log(&text("run-a")).unwrap();
    let tail = &records[records.len() - 2..];
    assert_eq!(kinds(tail), ["spend", "proof"]);
    assert!(tail.iter().all(|record| record.stage == "kill"));
}

#[test]
fn an_evaluation_of_a_closed_run_changes_nothing() {
    let dir = TestDir::new("closed-eval");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.pull(&text("run-a"), &at(1)).unwrap();
    let before = state_bytes(&dir);
    let report = switch.stage(&text("run-a"), &at(500)).unwrap();
    assert_eq!(report.stage, "kill");
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn a_quiet_evaluation_at_none_changes_nothing() {
    let dir = TestDir::new("quiet-eval");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let before = state_bytes(&dir);
    assert_eq!(switch.stage(&text("run-a"), &at(30)).unwrap().stage, "none");
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn a_held_stage_records_every_evaluation_that_holds_it() {
    let dir = TestDir::new("held-stage");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(61)).unwrap();
    switch.stage(&text("run-a"), &at(61)).unwrap();
    switch.stage(&text("run-a"), &at(61)).unwrap();
    let records = switch.log(&text("run-a")).unwrap();
    let stage_records: Vec<&Record> = records
        .iter()
        .filter(|record| record.kind == "stage")
        .collect();
    assert_eq!(stage_records.len(), 3);
    assert!(stage_records.iter().all(|record| record.stage == "warn"));
    assert!(stage_records
        .iter()
        .all(|record| record.reasons == ["missed proofs"]));
}

#[test]
fn the_log_walks_a_full_life_in_order() {
    let dir = TestDir::new("log-walks");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(10), Some(100));
    switch
        .prove(&text("run-a"), &text("working"), &at(10))
        .unwrap();
    switch.spend(&text("run-a"), &int(10), &at(11)).unwrap();
    switch.stage(&text("run-a"), &at(12)).unwrap();
    switch.pull(&text("run-a"), &at(13)).unwrap();
    let records = switch.log(&text("run-a")).unwrap();
    let seqs: Vec<u64> = records.iter().map(|record| record.seq).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    assert_eq!(
        kinds(&records),
        ["registered", "proof", "spend", "stage", "pull"]
    );
    assert_eq!(records[3].stage, "cut budget");
    assert_eq!(records[3].reasons, ["soft ceiling"]);
    assert_eq!(records[4].reasons, ["operator pull"]);
}

#[test]
fn reserved_fields_stand_empty_in_every_record() {
    let dir = TestDir::new("reserved-fields");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, None, Some(10));
    switch
        .prove(&text("run-a"), &text("working"), &at(1))
        .unwrap();
    switch.spend(&text("run-a"), &int(10), &at(2)).unwrap();
    switch.stage(&text("run-a"), &at(3)).unwrap();
    let records = switch.log(&text("run-a")).unwrap();
    assert_eq!(records.len(), 4);
    let document = state_text(&dir);
    assert_eq!(document.matches("\"model_name\": null").count(), 4);
    assert_eq!(document.matches("\"model_confidence\": null").count(), 4);
    assert_eq!(document.matches("\"fallback_taken\": null").count(), 4);
}

#[test]
fn evaluation_is_deterministic_on_the_same_state_and_instant() {
    let dir = TestDir::new("deterministic");
    let first = Switch::new(dir.path().join("first.json"));
    let second = Switch::new(dir.path().join("second.json"));
    for switch in [&first, &second] {
        register_full(switch, "run-a", &t0(), 60, 5, Some(10), Some(100));
        switch.spend(&text("run-a"), &int(10), &at(5)).unwrap();
    }
    let report_first = first.stage(&text("run-a"), &at(6)).unwrap();
    let report_second = second.stage(&text("run-a"), &at(6)).unwrap();
    assert_eq!(report_first, report_second);
    let first_bytes = std::fs::read(dir.path().join("first.json")).unwrap();
    let second_bytes = std::fs::read(dir.path().join("second.json")).unwrap();
    assert_eq!(first_bytes, second_bytes);
    let again_first = first.stage(&text("run-a"), &at(6)).unwrap();
    let again_second = second.stage(&text("run-a"), &at(6)).unwrap();
    assert_eq!(again_first, again_second);
    let first_bytes = std::fs::read(dir.path().join("first.json")).unwrap();
    let second_bytes = std::fs::read(dir.path().join("second.json")).unwrap();
    assert_eq!(first_bytes, second_bytes);
}

#[test]
fn registering_a_standing_id_fails_live_or_closed() {
    let dir = TestDir::new("standing-id");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    assert!(matches!(
        register_values(
            &switch,
            &text("run-a"),
            &at(1),
            &int(60),
            &int(5),
            &Value::Null,
            &Value::Null
        ),
        Err(Error::Refusal(_))
    ));
    switch.pull(&text("run-a"), &at(2)).unwrap();
    assert!(matches!(
        register_values(
            &switch,
            &text("run-a"),
            &at(3),
            &int(60),
            &int(5),
            &Value::Null,
            &Value::Null
        ),
        Err(Error::Refusal(_))
    ));
}

macro_rules! unknown_run_case {
    ($name:ident, $op:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("unknown-run");
            let switch = make_switch(&dir);
            register(&switch, "run-a", &t0());
            let operation: fn(&Switch) -> Result<(), Error> = $op;
            assert_not_found(operation(&switch));
        }
    };
}

unknown_run_case!(unknown_run_prove_is_not_found, |switch: &Switch| {
    switch.prove(&text("ghost"), &text("note"), &t0())
});
unknown_run_case!(unknown_run_spend_is_not_found, |switch: &Switch| {
    switch.spend(&text("ghost"), &int(1), &t0()).map(|_| ())
});
unknown_run_case!(unknown_run_pull_is_not_found, |switch: &Switch| {
    switch.pull(&text("ghost"), &t0()).map(|_| ())
});
unknown_run_case!(unknown_run_stage_is_not_found, |switch: &Switch| {
    switch.stage(&text("ghost"), &t0()).map(|_| ())
});
unknown_run_case!(unknown_run_log_is_not_found, |switch: &Switch| {
    switch.log(&text("ghost")).map(|_| ())
});

#[test]
fn a_missing_state_file_reads_as_not_found_and_creates_nothing() {
    let dir = TestDir::new("missing-file");
    let path = dir.path().join("deep").join("state.json");
    let switch = Switch::new(path.clone());
    assert_not_found(switch.log(&text("run-a")));
    assert!(!path.exists());
    register(&switch, "run-a", &t0());
    assert!(path.is_file());
    assert_eq!(switch.log(&text("run-a")).unwrap()[0].kind, "registered");
}

macro_rules! invalid_id_case {
    ($name:ident, $bad:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("invalid-id");
            let switch = make_switch(&dir);
            let bad: Value = $bad;
            assert_invalid_name(register_values(
                &switch,
                &bad,
                &t0(),
                &int(60),
                &int(5),
                &Value::Null,
                &Value::Null,
            ));
            assert_invalid_name(switch.stage(&bad, &t0()).map(|_| ()));
        }
    };
}

invalid_id_case!(invalid_run_id_empty, text(""));
invalid_id_case!(invalid_run_id_leading_hyphen, text("-run"));
invalid_id_case!(invalid_run_id_trailing_hyphen, text("run-"));
invalid_id_case!(invalid_run_id_leading_underscore, text("_run"));
invalid_id_case!(invalid_run_id_trailing_underscore, text("run_"));
invalid_id_case!(invalid_run_id_uppercase, text("Run"));
invalid_id_case!(invalid_run_id_with_space, text("run a"));
invalid_id_case!(invalid_run_id_with_slash, text("run/a"));
invalid_id_case!(invalid_run_id_with_dot, text("run.a"));
invalid_id_case!(invalid_run_id_sixty_five_letters, text(&"r".repeat(65)));
invalid_id_case!(invalid_run_id_an_integer, Value::Int(7));
invalid_id_case!(invalid_run_id_none, Value::Null);

macro_rules! valid_id_case {
    ($name:ident, $good:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("valid-id");
            let switch = make_switch(&dir);
            register(&switch, $good, &t0());
            assert_eq!(switch.stage(&text($good), &t0()).unwrap().stage, "none");
        }
    };
}

valid_id_case!(valid_run_id_single_letter, "a");
valid_id_case!(valid_run_id_single_digit, "1");
valid_id_case!(valid_run_id_hyphenated, "run-1");
valid_id_case!(valid_run_id_underscored, "a_b");
valid_id_case!(valid_run_id_sixty_four_letters, &"r".repeat(64));
valid_id_case!(valid_run_id_mixed_separators, "a-1_b-2");
valid_id_case!(valid_run_id_leading_digit, "9run");
valid_id_case!(valid_run_id_trailing_digit, "run9");
valid_id_case!(valid_run_id_all_digits, "407");
valid_id_case!(valid_run_id_digits_and_separators_inside, "r2-d2_c3");

fn assert_invalid_name_message<T: std::fmt::Debug>(result: Result<T, Error>, expected: &str) {
    match result {
        Err(Error::InvalidName(message)) => assert_eq!(message, expected),
        other => panic!("expected an invalid name error, got {other:?}"),
    }
}

macro_rules! invalid_id_message_case {
    ($name:ident, $bad:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("invalid-id-message");
            let switch = make_switch(&dir);
            let bad: Value = $bad;
            assert_invalid_name_message(
                register_values(
                    &switch,
                    &bad,
                    &t0(),
                    &int(60),
                    &int(5),
                    &Value::Null,
                    &Value::Null,
                ),
                $expected,
            );
        }
    };
}

invalid_id_message_case!(
    invalid_run_id_non_ascii_letters_refused_with_exact_message,
    text("rún-ü"),
    "invalid run id 'rún-ü', use one component of ASCII lowercase letters \
     and ASCII digits and hyphens and underscores, starting and ending \
     with an ASCII lowercase letter or an ASCII digit, at most 64 \
     characters long"
);
invalid_id_message_case!(
    invalid_run_id_uppercase_ascii_refused_with_exact_message,
    text("RUN-1"),
    "invalid run id 'RUN-1', use one component of ASCII lowercase letters \
     and ASCII digits and hyphens and underscores, starting and ending \
     with an ASCII lowercase letter or an ASCII digit, at most 64 \
     characters long"
);
invalid_id_message_case!(
    invalid_run_id_non_ascii_digit_refused_with_exact_message,
    text("run\u{0661}"),
    "invalid run id 'run\u{0661}', use one component of ASCII lowercase \
     letters and ASCII digits and hyphens and underscores, starting and \
     ending with an ASCII lowercase letter or an ASCII digit, at most 64 \
     characters long"
);
invalid_id_message_case!(
    invalid_run_id_trailing_hyphen_refused_with_exact_message,
    text("run-"),
    "invalid run id 'run-', use one component of ASCII lowercase letters \
     and ASCII digits and hyphens and underscores, starting and ending \
     with an ASCII lowercase letter or an ASCII digit, at most 64 \
     characters long"
);

macro_rules! invalid_registration_case {
    ($name:ident, $pi:expr, $ml:expr, $soft:expr, $hard:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("invalid-registration");
            let switch = make_switch(&dir);
            assert_invalid_request(register_values(
                &switch,
                &text("run-a"),
                &t0(),
                &$pi,
                &$ml,
                &$soft,
                &$hard,
            ));
            assert!(!state_path(&dir).exists());
        }
    };
}

invalid_registration_case!(
    invalid_registration_interval_zero,
    int(0),
    int(5),
    Value::Null,
    Value::Null
);
invalid_registration_case!(
    invalid_registration_interval_negative,
    int(-5),
    int(5),
    Value::Null,
    Value::Null
);
invalid_registration_case!(
    invalid_registration_interval_text,
    text("60"),
    int(5),
    Value::Null,
    Value::Null
);
invalid_registration_case!(
    invalid_registration_interval_boolean,
    Value::Bool(true),
    int(5),
    Value::Null,
    Value::Null
);
invalid_registration_case!(
    invalid_registration_limit_zero,
    int(60),
    int(0),
    Value::Null,
    Value::Null
);
invalid_registration_case!(
    invalid_registration_limit_negative,
    int(60),
    int(-1),
    Value::Null,
    Value::Null
);
invalid_registration_case!(
    invalid_registration_soft_ceiling_negative,
    int(60),
    int(5),
    int(-1),
    Value::Null
);
invalid_registration_case!(
    invalid_registration_hard_ceiling_negative,
    int(60),
    int(5),
    Value::Null,
    int(-1)
);
invalid_registration_case!(
    invalid_registration_soft_above_hard,
    int(60),
    int(5),
    int(50),
    int(49)
);

macro_rules! invalid_spend_case {
    ($name:ident, $amount:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("invalid-spend");
            let switch = make_switch(&dir);
            register(&switch, "run-a", &t0());
            let before = state_bytes(&dir);
            assert_invalid_request(switch.spend(&text("run-a"), &$amount, &at(1)).map(|_| ()));
            assert_eq!(state_bytes(&dir), before);
        }
    };
}

invalid_spend_case!(invalid_spend_amount_negative, int(-1));
invalid_spend_case!(invalid_spend_amount_text, text("5"));
invalid_spend_case!(invalid_spend_amount_boolean, Value::Bool(true));
invalid_spend_case!(invalid_spend_amount_float, Value::Float(1.5));

#[test]
fn a_note_at_the_bound_is_accepted_and_past_it_refused() {
    let dir = TestDir::new("note-bound");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch
        .prove(&text("run-a"), &text(&"x".repeat(200)), &at(1))
        .unwrap();
    assert_invalid_request(
        switch
            .prove(&text("run-a"), &text(&"x".repeat(201)), &at(2))
            .map(|_| ()),
    );
}

#[test]
fn a_naive_instant_is_refused() {
    let dir = TestDir::new("naive-instant");
    let switch = make_switch(&dir);
    let naive =
        Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, None).expect("the test instant is in range");
    assert_invalid_request(register_values(
        &switch,
        &text("run-a"),
        &naive,
        &int(60),
        &int(5),
        &Value::Null,
        &Value::Null,
    ));
    register(&switch, "run-a", &t0());
    assert_invalid_request(switch.stage(&text("run-a"), &naive).map(|_| ()));
}

#[test]
fn a_proof_interval_at_the_representable_bound_registers() {
    let dir = TestDir::new("interval-at-bound");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 86_399_999_999_999, 5, None, None);
    assert!(state_text(&dir).contains("\"proof_interval_seconds\": 86399999999999"));
    assert_eq!(switch.stage(&text("run-a"), &t0()).unwrap().stage, "none");
}

#[test]
fn a_proof_interval_past_the_representable_bound_is_refused() {
    let dir = TestDir::new("interval-past-bound");
    let switch = make_switch(&dir);
    assert_invalid_request(register_values(
        &switch,
        &text("run-a"),
        &t0(),
        &int(86_400_000_000_000),
        &int(5),
        &Value::Null,
        &Value::Null,
    ));
    let huge = Value::BigInt(format!("1{}", "0".repeat(5000)));
    assert_invalid_request(register_values(
        &switch,
        &text("run-a"),
        &t0(),
        &huge,
        &int(5),
        &Value::Null,
        &Value::Null,
    ));
    assert!(!state_path(&dir).exists());
}

#[test]
fn the_cli_refuses_an_interval_past_the_bound_without_a_traceback() {
    let dir = TestDir::new("cli-interval-bound");
    let state = state_path(&dir).to_string_lossy().into_owned();
    let (_, stderr) = run_cli(
        &[
            "--state",
            &state,
            "register",
            "run-a",
            "--proof-interval",
            "86400000000000",
        ],
        2,
    );
    assert!(stderr.starts_with("error:"), "{stderr}");
    assert_eq!(stderr.trim().lines().count(), 1);
    assert!(!stderr.contains("Traceback"));
}

#[test]
fn an_instant_whose_utc_form_overflows_is_refused() {
    let dir = TestDir::new("utc-overflow");
    let switch = make_switch(&dir);
    let extreme = Instant::from_civil(9999, 12, 31, 23, 0, 0, 0, Some(-14 * 3600 * 1_000_000))
        .expect("the test instant is in range");
    assert_invalid_request(register_values(
        &switch,
        &text("run-a"),
        &extreme,
        &int(60),
        &int(5),
        &Value::Null,
        &Value::Null,
    ));
    assert!(!state_path(&dir).exists());
}

#[test]
fn from_civil_accepts_parts_at_the_valid_bounds() {
    let earliest = Instant::from_civil(1, 1, 1, 0, 0, 0, 0, Some(0)).unwrap();
    assert_eq!(
        format_instant(&earliest).unwrap(),
        "0001-01-01T00:00:00+00:00"
    );
    let latest = Instant::from_civil(9999, 12, 31, 23, 59, 59, 999_999, Some(0)).unwrap();
    assert_eq!(
        format_instant(&latest).unwrap(),
        "9999-12-31T23:59:59.999999+00:00"
    );
    let leap_day = Instant::from_civil(2032, 2, 29, 23, 59, 59, 999_999, None).unwrap();
    assert!(!leap_day.is_aware());
    let offset_edge = Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(86_399_999_999)).unwrap();
    assert!(offset_edge.is_aware());
    let offset_edge_low =
        Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(-86_399_999_999)).unwrap();
    assert!(offset_edge_low.is_aware());
}

#[test]
fn from_civil_refuses_parts_past_the_valid_bounds() {
    // Every refusal is the typed invalid request error. The
    // absurd magnitudes here are the ones the standards probe
    // drove against the pre repair constructor, an hour of
    // 2_000_000 and a year of i64::MAX / 2, which panicked
    // inside the second and day arithmetic.
    let refused = [
        (2031, 5, 4, 24, 0, 0, 0, Some(0)),
        (2031, 5, 4, 2_000_000, 0, 0, 0, Some(0)),
        (2031, 5, 4, 12, 60, 0, 0, Some(0)),
        (2031, 5, 4, 12, 0, 60, 0, Some(0)),
        (2031, 5, 4, 12, 0, 0, 1_000_000, Some(0)),
        (2031, 0, 4, 12, 0, 0, 0, Some(0)),
        (2031, 13, 4, 12, 0, 0, 0, Some(0)),
        (2031, 5, 0, 12, 0, 0, 0, Some(0)),
        (2031, 2, 29, 12, 0, 0, 0, Some(0)),
        (0, 5, 4, 12, 0, 0, 0, Some(0)),
        (10_000, 5, 4, 12, 0, 0, 0, Some(0)),
        (i64::MAX / 2, 5, 4, 12, 0, 0, 0, Some(0)),
        (2031, 5, 4, 12, 0, 0, 0, Some(86_400_000_000)),
        (2031, 5, 4, 12, 0, 0, 0, Some(-86_400_000_000)),
        (2031, 5, 4, 12, 0, 0, 0, Some(i64::MIN)),
    ];
    for (year, month, day, hour, minute, second, micros, offset) in refused {
        assert_invalid_request(Instant::from_civil(
            year, month, day, hour, minute, second, micros, offset,
        ));
    }
}

#[test]
fn a_stored_interval_past_the_bound_fails_as_a_request_error() {
    let dir = TestDir::new("stored-interval");
    let valid = valid_document(&dir);
    let damaged = valid.replace(
        "\"proof_interval_seconds\": 60",
        "\"proof_interval_seconds\": 100000000000000",
    );
    assert_ne!(damaged, valid);
    std::fs::write(state_path(&dir), &damaged).unwrap();
    let switch = make_switch(&dir);
    assert_invalid_request(switch.stage(&text("run-a"), &at(10)).map(|_| ()));
    assert_eq!(state_bytes(&dir), damaged.as_bytes());
}

#[test]
fn a_spend_at_the_bound_is_accepted_and_past_it_refused() {
    let dir = TestDir::new("spend-at-bound");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let total = switch
        .spend(&text("run-a"), &int(1_000_000_000_000_000_000), &at(1))
        .unwrap();
    assert_eq!(total, 1_000_000_000_000_000_000);
    let before = state_bytes(&dir);
    assert_invalid_request(switch.spend(&text("run-a"), &int(1), &at(2)).map(|_| ()));
    assert_invalid_request(
        switch
            .spend(&text("run-a"), &int(1_000_000_000_000_000_001), &at(2))
            .map(|_| ()),
    );
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn a_spend_with_thousands_of_digits_is_refused_cleanly() {
    let dir = TestDir::new("spend-digits");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let before = state_bytes(&dir);
    let huge = Value::BigInt(format!("1{}", "0".repeat(5000)));
    assert_invalid_request(switch.spend(&text("run-a"), &huge, &at(1)).map(|_| ()));
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn a_stored_spend_total_past_the_bound_is_corruption() {
    let dir = TestDir::new("stored-spend");
    let valid = valid_document(&dir);
    let damaged = valid.replace(
        "\"spend_total\": 10",
        "\"spend_total\": 1000000000000000001",
    );
    assert_ne!(damaged, valid);
    std::fs::write(state_path(&dir), &damaged).unwrap();
    let switch = make_switch(&dir);
    assert_corrupt(switch.log(&text("run-a")));
    assert_eq!(state_bytes(&dir), damaged.as_bytes());
}

#[test]
fn a_stored_spend_total_with_thousands_of_digits_is_corruption() {
    let dir = TestDir::new("stored-spend-digits");
    let valid = valid_document(&dir);
    let damaged = valid.replace(
        "\"spend_total\": 10",
        &format!("\"spend_total\": {}", "9".repeat(5000)),
    );
    assert_ne!(damaged, valid);
    std::fs::write(state_path(&dir), &damaged).unwrap();
    let switch = make_switch(&dir);
    assert_corrupt(switch.log(&text("run-a")));
    assert_eq!(state_bytes(&dir), damaged.as_bytes());
}

#[test]
fn a_missed_proof_limit_at_the_bound_registers() {
    let dir = TestDir::new("limit-at-bound");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 1_000_000, None, None);
    assert!(state_text(&dir).contains("\"missed_proof_limit\": 1000000"));
    assert_eq!(switch.stage(&text("run-a"), &t0()).unwrap().stage, "none");
}

#[test]
fn a_missed_proof_limit_past_the_bound_is_refused() {
    let dir = TestDir::new("limit-past-bound");
    let switch = make_switch(&dir);
    assert_invalid_request(register_values(
        &switch,
        &text("run-a"),
        &t0(),
        &int(60),
        &int(1_000_001),
        &Value::Null,
        &Value::Null,
    ));
    let huge = Value::BigInt(format!("1{}", "0".repeat(5000)));
    assert_invalid_request(register_values(
        &switch,
        &text("run-a"),
        &t0(),
        &int(60),
        &huge,
        &Value::Null,
        &Value::Null,
    ));
    assert!(!state_path(&dir).exists());
}

#[test]
fn ceilings_at_the_bound_register() {
    let dir = TestDir::new("ceilings-at-bound");
    let switch = make_switch(&dir);
    register_full(
        &switch,
        "run-a",
        &t0(),
        60,
        5,
        Some(1_000_000_000_000_000_000),
        Some(1_000_000_000_000_000_000),
    );
    let document = state_text(&dir);
    assert!(
        document.contains("\"soft_ceiling\": 1000000000000000000"),
        "{document}"
    );
    assert!(
        document.contains("\"hard_ceiling\": 1000000000000000000"),
        "{document}"
    );
}

macro_rules! ceiling_past_bound_case {
    ($name:ident, $soft:expr, $hard:expr, $soft_huge:expr, $hard_huge:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("ceiling-past-bound");
            let switch = make_switch(&dir);
            assert_invalid_request(register_values(
                &switch,
                &text("run-a"),
                &t0(),
                &int(60),
                &int(5),
                &$soft,
                &$hard,
            ));
            assert_invalid_request(register_values(
                &switch,
                &text("run-a"),
                &t0(),
                &int(60),
                &int(5),
                &$soft_huge,
                &$hard_huge,
            ));
            assert!(!state_path(&dir).exists());
        }
    };
}

ceiling_past_bound_case!(
    a_soft_ceiling_past_the_bound_is_refused,
    int(1_000_000_000_000_000_001),
    Value::Null,
    Value::BigInt(format!("1{}", "0".repeat(5000))),
    Value::Null
);
ceiling_past_bound_case!(
    a_hard_ceiling_past_the_bound_is_refused,
    Value::Null,
    int(1_000_000_000_000_000_001),
    Value::Null,
    Value::BigInt(format!("1{}", "0".repeat(5000)))
);

#[test]
fn a_refused_registration_changes_no_standing_state() {
    let dir = TestDir::new("refused-registration");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let before = state_bytes(&dir);
    let huge = Value::BigInt(format!("1{}", "0".repeat(5000)));
    assert_invalid_request(register_values(
        &switch,
        &text("run-b"),
        &t0(),
        &int(60),
        &huge,
        &Value::Null,
        &Value::Null,
    ));
    assert_invalid_request(register_values(
        &switch,
        &text("run-b"),
        &t0(),
        &int(60),
        &int(5),
        &Value::Null,
        &huge,
    ));
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn the_cli_refuses_a_missed_limit_past_the_bound_without_a_traceback() {
    let dir = TestDir::new("cli-limit-bound");
    let state = state_path(&dir).to_string_lossy().into_owned();
    let (_, stderr) = run_cli(
        &[
            "--state",
            &state,
            "register",
            "run-a",
            "--missed-limit",
            "1000001",
        ],
        2,
    );
    assert!(stderr.starts_with("error:"), "{stderr}");
    assert_eq!(stderr.trim().lines().count(), 1);
    assert!(!stderr.contains("Traceback"));
    assert!(!state_path(&dir).exists());
}

#[test]
fn a_stored_missed_proof_limit_past_the_bound_is_corruption() {
    let dir = TestDir::new("stored-limit");
    let valid = valid_document(&dir);
    let damaged = valid.replace(
        "\"missed_proof_limit\": 5",
        "\"missed_proof_limit\": 1000001",
    );
    assert_ne!(damaged, valid);
    std::fs::write(state_path(&dir), &damaged).unwrap();
    let switch = make_switch(&dir);
    assert_corrupt(switch.log(&text("run-a")));
    assert_eq!(state_bytes(&dir), damaged.as_bytes());
}

macro_rules! stored_ceiling_case {
    ($name:ident, $damaged:expr) => {
        #[test]
        fn $name() {
            let dir = TestDir::new("stored-ceiling");
            let valid = valid_document(&dir);
            let damaged: String = $damaged(&valid);
            assert_ne!(damaged, valid);
            std::fs::write(state_path(&dir), &damaged).unwrap();
            let switch = make_switch(&dir);
            assert_corrupt(switch.log(&text("run-a")));
            assert_eq!(state_bytes(&dir), damaged.as_bytes());
        }
    };
}

stored_ceiling_case!(
    a_stored_hard_ceiling_past_the_bound_is_corruption,
    |valid: &String| {
        valid.replace(
            "\"hard_ceiling\": 100",
            "\"hard_ceiling\": 1000000000000000001",
        )
    }
);
stored_ceiling_case!(
    a_stored_soft_ceiling_past_the_bound_is_corruption,
    |valid: &String| {
        valid
            .replace(
                "\"soft_ceiling\": 10",
                "\"soft_ceiling\": 1000000000000000001",
            )
            .replace(
                "\"hard_ceiling\": 100",
                "\"hard_ceiling\": 1000000000000000002",
            )
    }
);

/// A valid standing document, produced by the operations the
/// Python helper drove, soft ceiling 10 and hard ceiling 100,
/// a proof, a spend of 10, and an evaluation at cut budget.
fn valid_document(dir: &TestDir) -> String {
    let switch = make_switch(dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(10), Some(100));
    switch
        .prove(&text("run-a"), &text("working"), &at(1))
        .unwrap();
    switch.spend(&text("run-a"), &int(10), &at(2)).unwrap();
    switch.stage(&text("run-a"), &at(3)).unwrap();
    state_text(dir)
}

fn replace_nth(text: &str, needle: &str, replacement: &str, n: usize) -> String {
    let mut index = 0;
    let mut from = 0;
    loop {
        let found = text[from..].find(needle).expect("the anchor stands") + from;
        if index == n {
            return format!(
                "{}{}{}",
                &text[..found],
                replacement,
                &text[found + needle.len()..]
            );
        }
        index += 1;
        from = found + needle.len();
    }
}

fn assert_corrupt_document(dir: &TestDir, damaged: &[u8]) {
    std::fs::write(state_path(dir), damaged).unwrap();
    let switch = make_switch(dir);
    assert_corrupt(switch.log(&text("run-a")));
    assert_corrupt(
        switch
            .prove(&text("run-a"), &text("working"), &at(9))
            .map(|_| ()),
    );
    assert_eq!(state_bytes(dir), damaged);
}

#[test]
fn damage_battery() {
    let base = TestDir::new("damage-battery");
    let valid = valid_document(&base);
    let run_start = valid.find("    {\n").expect("the run block stands");
    let run_end = valid.rfind("\n    }\n  ]").expect("the run block ends") + "\n    }".len();
    let run_block = &valid[run_start..run_end];
    let duplicated_run = format!("{},\n{}\n  ]\n}}\n", &valid[..run_end], run_block);
    let cases: Vec<String> = vec![
        valid.replacen(
            "\"format\": \"deadman-state/1\"",
            "\"format\": \"deadman-state/2\"",
            1,
        ),
        valid.replacen("{\n  \"format\"", "{\n  \"extra\": 1,\n  \"format\"", 1),
        "{\"format\": \"deadman-state/1\", \"runs\": {}}".to_string(),
        valid.replacen("\"id\": \"run-a\",", "\"id\": \"run-a\", \"extra\": 1,", 1),
        valid.replacen(
            "\"stage\": \"cut budget\",\n      \"clear_streak\": 0,",
            "\"clear_streak\": 0,",
            1,
        ),
        valid.replacen("\"stage\": \"cut budget\"", "\"stage\": \"pause\"", 1),
        valid.replacen("\"spend_total\": 10", "\"spend_total\": \"10\"", 1),
        valid.replacen("\"killed\": false", "\"killed\": \"no\"", 1),
        valid.replacen("\"killed\": false", "\"killed\": true", 1),
        valid.replacen("\"soft_ceiling\": 10,", "\"soft_ceiling\": 200,", 1),
        valid.replacen("\"kind\": \"registered\"", "\"kind\": \"vanished\"", 1),
        valid.replacen("\"kind\": \"registered\"", "\"kind\": []", 1),
        valid.replacen("\"kind\": \"proof\"", "\"kind\": {\"x\": 1}", 1),
        valid.replacen("\"seq\": 1,", "\"seq\": 7,", 1),
        replace_nth(&valid, "\"reasons\": []", "\"reasons\": [\"bad luck\"]", 1),
        replace_nth(
            &valid,
            "\"model_name\": null",
            "\"model_name\": \"a model\"",
            1,
        ),
        valid.replacen("\"kind\": \"registered\"", "\"kind\": \"proof\"", 1),
        valid.replacen(
            "\"registered_at\": \"2031-05-04T12:00:00+00:00\"",
            "\"registered_at\": \"2031-05-04T12:00:00\"",
            1,
        ),
        duplicated_run,
    ];
    assert_eq!(cases.len(), 19);
    for (index, damaged) in cases.iter().enumerate() {
        assert_ne!(damaged, &valid, "case {index} changes the document");
        let case_dir = TestDir::new(&format!("damage-case-{index}"));
        assert_corrupt_document(&case_dir, damaged.as_bytes());
    }
}

#[test]
fn damage_battery_text_shapes() {
    let base = TestDir::new("damage-text-base");
    let valid = valid_document(&base);
    let duplicated = valid.replacen(
        "\"format\": \"deadman-state/1\"",
        "\"format\": \"deadman-state/1\", \"format\": \"deadman-state/1\"",
        1,
    );
    let first = TestDir::new("damage-text-duplicate");
    assert_corrupt_document(&first, duplicated.as_bytes());
    let second = TestDir::new("damage-text-truncated");
    let raw = valid.as_bytes();
    assert_corrupt_document(&second, &raw[..raw.len() / 2]);
    let third = TestDir::new("damage-text-not-json");
    assert_corrupt_document(&third, b"this is not json");
}

/// Replace the reasons array at the given index among the
/// reasons arrays of a document, counting from 0.
fn set_reasons(text: &str, occurrence: usize, replacement: &str) -> String {
    let mut from = 0;
    for index in 0..=occurrence {
        let start = text[from..]
            .find("\"reasons\":")
            .expect("the anchor stands")
            + from;
        let open = text[start..].find('[').expect("the array opens") + start;
        let close = text[open..].find(']').expect("the array closes") + open;
        if index == occurrence {
            return format!("{}{}{}", &text[..open], replacement, &text[close + 1..]);
        }
        from = close + 1;
    }
    unreachable!()
}

fn replace_last(text: &str, needle: &str, replacement: &str) -> String {
    let found = text.rfind(needle).expect("the anchor stands");
    format!(
        "{}{}{}",
        &text[..found],
        replacement,
        &text[found + needle.len()..]
    )
}

/// A cross validation damage pin. The damaged document is
/// refused as corruption carrying the named disagreement on
/// the log read, on the stage operation, and on a proof, and
/// the state bytes stand unchanged afterwards.
fn assert_xval_corrupt(dir: &TestDir, damaged: &str, needle: &str) {
    std::fs::write(state_path(dir), damaged).unwrap();
    let switch = make_switch(dir);
    let err = switch.log(&text("run-a")).unwrap_err();
    assert!(
        matches!(err, Error::Corrupt(_)),
        "expected a corrupt state error, got {err:?}"
    );
    assert!(
        err.message().contains(needle),
        "expected the refusal to name {needle:?}, got {:?}",
        err.message()
    );
    assert_corrupt(switch.stage(&text("run-a"), &at(9)).map(|_| ()));
    assert_corrupt(
        switch
            .prove(&text("run-a"), &text("working"), &at(9))
            .map(|_| ()),
    );
    assert_eq!(state_bytes(dir), damaged.as_bytes());
}

fn base_killed_by_pull(dir: &TestDir) -> String {
    let switch = make_switch(dir);
    register(&switch, "run-a", &t0());
    switch
        .prove(&text("run-a"), &text("working"), &at(1))
        .unwrap();
    switch.pull(&text("run-a"), &at(2)).unwrap();
    state_text(dir)
}

fn base_continued_killed(dir: &TestDir) -> String {
    let switch = make_switch(dir);
    register_full(&switch, "run-a", &t0(), 60, 5, None, Some(10));
    switch.spend(&text("run-a"), &int(10), &at(1)).unwrap();
    switch.stage(&text("run-a"), &at(2)).unwrap();
    switch.spend(&text("run-a"), &int(500), &at(3)).unwrap();
    switch
        .prove(&text("run-a"), &text("late proof"), &at(4))
        .unwrap();
    state_text(dir)
}

fn base_step_down(dir: &TestDir) -> String {
    let switch = make_switch(dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(181)).unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(181))
        .unwrap();
    switch.stage(&text("run-a"), &at(181)).unwrap();
    switch.stage(&text("run-a"), &at(181)).unwrap();
    state_text(dir)
}

fn base_warn(dir: &TestDir) -> String {
    let switch = make_switch(dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(61)).unwrap();
    state_text(dir)
}

fn base_mid_both_sides(dir: &TestDir) -> String {
    let switch = make_switch(dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(100), Some(1000));
    switch.spend(&text("run-a"), &int(40), &at(1)).unwrap();
    switch.stage(&text("run-a"), &at(121)).unwrap();
    switch.spend(&text("run-a"), &int(60), &at(122)).unwrap();
    switch.stage(&text("run-a"), &at(123)).unwrap();
    state_text(dir)
}

#[test]
fn xval_standing_stage_disagrees_with_log_end() {
    let base = TestDir::new("xval-standing-stage-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen(
        "\"stage\": \"cut budget\",\n      \"clear_streak\": 0,",
        "\"stage\": \"throttle\",\n      \"clear_streak\": 0,",
        1,
    );
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-standing-stage");
    assert_xval_corrupt(&dir, &damaged, "where its log ends at stage");
}

#[test]
fn xval_proof_record_stage_disagrees_with_log() {
    let base = TestDir::new("xval-proof-stage-base");
    let valid = valid_document(&base);
    let damaged = replace_nth(&valid, "\"stage\": \"none\"", "\"stage\": \"warn\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-proof-stage");
    assert_xval_corrupt(&dir, &damaged, "not the stage");
}

#[test]
fn xval_spend_record_stage_disagrees_with_log() {
    let base = TestDir::new("xval-spend-stage-base");
    let valid = valid_document(&base);
    let damaged = replace_nth(&valid, "\"stage\": \"none\"", "\"stage\": \"warn\"", 2);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-spend-stage");
    assert_xval_corrupt(&dir, &damaged, "not the stage");
}

#[test]
fn xval_second_registered_record_refused() {
    let base = TestDir::new("xval-second-registered-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen("\"kind\": \"spend\"", "\"kind\": \"registered\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-second-registered");
    assert_xval_corrupt(&dir, &damaged, "second registered record");
}

#[test]
fn xval_stage_record_dropping_two_stages_refused() {
    let base = TestDir::new("xval-drop-two-base");
    let valid = base_step_down(&base);
    let damaged = replace_nth(&valid, "\"stage\": \"throttle\"", "\"stage\": \"none\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-drop-two");
    assert_xval_corrupt(&dir, &damaged, "a succession no evaluation writes");
}

#[test]
fn xval_stage_record_after_kill_refused() {
    let base = TestDir::new("xval-stage-after-kill-base");
    let valid = base_continued_killed(&base);
    let damaged = valid.replacen("\"kind\": \"proof\"", "\"kind\": \"stage\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-stage-after-kill");
    assert_xval_corrupt(&dir, &damaged, "follows the kill record");
}

#[test]
fn xval_pull_record_after_kill_refused() {
    let base = TestDir::new("xval-pull-after-kill-base");
    let valid = base_continued_killed(&base);
    let damaged = valid.replacen("\"kind\": \"proof\"", "\"kind\": \"pull\"", 1);
    let damaged = set_reasons(&damaged, 4, "[\"operator pull\"]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-pull-after-kill");
    assert_xval_corrupt(&dir, &damaged, "follows the kill record");
}

#[test]
fn xval_last_proof_instant_no_proof_record_carries() {
    let base = TestDir::new("xval-last-proof-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen(
        "\"last_proof_at\": \"2031-05-04T12:00:01+00:00\"",
        "\"last_proof_at\": \"2031-05-04T12:00:09+00:00\"",
        1,
    );
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-last-proof");
    assert_xval_corrupt(
        &dir,
        &damaged,
        "where its latest live proof record stands at",
    );
}

#[test]
fn xval_last_proof_unset_where_live_proof_stands() {
    let base = TestDir::new("xval-last-proof-unset-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen(
        "\"last_proof_at\": \"2031-05-04T12:00:01+00:00\"",
        "\"last_proof_at\": null",
        1,
    );
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-last-proof-unset");
    assert_xval_corrupt(&dir, &damaged, "stands with no last proof");
}

#[test]
fn xval_last_proof_standing_where_only_post_kill_proof_stands() {
    let base = TestDir::new("xval-last-proof-postkill-base");
    let valid = base_continued_killed(&base);
    let damaged = valid.replacen(
        "\"last_proof_at\": null",
        "\"last_proof_at\": \"2031-05-04T12:00:04+00:00\"",
        1,
    );
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-last-proof-postkill");
    assert_xval_corrupt(&dir, &damaged, "holds no live proof record");
}

#[test]
fn xval_spend_total_without_live_spend_record() {
    let base = TestDir::new("xval-spend-ghost-base");
    let valid = base_warn(&base);
    let damaged = valid.replacen("\"spend_total\": 0", "\"spend_total\": 40", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-spend-ghost");
    assert_xval_corrupt(&dir, &damaged, "no live spend record");
}

#[test]
fn xval_clear_streak_above_one_refused() {
    let base = TestDir::new("xval-streak-two-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen(
        "\"clear_streak\": 0,\n      \"killed\": false",
        "\"clear_streak\": 2,\n      \"killed\": false",
        1,
    );
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-streak-two");
    assert_xval_corrupt(&dir, &damaged, "no standing clear streak stands above 1");
}

#[test]
fn xval_clear_streak_nonzero_at_kill_refused() {
    let base = TestDir::new("xval-streak-kill-base");
    let valid = base_killed_by_pull(&base);
    let damaged = valid.replacen(
        "\"clear_streak\": 0,\n      \"killed\": true",
        "\"clear_streak\": 1,\n      \"killed\": true",
        1,
    );
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-streak-kill");
    assert_xval_corrupt(&dir, &damaged, "holds a clear streak of 0");
}

#[test]
fn xval_stage_record_naming_operator_pull_refused() {
    let base = TestDir::new("xval-op-pull-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen("\"soft ceiling\"", "\"operator pull\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-op-pull");
    assert_xval_corrupt(&dir, &damaged, "names operator pull");
}

#[test]
fn xval_stage_record_reasons_out_of_order_refused() {
    let base = TestDir::new("xval-reason-order-base");
    let valid = base_mid_both_sides(&base);
    let damaged = set_reasons(&valid, 4, "[\"missed proofs\", \"soft ceiling\"]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-reason-order");
    assert_xval_corrupt(&dir, &damaged, "out of the order");
}

#[test]
fn xval_stage_record_naming_missed_proofs_with_none_missed() {
    let base = TestDir::new("xval-missed-named-base");
    let valid = valid_document(&base);
    let damaged = set_reasons(&valid, 3, "[\"soft ceiling\", \"missed proofs\"]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-missed-named");
    assert_xval_corrupt(
        &dir,
        &damaged,
        "names missed proofs where no whole proof interval",
    );
}

#[test]
fn xval_stage_record_missing_missed_proofs_where_missed_stand() {
    let base = TestDir::new("xval-missed-missing-base");
    let valid = base_mid_both_sides(&base);
    let damaged = set_reasons(&valid, 2, "[]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-missed-missing");
    assert_xval_corrupt(
        &dir,
        &damaged,
        "names no missed proofs where the missed proof count",
    );
}

#[test]
fn xval_stage_record_naming_ceiling_spend_cannot_reach() {
    let base = TestDir::new("xval-ceiling-unreached-base");
    let valid = valid_document(&base);
    let damaged = valid.replacen("\"soft ceiling\"", "\"hard ceiling\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-ceiling-unreached");
    assert_xval_corrupt(&dir, &damaged, "cannot reach it");
}

#[test]
fn xval_stage_record_missing_ceiling_spend_reaches() {
    let base = TestDir::new("xval-ceiling-missing-base");
    let valid = valid_document(&base);
    let damaged = set_reasons(&valid, 3, "[]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-ceiling-missing");
    assert_xval_corrupt(&dir, &damaged, "names no soft ceiling");
}

#[test]
fn xval_stage_record_naming_ceiling_the_run_does_not_hold() {
    let base = TestDir::new("xval-ceiling-unheld-base");
    let valid = base_warn(&base);
    let damaged = set_reasons(&valid, 1, "[\"soft ceiling\", \"missed proofs\"]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-ceiling-unheld");
    assert_xval_corrupt(&dir, &damaged, "where the run holds no soft ceiling");
}

#[test]
fn xval_stage_record_at_a_stage_no_trigger_warrants() {
    let base = TestDir::new("xval-unwarranted-base");
    let valid = base_warn(&base);
    let damaged = replace_nth(&valid, "\"stage\": \"warn\"", "\"stage\": \"throttle\"", 1);
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-unwarranted");
    assert_xval_corrupt(&dir, &damaged, "cannot warrant for this run");
}

#[test]
fn xval_step_down_record_where_no_warrant_falls_below() {
    let base = TestDir::new("xval-step-down-warrant-base");
    let valid = base_step_down(&base);
    let damaged = replace_last(
        &valid,
        "\"at\": \"2031-05-04T12:03:01+00:00\"",
        "\"at\": \"2031-05-04T12:08:01+00:00\"",
    );
    let damaged = set_reasons(&damaged, 3, "[\"missed proofs\"]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-step-down-warrant");
    assert_xval_corrupt(&dir, &damaged, "warrant no stage below");
}

#[test]
fn xval_stage_record_naming_ceiling_below_its_warrant() {
    let base = TestDir::new("xval-ceiling-warrant-base");
    let valid = base_mid_both_sides(&base);
    let damaged = set_reasons(&valid, 2, "[\"soft ceiling\", \"missed proofs\"]");
    assert_ne!(damaged, valid);
    let dir = TestDir::new("xval-ceiling-warrant");
    assert_xval_corrupt(&dir, &damaged, "cannot warrant for this run");
}

/// Boundary control. The stage record of the mid ladder base
/// stands between live spend records on both sides, so the
/// spend standing at it is a range the soft ceiling falls
/// inside. Naming the soft ceiling there bounds the spend to
/// the ceiling itself, and standing at cut budget on that
/// bound is a state the writers could have written with the
/// whole spend standing before the record and nothing after
/// it. It reads clean.
#[test]
fn xval_undetermined_ceiling_reason_reads_clean() {
    let base = TestDir::new("xval-undetermined-base");
    let valid = base_mid_both_sides(&base);
    let varied = set_reasons(&valid, 2, "[\"soft ceiling\", \"missed proofs\"]");
    let varied = replace_nth(
        &varied,
        "\"stage\": \"throttle\"",
        "\"stage\": \"cut budget\"",
        0,
    );
    let varied = replace_nth(
        &varied,
        "\"stage\": \"throttle\"",
        "\"stage\": \"cut budget\"",
        0,
    );
    assert_ne!(varied, valid);
    let dir = TestDir::new("xval-undetermined");
    std::fs::write(state_path(&dir), &varied).unwrap();
    let switch = make_switch(&dir);
    switch.log(&text("run-a")).expect("the log reads clean");
    assert_eq!(state_bytes(&dir), varied.as_bytes());
}

/// Scale pin for the linearity the amendment states. The log
/// stands in the adversarial arrangement for the spend range
/// question, with 32000 stage records standing before 32000
/// live spend records in one run, so a reader that rescans
/// the live spend positions for every stage record pays for
/// every pair. Every record is valid and the state reads
/// clean. The bound is 12 seconds for the one joined read in
/// a debug test build, set from measurements on both readers
/// recorded in the gate report, where the reader that
/// rescans per record took about 21 seconds on this
/// arrangement and the repaired reader took about 7.
#[test]
fn xval_many_stage_records_before_many_live_spends_read_clean_in_linear_time() {
    let stage_records = 32_000usize;
    let spend_records = 32_000usize;
    let mut document = String::with_capacity((stage_records + spend_records) * 320);
    document.push_str(
        "{\n  \"format\": \"deadman-state/1\",\n  \"runs\": [\n    {\n      \
         \"id\": \"run-a\",\n      \
         \"registered_at\": \"2031-05-04T12:00:00+00:00\",\n      \
         \"proof_interval_seconds\": 60,\n      \"missed_proof_limit\": 5,\n      \
         \"soft_ceiling\": null,\n      \"hard_ceiling\": null,\n      \
         \"spend_total\": 0,\n      \"last_proof_at\": null,\n      \
         \"stage\": \"warn\",\n      \"clear_streak\": 0,\n      \
         \"killed\": false,\n      \"log\": [\n",
    );
    let mut seq = 0usize;
    let mut push_record =
        |document: &mut String, kind: &str, at: &str, stage: &str, reasons: &str| {
            seq += 1;
            document.push_str(&format!(
                "        {{\n          \"seq\": {seq},\n          \
             \"kind\": \"{kind}\",\n          \"at\": \"{at}\",\n          \
             \"stage\": \"{stage}\",\n          \"reasons\": {reasons},\n          \
             \"model_name\": null,\n          \"model_confidence\": null,\n          \
             \"fallback_taken\": null\n        }},\n"
            ));
        };
    push_record(
        &mut document,
        "registered",
        "2031-05-04T12:00:00+00:00",
        "none",
        "[]",
    );
    for _ in 0..stage_records {
        push_record(
            &mut document,
            "stage",
            "2031-05-04T12:01:01+00:00",
            "warn",
            "[\"missed proofs\"]",
        );
    }
    for _ in 0..spend_records {
        push_record(
            &mut document,
            "spend",
            "2031-05-04T12:00:00+00:00",
            "warn",
            "[]",
        );
    }
    let last_comma = document.rfind("},\n").expect("the records stand");
    document.replace_range(last_comma..last_comma + 2, "}\n");
    document.push_str("      ]\n    }\n  ]\n}\n");
    let dir = TestDir::new("xval-scale");
    std::fs::write(state_path(&dir), &document).unwrap();
    let switch = make_switch(&dir);
    let started = std::time::Instant::now();
    let records = switch.log(&text("run-a")).expect("the log reads clean");
    let elapsed = started.elapsed();
    assert_eq!(records.len(), stage_records + spend_records + 1);
    assert!(
        elapsed < std::time::Duration::from_secs(12),
        "the joined read took {elapsed:?}, over the 12 second bound the \
         linear reader meets and the per record rescan exceeds"
    );
    assert_eq!(state_bytes(&dir), document.as_bytes());
}

/// Lifecycle control. Writer produced states read clean at
/// every step of two lives, through a ceiling of zero, out of
/// order proofs, a standing clear streak of one, step downs,
/// a pull, and no operation records on a closed run.
#[test]
fn xval_lifecycle_states_read_clean_at_every_step() {
    let dir = TestDir::new("xval-lifecycle");
    let switch = make_switch(&dir);
    let clean = |run_id: &str| {
        switch
            .log(&text(run_id))
            .expect("a writer produced state reads clean");
    };
    register_full(&switch, "run-a", &t0(), 60, 5, Some(0), Some(100));
    clean("run-a");
    switch.stage(&text("run-a"), &at(0)).unwrap();
    clean("run-a");
    switch.spend(&text("run-a"), &int(0), &at(1)).unwrap();
    clean("run-a");
    switch.spend(&text("run-a"), &int(50), &at(2)).unwrap();
    clean("run-a");
    switch
        .prove(&text("run-a"), &text("working"), &at(10))
        .unwrap();
    clean("run-a");
    switch
        .prove(&text("run-a"), &text("older"), &at(5))
        .unwrap();
    clean("run-a");
    switch.stage(&text("run-a"), &at(10)).unwrap();
    clean("run-a");
    switch.pull(&text("run-a"), &at(11)).unwrap();
    clean("run-a");
    switch
        .prove(&text("run-a"), &text("late"), &at(12))
        .unwrap();
    clean("run-a");
    switch.spend(&text("run-a"), &int(10), &at(13)).unwrap();
    clean("run-a");
    register(&switch, "run-b", &t0());
    clean("run-b");
    switch.stage(&text("run-b"), &at(61)).unwrap();
    clean("run-b");
    switch.stage(&text("run-b"), &at(62)).unwrap();
    clean("run-b");
    switch
        .prove(&text("run-b"), &text("working"), &at(62))
        .unwrap();
    clean("run-b");
    switch.stage(&text("run-b"), &at(62)).unwrap();
    clean("run-b");
    let standing = state_text(&dir);
    assert!(
        standing.contains("\"clear_streak\": 1"),
        "the lifecycle reaches a standing clear streak of one"
    );
    switch.stage(&text("run-b"), &at(62)).unwrap();
    clean("run-b");
    clean("run-a");
}

/// Fixture control. Every shipped fixture reads clean under
/// the joined reader, and the empty fixture reads as an empty
/// document whose runs stand not found.
#[test]
fn xval_shipped_fixtures_read_clean() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(&fixtures)
        .expect("the fixtures folder stands")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 10);
    for name in names {
        let dir = TestDir::new(&format!("xval-fixture-{name}"));
        let raw = std::fs::read(fixtures.join(&name)).expect("the fixture reads");
        std::fs::write(state_path(&dir), &raw).unwrap();
        let switch = make_switch(&dir);
        if name == "empty.json" {
            assert_not_found(switch.log(&text("run-a")));
        } else {
            switch
                .log(&text("run-a"))
                .unwrap_or_else(|err| panic!("fixture {name} reads clean, got {err:?}"));
        }
        assert_eq!(state_bytes(&dir), raw);
    }
}

/// The worker entry the race pins reenter this test binary
/// through. With no mode in the environment it stands as a
/// plain passing test and does nothing.
#[test]
fn race_worker() {
    let Ok(mode) = std::env::var("DEADMAN_RACE_MODE") else {
        return;
    };
    let state = std::env::var("DEADMAN_RACE_STATE").expect("the state path stands");
    let instant = parse_instant(&std::env::var("DEADMAN_RACE_AT").expect("the instant stands"))
        .expect("the instant parses");
    let switch = Switch::new(PathBuf::from(state));
    let run = text("run-a");
    match mode.as_str() {
        "spend" => {
            for _ in 0..5 {
                switch.spend(&run, &int(10), &instant).unwrap();
            }
        }
        "prove" => switch.prove(&run, &text("working"), &instant).unwrap(),
        "stage" => {
            let report = switch.stage(&run, &instant).unwrap();
            let out = std::env::var("DEADMAN_RACE_OUT").expect("the out path stands");
            std::fs::write(out, report.stage).unwrap();
        }
        _ => unreachable!(),
    }
}

fn spawn_workers(dir: &TestDir, mode: &str, count: usize, instant: &Instant) -> Vec<String> {
    let mut children = Vec::new();
    let mut out_paths = Vec::new();
    for index in 0..count {
        let out_path = dir.path().join(format!("worker-out-{index}.txt"));
        let child = Command::new(std::env::current_exe().expect("the test binary stands"))
            .args(["race_worker", "--exact", "--nocapture"])
            .env("DEADMAN_RACE_MODE", mode)
            .env("DEADMAN_RACE_STATE", state_path(dir))
            .env("DEADMAN_RACE_AT", format_instant(instant).unwrap())
            .env("DEADMAN_RACE_OUT", &out_path)
            .spawn()
            .expect("the worker spawns");
        children.push(child);
        out_paths.push(out_path);
    }
    let mut results = Vec::new();
    for (mut child, out_path) in children.into_iter().zip(out_paths) {
        let status = child.wait().expect("the worker is waited on");
        assert!(status.success(), "the worker exits clean");
        results.push(std::fs::read_to_string(out_path).unwrap_or_default());
    }
    results
}

#[test]
fn parallel_spend_and_prove_lose_no_updates() {
    let dir = TestDir::new("parallel-spend-prove");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    spawn_workers(&dir, "spend", 6, &at(10));
    assert!(state_text(&dir).contains("\"spend_total\": 300"));
    spawn_workers(&dir, "prove", 6, &at(20));
    let records = switch.log(&text("run-a")).unwrap();
    let seqs: Vec<u64> = records.iter().map(|record| record.seq).collect();
    let expected: Vec<u64> = (1..=records.len() as u64).collect();
    assert_eq!(seqs, expected);
    assert_eq!(
        records
            .iter()
            .filter(|record| record.kind == "spend")
            .count(),
        30
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.kind == "proof")
            .count(),
        6
    );
    assert!(state_text(&dir).contains("\"spend_total\": 300"));
}

#[test]
fn parallel_evaluations_at_one_kill_instant_record_one_outcome() {
    let dir = TestDir::new("parallel-kill");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    let outputs = spawn_workers(&dir, "stage", 6, &at(301));
    assert!(outputs.iter().all(|out| out == "kill"));
    let records = switch.log(&text("run-a")).unwrap();
    let stage_records: Vec<&Record> = records
        .iter()
        .filter(|record| record.kind == "stage")
        .collect();
    assert_eq!(stage_records.len(), 1);
    assert_eq!(stage_records[0].stage, "kill");
    assert!(state_text(&dir).contains("\"killed\": true"));
}

#[test]
fn parallel_evaluations_at_one_escalation_instant_agree() {
    let dir = TestDir::new("parallel-escalation");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(10), Some(100));
    switch.spend(&text("run-a"), &int(10), &at(1)).unwrap();
    let outputs = spawn_workers(&dir, "stage", 6, &at(2));
    assert!(outputs.iter().all(|out| out == "cut budget"));
    let records = switch.log(&text("run-a")).unwrap();
    let seqs: Vec<u64> = records.iter().map(|record| record.seq).collect();
    let expected: Vec<u64> = (1..=records.len() as u64).collect();
    assert_eq!(seqs, expected);
    let stage_records: Vec<&Record> = records
        .iter()
        .filter(|record| record.kind == "stage")
        .collect();
    assert_eq!(stage_records.len(), 6);
    assert!(stage_records
        .iter()
        .all(|record| record.stage == "cut budget"));
}

#[test]
fn cli_walks_a_run_through_its_life() {
    let dir = TestDir::new("cli-life");
    let state = state_path(&dir).to_string_lossy().into_owned();
    let (stdout, _) = run_cli(
        &[
            "--state",
            &state,
            "register",
            "run-a",
            "--hard-ceiling",
            "10",
        ],
        0,
    );
    assert_eq!(stdout.trim(), "registered run-a");
    let (stdout, _) = run_cli(
        &["--state", &state, "prove", "run-a", "--note", "working"],
        0,
    );
    assert_eq!(stdout.trim(), "proved run-a");
    let (stdout, _) = run_cli(&["--state", &state, "spend", "run-a", "--amount", "10"], 0);
    assert_eq!(stdout.trim(), "spend_total 10");
    let (stdout, _) = run_cli(&["--state", &state, "stage", "run-a"], 0);
    assert_eq!(stdout.lines().next(), Some("stage kill"));
    assert!(stdout.contains("reason hard ceiling"));
    let (stdout, _) = run_cli(&["--state", &state, "log", "run-a"], 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].starts_with("1 registered"));
    assert!(lines[lines.len() - 1].starts_with("4 stage"));
    let (stdout, _) = run_cli(&["--state", &state, "pull", "run-a"], 0);
    assert_eq!(stdout.trim(), "stage kill");
}

#[test]
fn cli_register_defaults_land_in_the_stored_document() {
    let dir = TestDir::new("cli-register-defaults");
    let state = state_path(&dir).to_string_lossy().into_owned();
    run_cli(&["--state", &state, "register", "run-a"], 0);
    let document = state_text(&dir);
    assert!(
        document.contains("\"proof_interval_seconds\": 60"),
        "{document}"
    );
    assert!(document.contains("\"missed_proof_limit\": 5"), "{document}");
}

#[test]
fn cli_carries_no_time_override_flag() {
    let dir = TestDir::new("cli-no-override");
    let state = state_path(&dir).to_string_lossy().into_owned();
    run_cli(&["--state", &state, "register", "run-a"], 0);
    let (_, stderr) = run_cli(
        &["--state", &state, "stage", "run-a", "--now", "2031-01-01"],
        2,
    );
    assert!(stderr.contains("unrecognized"), "{stderr}");
}

#[test]
fn cli_error_exits_and_messages() {
    let dir = TestDir::new("cli-errors");
    let state = state_path(&dir).to_string_lossy().into_owned();
    let (_, stderr) = run_cli(&["--state", &state, "stage", "ghost"], 1);
    assert!(stderr.starts_with("error:"), "{stderr}");
    assert!(!stderr.contains("Traceback"));
    let (_, stderr) = run_cli(&["--state", &state, "register", "Bad Name"], 2);
    assert!(stderr.starts_with("error:"), "{stderr}");
    run_cli(&["--state", &state, "register", "run-a"], 0);
    let (_, stderr) = run_cli(&["--state", &state, "register", "run-a"], 1);
    assert!(stderr.starts_with("error:"), "{stderr}");
    let (_, stderr) = run_cli(
        &[
            "--state",
            &state,
            "register",
            "run-b",
            "--soft-ceiling",
            "9",
            "--hard-ceiling",
            "8",
        ],
        2,
    );
    assert!(stderr.starts_with("error:"), "{stderr}");
}

#[test]
fn a_stored_instant_with_a_non_ascii_separator_reads() {
    // The Python instant reader takes any single character as
    // the separator between the date and the time, including a
    // character outside ASCII. A stored spelling that uses one
    // reads as the instant it names, evaluates, and stands
    // unchanged when the state is published again.
    let dir = TestDir::new("non-ascii-separator");
    let state = state_path(&dir);
    let switch = Switch::new(state.clone());
    switch
        .register(
            &text("run-a"),
            &t0(),
            &int(60),
            &int(5),
            &Value::Null,
            &Value::Null,
        )
        .unwrap();
    let written =
        state_text(&dir).replace("2031-05-04T12:00:00+00:00", "2031-05-04é12:00:00+00:00");
    std::fs::write(&state, &written).unwrap();
    let report = switch.stage(&text("run-a"), &at(61)).unwrap();
    assert_eq!(report.stage, "warn");
    // The spelling stands in its escaped written form, as the
    // writer escapes every character outside ASCII.
    assert!(state_text(&dir).contains("2031-05-04\\u00e912:00:00+00:00"));
}

#[test]
fn cli_rejects_an_explicit_value_on_the_help_option() {
    let dir = TestDir::new("help-value");
    let state = state_path(&dir).to_string_lossy().into_owned();
    let (_, stderr) = run_cli(&["--help=x"], 2);
    assert!(
        stderr.contains("argument -h/--help: ignored explicit argument 'x'"),
        "{stderr}"
    );
    let (_, stderr) = run_cli(&["--state", &state, "register", "--help=x"], 2);
    assert!(
        stderr.contains("argument -h/--help: ignored explicit argument 'x'"),
        "{stderr}"
    );
}

#[test]
fn cli_addresses_a_state_path_that_is_not_utf8() {
    use std::os::unix::ffi::OsStringExt;
    let dir = TestDir::new("non-utf8-state");
    let raw_name = std::ffi::OsString::from_vec(b"state-\xff.json".to_vec());
    let state = dir.path().join(raw_name);
    let output = Command::new(env!("CARGO_BIN_EXE_deadman"))
        .arg("--state")
        .arg(&state)
        .args(["register", "run-a"])
        .output()
        .expect("the binary runs");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).into_owned(),
        "registered run-a\n"
    );
    assert!(state.exists());
    let document = std::fs::read_to_string(&state).unwrap();
    assert!(document.contains("\"format\": \"deadman-state/1\""));
}

// ----------------------------------------------------------------
// The escalation slice pins of Round 3, obligations DM-13 to
// DM-20. Every consultation runs through a fixture transport or
// a scripted fake helper, and no test performs a network call
// or spawns the real helper.

fn verdict(verdict: Verdict, confidence: f64, score: f64) -> Consultation {
    Consultation::Verdict {
        model_name: MODEL_NAME.to_string(),
        verdict,
        confidence,
        score,
    }
}

fn fixture_transport(replies: Vec<Consultation>) -> FixtureTransport {
    FixtureTransport::new(MODEL_NAME, replies)
}

/// A run standing at warn with the held seat open, raised by
/// one missed proof at 61 seconds and evaluated again at the
/// same instant, where the warrant stands at the stage.
fn switch_at_warn(dir: &TestDir) -> Switch {
    let switch = make_switch(dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(61)).unwrap();
    switch
}

/// A run standing at throttle with the fall count at 1 and the
/// fall seat armed, a rise at 121 seconds, a proof at 121, and
/// one clean evaluation at 122.
fn switch_at_throttle_fall_seat(dir: &TestDir) -> Switch {
    let switch = make_switch(dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(121)).unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(121))
        .unwrap();
    switch.stage(&text("run-a"), &at(122)).unwrap();
    switch
}

fn last_record(switch: &Switch) -> Record {
    switch
        .log(&text("run-a"))
        .unwrap()
        .pop()
        .expect("a record stands")
}

#[test]
fn model_bars_stand_as_named_constants() {
    assert_eq!(ESCALATION_BAR, 0.60);
    assert_eq!(DE_ESCALATION_BAR, 0.85);
    assert!(DE_ESCALATION_BAR > ESCALATION_BAR);
    assert_eq!(MODEL_NAME, "jev-1.13.0");
}

#[test]
fn held_seat_escalate_at_the_bar_rises_one_step() {
    let dir = TestDir::new("model-held-escalate-at-bar");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.60, 0.5)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "throttle");
    assert_eq!(transport.shown().len(), 1);
    let record = last_record(&switch);
    assert_eq!(record.kind, "stage");
    assert_eq!(record.stage, "throttle");
    assert_eq!(record.model_name.as_deref(), Some(MODEL_NAME));
    assert_eq!(record.model_verdict.as_deref(), Some("escalate"));
    assert_eq!(record.model_confidence, Some(0.60));
    assert_eq!(record.model_score, Some(0.5));
    assert_eq!(record.fallback_taken, None);
    let document = state_text(&dir);
    assert!(document.contains("\"model_verdict\": \"escalate\""));
    assert!(document.contains("\"model_score\": 0.5"));
}

#[test]
fn held_seat_escalate_below_the_bar_holds_and_records_the_verdict() {
    let dir = TestDir::new("model-held-escalate-below-bar");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.59, 0.5)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    let record = last_record(&switch);
    assert_eq!(record.stage, "warn");
    assert_eq!(record.model_verdict.as_deref(), Some("escalate"));
    assert_eq!(record.model_confidence, Some(0.59));
    // A verdict under its bar is no fallback class, and the
    // record says so, so the log distinguishes a model that
    // answered and was refused from one that never answered.
    assert_eq!(record.fallback_taken, None);
}

#[test]
fn held_seat_de_escalate_at_full_confidence_holds() {
    let dir = TestDir::new("model-held-de-escalate");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::DeEscalate, 0.99, 0.1)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    let record = last_record(&switch);
    assert_eq!(record.model_verdict.as_deref(), Some("de-escalate"));
    assert_eq!(record.model_confidence, Some(0.99));
    assert_eq!(record.fallback_taken, None);
}

#[test]
fn held_seat_hold_verdict_holds() {
    let dir = TestDir::new("model-held-hold");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Hold, 0.9, 0.4)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    let record = last_record(&switch);
    assert_eq!(record.model_verdict.as_deref(), Some("hold"));
    assert_eq!(record.model_score, Some(0.4));
}

#[test]
fn fall_seat_de_escalate_at_the_bar_steps_down() {
    let dir = TestDir::new("model-fall-at-bar");
    let switch = switch_at_throttle_fall_seat(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::DeEscalate, 0.85, 0.2)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(123), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    let record = last_record(&switch);
    assert_eq!(record.stage, "warn");
    assert_eq!(record.model_verdict.as_deref(), Some("de-escalate"));
    assert_eq!(record.model_confidence, Some(0.85));
    assert!(state_text(&dir).contains("\"clear_streak\": 0"));
}

#[test]
fn fall_seat_de_escalate_below_the_bar_holds_and_spends_the_streak() {
    let dir = TestDir::new("model-fall-below-bar");
    let switch = switch_at_throttle_fall_seat(&dir);
    let before = switch.log(&text("run-a")).unwrap().len();
    let transport = fixture_transport(vec![verdict(Verdict::DeEscalate, 0.84, 0.2)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(123), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "throttle");
    // The unendorsed completion appends its hold record and
    // spends the streak, so the fall must be re-earned.
    assert_eq!(switch.log(&text("run-a")).unwrap().len(), before + 1);
    let record = last_record(&switch);
    assert_eq!(record.stage, "throttle");
    assert_eq!(record.model_verdict.as_deref(), Some("de-escalate"));
    assert!(state_text(&dir).contains("\"clear_streak\": 0"));
    let report = switch.stage(&text("run-a"), &at(124)).unwrap();
    assert_eq!(report.stage, "throttle");
    assert!(state_text(&dir).contains("\"clear_streak\": 1"));
}

#[test]
fn fall_seat_fallback_spends_the_streak_and_a_fall_re_earns() {
    let dir = TestDir::new("model-fall-fallback-spend");
    let switch = switch_at_throttle_fall_seat(&dir);
    let transport = fixture_transport(vec![
        Consultation::Silence,
        verdict(Verdict::DeEscalate, 0.90, 0.2),
    ]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(123), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "throttle");
    let record = last_record(&switch);
    assert_eq!(record.fallback_taken.as_deref(), Some("silence"));
    assert_eq!(record.model_name.as_deref(), Some(MODEL_NAME));
    assert_eq!(record.model_verdict, None);
    assert_eq!(record.model_confidence, None);
    assert_eq!(record.model_score, None);
    assert!(state_text(&dir).contains("\"clear_streak\": 0"));
    // The next clean evaluation consults nothing, because the
    // streak was spent and the seat stands only at a streak
    // of 1, and it re-earns the count.
    let report = switch
        .stage_with_model(&text("run-a"), &at(124), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "throttle");
    assert_eq!(transport.shown().len(), 1);
    assert!(state_text(&dir).contains("\"clear_streak\": 1"));
    let report = switch
        .stage_with_model(&text("run-a"), &at(125), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    assert_eq!(transport.shown().len(), 2);
}

#[test]
fn abstention_holds_at_both_seats_and_at_cut_tools() {
    // The held seat at warn.
    let dir = TestDir::new("model-abstain-warn");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![Consultation::Abstention {
        model_name: MODEL_NAME.to_string(),
    }]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    assert_eq!(
        last_record(&switch).fallback_taken.as_deref(),
        Some("abstention")
    );
    // The fall seat at throttle spends the streak.
    let dir = TestDir::new("model-abstain-throttle");
    let switch = switch_at_throttle_fall_seat(&dir);
    let transport = fixture_transport(vec![Consultation::Abstention {
        model_name: MODEL_NAME.to_string(),
    }]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(123), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "throttle");
    assert_eq!(
        last_record(&switch).fallback_taken.as_deref(),
        Some("abstention")
    );
    assert!(state_text(&dir).contains("\"clear_streak\": 0"));
    // The held seat at cut tools.
    let dir = TestDir::new("model-abstain-cut-tools");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(181)).unwrap();
    let transport = fixture_transport(vec![Consultation::Abstention {
        model_name: MODEL_NAME.to_string(),
    }]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(181), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "cut tools");
    assert_eq!(
        last_record(&switch).fallback_taken.as_deref(),
        Some("abstention")
    );
}

#[test]
fn call_failure_records_its_class_at_the_held_seat() {
    let dir = TestDir::new("model-call-failed");
    let switch = switch_at_warn(&dir);
    // An empty fixture transport answers call failed, the
    // class a script that has run out has honestly earned.
    let transport = fixture_transport(vec![]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    let record = last_record(&switch);
    assert_eq!(record.fallback_taken.as_deref(), Some("call failed"));
    assert_eq!(record.model_name.as_deref(), Some(MODEL_NAME));
}

#[test]
fn model_escalation_from_cut_tools_lands_at_cut_budget_and_never_kill() {
    let dir = TestDir::new("model-escalate-cut-tools");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(181)).unwrap();
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 1.00, 0.9)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(181), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "cut budget");
    assert!(state_text(&dir).contains("\"killed\": false"));
    let report = switch.stage(&text("run-a"), &at(182)).unwrap();
    assert_eq!(report.stage, "cut budget");
}

#[test]
fn consultations_stand_only_at_the_seats() {
    let dir = TestDir::new("model-seats-only");
    let switch = make_switch(&dir);
    let transport = fixture_transport(vec![
        verdict(Verdict::Hold, 0.9, 0.5),
        verdict(Verdict::Hold, 0.9, 0.5),
        verdict(Verdict::Hold, 0.9, 0.5),
        verdict(Verdict::Hold, 0.9, 0.5),
    ]);
    register(&switch, "run-a", &t0());
    // A rise evaluation consults nothing.
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(transport.shown().len(), 0);
    // A quiet evaluation at none consults nothing.
    register(&switch, "run-b", &t0());
    switch
        .stage_with_model(&text("run-b"), &at(10), Some(&transport), None)
        .unwrap();
    assert_eq!(transport.shown().len(), 0);
    // A clean evaluation at a streak of 0 consults nothing,
    // because the fall seat stands only at a streak of 1.
    switch
        .prove(&text("run-a"), &text("working"), &at(61))
        .unwrap();
    switch
        .stage_with_model(&text("run-a"), &at(62), Some(&transport), None)
        .unwrap();
    assert_eq!(transport.shown().len(), 0);
    // The fall seat armed consults exactly once.
    switch
        .stage_with_model(&text("run-a"), &at(63), Some(&transport), None)
        .unwrap();
    assert_eq!(transport.shown().len(), 1);
    // Cut budget stands outside the middle stages, at its
    // rise and at its hold alike.
    register_full(&switch, "run-c", &t0(), 60, 5, Some(100), None);
    switch.spend(&text("run-c"), &int(100), &at(1)).unwrap();
    switch
        .stage_with_model(&text("run-c"), &at(2), Some(&transport), None)
        .unwrap();
    switch
        .stage_with_model(&text("run-c"), &at(3), Some(&transport), None)
        .unwrap();
    assert_eq!(transport.shown().len(), 1);
    // A kill evaluation and a closed run consult nothing.
    register_full(&switch, "run-d", &t0(), 60, 5, None, Some(10));
    switch.spend(&text("run-d"), &int(10), &at(1)).unwrap();
    switch
        .stage_with_model(&text("run-d"), &at(2), Some(&transport), None)
        .unwrap();
    switch.pull(&text("run-d"), &at(3)).unwrap();
    switch
        .stage_with_model(&text("run-d"), &at(4), Some(&transport), None)
        .unwrap();
    assert_eq!(transport.shown().len(), 1);
    assert_eq!(transport.remaining(), 3);
}

#[test]
fn stale_evaluations_earn_no_fall_credit() {
    let dir = TestDir::new("model-stale-no-credit");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(121)).unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(121))
        .unwrap();
    // Two clean evaluations stamped before the reference
    // instant of 121 advance nothing and append nothing.
    for stale in [100, 110] {
        let report = switch.stage(&text("run-a"), &at(stale)).unwrap();
        assert_eq!(report.stage, "throttle");
        assert_eq!(switch.log(&text("run-a")).unwrap().len(), 3);
        assert!(state_text(&dir).contains("\"clear_streak\": 0"));
    }
    // A fresh pair after them steps the stage down, the first
    // advancing the count and the second completing it.
    switch.stage(&text("run-a"), &at(122)).unwrap();
    assert!(state_text(&dir).contains("\"clear_streak\": 1"));
    let report = switch.stage(&text("run-a"), &at(123)).unwrap();
    assert_eq!(report.stage, "warn");
}

#[test]
fn a_stale_evaluation_neither_completes_nor_resets_the_streak() {
    let dir = TestDir::new("model-stale-no-complete");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(121)).unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(121))
        .unwrap();
    switch.stage(&text("run-a"), &at(122)).unwrap();
    assert!(state_text(&dir).contains("\"clear_streak\": 1"));
    // A stale clean evaluation leaves the honestly earned
    // count standing, neither completing the fall nor
    // destroying the progress toward it.
    let report = switch.stage(&text("run-a"), &at(100)).unwrap();
    assert_eq!(report.stage, "throttle");
    assert!(state_text(&dir).contains("\"clear_streak\": 1"));
    let report = switch.stage(&text("run-a"), &at(123)).unwrap();
    assert_eq!(report.stage, "warn");
}

#[test]
fn a_stale_evaluation_applies_its_rises() {
    let dir = TestDir::new("model-stale-rise");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(100), None);
    switch.stage(&text("run-a"), &at(61)).unwrap();
    switch.spend(&text("run-a"), &int(100), &at(62)).unwrap();
    // The evaluation stamped at 50 seconds stands before the
    // reference instant of 61, and the spend warrant it
    // computes is instant independent, so the rise applies
    // and its record appends stamped as it stands.
    let report = switch.stage(&text("run-a"), &at(50)).unwrap();
    assert_eq!(report.stage, "cut budget");
    let record = last_record(&switch);
    assert_eq!(record.kind, "stage");
    assert_eq!(record.stage, "cut budget");
    assert_eq!(record.at, "2031-05-04T12:00:50+00:00");
    assert_eq!(record.reasons, ["soft ceiling"]);
}

const EXPECTED_PREAMBLE: &str = "You are judging one evaluation of a staged kill switch for an agent run. The switch stands at one stage of a ladder, none and warn and throttle and cut tools and cut budget and kill, and it moves a run up the ladder to protect the systems around it and down the ladder as danger passes. Deterministic rules own the spend ceilings and the kill and the first move into warn, and none of that is before you. You judge only a run standing at warn or throttle or cut tools, and you answer with one verdict among hold and escalate and de-escalate, with a confidence, and with a score for how likely the run is to be running away.\n\nThe fall rules of the switch bind your verdict. A stage falls only after two consecutive clean evaluations, never after one, however clean the one looks. A stage entered for structural reasons, a planned spend profile that is proceeding to plan among them, is not relaxed because the current picture is tidy. Recommend de-escalate only where the signals that raised the stage have actually cleared and the fall has two consecutive clean evaluations behind it, because where fewer stand, hold is the truthful verdict even when nothing currently looks wrong. Recommend escalate only where the run should stand one stage higher than it stands, never more, because an escalation takes effect one step at a time. Where the evidence cuts both ways, hold. Where you cannot judge the picture before you, abstain rather than guess.";

#[test]
fn the_vignette_stands_byte_for_byte_at_the_held_seat() {
    let dir = TestDir::new("model-vignette-held");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Hold, 0.9, 0.5)]);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    let shown = transport.shown();
    assert_eq!(shown.len(), 1);
    let expected = format!(
        "{EXPECTED_PREAMBLE}\n\nThe state of the run at this evaluation stands as follows.\n{}",
        [
            "The run stands at stage warn.",
            "The triggers at this instant warrant stage warn.",
            "The live reasons are missed proofs.",
            "The missed proof count stands at 1 and the missed proof limit stands at 5 and the proof interval stands at 60 seconds.",
            "No proof stands, and the registration stands at an age of 61 whole seconds.",
            "The accumulated spend stands at 0.",
            "No soft ceiling stands.",
            "No hard ceiling stands.",
            "The standing clear streak stands at 0.",
            "The last stage records in sequence order stand as follows.",
            "A stage record at stage warn naming missed proofs stands at an age of 0 whole seconds.",
            "Since the run entered stage warn it has recorded 0 proof records and 0 spend records.",
            "No proof note stands recorded in the decision log.",
        ]
        .join("\n")
    );
    assert_eq!(shown[0], expected);
    // The run id and the confidence bars never cross.
    assert!(!shown[0].contains("run-a"));
    assert!(!shown[0].contains("0.60"));
    assert!(!shown[0].contains("0.85"));
}

#[test]
fn the_vignette_at_the_fall_seat_with_a_host_context() {
    let dir = TestDir::new("model-vignette-fall");
    let switch = make_switch(&dir);
    register_full(&switch, "run-a", &t0(), 60, 5, Some(1000), None);
    switch.spend(&text("run-a"), &int(40), &at(5)).unwrap();
    switch.stage(&text("run-a"), &at(121)).unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(121))
        .unwrap();
    switch.stage(&text("run-a"), &at(122)).unwrap();
    let transport = fixture_transport(vec![verdict(Verdict::Hold, 0.9, 0.5)]);
    let context = "Planned build window, spend is structural.";
    switch
        .stage_with_model(&text("run-a"), &at(123), Some(&transport), Some(context))
        .unwrap();
    let shown = transport.shown();
    assert_eq!(shown.len(), 1);
    let expected = format!(
        "{EXPECTED_PREAMBLE}\n\nThe state of the run at this evaluation stands as follows.\n{}\n\nHost supplied context follows. It is marked as host supplied and it is recorded nowhere.\n{context}",
        [
            "The run stands at stage throttle.",
            "The triggers at this instant warrant stage none.",
            "No trigger stands live at this instant.",
            "The missed proof count stands at 0 and the missed proof limit stands at 5 and the proof interval stands at 60 seconds.",
            "The last proof stands at an age of 2 whole seconds.",
            "The accumulated spend stands at 40.",
            "The soft ceiling stands at 1000 and the spend stands below it by 960.",
            "No hard ceiling stands.",
            "The standing clear streak stands at 1.",
            "The last stage records in sequence order stand as follows.",
            "A stage record at stage throttle naming missed proofs stands at an age of 2 whole seconds.",
            "Since the run entered stage throttle it has recorded 1 proof record and 0 spend records.",
            "No proof note stands recorded in the decision log.",
        ]
        .join("\n")
    );
    assert_eq!(shown[0], expected);
    // The context is never written into the state document.
    assert!(!state_text(&dir).contains("Planned build window"));
}

#[test]
fn host_context_refusals_change_nothing() {
    let dir = TestDir::new("model-context-refusals");
    let switch = switch_at_warn(&dir);
    let before = state_bytes(&dir);
    // A context with no transport to carry it.
    assert_invalid_request(switch.stage_with_model(
        &text("run-a"),
        &at(61),
        None,
        Some("context"),
    ));
    assert_eq!(state_bytes(&dir), before);
    // A context past the bound of 2000 characters.
    let transport = fixture_transport(vec![verdict(Verdict::Hold, 0.9, 0.5)]);
    let long = "x".repeat(2001);
    assert_invalid_request(switch.stage_with_model(
        &text("run-a"),
        &at(61),
        Some(&transport),
        Some(&long),
    ));
    assert_eq!(transport.shown().len(), 0);
    assert_eq!(state_bytes(&dir), before);
    // A context at the bound is carried.
    let exact = "x".repeat(2000);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), Some(&exact))
        .unwrap();
    assert_eq!(transport.shown().len(), 1);
    assert!(transport.shown()[0].ends_with(&exact));
}

#[test]
fn a_transport_pinned_outside_the_recordable_shape_is_refused() {
    let dir = TestDir::new("model-pinned-shape");
    let switch = switch_at_warn(&dir);
    let before = state_bytes(&dir);
    let empty = FixtureTransport::new("", vec![verdict(Verdict::Hold, 0.9, 0.5)]);
    assert_invalid_request(switch.stage_with_model(
        &text("run-a"),
        &at(61),
        Some(&empty),
        None,
    ));
    let wide = FixtureTransport::new(&"x".repeat(129), vec![verdict(Verdict::Hold, 0.9, 0.5)]);
    assert_invalid_request(switch.stage_with_model(
        &text("run-a"),
        &at(61),
        Some(&wide),
        None,
    ));
    assert_eq!(state_bytes(&dir), before);
}

#[test]
fn an_out_of_shape_verdict_lands_as_a_malformed_reply() {
    // A confidence past the unit interval.
    let dir = TestDir::new("model-verdict-shape-confidence");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 1.5, 0.5)]);
    let report = switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(report.stage, "warn");
    let record = last_record(&switch);
    assert_eq!(record.fallback_taken.as_deref(), Some("malformed reply"));
    assert_eq!(record.model_name.as_deref(), Some(MODEL_NAME));
    // A score that is not a number in the interval.
    let dir = TestDir::new("model-verdict-shape-score");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.9, f64::NAN)]);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    assert_eq!(
        last_record(&switch).fallback_taken.as_deref(),
        Some("malformed reply")
    );
}

#[test]
fn model_evaluation_is_deterministic_on_the_same_state_and_reply() {
    let dir = TestDir::new("model-deterministic");
    let first = Switch::new(dir.path().join("first.json"));
    let second = Switch::new(dir.path().join("second.json"));
    for switch in [&first, &second] {
        register(switch, "run-a", &t0());
        switch.stage(&text("run-a"), &at(61)).unwrap();
    }
    let first_transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.9, 0.5)]);
    let second_transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.9, 0.5)]);
    let report_first = first
        .stage_with_model(&text("run-a"), &at(61), Some(&first_transport), None)
        .unwrap();
    let report_second = second
        .stage_with_model(&text("run-a"), &at(61), Some(&second_transport), None)
        .unwrap();
    assert_eq!(report_first, report_second);
    assert_eq!(
        std::fs::read(dir.path().join("first.json")).unwrap(),
        std::fs::read(dir.path().join("second.json")).unwrap()
    );
}

#[test]
fn no_transport_model_evaluation_matches_the_stage_operation_byte_for_byte() {
    let dir = TestDir::new("model-no-transport-equivalence");
    let plain = Switch::new(dir.path().join("plain.json"));
    let model = Switch::new(dir.path().join("model.json"));
    for switch in [&plain, &model] {
        register_full(switch, "run-a", &t0(), 60, 5, Some(100), Some(1000));
        switch.spend(&text("run-a"), &int(40), &at(1)).unwrap();
    }
    // One scripted life driven through both entry points,
    // rises and holds and a stale evaluation and a fall and
    // a ceiling rise among the steps.
    for instant in [121, 122, 100, 123, 125, 126] {
        let plain_report = plain.stage(&text("run-a"), &at(instant)).unwrap();
        let model_report = model
            .stage_with_model(&text("run-a"), &at(instant), None, None)
            .unwrap();
        assert_eq!(plain_report, model_report, "at {instant}");
        if instant == 121 {
            for switch in [&plain, &model] {
                switch
                    .prove(&text("run-a"), &text("working"), &at(121))
                    .unwrap();
            }
        }
        if instant == 123 {
            for switch in [&plain, &model] {
                switch.spend(&text("run-a"), &int(60), &at(124)).unwrap();
            }
        }
    }
    assert_eq!(
        std::fs::read(dir.path().join("plain.json")).unwrap(),
        std::fs::read(dir.path().join("model.json")).unwrap()
    );
}

/// Write a state text into a fresh directory and read its log,
/// expecting the corruption refusal, with the bytes standing
/// unchanged after the refused read.
fn assert_corrupt_state_text(text: &str) {
    let dir = TestDir::new("model-damage");
    std::fs::write(state_path(&dir), text).expect("the damaged state is written");
    let switch = make_switch(&dir);
    assert_corrupt(switch.log(&text("run-a")));
    assert_eq!(state_text(&dir), text);
}

/// Split a state text at the first record of a kind, so a
/// damage lands on that record and no other.
fn split_at_kind(text: &str, kind: &str) -> (String, String) {
    let needle = format!("\"kind\": \"{kind}\"");
    let position = text.find(&needle).expect("the record kind stands");
    (
        text[..position].to_string(),
        text[position..].to_string(),
    )
}

/// Split a state text at the last record of a kind.
fn split_at_last_kind(text: &str, kind: &str) -> (String, String) {
    let needle = format!("\"kind\": \"{kind}\"");
    let position = text.rfind(&needle).expect("the record kind stands");
    (
        text[..position].to_string(),
        text[position..].to_string(),
    )
}

#[test]
fn a_step_down_record_stands_under_the_floor_of_dm19() {
    let dir = TestDir::new("model-floor-damage");
    let switch = make_switch(&dir);
    register(&switch, "run-a", &t0());
    switch.stage(&text("run-a"), &at(121)).unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(121))
        .unwrap();
    switch.stage(&text("run-a"), &at(122)).unwrap();
    switch.stage(&text("run-a"), &at(123)).unwrap();
    let valid = state_text(&dir);
    assert!(valid.contains("\"at\": \"2031-05-04T12:02:03+00:00\""));
    // The step down restamped below the reference instant its
    // preceding records establish, the proof and the rise at
    // 121 seconds, is refused by the join.
    let damaged = valid.replace(
        "\"at\": \"2031-05-04T12:02:03+00:00\"",
        "\"at\": \"2031-05-04T12:00:30+00:00\"",
    );
    assert_corrupt_state_text(&damaged);
}

#[test]
fn model_record_damage_battery() {
    // A verdict pattern standing in the file, written by a
    // consultation at the held seat.
    let dir = TestDir::new("model-damage-base");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::DeEscalate, 0.99, 0.5)]);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    let verdict_state = state_text(&dir);
    // A verdict pattern missing its score stands in no
    // recorded pattern.
    assert_corrupt_state_text(&verdict_state.replace(
        "\"model_score\": 0.5",
        "\"model_score\": null",
    ));
    // A confidence past the unit interval.
    assert_corrupt_state_text(&verdict_state.replace(
        "\"model_confidence\": 0.99",
        "\"model_confidence\": 1.5",
    ));
    // A verdict outside the recorded names.
    assert_corrupt_state_text(&verdict_state.replace(
        "\"model_verdict\": \"de-escalate\"",
        "\"model_verdict\": \"sideways\"",
    ));
    // A fallback pattern carrying a verdict.
    let (head, tail) = split_at_last_kind(&verdict_state, "stage");
    let damaged = format!(
        "{head}{}",
        tail.replacen(
            "\"fallback_taken\": null",
            "\"fallback_taken\": \"silence\"",
            1
        )
    );
    assert_corrupt_state_text(&damaged);
    // Model fields standing on a proof record.
    let dir = TestDir::new("model-damage-proof");
    let switch = switch_at_throttle_fall_seat(&dir);
    let proof_state = state_text(&dir);
    let (head, tail) = split_at_kind(&proof_state, "proof");
    let damaged = format!(
        "{head}{}",
        tail.replacen(
            "\"fallback_taken\": null",
            "\"fallback_taken\": null, \"model_verdict\": null, \"model_score\": null",
            1
        )
    );
    assert_corrupt_state_text(&damaged);
    // A rise one step above the warrant with its model fields
    // nulled stands behind no consultation and is refused.
    let dir = TestDir::new("model-damage-rise");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.9, 0.5)]);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    let rise_state = state_text(&dir);
    let (head, tail) = split_at_kind(&rise_state, "stage");
    let tail = tail
        .replacen("\"model_name\": \"jev-1.13.0\"", "\"model_name\": null", 1)
        .replacen("\"model_verdict\": \"escalate\"", "\"model_verdict\": null", 1)
        .replacen("\"model_confidence\": 0.9", "\"model_confidence\": null", 1)
        .replacen("\"model_score\": 0.5", "\"model_score\": null", 1);
    assert_corrupt_state_text(&format!("{head}{tail}"));
    // A step down carrying de-escalate below the bar.
    let dir = TestDir::new("model-damage-step-down");
    let switch = switch_at_throttle_fall_seat(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::DeEscalate, 0.90, 0.2)]);
    switch
        .stage_with_model(&text("run-a"), &at(123), Some(&transport), None)
        .unwrap();
    let step_state = state_text(&dir);
    assert_corrupt_state_text(&step_state.replace(
        "\"model_confidence\": 0.9",
        "\"model_confidence\": 0.84",
    ));
    // A fallback class outside the closed vocabulary.
    let dir = TestDir::new("model-damage-fallback");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![Consultation::Silence]);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    let silence_state = state_text(&dir);
    assert_corrupt_state_text(&silence_state.replace(
        "\"fallback_taken\": \"silence\"",
        "\"fallback_taken\": \"quiet\"",
    ));
}

fn write_helper(dir: &TestDir, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("the helper is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the helper is executable");
    path
}

fn subprocess(program: PathBuf, timeout: std::time::Duration) -> SubprocessTransport {
    SubprocessTransport::new(program, Vec::new(), timeout)
}

#[test]
fn subprocess_transport_reads_a_verdict_and_removes_its_payload() {
    let dir = TestDir::new("model-subprocess-verdict");
    let marker = dir.path().join("payload-path.txt");
    let body = format!(
        "#!/bin/sh\ngrep -q \"jev-1.13.0\" \"$2\" || exit 3\ngrep -q \"The run stands at stage\" \"$2\" || exit 3\nprintf '%s' \"$2\" > \"{}\"\nprintf '%s' '{{\"model\": \"jev-1.13.0\", \"choice\": {{\"label\": \"escalate\", \"confidence\": 0.9}}, \"score\": {{\"level\": 7}}}}'\n",
        marker.display()
    );
    let helper = write_helper(&dir, "verdict.sh", &body);
    let transport = subprocess(helper, std::time::Duration::from_secs(10));
    let outcome = transport.consult("The run stands at stage warn.");
    assert_eq!(
        outcome,
        Consultation::Verdict {
            model_name: MODEL_NAME.to_string(),
            verdict: Verdict::Escalate,
            confidence: 0.9,
            score: 6.0 / 9.0,
        }
    );
    // The payload file the helper was handed is removed when
    // the call ends.
    let payload_path = std::fs::read_to_string(&marker).unwrap();
    assert!(!Path::new(&payload_path).exists());
}

#[test]
fn subprocess_transport_classes() {
    let dir = TestDir::new("model-subprocess-classes");
    let cases: Vec<(&str, Consultation)> = vec![
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": null}}'\n",
            Consultation::Abstention {
                model_name: MODEL_NAME.to_string(),
            },
        ),
        ("#!/bin/sh\nexit 0\n", Consultation::Silence),
        ("#!/bin/sh\nexit 2\n", Consultation::CallFailed),
        ("#!/bin/sh\nprintf 'not json'\n", Consultation::MalformedReply),
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": null}}{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": null}}'\n",
            Consultation::MalformedReply,
        ),
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"other-1.0\", \"choice\": {\"label\": null}}'\n",
            Consultation::MalformedReply,
        ),
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": \"hold\", \"confidence\": 1.5}, \"score\": {\"level\": 5}}'\n",
            Consultation::MalformedReply,
        ),
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": \"hold\", \"confidence\": 0.5}, \"score\": {\"level\": 11}}'\n",
            Consultation::MalformedReply,
        ),
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": \"hold\", \"confidence\": 0.5}, \"score\": {\"level\": 1}}'\n",
            Consultation::Verdict {
                model_name: MODEL_NAME.to_string(),
                verdict: Verdict::Hold,
                confidence: 0.5,
                score: 0.0,
            },
        ),
        (
            "#!/bin/sh\nprintf '%s' '{\"model\": \"jev-1.13.0\", \"choice\": {\"label\": \"hold\", \"confidence\": 0.5}, \"score\": {\"level\": 10}}'\n",
            Consultation::Verdict {
                model_name: MODEL_NAME.to_string(),
                verdict: Verdict::Hold,
                confidence: 0.5,
                score: 1.0,
            },
        ),
    ];
    for (index, (script, expected)) in cases.into_iter().enumerate() {
        let helper = write_helper(&dir, &format!("case-{index}.sh"), script);
        let transport = subprocess(helper, std::time::Duration::from_secs(10));
        assert_eq!(transport.consult("vignette"), expected, "case {index}");
    }
    // A program that does not stand is a call failure.
    let transport = subprocess(
        dir.path().join("no-such-helper"),
        std::time::Duration::from_secs(10),
    );
    assert_eq!(transport.consult("vignette"), Consultation::CallFailed);
}

#[test]
fn subprocess_transport_enforces_its_deadline() {
    let dir = TestDir::new("model-subprocess-deadline");
    let helper = write_helper(&dir, "slow.sh", "#!/bin/sh\nsleep 5\n");
    let transport = subprocess(helper, std::time::Duration::from_secs(1));
    let started = std::time::Instant::now();
    let outcome = transport.consult("vignette");
    assert_eq!(outcome, Consultation::CallFailed);
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn subprocess_transport_bounds_the_reply() {
    let dir = TestDir::new("model-subprocess-bounds");
    let helper = write_helper(
        &dir,
        "wide.sh",
        "#!/bin/sh\nhead -c 70000 /dev/zero | tr '\\0' 'x'\n",
    );
    let transport = subprocess(helper, std::time::Duration::from_secs(10));
    assert_eq!(transport.consult("vignette"), Consultation::MalformedReply);
}

#[test]
fn cli_stage_and_log_carry_no_model_surface() {
    let dir = TestDir::new("model-cli-surface");
    let switch = switch_at_warn(&dir);
    let transport = fixture_transport(vec![verdict(Verdict::Escalate, 0.9, 0.5)]);
    switch
        .stage_with_model(&text("run-a"), &at(61), Some(&transport), None)
        .unwrap();
    let state = state_path(&dir).to_string_lossy().into_owned();
    let (stdout, _) = run_cli(&["--state", &state, "stage", "run-a"], 0);
    assert_eq!(stdout.lines().next(), Some("stage throttle"));
    let (stdout, _) = run_cli(&["--state", &state, "log", "run-a"], 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[lines.len() - 1].starts_with("3 stage"));
    assert!(!stdout.contains("model"));
}
