"""Smoke tests for the Python binding over the deadman library.

The suite drives the full switch lifecycle through the binding
against a real state file the library builds on disk, and
checks the facts each call returns against each other and
against the state file itself, and against the deadman
command line reading and writing the same files. The cargo
suite remains the gate for the library itself, and this file
is the consumer proof that a Python caller can drive the same
lifecycle the command line drives, at instants the caller
names outright.

The cross check drives the deadman binary from the workspace
build. The DEADMAN_BIN environment variable names the binary
when it stands somewhere else, and otherwise the suite builds
it with cargo when no built binary stands in the target
directory.
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

import deadman

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
    }


def stored_run(state):
    document = json.loads(Path(state).read_text())
    assert document["format"] == "deadman-state/1"
    return document["runs"][0]


def cli_binary():
    named = os.environ.get("DEADMAN_BIN")
    if named:
        return named
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO_ROOT / "target"))
    binary = target / "debug" / "deadman"
    if not binary.exists():
        subprocess.run(
            ["cargo", "build", "--bin", "deadman"],
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

    opened = deadman.register(
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

    deadman.prove(state, "run-a", T_PROOF, note="halfway there")
    assert deadman.spend(state, "run-a", 150, T_SPEND) == 150

    report = deadman.stage(state, "run-a", T_SPEND)
    assert report == {
        "run": "run-a",
        "stage": "cut budget",
        "reasons": ["soft ceiling"],
    }

    assert deadman.pull(state, "run-a", T_PULL) == "kill"
    report = deadman.stage(state, "run-a", T_PULL)
    assert report["stage"] == "kill"
    assert report["reasons"] == ["soft ceiling"]

    entries = deadman.log(state, "run-a")
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

    opened = deadman.register(state, "run-a", "2031-05-04T14:00:00+02:00")
    assert opened["registered_at"] == T0
    assert opened["proof_interval_seconds"] == 60
    assert opened["missed_proof_limit"] == 5
    assert opened["soft_ceiling"] is None
    assert opened["hard_ceiling"] is None

    explicit = deadman.register(
        state, "run-b", T0, proof_interval=60, missed_limit=5
    )
    assert explicit["proof_interval_seconds"] == (
        opened["proof_interval_seconds"]
    )
    assert explicit["missed_proof_limit"] == opened["missed_proof_limit"]

    report = deadman.stage(state, "run-a", T0)
    assert report == {"run": "run-a", "stage": "none", "reasons": []}


def test_refusals_raise_with_the_library_messages(tmp_path):
    state = str(tmp_path / "state.json")

    for subclass in (
        deadman.InvalidNameError,
        deadman.InvalidRequestError,
        deadman.NotFoundError,
        deadman.RefusalError,
        deadman.CorruptError,
        deadman.FailureError,
    ):
        assert issubclass(subclass, deadman.DeadmanError)

    deadman.register(state, "run-a", T0)

    with pytest.raises(deadman.NotFoundError) as missing:
        deadman.stage(state, "run-x", T0)
    assert str(missing.value) == (
        "run 'run-x' does not stand in the state file"
    )
    with pytest.raises(deadman.DeadmanError):
        deadman.stage(state, "run-x", T0)

    with pytest.raises(deadman.RefusalError) as duplicate:
        deadman.register(state, "run-a", T0)
    assert str(duplicate.value) == (
        "run 'run-a' already stands in the state file"
    )

    with pytest.raises(deadman.InvalidNameError) as invalid:
        deadman.register(state, "Not A Name", T0)
    assert str(invalid.value) == (
        "invalid run id 'Not A Name', use one component of lowercase "
        "letters and digits and hyphens and underscores, starting and "
        "ending with a letter or a digit, at most 64 characters long"
    )

    with pytest.raises(deadman.InvalidRequestError) as naive:
        deadman.stage(state, "run-a", "2031-05-04T12:00:00")
    assert str(naive.value) == (
        "the current instant must be a timezone aware datetime"
    )

    with pytest.raises(deadman.InvalidRequestError) as unparseable:
        deadman.stage(state, "run-a", "not an instant")
    assert str(unparseable.value) == (
        "the current instant holds unparseable text 'not an instant'"
    )

    with pytest.raises(deadman.InvalidRequestError) as ceilings:
        deadman.register(
            state, "run-b", T0, soft_ceiling=500, hard_ceiling=100
        )
    assert str(ceilings.value) == (
        "the soft ceiling stands above the hard ceiling, declare the "
        "soft ceiling at or below the hard ceiling"
    )

    with pytest.raises(deadman.InvalidRequestError) as zero_interval:
        deadman.register(state, "run-b", T0, proof_interval=0)
    assert str(zero_interval.value) == (
        "invalid proof interval 0, use a positive integer"
    )

    with pytest.raises(deadman.InvalidRequestError) as wide_interval:
        deadman.register(state, "run-b", T0, proof_interval=86400000000000)
    assert str(wide_interval.value) == (
        "invalid proof interval 86400000000000, use a positive "
        "integer of at most 86399999999999 seconds"
    )

    with pytest.raises(deadman.InvalidRequestError) as wide_limit:
        deadman.register(state, "run-b", T0, missed_limit=1000001)
    assert str(wide_limit.value) == (
        "invalid missed proof limit 1000001, use a positive integer "
        "of at most 1000000"
    )

    corrupt = str(tmp_path / "corrupt.json")
    Path(corrupt).write_text("this is not a state document\n")
    with pytest.raises(deadman.CorruptError) as unreadable:
        deadman.log(corrupt, "run-a")
    assert str(unreadable.value).startswith("the state is corrupt, ")


def test_integer_arguments_refuse_booleans(tmp_path):
    state = str(tmp_path / "state.json")

    with pytest.raises(TypeError) as interval_bool:
        deadman.register(state, "run-a", T0, proof_interval=True)
    assert str(interval_bool.value) == (
        "proof_interval must be an integer, not a boolean"
    )

    with pytest.raises(TypeError) as ceiling_bool:
        deadman.register(state, "run-a", T0, soft_ceiling=True)
    assert str(ceiling_bool.value) == (
        "soft_ceiling must be an integer, not a boolean"
    )

    with pytest.raises(deadman.NotFoundError):
        deadman.log(state, "run-a")

    deadman.register(state, "run-a", T0)
    with pytest.raises(TypeError) as amount_bool:
        deadman.spend(state, "run-a", False, T0)
    assert str(amount_bool.value) == (
        "amount must be an integer, not a boolean"
    )
    assert deadman.spend(state, "run-a", 5, T0) == 5


def test_wide_integer_digits_come_from_the_value_not_its_methods(tmp_path):
    state = str(tmp_path / "state.json")
    deadman.register(state, "run-a", T0)

    whole = "1" + "0" * 100
    spend_tail = ", use a nonnegative integer of at most 1000000000000000000"

    with pytest.raises(deadman.InvalidRequestError) as capped:
        deadman.spend(state, "run-a", 10**5000, T0)
    assert str(capped.value) == (
        "invalid spend amount a value too large to print" + spend_tail
    )

    with pytest.raises(deadman.InvalidRequestError) as capped_negative:
        deadman.spend(state, "run-a", -(10**5000), T0)
    assert str(capped_negative.value) == (
        "invalid spend amount a value too large to print" + spend_tail
    )

    with pytest.raises(deadman.InvalidRequestError) as wide:
        deadman.spend(state, "run-a", 10**100, T0)
    assert str(wide.value) == "invalid spend amount " + whole + spend_tail

    with pytest.raises(deadman.InvalidRequestError) as wide_negative:
        deadman.spend(state, "run-a", -(10**100), T0)
    assert str(wide_negative.value) == (
        "invalid spend amount -" + whole + spend_tail
    )

    with pytest.raises(deadman.InvalidRequestError) as forged:
        deadman.spend(state, "run-a", ForgedBytesInt(10**100), T0)
    assert str(forged.value) == "invalid spend amount " + whole + spend_tail

    with pytest.raises(deadman.InvalidRequestError) as empty:
        deadman.spend(state, "run-a", EmptyBytesInt(10**100), T0)
    assert str(empty.value) == "invalid spend amount " + whole + spend_tail

    with pytest.raises(deadman.InvalidRequestError) as forged_width:
        deadman.spend(state, "run-a", ForgedBitLengthInt(10**100), T0)
    assert str(forged_width.value) == (
        "invalid spend amount " + whole + spend_tail
    )

    with pytest.raises(deadman.InvalidRequestError) as wide_ceiling:
        deadman.register(state, "run-b", T0, soft_ceiling=10**5000)
    assert str(wide_ceiling.value) == (
        "invalid soft ceiling a value too large to print, use a "
        "nonnegative integer of at most 1000000000000000000 or "
        "leave it unset"
    )

    with pytest.raises(deadman.InvalidRequestError) as wide_interval:
        deadman.register(state, "run-b", T0, proof_interval=-(10**5000))
    assert str(wide_interval.value) == (
        "invalid proof interval a value too large to print, use a "
        "positive integer"
    )

    assert deadman.spend(state, "run-a", 5, T0) == 5


def test_state_files_cross_between_the_binding_and_the_command_line(
    tmp_path,
):
    state = str(tmp_path / "state.json")
    deadman.register(
        state, "run-a", T0, soft_ceiling=100, hard_ceiling=1000
    )
    deadman.prove(state, "run-a", T_PROOF)
    assert deadman.spend(state, "run-a", 150, T_SPEND) == 150

    entries = deadman.log(state, "run-a")
    assert run_cli(state, "log", "run-a").splitlines() == [
        cli_line(entry) for entry in entries
    ]
    assert run_cli(state, "stage", "run-a") == (
        "stage cut budget\nreason soft ceiling\n"
    )
    report = deadman.stage(state, "run-a", T_SPEND)
    assert report["stage"] == "cut budget"
    assert report["reasons"] == ["soft ceiling"]

    other = str(tmp_path / "other.json")
    assert run_cli(other, "register", "run-b") == "registered run-b\n"
    assert run_cli(other, "spend", "--amount", "25", "run-b") == (
        "spend_total 25\n"
    )
    entries = deadman.log(other, "run-b")
    assert [entry["kind"] for entry in entries] == ["registered", "spend"]
    assert all(entry["at"].endswith("+00:00") for entry in entries)
    report = deadman.stage(other, "run-b", entries[-1]["at"])
    assert report == {"run": "run-b", "stage": "none", "reasons": []}
    assert deadman.spend(other, "run-b", 5, entries[-1]["at"]) == 30
    entries = deadman.log(other, "run-b")
    assert run_cli(other, "log", "run-b").splitlines() == [
        cli_line(entry) for entry in entries
    ]
