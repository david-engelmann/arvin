"""Tests for the deadman staged cutoff, slice 1.

Deterministic time comes only from injected instants. No test sleeps
to age a proof, and the only wall clock use is the command line smoke
path where no deadline is crossed. Fixtures are synthetic and written
for this repo.
"""

import json
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from deadman import core  # noqa: E402
from deadman.core import (  # noqa: E402
    STAGES,
    TRIGGERS,
    CorruptStateError,
    InvalidNameError,
    InvalidRequestError,
    NotFoundError,
    RefusalError,
    Switch,
)

T0 = datetime(2031, 5, 4, 12, 0, 0, tzinfo=timezone.utc)


def at(seconds):
    return T0 + timedelta(seconds=seconds)


def make_switch(tmp_path):
    return Switch(tmp_path / "state.json")


def read_document(tmp_path):
    return json.loads((tmp_path / "state.json").read_text(encoding="utf-8"))


def read_run(tmp_path, run_id="run-a"):
    document = read_document(tmp_path)
    return next(run for run in document["runs"] if run["id"] == run_id)


def run_cli(*argv, expect=0):
    result = subprocess.run(
        [sys.executable, "-m", "deadman", *argv],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    assert result.returncode == expect, result.stderr
    return result


def test_vocabulary_stands_in_the_contract_order():
    assert STAGES == ["none", "warn", "throttle", "cut tools", "cut budget", "kill"]
    assert TRIGGERS == ["operator pull", "hard ceiling", "soft ceiling", "missed proofs"]
    assert core.FORMAT_NAME == "deadman-state/1"


def test_registration_defaults_and_declared_thresholds(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    run = read_run(tmp_path)
    assert run["proof_interval_seconds"] == 60
    assert run["missed_proof_limit"] == 5
    assert run["soft_ceiling"] is None
    assert run["hard_ceiling"] is None
    assert run["spend_total"] == 0
    assert run["last_proof_at"] is None
    assert run["stage"] == "none"
    assert run["killed"] is False
    other = Switch(tmp_path / "other.json")
    other.register(
        "run-b", T0, proof_interval_seconds=30, missed_proof_limit=3,
        soft_ceiling=40, hard_ceiling=90,
    )
    document = json.loads((tmp_path / "other.json").read_text(encoding="utf-8"))
    declared = document["runs"][0]
    assert declared["proof_interval_seconds"] == 30
    assert declared["missed_proof_limit"] == 3
    assert declared["soft_ceiling"] == 40
    assert declared["hard_ceiling"] == 90


def test_register_records_the_registration_first(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    records = switch.log("run-a")
    assert len(records) == 1
    record = records[0]
    assert record["seq"] == 1
    assert record["kind"] == "registered"
    assert record["stage"] == "none"
    assert record["reasons"] == []
    assert record["at"] == core._format_instant(T0)


@pytest.mark.parametrize(
    "elapsed,expected",
    [
        (0, "none"),
        (60, "none"),
        (61, "warn"),
        (120, "warn"),
        (121, "throttle"),
        (181, "cut tools"),
        (241, "cut budget"),
        (299, "cut budget"),
        (301, "kill"),
    ],
)
def test_missed_proof_matrix_at_injected_instants(tmp_path, elapsed, expected):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    report = switch.stage("run-a", at(elapsed))
    assert report.stage == expected
    if expected == "none":
        assert report.reasons == []
    else:
        assert report.reasons == ["missed proofs"]


def test_missed_counts_with_a_declared_limit_of_three(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, missed_proof_limit=3)
    assert switch.stage("run-a", at(121)).stage == "throttle"
    other = Switch(tmp_path / "other.json")
    other.register("run-a", T0, missed_proof_limit=3)
    assert other.stage("run-a", at(181)).stage == "kill"


def test_missed_counts_above_four_stay_at_cut_budget_below_the_limit(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, missed_proof_limit=10)
    assert switch.stage("run-a", at(60 * 7 + 1)).stage == "cut budget"
    other = Switch(tmp_path / "other.json")
    other.register("run-a", T0, missed_proof_limit=10)
    assert other.stage("run-a", at(60 * 10 + 1)).stage == "kill"


def test_a_limit_of_one_kills_on_the_first_missed_proof(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, missed_proof_limit=1)
    assert switch.stage("run-a", at(61)).stage == "kill"
    assert read_run(tmp_path)["killed"] is True


def test_spend_boundaries_at_the_exact_ceilings(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, soft_ceiling=100, hard_ceiling=1000)
    switch.spend("run-a", 99, at(1))
    assert switch.stage("run-a", at(2)).stage == "none"
    switch.spend("run-a", 1, at(3))
    report = switch.stage("run-a", at(4))
    assert report.stage == "cut budget"
    assert report.reasons == ["soft ceiling"]
    other = Switch(tmp_path / "other.json")
    other.register("run-a", T0, soft_ceiling=100, hard_ceiling=1000)
    other.spend("run-a", 1000, at(1))
    report = other.stage("run-a", at(2))
    assert report.stage == "kill"
    assert report.reasons == ["hard ceiling", "soft ceiling"]


def test_spend_and_proofs_move_signals_only(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, hard_ceiling=10)
    total = switch.spend("run-a", 50, at(1))
    assert total == 50
    switch.prove("run-a", "working", at(2))
    run = read_run(tmp_path)
    assert run["stage"] == "none"
    assert run["spend_total"] == 50
    assert switch.stage("run-a", at(3)).stage == "kill"


def test_reasons_name_every_live_trigger(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, soft_ceiling=10, hard_ceiling=100)
    switch.spend("run-a", 100, at(1))
    report = switch.stage("run-a", at(122))
    assert report.stage == "kill"
    assert report.reasons == ["hard ceiling", "soft ceiling", "missed proofs"]


def test_a_proof_restores_freshness(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    switch.prove("run-a", "working", at(100))
    assert switch.stage("run-a", at(160)).stage == "none"
    assert switch.stage("run-a", at(161)).stage == "warn"


def test_a_late_proof_with_an_older_instant_moves_nothing(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    switch.prove("run-a", "working", at(100))
    switch.prove("run-a", "late arrival", at(50))
    run = read_run(tmp_path)
    assert run["last_proof_at"] == core._format_instant(at(100))
    assert switch.stage("run-a", at(160)).stage == "none"


def test_hysteresis_holds_once_then_steps_down_one_stage(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    assert switch.stage("run-a", at(61)).stage == "warn"
    switch.prove("run-a", "working", at(61))
    held = switch.stage("run-a", at(61))
    assert held.stage == "warn"
    stepped = switch.stage("run-a", at(61))
    assert stepped.stage == "none"
    kinds = [record["kind"] for record in switch.log("run-a")]
    assert kinds == ["registered", "stage", "proof", "stage"]


def test_hysteresis_steps_down_one_stage_at_a_time_from_high_stages(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    assert switch.stage("run-a", at(241)).stage == "cut budget"
    switch.prove("run-a", "working", at(241))
    assert switch.stage("run-a", at(241)).stage == "cut budget"
    assert switch.stage("run-a", at(241)).stage == "cut tools"
    assert switch.stage("run-a", at(241)).stage == "cut tools"
    assert switch.stage("run-a", at(241)).stage == "throttle"


def test_a_fresh_warrant_resets_the_fall_count(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    assert switch.stage("run-a", at(61)).stage == "warn"
    switch.prove("run-a", "working", at(61))
    assert switch.stage("run-a", at(61)).stage == "warn"
    assert switch.stage("run-a", at(122)).stage == "warn"
    switch.prove("run-a", "working again", at(122))
    assert switch.stage("run-a", at(122)).stage == "warn"
    assert switch.stage("run-a", at(122)).stage == "none"


def test_a_rise_jumps_stages_when_a_hard_trigger_warrants_it(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, hard_ceiling=10)
    switch.spend("run-a", 10, at(1))
    assert switch.stage("run-a", at(2)).stage == "kill"
    records = switch.log("run-a")
    assert [record["kind"] for record in records] == ["registered", "spend", "stage"]
    assert records[-1]["stage"] == "kill"


def test_pull_goes_straight_to_kill_and_latches(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    assert switch.stage("run-a", at(61)).stage == "warn"
    assert switch.pull("run-a", at(62)) == "kill"
    records = switch.log("run-a")
    assert records[-1]["kind"] == "pull"
    assert records[-1]["stage"] == "kill"
    assert records[-1]["reasons"] == ["operator pull"]
    before = (tmp_path / "state.json").read_bytes()
    assert switch.pull("run-a", at(63)) == "kill"
    assert (tmp_path / "state.json").read_bytes() == before


def test_a_trigger_kill_latches_against_clear_evaluations_and_proofs(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, hard_ceiling=10)
    switch.spend("run-a", 10, at(1))
    assert switch.stage("run-a", at(2)).stage == "kill"
    switch.prove("run-a", "still here", at(3))
    for step in range(4, 8):
        assert switch.stage("run-a", at(step)).stage == "kill"
    run = read_run(tmp_path)
    assert run["killed"] is True
    assert run["stage"] == "kill"


def test_a_pulled_run_stays_killed_when_no_trigger_stands(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    switch.pull("run-a", at(1))
    assert switch.stage("run-a", at(30)).stage == "kill"
    assert switch.stage("run-a", at(30)).stage == "kill"
    assert read_run(tmp_path)["stage"] == "kill"


def test_a_closed_run_records_proofs_and_spend_as_no_ops(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, soft_ceiling=5, hard_ceiling=10)
    switch.spend("run-a", 7, at(1))
    switch.pull("run-a", at(2))
    total = switch.spend("run-a", 500, at(3))
    assert total == 7
    switch.prove("run-a", "late proof", at(4))
    run = read_run(tmp_path)
    assert run["spend_total"] == 7
    assert run["last_proof_at"] is None
    assert run["stage"] == "kill"
    records = switch.log("run-a")
    assert [record["kind"] for record in records[-2:]] == ["spend", "proof"]
    assert all(record["stage"] == "kill" for record in records[-2:])


def test_an_evaluation_of_a_closed_run_changes_nothing(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    switch.pull("run-a", at(1))
    before = (tmp_path / "state.json").read_bytes()
    report = switch.stage("run-a", at(500))
    assert report.stage == "kill"
    assert (tmp_path / "state.json").read_bytes() == before


def test_a_quiet_evaluation_at_none_changes_nothing(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    before = (tmp_path / "state.json").read_bytes()
    assert switch.stage("run-a", at(30)).stage == "none"
    assert (tmp_path / "state.json").read_bytes() == before


def test_a_held_stage_records_every_evaluation_that_holds_it(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    switch.stage("run-a", at(61))
    switch.stage("run-a", at(61))
    switch.stage("run-a", at(61))
    stage_records = [
        record for record in switch.log("run-a") if record["kind"] == "stage"
    ]
    assert len(stage_records) == 3
    assert all(record["stage"] == "warn" for record in stage_records)
    assert all(record["reasons"] == ["missed proofs"] for record in stage_records)


def test_the_log_walks_a_full_life_in_order(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, soft_ceiling=10, hard_ceiling=100)
    switch.prove("run-a", "working", at(10))
    switch.spend("run-a", 10, at(11))
    switch.stage("run-a", at(12))
    switch.pull("run-a", at(13))
    records = switch.log("run-a")
    assert [record["seq"] for record in records] == [1, 2, 3, 4, 5]
    assert [record["kind"] for record in records] == [
        "registered",
        "proof",
        "spend",
        "stage",
        "pull",
    ]
    assert records[3]["stage"] == "cut budget"
    assert records[3]["reasons"] == ["soft ceiling"]
    assert records[4]["reasons"] == ["operator pull"]


def test_reserved_fields_stand_empty_in_every_record(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, hard_ceiling=10)
    switch.prove("run-a", "working", at(1))
    switch.spend("run-a", 10, at(2))
    switch.stage("run-a", at(3))
    run = read_run(tmp_path)
    assert len(run["log"]) == 4
    for record in run["log"]:
        assert record["model_name"] is None
        assert record["model_confidence"] is None
        assert record["fallback_taken"] is None


def test_evaluation_is_deterministic_on_the_same_state_and_instant(tmp_path):
    first = Switch(tmp_path / "first.json")
    second = Switch(tmp_path / "second.json")
    for switch in (first, second):
        switch.register("run-a", T0, soft_ceiling=10, hard_ceiling=100)
        switch.spend("run-a", 10, at(5))
    report_first = first.stage("run-a", at(6))
    report_second = second.stage("run-a", at(6))
    assert report_first == report_second
    assert (tmp_path / "first.json").read_bytes() == (
        tmp_path / "second.json"
    ).read_bytes()
    again_first = first.stage("run-a", at(6))
    again_second = second.stage("run-a", at(6))
    assert again_first == again_second
    assert (tmp_path / "first.json").read_bytes() == (
        tmp_path / "second.json"
    ).read_bytes()


def test_registering_a_standing_id_fails_live_or_closed(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    with pytest.raises(RefusalError):
        switch.register("run-a", at(1))
    switch.pull("run-a", at(2))
    with pytest.raises(RefusalError):
        switch.register("run-a", at(3))


@pytest.mark.parametrize(
    "operation",
    [
        lambda switch: switch.prove("ghost", "note", T0),
        lambda switch: switch.spend("ghost", 1, T0),
        lambda switch: switch.pull("ghost", T0),
        lambda switch: switch.stage("ghost", T0),
        lambda switch: switch.log("ghost"),
    ],
)
def test_every_operation_against_an_unknown_run_is_not_found(tmp_path, operation):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    with pytest.raises(NotFoundError):
        operation(switch)


def test_a_missing_state_file_reads_as_not_found_and_creates_nothing(tmp_path):
    path = tmp_path / "deep" / "state.json"
    switch = Switch(path)
    with pytest.raises(NotFoundError):
        switch.log("run-a")
    assert not path.exists()
    switch.register("run-a", T0)
    assert path.is_file()
    assert switch.log("run-a")[0]["kind"] == "registered"


@pytest.mark.parametrize(
    "bad",
    ["", "-run", "run-", "_run", "run_", "Run", "run a", "run/a", "run.a", "r" * 65, 7, None],
)
def test_invalid_run_ids_are_refused(tmp_path, bad):
    switch = make_switch(tmp_path)
    with pytest.raises(InvalidNameError):
        switch.register(bad, T0)
    with pytest.raises(InvalidNameError):
        switch.stage(bad, T0)


@pytest.mark.parametrize("good", ["a", "1", "run-1", "a_b", "r" * 64, "a-1_b-2"])
def test_valid_run_id_edges_are_accepted(tmp_path, good):
    switch = make_switch(tmp_path)
    switch.register(good, T0)
    assert switch.stage(good, T0).stage == "none"


@pytest.mark.parametrize(
    "kwargs",
    [
        {"proof_interval_seconds": 0},
        {"proof_interval_seconds": -5},
        {"proof_interval_seconds": "60"},
        {"proof_interval_seconds": True},
        {"missed_proof_limit": 0},
        {"missed_proof_limit": -1},
        {"soft_ceiling": -1},
        {"hard_ceiling": -1},
        {"soft_ceiling": 50, "hard_ceiling": 49},
    ],
)
def test_invalid_registration_values_are_refused_before_state_changes(tmp_path, kwargs):
    switch = make_switch(tmp_path)
    with pytest.raises(InvalidRequestError):
        switch.register("run-a", T0, **kwargs)
    assert not (tmp_path / "state.json").exists()


@pytest.mark.parametrize("amount", [-1, "5", True, 1.5])
def test_invalid_spend_amounts_are_refused(tmp_path, amount):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    before = (tmp_path / "state.json").read_bytes()
    with pytest.raises(InvalidRequestError):
        switch.spend("run-a", amount, at(1))
    assert (tmp_path / "state.json").read_bytes() == before


def test_a_note_at_the_bound_is_accepted_and_past_it_refused(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    switch.prove("run-a", "x" * 200, at(1))
    with pytest.raises(InvalidRequestError):
        switch.prove("run-a", "x" * 201, at(2))


def test_a_naive_instant_is_refused(tmp_path):
    switch = make_switch(tmp_path)
    naive = datetime(2031, 5, 4, 12, 0, 0)
    with pytest.raises(InvalidRequestError):
        switch.register("run-a", naive)
    switch.register("run-a", T0)
    with pytest.raises(InvalidRequestError):
        switch.stage("run-a", naive)


def _valid_document(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, soft_ceiling=10, hard_ceiling=100)
    switch.prove("run-a", "working", at(1))
    switch.spend("run-a", 10, at(2))
    switch.stage("run-a", at(3))
    return (tmp_path / "state.json").read_bytes()


def _damage(tmp_path, mutate):
    raw = _valid_document(tmp_path)
    document = json.loads(raw)
    mutate(document)
    return json.dumps(document).encode("utf-8")


def _assert_corrupt(tmp_path, damaged):
    path = tmp_path / "state.json"
    path.write_bytes(damaged)
    switch = Switch(path)
    with pytest.raises(CorruptStateError):
        switch.log("run-a")
    with pytest.raises(CorruptStateError):
        switch.prove("run-a", "working", at(9))
    assert path.read_bytes() == damaged


def test_damage_battery(tmp_path):
    cases = [
        lambda doc: doc.update({"format": "deadman-state/2"}),
        lambda doc: doc.update({"extra": 1}),
        lambda doc: doc.update({"runs": {}}),
        lambda doc: doc["runs"][0].update({"extra": 1}),
        lambda doc: doc["runs"][0].pop("stage"),
        lambda doc: doc["runs"][0].update({"stage": "pause"}),
        lambda doc: doc["runs"][0].update({"spend_total": "10"}),
        lambda doc: doc["runs"][0].update({"killed": "no"}),
        lambda doc: doc["runs"][0].update({"killed": True}),
        lambda doc: doc["runs"][0].update({"soft_ceiling": 200}),
        lambda doc: doc["runs"][0]["log"][0].update({"kind": "vanished"}),
        lambda doc: doc["runs"][0]["log"][0].update({"seq": 7}),
        lambda doc: doc["runs"][0]["log"][1].update({"reasons": ["bad luck"]}),
        lambda doc: doc["runs"][0]["log"][1].update({"model_name": "a model"}),
        lambda doc: doc["runs"][0]["log"][0].update({"kind": "proof"}),
        lambda doc: doc["runs"][0].update({"registered_at": "2031-05-04T12:00:00"}),
        lambda doc: doc["runs"].append(doc["runs"][0]),
    ]
    for index, mutate in enumerate(cases):
        case_dir = tmp_path / f"case-{index}"
        case_dir.mkdir()
        _assert_corrupt(case_dir, _damage(case_dir, mutate))


def test_damage_battery_text_shapes(tmp_path):
    raw = _valid_document(tmp_path)
    text = raw.decode("utf-8")
    duplicated = text.replace('"format": "deadman-state/1"', '"format": "deadman-state/1", "format": "deadman-state/1"', 1)
    _assert_corrupt(tmp_path, duplicated.encode("utf-8"))
    other = tmp_path / "other"
    other.mkdir()
    _assert_corrupt(other, raw[: len(raw) // 2])
    third = tmp_path / "third"
    third.mkdir()
    _assert_corrupt(third, b"this is not json")


WORKER = """
import sys
from datetime import datetime

sys.path.insert(0, sys.argv[1])
from deadman.core import Switch

mode = sys.argv[2]
switch = Switch(sys.argv[3])
now = datetime.fromisoformat(sys.argv[4])
if mode == "spend":
    for _ in range(5):
        switch.spend("run-a", 10, now)
elif mode == "prove":
    switch.prove("run-a", "working", now)
elif mode == "stage":
    report = switch.stage("run-a", now)
    print(report.stage)
"""


def _spawn_workers(tmp_path, mode, count, instant):
    worker = tmp_path / "worker.py"
    worker.write_text(WORKER, encoding="utf-8")
    children = [
        subprocess.Popen(
            [
                sys.executable,
                str(worker),
                str(ROOT),
                mode,
                str(tmp_path / "state.json"),
                core._format_instant(instant),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        for _ in range(count)
    ]
    results = []
    for child in children:
        out, err = child.communicate()
        assert child.returncode == 0, err
        results.append(out)
    return results


def test_parallel_spend_and_prove_lose_no_updates(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    _spawn_workers(tmp_path, "spend", 6, at(10))
    run = read_run(tmp_path)
    assert run["spend_total"] == 300
    _spawn_workers(tmp_path, "prove", 6, at(20))
    records = switch.log("run-a")
    seqs = [record["seq"] for record in records]
    assert seqs == list(range(1, len(records) + 1))
    kinds = [record["kind"] for record in records]
    assert kinds.count("spend") == 30
    assert kinds.count("proof") == 6
    run = read_run(tmp_path)
    assert run["spend_total"] == 300


def test_parallel_evaluations_at_one_kill_instant_record_one_outcome(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0)
    outputs = _spawn_workers(tmp_path, "stage", 6, at(301))
    assert all(out.strip() == "kill" for out in outputs)
    records = switch.log("run-a")
    stage_records = [record for record in records if record["kind"] == "stage"]
    assert len(stage_records) == 1
    assert stage_records[0]["stage"] == "kill"
    assert read_run(tmp_path)["killed"] is True


def test_parallel_evaluations_at_one_escalation_instant_agree(tmp_path):
    switch = make_switch(tmp_path)
    switch.register("run-a", T0, soft_ceiling=10, hard_ceiling=100)
    switch.spend("run-a", 10, at(1))
    outputs = _spawn_workers(tmp_path, "stage", 6, at(2))
    assert all(out.strip() == "cut budget" for out in outputs)
    records = switch.log("run-a")
    seqs = [record["seq"] for record in records]
    assert seqs == list(range(1, len(records) + 1))
    stage_records = [record for record in records if record["kind"] == "stage"]
    assert len(stage_records) == 6
    assert all(record["stage"] == "cut budget" for record in stage_records)


def test_cli_walks_a_run_through_its_life(tmp_path):
    state = str(tmp_path / "state.json")
    result = run_cli("--state", state, "register", "run-a", "--hard-ceiling", "10")
    assert result.stdout.strip() == "registered run-a"
    result = run_cli("--state", state, "prove", "run-a", "--note", "working")
    assert result.stdout.strip() == "proved run-a"
    result = run_cli("--state", state, "spend", "run-a", "--amount", "10")
    assert result.stdout.strip() == "spend_total 10"
    result = run_cli("--state", state, "stage", "run-a")
    assert result.stdout.splitlines()[0] == "stage kill"
    assert "reason hard ceiling" in result.stdout
    result = run_cli("--state", state, "log", "run-a")
    lines = result.stdout.splitlines()
    assert lines[0].startswith("1 registered")
    assert lines[-1].startswith("4 stage")
    result = run_cli("--state", state, "pull", "run-a")
    assert result.stdout.strip() == "stage kill"


def test_cli_carries_no_time_override_flag(tmp_path):
    state = str(tmp_path / "state.json")
    run_cli("--state", state, "register", "run-a")
    result = run_cli("--state", state, "stage", "run-a", "--now", "2031-01-01", expect=2)
    assert "unrecognized" in result.stderr


def test_cli_error_exits_and_messages(tmp_path):
    state = str(tmp_path / "state.json")
    result = run_cli("--state", state, "stage", "ghost", expect=1)
    assert result.stderr.startswith("error:")
    assert "Traceback" not in result.stderr
    result = run_cli("--state", state, "register", "Bad Name", expect=2)
    assert result.stderr.startswith("error:")
    run_cli("--state", state, "register", "run-a")
    result = run_cli("--state", state, "register", "run-a", expect=1)
    assert result.stderr.startswith("error:")
    result = run_cli(
        "--state", state, "register", "run-b", "--soft-ceiling", "9",
        "--hard-ceiling", "8", expect=2,
    )
    assert result.stderr.startswith("error:")
