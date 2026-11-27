//! Self-contained Rust quickstart for arvin.
//!
//! Exercises registration, progress proof, spend reporting, deterministic
//! stage evaluation, offline System One model consultation (jev-1.14.0),
//! read-only run listing, latched operator pull, and decision log reading
//! inside a temporary directory cleaned up on exit.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use arvin::{
    consultation_from_reply_body, parse_instant, FixtureTransport, Instant, Switch, Value,
    MODEL_NAME,
};

fn ts(text: &str) -> Result<Instant, Box<dyn std::error::Error>> {
    Ok(parse_instant(text).ok_or("invalid RFC 3339 instant")?)
}

struct TempDir(PathBuf);

impl TempDir {
    fn create(prefix: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}"));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run_quickstart() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::create("arvin-quickstart")?;
    let state_path = temp.0.join("state.json");
    let switch = Switch::new(state_path);

    // 1. Register a run with a 30s proof interval and soft/hard spend ceilings.
    let run_id = Value::Str("demo-run".to_string());
    let t0 = ts("2031-05-04T12:00:00+00:00")?;
    switch.register(
        &run_id,
        &t0,
        &Value::Int(30),
        &Value::Int(5),
        &Value::Int(100),
        &Value::Int(200),
    )?;
    println!("1. registered demo-run");

    // 2. Post a progress proof and report spend crossing the soft ceiling.
    let t1 = ts("2031-05-04T12:00:10+00:00")?;
    switch.prove(&run_id, &Value::Str("tests green".to_string()), &t1)?;

    let t2 = ts("2031-05-04T12:00:20+00:00")?;
    let total = switch.spend(&run_id, &Value::Int(150), &t2)?;
    assert_eq!(total, 150);

    let spend_report = switch.stage(&run_id, &t2)?;
    assert_eq!(spend_report.stage, "cut budget");
    assert_eq!(spend_report.reasons, vec!["soft ceiling".to_string()]);
    println!(
        "2. demo-run stage={} reasons={:?}",
        spend_report.stage, spend_report.reasons
    );

    // 3. Demonstrate an offline jev-1.14.0 consultation at the warn held seat.
    let consult_id = Value::Str("consult-run".to_string());
    switch.register(
        &consult_id,
        &t0,
        &Value::Int(30),
        &Value::Int(5),
        &Value::Int(100),
        &Value::Int(200),
    )?;
    let t3 = ts("2031-05-04T12:00:35+00:00")?;
    let warn_report = switch.stage(&consult_id, &t3)?;
    assert_eq!(warn_report.stage, "warn");

    let reply_json = r#"{"model": "jev-1.14.0", "choice": {"label": "escalate", "confidence": 0.77}, "score": {"level": 6}}"#;
    let canned = consultation_from_reply_body(reply_json, MODEL_NAME);
    let transport = FixtureTransport::new(MODEL_NAME, vec![canned]);

    let t4 = ts("2031-05-04T12:00:40+00:00")?;
    let consulted = switch.stage_with_model(
        &consult_id,
        &t4,
        Some(&transport),
        Some("repeated failing build step"),
    )?;
    assert_eq!(consulted.stage, "throttle");
    println!(
        "3. consult-run model={} stage={}",
        MODEL_NAME, consulted.stage
    );

    // 4. Inspect standing runs without mutating state, pull demo-run, and read its log.
    let summaries = switch.runs()?;
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].id, "demo-run");
    assert_eq!(summaries[1].id, "consult-run");
    println!("4. standing runs={}", summaries.len());

    let t5 = ts("2031-05-04T12:00:50+00:00")?;
    let pulled = switch.pull(&run_id, &t5)?;
    assert_eq!(pulled, "kill");

    let records = switch.log(&run_id)?;
    assert_eq!(records.len(), 5);
    assert_eq!(records.last().map(|r| r.stage.as_str()), Some("kill"));
    println!("5. demo-run pulled to kill (log entries={})", records.len());

    Ok(())
}

#[allow(dead_code)]
fn main() -> ExitCode {
    match run_quickstart() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}
