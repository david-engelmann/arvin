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
    format_instant, parse_instant, Error, Instant, Record, Switch, Value, FORMAT_NAME, STAGES,
    TRIGGERS,
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
