"""Smoke tests for the Python binding over the arvin library.

The suite drives the full switch lifecycle through the binding
against a real state file the library builds on disk, and
checks the facts each call returns against each other and
against the state file itself, and against the arvin
command line reading and writing the same files. The cargo
suite remains the gate for the library itself, and this file
is the consumer proof that a Python caller can drive the same
lifecycle the command line drives, at instants the caller
names outright.

The cross check drives the arvin binary from the workspace
build. The ARVIN_BIN environment variable names the binary
when it stands somewhere else, and otherwise the suite builds
it with cargo when no built binary stands in the target
directory.
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

import arvin

REPO_ROOT = Path(__file__).resolve().parents[1]

T0 = "2031-05-04T12:00:00+00:00"
T_PROOF = "2031-05-04T12:00:30+00:00"
T_SPEND = "2031-05-04T12:00:40+00:00"
T_PULL = "2031-05-04T12:00:50+00:00"


class ForgedBytesInt(int):
    """An int whose to_bytes hands back the bytes of a small number."""

    def to_bytes(self, length=1, byteorder="big", *, signed=False):
        return int.to_bytes(60, 8, byteorder, signed=True)


class EmptyBytesInt(int):
    """An int whose to_bytes hands back no bytes at all."""

    def to_bytes(self, length=1, byteorder="big", *, signed=False):
        return b""


class ForgedBitLengthInt(int):
    """An int whose bit_length reports a small width."""

    def bit_length(self):
        return 8


class IndexOnly:
    """An object that is no integer and only answers __index__."""

    def __index__(self):
        return 5


def record(seq, kind, at, stage, reasons):
    return {
        "seq": seq,
        "kind": kind,
        "at": at,
        "stage": stage,
        "reasons": reasons,
        "model_name": None,
        "model_confidence": None,
        "fallback_taken": None,
        "model_verdict": None,
        "model_score": None,
        "model_ancestry": None,
    }


def stored_run(state):
    document = json.loads(Path(state).read_text())
    assert document["format"] == "deadman-state/1"
    run = document["runs"][0]
    # A record written before the escalation slice carries no
    # model verdict or model score key in the file, and the
    # binding carries both as None, so the stored side reads
    # them as None for the comparisons below.
    for entry in run["log"]:
        entry.setdefault("model_verdict", None)
        entry.setdefault("model_score", None)
        entry.setdefault("model_ancestry", None)
    return run


def cli_binary():
    named = os.environ.get("ARVIN_BIN")
    if named:
        return named
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO_ROOT / "target"))
    binary = target / "debug" / "arvin"
    if not binary.exists():
        subprocess.run(
            ["cargo", "build", "--bin", "arvin"],
            cwd=REPO_ROOT,
            check=True,
        )
    return str(binary)


def run_cli(state, *args):
    result = subprocess.run(
        [cli_binary(), "--state", state, *args],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout


def cli_line(entry):
    line = (
        f"{entry['seq']} {entry['kind']} "
        f"at={entry['at']} stage={entry['stage']}"
    )
    for reason in entry["reasons"]:
        line += f" reason={reason}"
    return line


def test_the_full_lifecycle_through_the_binding(tmp_path):
    state = str(tmp_path / "state.json")

    opened = arvin.register(
        state, "run-a", T0, soft_ceiling=100, hard_ceiling=1000
    )
    assert opened == {
        "id": "run-a",
        "registered_at": T0,
        "proof_interval_seconds": 60,
        "missed_proof_limit": 5,
        "soft_ceiling": 100,
        "hard_ceiling": 1000,
        "spend_total": 0,
        "last_proof_at": None,
        "stage": "none",
        "clear_streak": 0,
        "killed": False,
        "log": [record(1, "registered", T0, "none", [])],
    }
    run = stored_run(state)
    for field in (
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
    ):
        assert run[field] == opened[field]
    assert run["log"] == opened["log"]

    arvin.prove(state, "run-a", T_PROOF, note="halfway there")
    assert arvin.spend(state, "run-a", 150, T_SPEND) == 150

    report = arvin.stage(state, "run-a", T_SPEND)
    assert report == {
        "run": "run-a",
        "stage": "cut budget",
        "reasons": ["soft ceiling"],
    }

    assert arvin.pull(state, "run-a", T_PULL) == "kill"
    report = arvin.stage(state, "run-a", T_PULL)
    assert report["stage"] == "kill"
    assert report["reasons"] == ["soft ceiling"]

    entries = arvin.log(state, "run-a")
    assert entries == [
        record(1, "registered", T0, "none", []),
        record(2, "proof", T_PROOF, "none", []),
        record(3, "spend", T_SPEND, "none", []),
        record(4, "stage", T_SPEND, "cut budget", ["soft ceiling"]),
        record(5, "pull", T_PULL, "kill", ["operator pull"]),
    ]
    run = stored_run(state)
    assert run["log"] == entries
    assert run["spend_total"] == 150
    assert run["last_proof_at"] == T_PROOF
    assert run["stage"] == "kill"
    assert run["killed"] is True


def test_registration_defaults_resolve_from_the_library(tmp_path):
    state = str(tmp_path / "state.json")

    opened = arvin.register(state, "run-a", "2031-05-04T14:00:00+02:00")
    assert opened["registered_at"] == T0
    assert opened["proof_interval_seconds"] == 60
    assert opened["missed_proof_limit"] == 5
    assert opened["soft_ceiling"] is None
    assert opened["hard_ceiling"] is None

    explicit = arvin.register(
        state, "run-b", T0, proof_interval=60, missed_limit=5
    )
    assert explicit["proof_interval_seconds"] == (
        opened["proof_interval_seconds"]
    )
    assert explicit["missed_proof_limit"] == opened["missed_proof_limit"]

    report = arvin.stage(state, "run-a", T0)
    assert report == {"run": "run-a", "stage": "none", "reasons": []}


def test_refusals_raise_with_the_library_messages(tmp_path):
    state = str(tmp_path / "state.json")

    for subclass in (
        arvin.InvalidNameError,
        arvin.InvalidRequestError,
        arvin.NotFoundError,
        arvin.RefusalError,
        arvin.CorruptError,
        arvin.FailureError,
    ):
        assert issubclass(subclass, arvin.ArvinError)

    arvin.register(state, "run-a", T0)

    with pytest.raises(arvin.NotFoundError) as missing:
        arvin.stage(state, "run-x", T0)
    assert str(missing.value) == (
        "run 'run-x' does not stand in the state file"
    )
    with pytest.raises(arvin.ArvinError):
        arvin.stage(state, "run-x", T0)

    with pytest.raises(arvin.RefusalError) as duplicate:
        arvin.register(state, "run-a", T0)
    assert str(duplicate.value) == (
        "run 'run-a' already stands in the state file"
    )

    with pytest.raises(arvin.InvalidNameError) as invalid:
        arvin.register(state, "Not A Name", T0)
    assert str(invalid.value) == (
        "invalid run id 'Not A Name', use one component of ASCII "
        "lowercase letters and ASCII digits and hyphens and "
        "underscores, starting and ending with an ASCII lowercase "
        "letter or an ASCII digit, at most 64 characters long"
    )

    with pytest.raises(arvin.InvalidRequestError) as naive:
        arvin.stage(state, "run-a", "2031-05-04T12:00:00")
    assert str(naive.value) == (
        "the current instant must be a timezone aware datetime"
    )

    with pytest.raises(arvin.InvalidRequestError) as unparseable:
        arvin.stage(state, "run-a", "not an instant")
    assert str(unparseable.value) == (
        "the current instant holds unparseable text 'not an instant'"
    )

    with pytest.raises(arvin.InvalidRequestError) as ceilings:
        arvin.register(
            state, "run-b", T0, soft_ceiling=500, hard_ceiling=100
        )
    assert str(ceilings.value) == (
        "the soft ceiling stands above the hard ceiling, declare the "
        "soft ceiling at or below the hard ceiling"
    )

    with pytest.raises(arvin.InvalidRequestError) as zero_interval:
        arvin.register(state, "run-b", T0, proof_interval=0)
    assert str(zero_interval.value) == (
        "invalid proof interval 0, use a positive integer"
    )

    with pytest.raises(arvin.InvalidRequestError) as wide_interval:
        arvin.register(state, "run-b", T0, proof_interval=86400000000000)
    assert str(wide_interval.value) == (
        "invalid proof interval 86400000000000, use a positive "
        "integer of at most 86399999999999 seconds"
    )

    with pytest.raises(arvin.InvalidRequestError) as wide_limit:
        arvin.register(state, "run-b", T0, missed_limit=1000001)
    assert str(wide_limit.value) == (
        "invalid missed proof limit 1000001, use a positive integer "
        "of at most 1000000"
    )

    corrupt = str(tmp_path / "corrupt.json")
    Path(corrupt).write_text("this is not a state document\n")
    with pytest.raises(arvin.CorruptError) as unreadable:
        arvin.log(corrupt, "run-a")
    assert str(unreadable.value).startswith("the state is corrupt, ")


def test_integer_arguments_refuse_booleans(tmp_path):
    state = str(tmp_path / "state.json")

    with pytest.raises(TypeError) as interval_bool:
        arvin.register(state, "run-a", T0, proof_interval=True)
    assert str(interval_bool.value) == (
        "proof_interval must be an integer, not a boolean"
    )

    with pytest.raises(TypeError) as ceiling_bool:
        arvin.register(state, "run-a", T0, soft_ceiling=True)
    assert str(ceiling_bool.value) == (
        "soft_ceiling must be an integer, not a boolean"
    )

    with pytest.raises(arvin.NotFoundError):
        arvin.log(state, "run-a")

    arvin.register(state, "run-a", T0)
    with pytest.raises(TypeError) as amount_bool:
        arvin.spend(state, "run-a", False, T0)
    assert str(amount_bool.value) == (
        "amount must be an integer, not a boolean"
    )
    assert arvin.spend(state, "run-a", 5, T0) == 5


def test_wide_integer_digits_come_from_the_value_not_its_methods(tmp_path):
    state = str(tmp_path / "state.json")
    arvin.register(state, "run-a", T0)

    whole = "1" + "0" * 100
    spend_tail = ", use a nonnegative integer of at most 1000000000000000000"

    with pytest.raises(arvin.InvalidRequestError) as capped:
        arvin.spend(state, "run-a", 10**5000, T0)
    assert str(capped.value) == (
        "invalid spend amount a value too large to print" + spend_tail
    )

    with pytest.raises(arvin.InvalidRequestError) as capped_negative:
        arvin.spend(state, "run-a", -(10**5000), T0)
    assert str(capped_negative.value) == (
        "invalid spend amount a value too large to print" + spend_tail
    )

    with pytest.raises(arvin.InvalidRequestError) as wide:
        arvin.spend(state, "run-a", 10**100, T0)
    assert str(wide.value) == "invalid spend amount " + whole + spend_tail

    with pytest.raises(arvin.InvalidRequestError) as wide_negative:
        arvin.spend(state, "run-a", -(10**100), T0)
    assert str(wide_negative.value) == (
        "invalid spend amount -" + whole + spend_tail
    )

    with pytest.raises(arvin.InvalidRequestError) as forged:
        arvin.spend(state, "run-a", ForgedBytesInt(10**100), T0)
    assert str(forged.value) == "invalid spend amount " + whole + spend_tail

    with pytest.raises(arvin.InvalidRequestError) as empty:
        arvin.spend(state, "run-a", EmptyBytesInt(10**100), T0)
    assert str(empty.value) == "invalid spend amount " + whole + spend_tail

    with pytest.raises(arvin.InvalidRequestError) as forged_width:
        arvin.spend(state, "run-a", ForgedBitLengthInt(10**100), T0)
    assert str(forged_width.value) == (
        "invalid spend amount " + whole + spend_tail
    )

    with pytest.raises(arvin.InvalidRequestError) as past_cap:
        arvin.spend(state, "run-a", 10**100000, T0)
    assert str(past_cap.value) == (
        "invalid spend amount a value too large to print" + spend_tail
    )

    with pytest.raises(arvin.InvalidRequestError) as past_cap_negative:
        arvin.spend(state, "run-a", -(10**100000), T0)
    assert str(past_cap_negative.value) == (
        "invalid spend amount a value too large to print" + spend_tail
    )

    with pytest.raises(arvin.InvalidRequestError) as wide_ceiling:
        arvin.register(state, "run-b", T0, soft_ceiling=10**5000)
    assert str(wide_ceiling.value) == (
        "invalid soft ceiling a value too large to print, use a "
        "nonnegative integer of at most 1000000000000000000 or "
        "leave it unset"
    )

    with pytest.raises(arvin.InvalidRequestError) as wide_interval:
        arvin.register(state, "run-b", T0, proof_interval=-(10**5000))
    assert str(wide_interval.value) == (
        "invalid proof interval a value too large to print, use a "
        "positive integer"
    )

    assert arvin.spend(state, "run-a", 5, T0) == 5


def test_integer_arguments_reject_non_integers(tmp_path):
    state = str(tmp_path / "state.json")
    arvin.register(state, "run-a", T0)

    with pytest.raises(TypeError) as indexed:
        arvin.spend(state, "run-a", IndexOnly(), T0)
    assert str(indexed.value) == (
        "'IndexOnly' object cannot be interpreted as an integer"
    )

    with pytest.raises(TypeError) as floated:
        arvin.spend(state, "run-a", 1.5, T0)
    assert str(floated.value) == (
        "'float' object cannot be interpreted as an integer"
    )

    with pytest.raises(TypeError) as spelled:
        arvin.spend(state, "run-a", "60", T0)
    assert str(spelled.value) == (
        "'str' object cannot be interpreted as an integer"
    )

    with pytest.raises(TypeError) as indexed_interval:
        arvin.register(state, "run-b", T0, proof_interval=IndexOnly())
    assert str(indexed_interval.value) == (
        "'IndexOnly' object cannot be interpreted as an integer"
    )

    assert arvin.spend(state, "run-a", 5, T0) == 5


def test_state_files_cross_between_the_binding_and_the_command_line(
    tmp_path,
):
    state = str(tmp_path / "state.json")
    arvin.register(
        state, "run-a", T0, soft_ceiling=100, hard_ceiling=1000
    )
    arvin.prove(state, "run-a", T_PROOF)
    assert arvin.spend(state, "run-a", 150, T_SPEND) == 150

    entries = arvin.log(state, "run-a")
    assert run_cli(state, "log", "run-a").splitlines() == [
        cli_line(entry) for entry in entries
    ]
    assert run_cli(state, "stage", "run-a") == (
        "stage cut budget\nreason soft ceiling\n"
    )
    report = arvin.stage(state, "run-a", T_SPEND)
    assert report["stage"] == "cut budget"
    assert report["reasons"] == ["soft ceiling"]

    other = str(tmp_path / "other.json")
    assert run_cli(other, "register", "run-b") == "registered run-b\n"
    assert run_cli(other, "spend", "--amount", "25", "run-b") == (
        "spend_total 25\n"
    )
    entries = arvin.log(other, "run-b")
    assert [entry["kind"] for entry in entries] == ["registered", "spend"]
    assert all(entry["at"].endswith("+00:00") for entry in entries)
    report = arvin.stage(other, "run-b", entries[-1]["at"])
    assert report == {"run": "run-b", "stage": "none", "reasons": []}
    assert arvin.spend(other, "run-b", 5, entries[-1]["at"]) == 30
    entries = arvin.log(other, "run-b")
    assert run_cli(other, "log", "run-b").splitlines() == [
        cli_line(entry) for entry in entries
    ]


def test_log_carries_the_model_fields_of_a_consultation_record(tmp_path):
    # The consultation stands on a hold record at warn, the
    # shape a consultation at the held seat writes. The rise
    # into warn before it is the deterministic rise and
    # carries null model fields, as DM-18 requires, because
    # no consultation stands behind a deterministic rise.
    state = str(tmp_path / "state.json")
    document = {
        "format": "deadman-state/1",
        "runs": [
            {
                "id": "run-a",
                "registered_at": T0,
                "proof_interval_seconds": 60,
                "missed_proof_limit": 5,
                "soft_ceiling": None,
                "hard_ceiling": None,
                "spend_total": 0,
                "last_proof_at": None,
                "clear_streak": 0,
                "stage": "warn",
                "killed": False,
                "log": [
                    {
                        "seq": 1,
                        "kind": "registered",
                        "at": T0,
                        "stage": "none",
                        "reasons": [],
                        "model_name": None,
                        "model_confidence": None,
                        "fallback_taken": None,
                    },
                    {
                        "seq": 2,
                        "kind": "stage",
                        "at": "2031-05-04T12:01:01+00:00",
                        "stage": "warn",
                        "reasons": ["missed proofs"],
                        "model_name": None,
                        "model_confidence": None,
                        "fallback_taken": None,
                    },
                    {
                        "seq": 3,
                        "kind": "stage",
                        "at": "2031-05-04T12:01:01+00:00",
                        "stage": "warn",
                        "reasons": ["missed proofs"],
                        "model_name": "jev-1.13.0",
                        "model_confidence": 0.5,
                        "fallback_taken": None,
                        "model_verdict": "hold",
                        "model_score": 0.25,
                    },
                ],
            }
        ],
    }
    Path(state).write_text(json.dumps(document, indent=2) + "\n")
    entries = arvin.log(state, "run-a")
    expected = record(3, "stage", "2031-05-04T12:01:01+00:00", "warn", ["missed proofs"])
    expected.update(
        {
            "model_name": "jev-1.13.0",
            "model_confidence": 0.5,
            "model_verdict": "hold",
            "model_score": 0.25,
        }
    )
    assert entries == [
        record(1, "registered", T0, "none", []),
        record(2, "stage", "2031-05-04T12:01:01+00:00", "warn", ["missed proofs"]),
        expected,
    ]


T_WARN = "2031-05-04T12:01:01+00:00"


def seat_at_warn(state):
    arvin.register(state, "run-a", T0)
    report = arvin.stage(state, "run-a", T_WARN)
    assert report == {
        "run": "run-a",
        "stage": "warn",
        "reasons": ["missed proofs"],
    }


def test_stage_with_model_fixture_verdict_applies_its_pattern(tmp_path):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    report = arvin.stage_with_model(
        state,
        "run-a",
        T_WARN,
        fixture={
            "model_name": "jev-1.13.0",
            "verdict": "escalate",
            "confidence": 0.77,
            "score": 5 / 9,
        },
    )
    assert report == {
        "run": "run-a",
        "stage": "throttle",
        "reasons": ["missed proofs"],
    }
    entries = arvin.log(state, "run-a")
    last = entries[-1]
    assert last["model_name"] == "jev-1.13.0"
    assert last["model_verdict"] == "escalate"
    assert last["model_confidence"] == 0.77
    assert last["model_score"] == 5 / 9
    assert last["fallback_taken"] is None
    assert last["stage"] == "throttle"
    assert stored_run(state)["stage"] == "throttle"


def test_stage_with_model_fixture_fallback_records_its_class(tmp_path):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    report = arvin.stage_with_model(
        state,
        "run-a",
        T_WARN,
        fixture={"model_name": "jev-1.13.0", "fallback": "silence"},
    )
    assert report == {
        "run": "run-a",
        "stage": "warn",
        "reasons": ["missed proofs"],
    }
    last = arvin.log(state, "run-a")[-1]
    assert last["model_name"] == "jev-1.13.0"
    assert last["fallback_taken"] == "silence"
    assert last["model_verdict"] is None
    assert last["model_confidence"] is None
    assert last["model_score"] is None


def test_stage_with_model_refuses_two_judge_forms_together(tmp_path):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as both:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge="a-helper",
            fixture={"model_name": "jev-1.13.0", "fallback": "silence"},
        )
    assert str(both.value) == (
        "exactly one judge form may stand named on one call"
    )
    assert len(arvin.log(state, "run-a")) == 2


def test_stage_with_model_refuses_a_label_outside_the_three(tmp_path):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as label:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            fixture={
                "model_name": "jev-1.13.0",
                "verdict": "freeze",
                "confidence": 0.5,
                "score": 0.5,
            },
        )
    assert "outside the three verdicts" in str(label.value)
    assert len(arvin.log(state, "run-a")) == 2


def test_stage_with_model_fixture_shape_edges_refuse(tmp_path):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as carried:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            fixture={
                "model_name": "jev-1.13.0",
                "fallback": "silence",
                "confidence": 0.5,
            },
        )
    assert "verdict form" in str(carried.value)
    assert len(arvin.log(state, "run-a")) == 2

    with pytest.raises(arvin.InvalidRequestError) as overflow:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge="a-helper",
            judge_timeout=float(2**64),
        )
    assert "does not stand as a positive number of seconds" in str(
        overflow.value
    )
    assert len(arvin.log(state, "run-a")) == 2


def test_stage_with_model_host_context_without_a_transport_refuses(
    tmp_path,
):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as context:
        arvin.stage_with_model(
            state, "run-a", T_WARN, host_context="a note from the host"
        )
    assert str(context.value) == (
        "a host context stands supplied with no model transport "
        "to carry it"
    )
    assert len(arvin.log(state, "run-a")) == 2


def seat_at_the_fall_decision(state):
    # A run standing at throttle on a standing clear streak of
    # one, where the next clean evaluation warrants warn and so
    # stands at the fall decision seat of DM-13, the seat a
    # scripted consultation can endorse a step down from.
    arvin.register(
        state, "run-a", T0, proof_interval=3600, missed_limit=5
    )
    arvin.prove(state, "run-a", "2031-05-04T12:30:00+00:00")
    report = arvin.stage(state, "run-a", "2031-05-04T14:31:00+00:00")
    assert report == {
        "run": "run-a",
        "stage": "throttle",
        "reasons": ["missed proofs"],
    }
    arvin.prove(state, "run-a", "2031-05-04T14:32:00+00:00")
    report = arvin.stage(state, "run-a", "2031-05-04T14:33:00+00:00")
    assert report == {
        "run": "run-a",
        "stage": "throttle",
        "reasons": [],
    }


def test_stage_with_model_without_a_transport_consults_nothing(tmp_path):
    # The plain call with no judge form named is the library no
    # transport case, judged at the fall decision seat after
    # the evaluation the seat helper's 14:33 stage call stands
    # at. The deterministic evaluation of DM-9 falls on the
    # second consecutive clean evaluation, so the call at
    # 14:35, the second clean evaluation after the 14:32
    # proof, steps the stage down to warn on the deterministic
    # rule alone. A consultation in the same position stands
    # where the fall could have been endorsed instead, and the
    # record the call leaves carries the consultation fields
    # as null where a consulted record carries the model name,
    # so the record's shape is what proves nothing stood
    # consulted.
    state = str(tmp_path / "state.json")
    seat_at_the_fall_decision(state)
    arvin.prove(state, "run-a", "2031-05-04T14:34:00+00:00")

    report = arvin.stage_with_model(
        state, "run-a", "2031-05-04T14:35:00+00:00"
    )
    assert report == {
        "run": "run-a",
        "stage": "warn",
        "reasons": [],
    }
    last = arvin.log(state, "run-a")[-1]
    assert last["model_name"] is None
    assert last["model_verdict"] is None
    assert last["model_confidence"] is None
    assert last["model_score"] is None
    assert last["fallback_taken"] is None


def test_stage_with_model_fixture_without_a_model_name_refuses(tmp_path):
    # Both dictionary forms carry the model name they stand
    # pinned to, and a dictionary without one is refused in
    # the surface message before any state changes, at a seat
    # and off one alike, because the refusal judges the
    # dictionary and not the evaluation.
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as verdict_form:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            fixture={
                "verdict": "escalate",
                "confidence": 0.77,
                "score": 0.5,
            },
        )
    assert str(verdict_form.value) == (
        "a fixture stands without the model name it stands pinned to"
    )
    assert len(arvin.log(state, "run-a")) == 2

    with pytest.raises(arvin.InvalidRequestError) as fallback_form:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            fixture={"fallback": "silence"},
        )
    assert str(fallback_form.value) == (
        "a fixture stands without the model name it stands pinned to"
    )
    assert len(arvin.log(state, "run-a")) == 2


def test_stage_with_model_fixture_values_of_the_wrong_type_refuse(
    tmp_path,
):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    cases = [
        (
            {
                "model_name": 5,
                "verdict": "hold",
                "confidence": 0.5,
                "score": 0.5,
            },
            "model name",
        ),
        (
            {
                "model_name": "jev-1.13.0",
                "verdict": 5,
                "confidence": 0.5,
                "score": 0.5,
            },
            "verdict",
        ),
        (
            {
                "model_name": "jev-1.13.0",
                "verdict": None,
                "confidence": 0.5,
                "score": 0.5,
            },
            "verdict",
        ),
        (
            {"model_name": "jev-1.13.0", "fallback": 5},
            "fallback",
        ),
        (
            {
                "model_name": "jev-1.13.0",
                "verdict": "hold",
                "confidence": "high",
                "score": 0.5,
            },
            "confidence",
        ),
        (
            {
                "model_name": "jev-1.13.0",
                "verdict": "hold",
                "confidence": 0.5,
                "score": "low",
            },
            "score",
        ),
    ]
    for fixture, field in cases:
        with pytest.raises(arvin.InvalidRequestError) as raised:
            arvin.stage_with_model(
                state, "run-a", T_WARN, fixture=fixture
            )
        assert field in str(raised.value), str(raised.value)
        assert len(arvin.log(state, "run-a")) == 2


def test_binding_log_records_carry_the_ancestry_field(tmp_path):
    # The certifying sequence of DM-23 driven through the
    # binding alone. The run rises to throttle on missed
    # proofs, a proof and a clean evaluation arm the fall
    # seat, the fall seat consultation answers de-escalate
    # below the bar and holds, and two plain evaluations
    # complete the streak and step the stage down.
    state = str(tmp_path / "state.json")
    arvin.register(state, "run-a", T0)
    report = arvin.stage(state, "run-a", "2031-05-04T12:02:01+00:00")
    assert report["stage"] == "throttle"
    arvin.prove(state, "run-a", "2031-05-04T12:02:01+00:00")
    arvin.stage(state, "run-a", "2031-05-04T12:02:02+00:00")

    report = arvin.stage_with_model(
        state,
        "run-a",
        "2031-05-04T12:02:03+00:00",
        fixture={
            "model_name": "jev-1.13.0",
            "verdict": "de-escalate",
            "confidence": 0.79,
            "score": 0.2,
        },
    )
    assert report["stage"] == "throttle"
    arvin.stage(state, "run-a", "2031-05-04T12:02:04+00:00")
    report = arvin.stage(state, "run-a", "2031-05-04T12:02:05+00:00")
    assert report["stage"] == "warn"

    entries = arvin.log(state, "run-a")
    step_down = entries[-1]
    assert step_down["kind"] == "stage"
    assert step_down["stage"] == "warn"
    assert step_down["model_ancestry"] == "de-escalate below bar"
    assert step_down["model_name"] is None
    assert step_down["model_verdict"] is None
    assert step_down["model_confidence"] is None
    assert step_down["model_score"] is None
    assert step_down["fallback_taken"] is None
    # A record in a landed shape returns the key at null, the
    # registered record among them, and the consultation
    # record that certified the step down carries it at null
    # in its eleven key shape in the stored document.
    assert entries[0]["model_ancestry"] is None
    consultation = entries[-2]
    assert consultation["model_verdict"] == "de-escalate"
    assert consultation["model_ancestry"] is None
    run = json.loads(Path(state).read_text())["runs"][0]
    assert run["log"][-1]["model_ancestry"] == (
        "de-escalate below bar"
    )
    assert run["log"][-2]["model_ancestry"] is None
    assert "streak_ancestry" not in run
