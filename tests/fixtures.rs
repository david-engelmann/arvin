//! Equivalence checks over the frozen state fixtures.
//!
//! Every fixture in tests/fixtures was written by the Python
//! implementation of Round 1, and the manifest beside them
//! records their hashes, their production operations, and their
//! truths. This suite checks the hashes, reads each fixture and
//! compares its decision log and its stage at the recorded
//! probe instant against the record, replays every production
//! operation under Rust and compares the written state byte for
//! byte, and continues three fixtures under Rust and compares
//! the resulting log against the continuation files the Python
//! implementation produced. The continuation comparison stood
//! on bytes until the Round 3 amendment retired the byte form
//! by name, as its text states, and the form here is the
//! reading half that amendment keeps, the documents operating
//! to the recorded log under the extended reader.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use deadman::{Error, Instant, Switch, Value};
use sha2::{Digest, Sha256};

fn t0() -> Instant {
    Instant::from_civil(2031, 5, 4, 12, 0, 0, 0, Some(0)).expect("the test instant is in range")
}

fn at(seconds: i64) -> Instant {
    t0().plus_seconds(seconds)
}

fn text(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn int(value: i64) -> Value {
    Value::Int(value as i128)
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> TestDir {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "deadman-fixtures-{}-{name}-{unique}",
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

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(name)).expect("the fixture stands")
}

fn fixture_copy(dir: &TestDir, name: &str) -> Switch {
    let path = dir.path().join("state.json");
    std::fs::write(&path, fixture_bytes(name)).expect("the copy is written");
    Switch::new(path)
}

fn log_lines(switch: &Switch, run_id: &str) -> Vec<String> {
    switch
        .log(&text(run_id))
        .unwrap()
        .iter()
        .map(|record| {
            let mut line = format!(
                "{} {} at={} stage={}",
                record.seq, record.kind, record.at, record.stage
            );
            for reason in &record.reasons {
                line.push_str(&format!(" reason={reason}"));
            }
            line
        })
        .collect()
}

const INVENTORY: [(&str, &str, usize); 10] = [
    (
        "continued-killed.json",
        "0d63bdb471b518351c91987a13f4deefa196d1749a4cc6e7619d7da91a5e5d2d",
        1766,
    ),
    (
        "continued-mid-ladder.json",
        "6c74595ca2163fedf1c64fcfecc342b1a2de3e283208a9d296bf2a474871233c",
        1857,
    ),
    (
        "continued-warn.json",
        "9c9e899c7164baccb4ca679a6e20fea9a7c59b267099f312148ac89257d2365a",
        1528,
    ),
    (
        "empty.json",
        "00189e79df175357140f313d447b17be5fc15394d566d410cb3aa1bbd3ba4c81",
        48,
    ),
    (
        "full-log.json",
        "dd6b6795475c57ecd27bf21a74c95caa93a1de16ee15d6b58224792c5272b778",
        2506,
    ),
    (
        "killed-by-hard-ceiling.json",
        "eaffc5ff29992a923995da0d7d809baf97de79de8a086b49608dec735d6bb1f6",
        1238,
    ),
    (
        "killed-by-pull.json",
        "beaea47112afe73659c3d9856b8d0e10ceafd6e71dfeb84c9b3c8993840be0cf",
        1262,
    ),
    (
        "mid-ladder.json",
        "b72ef987323b4dd28ed9dd6b27c2e8777ea3ec6e9ae74afe0b1d66d29b39c8a8",
        1249,
    ),
    (
        "registered.json",
        "eb179b0e60e40b3778ae11713cc06688aa002eeb4678fce56e49d1e079be99c1",
        674,
    ),
    (
        "warn.json",
        "6d0687d0cee3862a8ffaed18791a3d8acd622f700513f0228e812f633e0ab8d4",
        977,
    ),
];

#[test]
fn the_fixtures_match_their_hash_inventory() {
    for (name, sha256, bytes) in INVENTORY {
        let raw = fixture_bytes(name);
        assert_eq!(raw.len(), bytes, "{name} stands at its recorded size");
        let digest = Sha256::digest(&raw);
        assert_eq!(
            format!("{digest:x}"),
            sha256,
            "{name} stands at its recorded hash"
        );
    }
}

#[test]
fn fixture_logs_match_the_record() {
    let expected: [(&str, &[&str]); 9] = [
        (
            "registered.json",
            &["1 registered at=2031-05-04T12:00:00+00:00 stage=none"],
        ),
        (
            "warn.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 stage at=2031-05-04T12:01:01+00:00 stage=warn reason=missed proofs",
            ],
        ),
        (
            "mid-ladder.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 spend at=2031-05-04T12:00:01+00:00 stage=none",
                "3 stage at=2031-05-04T12:02:01+00:00 stage=throttle reason=missed proofs",
            ],
        ),
        (
            "killed-by-pull.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 proof at=2031-05-04T12:00:01+00:00 stage=none",
                "3 pull at=2031-05-04T12:00:02+00:00 stage=kill reason=operator pull",
            ],
        ),
        (
            "killed-by-hard-ceiling.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 spend at=2031-05-04T12:00:01+00:00 stage=none",
                "3 stage at=2031-05-04T12:00:02+00:00 stage=kill reason=hard ceiling",
            ],
        ),
        (
            "full-log.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 proof at=2031-05-04T12:00:10+00:00 stage=none",
                "3 spend at=2031-05-04T12:00:11+00:00 stage=none",
                "4 stage at=2031-05-04T12:00:12+00:00 stage=cut budget reason=soft ceiling",
                "5 stage at=2031-05-04T12:03:20+00:00 stage=cut budget reason=soft ceiling reason=missed proofs",
                "6 stage at=2031-05-04T12:03:21+00:00 stage=cut budget reason=soft ceiling reason=missed proofs",
                "7 pull at=2031-05-04T12:03:22+00:00 stage=kill reason=operator pull",
            ],
        ),
        (
            "continued-warn.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 stage at=2031-05-04T12:01:01+00:00 stage=warn reason=missed proofs",
                "3 proof at=2031-05-04T12:01:01+00:00 stage=warn",
                "4 stage at=2031-05-04T12:01:01+00:00 stage=none",
            ],
        ),
        (
            "continued-mid-ladder.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 spend at=2031-05-04T12:00:01+00:00 stage=none",
                "3 stage at=2031-05-04T12:02:01+00:00 stage=throttle reason=missed proofs",
                "4 spend at=2031-05-04T12:02:02+00:00 stage=throttle",
                "5 stage at=2031-05-04T12:02:03+00:00 stage=cut budget reason=soft ceiling reason=missed proofs",
            ],
        ),
        (
            "continued-killed.json",
            &[
                "1 registered at=2031-05-04T12:00:00+00:00 stage=none",
                "2 spend at=2031-05-04T12:00:01+00:00 stage=none",
                "3 stage at=2031-05-04T12:00:02+00:00 stage=kill reason=hard ceiling",
                "4 spend at=2031-05-04T12:00:03+00:00 stage=kill",
                "5 proof at=2031-05-04T12:00:04+00:00 stage=kill",
            ],
        ),
    ];
    for (name, lines) in expected {
        let dir = TestDir::new("fixture-logs");
        let switch = fixture_copy(&dir, name);
        assert_eq!(log_lines(&switch, "run-a"), lines, "{name} log matches");
    }
    // The empty fixture holds no run, and a log read against it
    // is a not found refusal.
    let dir = TestDir::new("fixture-logs-empty");
    let switch = fixture_copy(&dir, "empty.json");
    assert!(matches!(
        switch.log(&text("run-a")),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn fixture_stages_match_the_record() {
    let probes: [(&str, i64, &str, &[&str]); 6] = [
        ("registered.json", 30, "none", &[]),
        ("warn.json", 61, "warn", &["missed proofs"]),
        ("mid-ladder.json", 122, "throttle", &["missed proofs"]),
        ("killed-by-pull.json", 500, "kill", &["missed proofs"]),
        (
            "killed-by-hard-ceiling.json",
            500,
            "kill",
            &["hard ceiling", "missed proofs"],
        ),
        (
            "full-log.json",
            500,
            "kill",
            &["soft ceiling", "missed proofs"],
        ),
    ];
    for (name, probe, stage, reasons) in probes {
        let dir = TestDir::new("fixture-stages");
        let switch = fixture_copy(&dir, name);
        let report = switch.stage(&text("run-a"), &at(probe)).unwrap();
        assert_eq!(report.stage, stage, "{name} stage at the probe");
        assert_eq!(report.reasons, reasons, "{name} reasons at the probe");
    }
}

#[test]
fn fixture_productions_replay_byte_for_byte() {
    // registered: a registration at the base instant.
    let dir = TestDir::new("replay-registered");
    let switch = Switch::new(dir.path().join("state.json"));
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
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("registered.json")
    );
    // warn: registration, then an evaluation at 61 seconds.
    let dir = TestDir::new("replay-warn");
    let switch = Switch::new(dir.path().join("state.json"));
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
    switch.stage(&text("run-a"), &at(61)).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("warn.json")
    );
    // mid-ladder: declared ceilings, a spend, an evaluation.
    let dir = TestDir::new("replay-mid-ladder");
    let switch = Switch::new(dir.path().join("state.json"));
    switch
        .register(
            &text("run-a"),
            &t0(),
            &int(60),
            &int(5),
            &int(100),
            &int(1000),
        )
        .unwrap();
    switch.spend(&text("run-a"), &int(40), &at(1)).unwrap();
    switch.stage(&text("run-a"), &at(121)).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("mid-ladder.json")
    );
    // killed-by-pull: registration, a proof, a pull.
    let dir = TestDir::new("replay-killed-by-pull");
    let switch = Switch::new(dir.path().join("state.json"));
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
    switch
        .prove(&text("run-a"), &text("working"), &at(1))
        .unwrap();
    switch.pull(&text("run-a"), &at(2)).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("killed-by-pull.json")
    );
    // killed-by-hard-ceiling: a ceiling spend and an evaluation.
    let dir = TestDir::new("replay-killed-by-ceiling");
    let switch = Switch::new(dir.path().join("state.json"));
    switch
        .register(
            &text("run-a"),
            &t0(),
            &int(60),
            &int(5),
            &Value::Null,
            &int(10),
        )
        .unwrap();
    switch.spend(&text("run-a"), &int(10), &at(1)).unwrap();
    switch.stage(&text("run-a"), &at(2)).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("killed-by-hard-ceiling.json")
    );
    // full-log: the full recorded life.
    let dir = TestDir::new("replay-full-log");
    let switch = Switch::new(dir.path().join("state.json"));
    switch
        .register(
            &text("run-a"),
            &t0(),
            &int(60),
            &int(5),
            &int(10),
            &int(100),
        )
        .unwrap();
    switch
        .prove(&text("run-a"), &text("working"), &at(10))
        .unwrap();
    switch.spend(&text("run-a"), &int(10), &at(11)).unwrap();
    switch.stage(&text("run-a"), &at(12)).unwrap();
    switch.stage(&text("run-a"), &at(200)).unwrap();
    switch.stage(&text("run-a"), &at(201)).unwrap();
    switch.pull(&text("run-a"), &at(202)).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("full-log.json")
    );
}

