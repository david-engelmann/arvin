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
    # Portability adaptation appends .exe on Windows so Cargo's built binary resolves.
    exe = "arvin.exe" if os.name == "nt" else "arvin"
    binary = target / "debug" / exe
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
        "appended": record(
            4, "stage", T_SPEND, "cut budget", ["soft ceiling"]
        ),
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
    assert report == {
        "run": "run-a",
        "stage": "none",
        "reasons": [],
        "appended": None,
    }


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
    assert report == {
        "run": "run-b",
        "stage": "none",
        "reasons": [],
        "appended": None,
    }
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
        "appended": record(
            2, "stage", T_WARN, "warn", ["missed proofs"]
        ),
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
    appended = record(
        3, "stage", T_WARN, "throttle", ["missed proofs"]
    )
    appended.update(
        {
            "model_name": "jev-1.13.0",
            "model_verdict": "escalate",
            "model_confidence": 0.77,
            "model_score": 5 / 9,
        }
    )
    assert report == {
        "run": "run-a",
        "stage": "throttle",
        "reasons": ["missed proofs"],
        "appended": appended,
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
    appended = record(3, "stage", T_WARN, "warn", ["missed proofs"])
    appended.update(
        {
            "model_name": "jev-1.13.0",
            "fallback_taken": "silence",
        }
    )
    assert report == {
        "run": "run-a",
        "stage": "warn",
        "reasons": ["missed proofs"],
        "appended": appended,
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


def test_stage_with_model_refuses_orphan_judge_parts(tmp_path):
    # The two orphan part refusals of the judge form stand
    # pinned on the command line and stand unpinned here
    # without this test (DM-A45). A judge argument or a judge
    # timeout named with no judge program to carry it refuses
    # in the shared words, before any state stands touched.
    # The pin fails where the binding's form call passes a
    # wrong named predicate for either part, because the call
    # then runs the no transport path and appends a record
    # instead of raising. The empty argument list and the
    # zero timeout are named parts as well and refuse in the
    # same words, the zero before any timeout parsing; a
    # conversion that read either as absent would let the
    # call through, so both stand pinned here too.
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as argument:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge_args=["--flag"],
        )
    assert str(argument.value) == (
        "a judge argument stands named with no judge program to carry it"
    )
    assert len(arvin.log(state, "run-a")) == 2

    with pytest.raises(arvin.InvalidRequestError) as timeout:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge_timeout=30,
        )
    assert str(timeout.value) == (
        "a judge timeout stands named with no judge program to carry it"
    )
    assert len(arvin.log(state, "run-a")) == 2

    with pytest.raises(arvin.InvalidRequestError) as empty:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge_args=[],
        )
    assert str(empty.value) == (
        "a judge argument stands named with no judge program to carry it"
    )
    assert len(arvin.log(state, "run-a")) == 2

    with pytest.raises(arvin.InvalidRequestError) as zero:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge_timeout=0,
        )
    assert str(zero.value) == (
        "a judge timeout stands named with no judge program to carry it"
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



def test_stage_with_model_judge_timeout_integer_ceiling_rendering(
    tmp_path,
):
    # The command line renders a whole number timeout
    # standing at or past the surfaces' whole ceiling as
    # the ceiling itself in its refusal, and the binding
    # holds its own copy of that rendering in its refusal.
    # The integer path stands pinned here in its exact
    # message, at the ceiling, where the rendered digits
    # equal the typed digits, and one past it, where the
    # rendered digits are the ceiling's and not the
    # caller's.
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    for value in (2**64, 2**64 + 1):
        with pytest.raises(arvin.InvalidRequestError) as raised:
            arvin.stage_with_model(
                state,
                "run-a",
                T_WARN,
                judge="a-helper",
                judge_timeout=value,
            )
        assert str(raised.value) == (
            "judge timeout 18446744073709551616 does not stand "
            "as a positive number of seconds the transport "
            "can hold"
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
        "appended": record(
            3,
            "stage",
            "2031-05-04T14:31:00+00:00",
            "throttle",
            ["missed proofs"],
        ),
    }
    arvin.prove(state, "run-a", "2031-05-04T14:32:00+00:00")
    report = arvin.stage(state, "run-a", "2031-05-04T14:33:00+00:00")
    assert report == {
        "run": "run-a",
        "stage": "throttle",
        "reasons": [],
        "appended": None,
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
        "appended": record(
            6, "stage", "2031-05-04T14:35:00+00:00", "warn", []
        ),
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


def test_stage_with_model_judge_args_of_the_wrong_shape_refuse(
    tmp_path,
):
    state = str(tmp_path / "state.json")
    seat_at_warn(state)

    with pytest.raises(arvin.InvalidRequestError) as element:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge="a-helper",
            judge_args=["--flag", 5],
        )
    assert str(element.value) == "a judge argument stands as no string"
    assert len(arvin.log(state, "run-a")) == 2

    with pytest.raises(arvin.InvalidRequestError) as bare:
        arvin.stage_with_model(
            state,
            "run-a",
            T_WARN,
            judge="a-helper",
            judge_args="--flag",
        )
    assert str(bare.value) == (
        "the judge arguments stand as no list or tuple"
    )
    assert len(arvin.log(state, "run-a")) == 2


def test_stage_with_model_judge_args_tuple_reaches_the_helper(
    tmp_path,
):
    # A tuple of strings stands as the documented second
    # shape of the argument, and its elements reach the
    # helper in order, ahead of the payload flag the
    # transport appends.
    state = str(tmp_path / "state.json")
    seat_at_warn(state)
    marker = tmp_path / "args.txt"
    # Portability adaptation stages a batch helper on Windows so judge_args
    # forwarding is exercised without requiring POSIX sh.
    # Mechanical consequence of DM-24 advancing default subprocess model
    # from jev-1.13.0 to jev-1.14.0.
    if os.name == "nt":
        reply_file = tmp_path / "reply.json"
        reply_file.write_text(
            '{"model": "jev-1.14.0", "choice": '
            '{"label": "hold", "confidence": 0.5}, "score": '
            '{"level": 5}}'
        )
        helper = tmp_path / "judge.bat"
        helper.write_text(
            "@echo off\r\n"
            f'<NUL set /p="%1 %2" > "{marker}"\r\n'
            f'type "{reply_file}"\r\n'
        )
    else:
        helper = tmp_path / "judge.sh"
        helper.write_text(
            "#!/bin/sh\n"
            f"printf '%s %s' \"$1\" \"$2\" > \"{marker}\"\n"
            "printf '%s' '{\"model\": \"jev-1.14.0\", \"choice\": "
            "{\"label\": \"hold\", \"confidence\": 0.5}, \"score\": "
            "{\"level\": 5}}'\n"
        )
        helper.chmod(0o755)
    report = arvin.stage_with_model(
        state,
        "run-a",
        T_WARN,
        judge=str(helper),
        judge_args=("--first", "--second"),
    )
    appended = record(3, "stage", T_WARN, "warn", ["missed proofs"])
    appended.update(
        {
            "model_name": "jev-1.14.0",
            "model_verdict": "hold",
            "model_confidence": 0.5,
            "model_score": 4 / 9,
        }
    )
    assert report == {
        "run": "run-a",
        "stage": "warn",
        "reasons": ["missed proofs"],
        "appended": appended,
    }
    assert marker.read_text() == "--first --second"
    last = arvin.log(state, "run-a")[-1]
    assert last["model_verdict"] == "hold"


def test_stage_with_model_accepts_the_widest_judge_timeout(tmp_path):
    # The widest whole second count the transport can hold
    # stands accepted by the binding's integer arm, one second
    # below the count the refusal pins close over (DM-A46).
    # The call raises no refusal, and the deadline the
    # transport cannot add to the current instant fails the
    # consultation ahead of any spawn, so the helper never
    # runs and the record carries the call failed class. The
    # pin fails where the arm narrows its ceiling by one
    # second, because the call then raises the refusal
    # instead, and where the deadline check moves behind the
    # spawn, because the helper then writes its marker.
    state = str(tmp_path / "state.json")
    seat_at_warn(state)
    marker = tmp_path / "payload-path.txt"
    # Portability adaptation so Windows hosts stage an executable batch helper
    # that writes the marker and prints a valid reply if erroneously spawned.
    # Mechanical consequence of DM-24 updates helper replies to jev-1.14.0.
    if os.name == "nt":
        reply_file = tmp_path / "reply.json"
        reply_file.write_text(
            '{"model": "jev-1.14.0", "choice": '
            '{"label": "hold", "confidence": 0.5}, "score": '
            '{"level": 5}}'
        )
        helper = tmp_path / "judge.bat"
        helper.write_text(
            "@echo off\r\n"
            f'echo %2 > "{marker}"\r\n'
            f'type "{reply_file}"\r\n'
        )
    else:
        helper = tmp_path / "judge.sh"
        helper.write_text(
            "#!/bin/sh\n"
            f"printf '%s' \"$2\" > \"{marker}\"\n"
            "printf '%s' '{\"model\": \"jev-1.14.0\", \"choice\": "
            "{\"label\": \"hold\", \"confidence\": 0.5}, \"score\": "
            "{\"level\": 5}}'\n"
        )
        helper.chmod(0o755)
    report = arvin.stage_with_model(
        state,
        "run-a",
        T_WARN,
        judge=str(helper),
        judge_timeout=2**64 - 1,
    )
    assert report["stage"] == "warn"
    assert report["appended"]["fallback_taken"] == "call failed"
    assert report["appended"]["model_verdict"] is None
    assert not marker.exists()
    last = arvin.log(state, "run-a")[-1]
    assert last["fallback_taken"] == "call failed"


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


def test_dm24_constants_callable_transport_and_provider_codecs(tmp_path):
    assert arvin.MODEL_NAME == "jev-1.14.0"
    assert arvin.LEGACY_MODEL_NAME == "jev-1.13.0"
    assert arvin.JUDGE_MODELS == (
        "jev-1.14.0",
        "jev-1.13.0",
        "gemini-decision-1",
        "gpt-6-luna",
        "gpt-oss-safeguard-120b",
        "gpt-oss-safeguard-20b",
        "pplx-decider-v1.3",
        "pplx-decider-latest",
        "pplx-decider-flash-v1.3",
        "pplx-decider-flash-latest",
        "@cf/cloudflare/clef",
        "@cf/cloudflare/clef-flash",
        "clef",
        "clef-flash",
    )
    assert arvin.detect_provider("jev-1.14.0") == "typesafe"
    assert arvin.detect_provider("gemini-decision-1") == "google"
    assert arvin.detect_provider("gpt-6-luna") == "openai"
    assert arvin.detect_provider("pplx-decider-v1.3") == "perplexity"
    assert arvin.detect_provider("@cf/cloudflare/clef") == "cloudflare"
    assert arvin.detect_provider("clef-preview") == "cloudflare"

    # Callable transport receives the DM-21 payload dict and applies provider normalization.
    seen_payloads = []

    def google_transport(payload):
        seen_payloads.append(payload)
        req_json = arvin.format_provider_request(payload, provider="google")
        req = json.loads(req_json)
        assert req["model"] == "gemini-decision-1"
        assert len(req["questions"]["score"]["criteria"]) == 10
        raw = json.dumps(
            {
                "answers": {
                    "verdict": {
                        "choice": "escalate",
                        "probabilities": {
                            "hold": 0.1,
                            "escalate": 0.85,
                            "de-escalate": 0.03,
                            "other": 0.02,
                        },
                    },
                    "score": {"expected_score": 4.5},
                }
            }
        )
        return arvin.normalize_provider_reply(
            raw, provider="google", model="gemini-decision-1"
        )

    state = str(tmp_path / "callable.json")
    seat_at_warn(state)
    report = arvin.stage_with_model(
        state,
        "run-a",
        T_WARN,
        judge_model="gemini-decision-1",
        transport=google_transport,
    )
    assert len(seen_payloads) == 1
    assert seen_payloads[0]["model"] == "gemini-decision-1"
    assert report["stage"] == "throttle"
    assert report["appended"]["model_name"] == "gemini-decision-1"
    assert report["appended"]["model_verdict"] == "escalate"
    assert report["appended"]["model_confidence"] == 0.85
    assert report["appended"]["model_score"] == 5 / 9

    # Off-seat call never invokes the callable transport.
    off_state = str(tmp_path / "off-seat.json")
    arvin.register(off_state, "run-a", T0)
    off_called = []
    off_report = arvin.stage_with_model(
        off_state,
        "run-a",
        T0,
        transport=lambda p: (off_called.append(p) or ""),
    )
    assert off_report["stage"] == "none"
    assert off_called == []

    # Discriminating 65536-byte cap pin using a valid JSON reply padded with trailing spaces.
    base_valid = (
        '{"model": "jev-1.14.0", "choice": {"label": "escalate", '
        '"confidence": 0.8}, "score": {"level": 6}}'
    )
    exact_65536 = base_valid + (" " * (65536 - len(base_valid)))
    over_65537 = base_valid + (" " * (65537 - len(base_valid)))
    assert len(exact_65536) == 65536
    assert len(over_65537) == 65537

    ok_cap_state = str(tmp_path / "cap-ok.json")
    seat_at_warn(ok_cap_state)
    ok_cap_report = arvin.stage_with_model(
        ok_cap_state, "run-a", T_WARN, transport=lambda _p: exact_65536
    )
    assert ok_cap_report["stage"] == "throttle"
    assert ok_cap_report["appended"]["model_verdict"] == "escalate"

    # Same-thread re-entry into the locked state file raises FailureError and lands call failed,
    # even if the callable unlinks the state path before re-entering.
    reentry_state = str(tmp_path / "reentry.json")
    seat_at_warn(reentry_state)
    reentry_errors = []

    def reentrant_transport(_payload):
        try:
            arvin.log(reentry_state, "run-a")
        except arvin.FailureError as exc:
            reentry_errors.append(str(exc))
            raise
        return base_valid

    reentry_report = arvin.stage_with_model(
        reentry_state, "run-a", T_WARN, transport=reentrant_transport
    )
    assert len(reentry_errors) == 1
    assert "cannot re-enter" in reentry_errors[0]
    assert reentry_report["appended"]["fallback_taken"] == "call failed"

    unlink_state = str(tmp_path / "reentry-unlink.json")
    seat_at_warn(unlink_state)
    unlink_errors = []

    def unlink_reentrant_transport(_payload):
        Path(unlink_state).unlink()
        try:
            arvin.register(unlink_state, "run-b", T0)
        except arvin.FailureError as exc:
            unlink_errors.append(str(exc))
            raise
        return base_valid

    unlink_report = arvin.stage_with_model(
        unlink_state, "run-a", T_WARN, transport=unlink_reentrant_transport
    )
    assert len(unlink_errors) == 1
    assert "cannot re-enter" in unlink_errors[0]
    assert unlink_report["appended"]["fallback_taken"] == "call failed"

    # Callable failure, silence, unpaired surrogate, grammar invalid, and 65537-byte classifications.
    for index, (fn, expected_fallback) in enumerate(
        [
            (lambda _p: (_ for _ in ()).throw(RuntimeError("boom")), "call failed"),
            (lambda _p: 12345, "call failed"),
            (lambda _p: "   ", "silence"),
            (lambda _p: "\ud800", "malformed reply"),
            (lambda _p: "not json", "malformed reply"),
            (lambda _p: over_65537, "malformed reply"),
        ]
    ):
        sub_state = str(tmp_path / f"fb-{index}.json")
        seat_at_warn(sub_state)
        fb_report = arvin.stage_with_model(
            sub_state, "run-a", T_WARN, transport=fn
        )
        assert fb_report["stage"] == "warn"
        assert fb_report["appended"]["model_name"] == "jev-1.14.0"
        assert fb_report["appended"]["fallback_taken"] == expected_fallback

    # Validation refusals on transport, judge_model, and provider names.
    refusal_state = str(tmp_path / "refusals.json")
    seat_at_warn(refusal_state)
    with pytest.raises(TypeError, match="transport must be callable"):
        arvin.stage_with_model(refusal_state, "run-a", T_WARN, transport="not-callable")
    with pytest.raises(arvin.InvalidRequestError, match="no judge or transport"):
        arvin.stage_with_model(
            refusal_state, "run-a", T_WARN, judge_model="gemini-decision-1"
        )
    with pytest.raises(arvin.InvalidRequestError, match="exactly one judge form"):
        arvin.stage_with_model(
            refusal_state,
            "run-a",
            T_WARN,
            fixture={"model_name": "jev-1.14.0", "fallback": "silence"},
            transport=lambda _p: "",
        )
    with pytest.raises(arvin.InvalidRequestError, match="printable ASCII"):
        arvin.stage_with_model(
            refusal_state,
            "run-a",
            T_WARN,
            judge_model="bad model",
            transport=lambda _p: "",
        )
    with pytest.raises(arvin.InvalidRequestError, match="unknown decision provider"):
        arvin.format_provider_request("{}", provider="bogus")
    with pytest.raises(arvin.InvalidRequestError, match="printable ASCII"):
        arvin.stage_with_model(
            refusal_state, "run-a", T_WARN, judge_model="\ud800", transport=lambda _p: ""
        )
    with pytest.raises(arvin.InvalidRequestError, match="not valid UTF-8"):
        arvin.detect_provider("\ud800")
    with pytest.raises(arvin.InvalidRequestError, match="not valid UTF-8"):
        arvin.normalize_provider_reply("\ud800")
    with pytest.raises(arvin.InvalidRequestError, match="not valid UTF-8"):
        arvin.format_provider_request("\ud800")
    with pytest.raises(arvin.InvalidRequestError, match="key is not a string"):
        arvin.format_provider_request({**seen_payloads[0], 1: "non-string-key"})
    with pytest.raises(arvin.InvalidRequestError, match="non-JSON or non-finite"):
        arvin.format_provider_request({**seen_payloads[0], "extra": float("nan")})
    with pytest.raises(arvin.InvalidRequestError, match="not valid UTF-8"):
        arvin.format_provider_request({**seen_payloads[0], "tuple_surrogate": ("\ud800",)})
    nested_ok = "leaf"
    for _ in range(255):
        nested_ok = [nested_ok]
    assert arvin.format_provider_request({**seen_payloads[0], "nested": nested_ok})
    nested_too_deep = [nested_ok]
    with pytest.raises(arvin.InvalidRequestError, match="nests deeper than 256 levels"):
        arvin.format_provider_request({**seen_payloads[0], "nested": nested_too_deep})
    cyclic = dict(seen_payloads[0])
    cyclic["self"] = cyclic
    with pytest.raises(arvin.InvalidRequestError, match="nests deeper than 256 levels"):
        arvin.format_provider_request(cyclic)


def test_dm25_runs_binding_summaries_and_reentry_guard(tmp_path):
    missing_state = tmp_path / "missing.json"
    assert arvin.runs(str(missing_state)) == []
    assert not missing_state.exists()
    assert not (tmp_path / "missing.json.lock").exists()

    state = str(tmp_path / "runs.json")
    arvin.register(state, "run-a", T0, soft_ceiling=100, hard_ceiling=500)
    arvin.register(state, "run-b", T_PROOF, proof_interval=120, missed_limit=3)
    arvin.prove(state, "run-a", T_PROOF, note="ok")
    arvin.spend(state, "run-a", 125, T_SPEND)
    arvin.stage(state, "run-a", T_SPEND)
    arvin.pull(state, "run-b", T_PULL)

    before_bytes = Path(state).read_bytes()
    summaries = arvin.runs(state)
    assert Path(state).read_bytes() == before_bytes
    assert summaries == [
        {
            "id": "run-a",
            "registered_at": T0,
            "proof_interval_seconds": 60,
            "missed_proof_limit": 5,
            "soft_ceiling": 100,
            "hard_ceiling": 500,
            "spend_total": 125,
            "last_proof_at": T_PROOF,
            "stage": "cut budget",
            "killed": False,
        },
        {
            "id": "run-b",
            "registered_at": T_PROOF,
            "proof_interval_seconds": 120,
            "missed_proof_limit": 3,
            "soft_ceiling": None,
            "hard_ceiling": None,
            "spend_total": 0,
            "last_proof_at": None,
            "stage": "kill",
            "killed": True,
        },
    ]
    for summary in summaries:
        assert type(summary["killed"]) is bool
        assert type(summary["proof_interval_seconds"]) is int
        assert type(summary["missed_proof_limit"]) is int
        assert type(summary["spend_total"]) is int
        if summary["soft_ceiling"] is not None:
            assert type(summary["soft_ceiling"]) is int
        if summary["hard_ceiling"] is not None:
            assert type(summary["hard_ceiling"]) is int

    # Wide >128-bit stored proof_interval_seconds crosses back as an exact Python int.
    wide_val = 10**60
    wide_state = tmp_path / "wide.json"
    wide_state.write_text(
        json.dumps(
            {
                "format": "deadman-state/1",
                "runs": [
                    {
                        "id": "wide-run",
                        "registered_at": T0,
                        "proof_interval_seconds": wide_val,
                        "missed_proof_limit": 5,
                        "soft_ceiling": None,
                        "hard_ceiling": None,
                        "spend_total": 0,
                        "last_proof_at": None,
                        "stage": "none",
                        "clear_streak": 0,
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
                            }
                        ],
                    }
                ],
            }
        )
    )
    wide_runs = arvin.runs(str(wide_state))
    assert len(wide_runs) == 1
    assert type(wide_runs[0]["proof_interval_seconds"]) is int
    assert wide_runs[0]["proof_interval_seconds"] == wide_val

    corrupt_state = tmp_path / "corrupt-runs.json"
    corrupt_state.write_text("corrupt\n")
    with pytest.raises(arvin.CorruptError):
        arvin.runs(str(corrupt_state))

    # Same-thread re-entry into arvin.runs during an active consultation raises FailureError.
    re_state = str(tmp_path / "reentry-runs.json")
    seat_at_warn(re_state)
    re_seen = []

    def runs_reentrant(_payload):
        try:
            arvin.runs(re_state)
        except arvin.FailureError as exc:
            re_seen.append(str(exc))
            raise
        return ""

    rep = arvin.stage_with_model(re_state, "run-a", T_WARN, transport=runs_reentrant)
    assert len(re_seen) == 1
    assert "cannot re-enter" in re_seen[0]
    assert rep["appended"]["fallback_taken"] == "call failed"


