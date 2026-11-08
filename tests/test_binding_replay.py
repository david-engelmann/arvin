"""The recorded escalation corpus replayed through the binding.

The binding equivalence pin of Round 5, the DM-20 amendment.
Every one of the 48 recorded cases of the escalation corpus
stands rebuilt through binding calls alone, at the ISO
instants the recorded offsets name from the corpus anchor,
and judged twice on twin documents, once through the fixture
dictionary the manifest record describes and once through a
scripted helper standing as the judge program, with the
recorded reply body as its answer. The two routes must land
byte identical state files, the reports must name the stage
the manifest records, the record the evaluation appends must
carry the five model fields and the stage the manifest
records, and the payload bytes the helper was handed must
equal the recorded request bytes of the case. The corpus
files are read and never written.

The fixture dictionary translation this pin relies on reads
the manifest record directly. A model verdict in the record
becomes the verdict form, and a fallback class in the record
becomes the fallback form, so the pin covers the dictionary
forms the manifest exercises and would cover a fallback case
the day the corpus gains one.
"""

import json
import shutil
from datetime import datetime, timedelta, timezone
from pathlib import Path

import arvin

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "escalation"
ANCHOR = datetime(2031, 5, 4, 12, 0, 0, tzinfo=timezone.utc)


def instant(offset):
    return (ANCHOR + timedelta(seconds=offset)).isoformat()


def load_corpus():
    corpus = json.loads((FIXTURES / "corpus.json").read_text())
    manifest = json.loads((FIXTURES / "manifest.json").read_text())
    entries = {entry["id"]: entry for entry in manifest["cases"]}
    return corpus["cases"], entries


def build_document(state, case):
    register = case["register"]
    arvin.register(
        state,
        register["run_id"],
        instant(0),
        proof_interval=register["proof_interval_seconds"],
        missed_limit=register["missed_proof_limit"],
        soft_ceiling=register["soft_ceiling"],
        hard_ceiling=register["hard_ceiling"],
    )
    for op in case["ops"]:
        at = instant(op["at"])
        if op["op"] == "prove":
            arvin.prove(state, register["run_id"], at, note=op.get("note", ""))
        elif op["op"] == "spend":
            arvin.spend(state, register["run_id"], op["amount"], at)
        elif op["op"] == "stage":
            arvin.stage(state, register["run_id"], at)
        else:
            raise AssertionError(f"unknown op {op['op']}")


def fixture_for(recorded):
    if recorded["model_verdict"] is not None:
        return {
            "model_name": recorded["model_name"],
            "verdict": recorded["model_verdict"],
            "confidence": recorded["model_confidence"],
            "score": recorded["model_score"],
        }
    return {
        "model_name": recorded["model_name"],
        "fallback": recorded["fallback_taken"],
    }


def write_helper(directory, reply_text):
    capture = directory / "captured-payload.json"
    reply = directory / "reply.json"
    reply.write_text(reply_text)
    helper = directory / "helper.sh"
    helper.write_text(f'#!/bin/sh\ncp "$2" "{capture}"\ncat "{reply}"\n')
    helper.chmod(0o755)
    return str(helper), capture


def test_recorded_cases_replay_through_the_binding(tmp_path):
    cases, entries = load_corpus()
    assert len(cases) == 48
    shell = shutil.which("sh")
    assert shell is not None
    for case in cases:
        entry = entries[case["id"]]
        recorded = entry["recorded"]
        run_id = case["register"]["run_id"]
        at = instant(case["judge"]["at"])
        context = case["judge"]["context"]
        case_dir = tmp_path / case["id"].lower()
        case_dir.mkdir()
        fixture_state = str(case_dir / "fixture.json")
        subprocess_state = str(case_dir / "subprocess.json")
        build_document(fixture_state, case)
        build_document(subprocess_state, case)

        fixture_report = arvin.stage_with_model(
            fixture_state,
            run_id,
            at,
            fixture=fixture_for(recorded),
            host_context=context,
        )
        reply_text = (FIXTURES / "replies" / f"{case['id']}.json").read_text()
        helper, capture = write_helper(case_dir, reply_text)
        subprocess_report = arvin.stage_with_model(
            subprocess_state,
            run_id,
            at,
            judge=helper,
            host_context=context,
        )

        assert fixture_report == subprocess_report, case["id"]
        assert fixture_report["stage"] == entry["stage_after"], case["id"]
        assert Path(fixture_state).read_bytes() == Path(
            subprocess_state
        ).read_bytes(), case["id"]
        last = arvin.log(fixture_state, run_id)[-1]
        for field in (
            "model_name",
            "model_verdict",
            "model_confidence",
            "model_score",
            "fallback_taken",
        ):
            assert last[field] == recorded[field], (case["id"], field)
        assert last["stage"] == entry["stage_after"], case["id"]
        expected_payload = (
            FIXTURES / "requests" / f"{case['id']}.json"
        ).read_bytes()
        assert capture.read_bytes() == expected_payload, case["id"]


def test_the_fixture_route_replays_identically_twice(tmp_path):
    cases, entries = load_corpus()
    case = cases[0]
    recorded = entries[case["id"]]["recorded"]
    run_id = case["register"]["run_id"]
    landings = []
    for name in ("first", "second"):
        state = str(tmp_path / f"{name}.json")
        build_document(state, case)
        arvin.stage_with_model(
            state,
            run_id,
            instant(case["judge"]["at"]),
            fixture=fixture_for(recorded),
            host_context=case["judge"]["context"],
        )
        landings.append(Path(state).read_bytes())
    assert landings[0] == landings[1]