#[test]
fn fixture_continuations_operate_to_the_recorded_log() {
    // The warn fixture continued: a proof and two evaluations
    // walk the run back down through the hysteresis.
    let dir = TestDir::new("continue-warn");
    let switch = fixture_copy(&dir, "warn.json");
    switch
        .prove(&text("run-a"), &text("working"), &at(61))
        .unwrap();
    switch.stage(&text("run-a"), &at(61)).unwrap();
    switch.stage(&text("run-a"), &at(61)).unwrap();
    let expected_dir = TestDir::new("continue-warn-expected");
    let expected = fixture_copy(&expected_dir, "continued-warn.json");
    assert_eq!(log_lines(&switch, "run-a"), log_lines(&expected, "run-a"));
    // The mid ladder fixture continued: a spend to the soft
    // ceiling and an evaluation.
    let dir = TestDir::new("continue-mid-ladder");
    let switch = fixture_copy(&dir, "mid-ladder.json");
    switch.spend(&text("run-a"), &int(60), &at(122)).unwrap();
    switch.stage(&text("run-a"), &at(123)).unwrap();
    let expected_dir = TestDir::new("continue-mid-ladder-expected");
    let expected = fixture_copy(&expected_dir, "continued-mid-ladder.json");
    assert_eq!(log_lines(&switch, "run-a"), log_lines(&expected, "run-a"));
    // The killed fixture continued: no operation records on the
    // closed run.
    let dir = TestDir::new("continue-killed");
    let switch = fixture_copy(&dir, "killed-by-hard-ceiling.json");
    switch.spend(&text("run-a"), &int(500), &at(3)).unwrap();
    switch
        .prove(&text("run-a"), &text("late proof"), &at(4))
        .unwrap();
    let expected_dir = TestDir::new("continue-killed-expected");
    let expected = fixture_copy(&expected_dir, "continued-killed.json");
    assert_eq!(log_lines(&switch, "run-a"), log_lines(&expected, "run-a"));
}

#[test]
fn the_empty_fixture_takes_a_registration_under_rust() {
    let dir = TestDir::new("empty-registration");
    let switch = fixture_copy(&dir, "empty.json");
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
    assert_eq!(
        std::fs::read(dir.path().join("state.json")).unwrap(),
        fixture_bytes("registered.json")
    );
}