def test_dm26_version_and_pep561_type_stubs(tmp_path):
    import ast
    import sys
    import zipfile

    assert arvin.__version__ == "0.2.0"
    assert "__version__" in arvin.__all__
    pkg_dir = Path(arvin.__file__).resolve().parent
    assert (pkg_dir / "py.typed").is_file()
    stub_path = pkg_dir / "__init__.pyi"
    assert stub_path.is_file()

    repo_root = Path(__file__).resolve().parents[1]
    wheel_dir = tmp_path / "wheels"
    wheel_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            sys.executable,
            "-m",
            "maturin",
            "build",
            "--offline",
            "--locked",
            "--out",
            str(wheel_dir),
            "--manifest-path",
            str(repo_root / "bindings" / "Cargo.toml"),
        ],
        cwd=str(repo_root),
        check=True,
    )
    wheels = list(wheel_dir.glob("arvin-0.2.0-*.whl"))
    assert len(wheels) == 1, wheels
    with zipfile.ZipFile(wheels[0]) as zf:
        names = set(zf.namelist())
        for req in ("arvin/__init__.py", "arvin/py.typed", "arvin/__init__.pyi"):
            assert req in names, names
        assert any(n.endswith("/LICENSE") or n == "LICENSE" for n in names), names
        stub_text = zf.read("arvin/__init__.pyi").decode("utf-8")

    assert "judge_args: list[str] | tuple[str, ...] | None = None" in stub_text
    assert "fixture: dict[str, Any] | None = None" in stub_text
    tree = ast.parse(stub_text)
    declared = set()
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
            declared.add(node.name)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            declared.add(node.target.id)
    assert declared - {"__all__"} == set(arvin.__all__)
    for name in arvin.__all__:
        assert hasattr(arvin, name), name

