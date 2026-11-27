//! Integration pins for obligation DM-27 covering docs/GUIDE.md,
//! verified README.md installation commands, examples/quickstart.rs,
//! and examples/supervisor_loop.py.

#[path = "../examples/quickstart.rs"]
mod quickstart;

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn find_python() -> &'static str {
    for candidate in ["python3", "python"] {
        if Command::new(candidate)
            .args(["-c", "import sys"])
            .status()
            .is_ok_and(|status| status.success())
        {
            return candidate;
        }
    }
    panic!("neither python3 nor python is runnable on PATH");
}

fn strip_markdown_link_targets(line: &str) -> String {
    let mut current = line.to_string();
    loop {
        let mut out = String::with_capacity(current.len());
        let bytes = current.as_bytes();
        let mut i = 0;
        let mut changed = false;
        while i < bytes.len() {
            let bang = bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[';
            let open = if bang { i + 1 } else { i };
            if bytes[open] == b'[' {
                if let Some(rel_close) = current[open + 1..].find(']') {
                    let close = open + 1 + rel_close;
                    let label = &current[open + 1..close];
                    if !label.contains('[')
                        && !label.contains(']')
                        && close + 1 < bytes.len()
                        && bytes[close + 1] == b'('
                    {
                        if let Some(rel_end) = current[close + 2..].find(')') {
                            out.push_str(label);
                            i = close + 2 + rel_end + 1;
                            changed = true;
                            continue;
                        }
                    }
                }
            }
            let ch = current[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        if !changed {
            return out;
        }
        current = out;
    }
}

fn strip_inline_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for ch in line.chars() {
        if ch == '`' {
            in_code = !in_code;
        } else if !in_code {
            out.push(ch);
        }
    }
    out
}

fn is_table_delimiter_row(trimmed: &str) -> bool {
    let inner = trimmed.trim_matches('|').trim();
    !inner.is_empty()
        && inner.split('|').all(|cell| {
            let c = cell.trim();
            c.contains('-') && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ' | '\t'))
        })
}

fn check_prose_violations(text: &str) -> Vec<String> {
    let mut violations = Vec::new();
    if text.contains('\u{2014}') {
        violations.push("em dash U+2014".to_string());
    }
    if text.contains('\u{2013}') {
        violations.push("en dash U+2013".to_string());
    }
    let mut in_fence = false;
    let mut fence_marker = String::new();
    for (idx, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let ch = trimmed.chars().next().unwrap();
            let count = trimmed.chars().take_while(|c| *c == ch).count();
            let marker: String = std::iter::repeat_n(ch, count).collect();
            if !in_fence {
                in_fence = true;
                fence_marker = marker;
            } else if trimmed.starts_with(&fence_marker) {
                in_fence = false;
            }
            continue;
        }
        if in_fence || trimmed.starts_with("<!--") {
            continue;
        }
        let cleaned = if trimmed.starts_with('|') {
            if is_table_delimiter_row(trimmed) {
                continue;
            }
            strip_markdown_link_targets(&strip_inline_code_spans(raw))
        } else {
            strip_markdown_link_targets(raw)
        };
        if cleaned.contains(':') {
            violations.push(format!("line {}: colon in prose: {raw}", idx + 1));
        }
        if cleaned.contains(';') {
            violations.push(format!("line {}: semicolon in prose: {raw}", idx + 1));
        }
    }
    violations
}

fn assert_clean_prose(path: &Path, text: &str) {
    let violations = check_prose_violations(text);
    assert!(
        violations.is_empty(),
        "{} failed prose check: {violations:?}",
        path.display()
    );
}

fn assert_sanitized(path: &Path, text: &str) {
    for forbidden in ["/Users/", "/home/david", "@gmail.com"] {
        assert!(
            !text.contains(forbidden),
            "{} contains unsanitized marker {forbidden:?}",
            path.display()
        );
    }
}

#[test]
fn prose_scanner_rejects_colons_semicolons_and_table_prose_punctuation() {
    assert!(!check_prose_violations("a\u{2014}b").is_empty());
    assert!(!check_prose_violations("a\u{2013}b").is_empty());
    assert!(!check_prose_violations("Plain sentence: bad.").is_empty());
    assert!(!check_prose_violations("Plain sentence; bad.").is_empty());
    assert!(!check_prose_violations("| col | cell: bad |\n").is_empty());
    assert!(!check_prose_violations("| col | cell; bad |\n").is_empty());
    assert!(
        check_prose_violations(
            "Clean [guide](https://example.test/a:b).\n| col | desc |\n| --- | :---: |\n| `a:b` | clean cell |\n"
        )
        .is_empty()
    );
}

#[test]
fn operator_guide_and_contract_amendment_satisfy_dm27() {
    let contract_path = repo_root().join("CONTRACT.md");
    let contract = std::fs::read_to_string(&contract_path).expect("CONTRACT.md exists");
    let (_, dm27_section) = contract
        .split_once(
            "## Operator guide, runnable quickstarters, and verified install paths amendment",
        )
        .expect("DM-27 amendment section present");
    assert_clean_prose(&contract_path, dm27_section);
    assert_sanitized(&contract_path, dm27_section);

    let guide_path = repo_root().join("docs/GUIDE.md");
    let guide = std::fs::read_to_string(&guide_path).expect("docs/GUIDE.md exists");
    assert_clean_prose(&guide_path, &guide);
    assert_sanitized(&guide_path, &guide);
    assert!(
        !guide.contains("pip install arvin"),
        "docs/GUIDE.md must not instruct bare pip install arvin"
    );

    for needle in [
        "deadman-state/1",
        "proof_interval_seconds",
        "missed_proof_limit",
        "base + proof_interval_seconds",
        "soft_ceiling",
        "hard_ceiling",
        "cut tools",
        "cut budget",
        "Two-evaluation fall hysteresis",
        "Latched `kill`",
        "arvin --state runs.json runs",
        "--judge-arg=--provider",
        "jev-1.14.0",
        "gemini-decision-1",
        "gpt-6-luna",
        "pplx-decider-v1.3",
        "@cf/cloudflare/clef",
        "examples/decision_judge.py",
        "examples/quickstart.rs",
        "examples/supervisor_loop.py",
        "portcullis",
        "deadbolt",
        "mayfly",
    ] {
        assert!(guide.contains(needle), "docs/GUIDE.md missing {needle:?}");
    }
}

#[test]
fn readme_install_and_guides_sections_are_verified() {
    let readme_path = repo_root().join("README.md");
    let readme = std::fs::read_to_string(&readme_path).expect("README.md exists");
    assert_clean_prose(&readme_path, &readme);
    assert_sanitized(&readme_path, &readme);

    for required in [
        "cargo install arvin --bin arvin",
        "cargo install --git https://github.com/david-engelmann/arvin.git --tag v0.2.0 --bin arvin",
        "https://github.com/david-engelmann/arvin/releases/tag/v0.2.0",
        "arvin-v0.2.0-x86_64-unknown-linux-gnu.tar.gz",
        "arvin-0.2.0-cp39-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
        "pip install \"git+https://github.com/david-engelmann/arvin.git@v0.2.0\"",
        "cargo install --path . --bin arvin",
        "[docs/GUIDE.md](docs/GUIDE.md)",
        "[examples/quickstart.rs](examples/quickstart.rs)",
        "[examples/supervisor_loop.py](examples/supervisor_loop.py)",
    ] {
        assert!(readme.contains(required), "README.md missing {required:?}");
    }
    assert!(
        !readme.contains("pip install arvin"),
        "README.md must not instruct bare pip install arvin while unpublished on PyPI"
    );
    for line in readme.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("cargo install ") {
            assert!(
                trimmed.contains("--bin arvin"),
                "cargo install line missing --bin arvin: {trimmed}"
            );
        }
    }
}

#[test]
fn rust_quickstart_example_runs_end_to_end() {
    assert_sanitized(
        &repo_root().join("examples/quickstart.rs"),
        &std::fs::read_to_string(repo_root().join("examples/quickstart.rs")).unwrap(),
    );
    quickstart::run_quickstart().expect("examples/quickstart.rs runs end to end");
}

#[test]
fn python_supervisor_loop_walkthrough_runs_end_to_end() {
    let script = repo_root().join("examples/supervisor_loop.py");
    assert_sanitized(&script, &std::fs::read_to_string(&script).unwrap());

    let output = Command::new(find_python())
        .arg(&script)
        .arg("--arvin-bin")
        .arg(env!("CARGO_BIN_EXE_arvin"))
        .output()
        .expect("spawn examples/supervisor_loop.py");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "supervisor_loop.py failed ({:?}): stdout={stdout} stderr={stderr}",
        output.status.code()
    );
    for needle in [
        "registered loop-run",
        "proved loop-run",
        "stage warn",
        "reason missed proofs",
        "loop-run stage=warn killed=false",
        "stage kill",
        "verified 4 log records",
    ] {
        assert!(
            stdout.contains(needle),
            "supervisor_loop.py stdout missing {needle:?}: {stdout}"
        );
    }
}
