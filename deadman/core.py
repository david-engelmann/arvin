"""Core logic for deadman, the staged kill switch for agent runs.

One state file holds every run as one JSON document under the
format name deadman-state/1, and the document holds each run with
its signals and its decision log, so a stage change and its record
publish in the same rename and can never disagree. Every mutating
operation holds an exclusive flock on the lock file beside the
state file across its whole read and evaluate and modify and write
cycle, and publishes by renaming a complete temporary file over
the state file. Reads take a shared lock and open the state file
fresh each time, and a reader that changes nothing never writes.

Time is an input. Every operation but the log read takes the
current instant as an argument, and this module reads no wall
clock and calls no model. The one random draw in the module
names a temporary file at publication, and it reaches no stage,
no reason, no record, and no state byte. The stage an evaluation
reports is a deterministic function of the recorded state and
the instant.

The stage vocabulary is program vocabulary and stands in one
exact order in STAGES. The trigger names of DM-3 stand in
TRIGGERS.
"""

import fcntl
import json
import os
import re
import secrets
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

FORMAT_NAME = "deadman-state/1"
LOCK_SUFFIX = ".lock"
NAME_MAX = 64
NOTE_MAX = 200

STAGES = ["none", "warn", "throttle", "cut tools", "cut budget", "kill"]
TRIGGERS = ["operator pull", "hard ceiling", "soft ceiling", "missed proofs"]
RECORD_KINDS = {"registered", "proof", "spend", "pull", "stage"}

DEFAULT_PROOF_INTERVAL_SECONDS = 60
DEFAULT_MISSED_PROOF_LIMIT = 5

PROOF_INTERVAL_MAX_SECONDS = 86_399_999_999_999
SPEND_MAX = 10**18

# The stage a missed proof count below the limit warrants under
# DM-3, keyed by the count itself, with every higher count below
# the limit standing at the last entry.
MISSED_COUNT_STAGES = {1: "warn", 2: "throttle", 3: "cut tools", 4: "cut budget"}

NAME_PATTERN = re.compile(r"^[a-z0-9]([a-z0-9_-]*[a-z0-9])?$")

DOCUMENT_KEYS = {"format", "runs"}
RUN_KEYS = {
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
    "log",
}
RECORD_KEYS = {
    "seq",
    "kind",
    "at",
    "stage",
    "reasons",
    "model_name",
    "model_confidence",
    "fallback_taken",
}
RESERVED_FIELDS = ("model_name", "model_confidence", "fallback_taken")


class DeadmanError(Exception):
    """An error the CLI reports with a nonzero exit and no traceback."""

    exit_code = 1


class InvalidNameError(DeadmanError):
    """A run id is not in the shape the switch accepts."""

    exit_code = 2


class InvalidRequestError(DeadmanError):
    """A request value, such as an instant or a threshold, is not usable."""

    exit_code = 2


class NotFoundError(DeadmanError):
    """The named run does not stand in the state file."""


class RefusalError(DeadmanError):
    """The standing state of the switch refuses the operation."""


class CorruptStateError(DeadmanError):
    """The state document cannot be read as it was recorded."""


@dataclass
class StageReport:
    """The outcome of one evaluation of one run."""

    run: str
    stage: str
    reasons: list


def _is_int(value):
    return type(value) is int


def validate_name(value):
    """Refuse any run id outside the name rule of DM-9."""
    if (
        not isinstance(value, str)
        or len(value) > NAME_MAX
        or not NAME_PATTERN.fullmatch(value)
    ):
        raise InvalidNameError(
            f"invalid run id {value!r}, use one component of lowercase "
            f"letters and digits and hyphens and underscores, starting and "
            f"ending with a letter or a digit, at most {NAME_MAX} characters long"
        )
    return value


def _validate_now(now):
    if not isinstance(now, datetime) or now.tzinfo is None or now.utcoffset() is None:
        raise InvalidRequestError("the current instant must be a timezone aware datetime")
    return now


def _printable(value):
    """Repr a request value for an error message, however large it stands."""
    try:
        return repr(value)
    except ValueError:
        return "a value too large to print"


def _validate_positive_int(value, field):
    if not _is_int(value) or value < 1:
        raise InvalidRequestError(
            f"invalid {field} {value!r}, use a positive integer"
        )
    return value


def _validate_ceiling(value, field):
    if value is not None and (not _is_int(value) or value < 0):
        raise InvalidRequestError(
            f"invalid {field} {value!r}, use a nonnegative integer or leave it unset"
        )
    return value


def _validate_note(note):
    if not isinstance(note, str) or len(note) > NOTE_MAX:
        raise InvalidRequestError(
            f"invalid note, use a string of at most {NOTE_MAX} characters"
        )
    return note


def format_instant(moment):
    """Format an instant in UTC, refusing one outside the recorded range."""
    try:
        return moment.astimezone(timezone.utc).isoformat()
    except OverflowError:
        raise InvalidRequestError(
            "the instant stands outside the range the switch can record"
        ) from None


def _parse_instant(text, corrupt):
    if not isinstance(text, str):
        raise corrupt(f"an instant field holds {text!r} instead of text")
    try:
        moment = datetime.fromisoformat(text)
    except ValueError:
        raise corrupt(f"an instant field holds unparseable text {text!r}") from None
    if moment.tzinfo is None or moment.utcoffset() is None:
        raise corrupt(f"an instant field holds a naive instant {text!r}")
    return moment


class _DuplicateKeyError(ValueError):
    pass


def _object_without_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise _DuplicateKeyError(key)
        result[key] = value
    return result


def _empty_document():
    return {"format": FORMAT_NAME, "runs": []}


def _corrupt(message):
    return CorruptStateError(f"the state is corrupt, {message}")


def _validate_stored_name(value):
    if (
        not isinstance(value, str)
        or len(value) > NAME_MAX
        or not NAME_PATTERN.fullmatch(value)
    ):
        raise _corrupt(f"a run holds an invalid run id {value!r}")


def _validate_record(record, position):
    if not isinstance(record, dict) or set(record) != RECORD_KEYS:
        raise _corrupt("a log record does not carry exactly the recorded fields")
    if not _is_int(record["seq"]) or record["seq"] != position + 1:
        raise _corrupt("the log sequence does not grow by 1 from 1 without gaps")
    kind = record["kind"]
    if not isinstance(kind, str) or kind not in RECORD_KINDS:
        raise _corrupt(f"a log record holds an unknown kind {kind!r}")
    _parse_instant(record["at"], _corrupt)
    if record["stage"] not in STAGES:
        raise _corrupt(f"a log record holds an unknown stage {record['stage']!r}")
    reasons = record["reasons"]
    if not isinstance(reasons, list) or any(
        not isinstance(reason, str) or reason not in TRIGGERS for reason in reasons
    ):
        raise _corrupt("a log record holds a reason outside the recorded names")
    if len(set(reasons)) != len(reasons):
        raise _corrupt("a log record names one reason twice")
    for field in RESERVED_FIELDS:
        if record[field] is not None:
            raise _corrupt(f"a log record carries a value in the reserved field {field}")
    if kind in ("registered", "proof", "spend") and reasons:
        raise _corrupt(f"a {kind} record names reasons")
    if kind == "registered" and record["stage"] != "none":
        raise _corrupt("a registered record stands at a stage other than none")
    if kind == "pull" and (
        record["stage"] != "kill" or reasons != ["operator pull"]
    ):
        raise _corrupt("a pull record does not record the operator pull at kill")


def _validate_run(run):
    if not isinstance(run, dict) or set(run) != RUN_KEYS:
        raise _corrupt("a run does not carry exactly the recorded fields")
    _validate_stored_name(run["id"])
    _parse_instant(run["registered_at"], _corrupt)
    if not _is_int(run["proof_interval_seconds"]) or run["proof_interval_seconds"] < 1:
        raise _corrupt(f"run {run['id']!r} holds an invalid proof interval")
    if not _is_int(run["missed_proof_limit"]) or run["missed_proof_limit"] < 1:
        raise _corrupt(f"run {run['id']!r} holds an invalid missed proof limit")
    for field in ("soft_ceiling", "hard_ceiling"):
        value = run[field]
        if value is not None and (not _is_int(value) or value < 0):
            raise _corrupt(f"run {run['id']!r} holds an invalid {field}")
    if (
        run["soft_ceiling"] is not None
        and run["hard_ceiling"] is not None
        and run["soft_ceiling"] > run["hard_ceiling"]
    ):
        raise _corrupt(f"run {run['id']!r} holds a soft ceiling above its hard ceiling")
    if (
        not _is_int(run["spend_total"])
        or run["spend_total"] < 0
        or run["spend_total"] > SPEND_MAX
    ):
        raise _corrupt(f"run {run['id']!r} holds an invalid spend total")
    if run["last_proof_at"] is not None:
        _parse_instant(run["last_proof_at"], _corrupt)
    if run["stage"] not in STAGES:
        raise _corrupt(f"run {run['id']!r} holds an unknown stage {run['stage']!r}")
    if not _is_int(run["clear_streak"]) or run["clear_streak"] < 0:
        raise _corrupt(f"run {run['id']!r} holds an invalid clear streak")
    if type(run["killed"]) is not bool:
        raise _corrupt(f"run {run['id']!r} holds a killed flag that is not a boolean")
    if run["killed"] != (run["stage"] == "kill"):
        raise _corrupt(f"run {run['id']!r} holds a killed flag that disagrees with its stage")
    if not isinstance(run["log"], list) or not run["log"]:
        raise _corrupt(f"run {run['id']!r} holds no log")
    for position, record in enumerate(run["log"]):
        _validate_record(record, position)
    first = run["log"][0]
    if first["kind"] != "registered" or first["at"] != run["registered_at"]:
        raise _corrupt(f"run {run['id']!r} does not open with its registration record")


def _validate_document(document):
    if not isinstance(document, dict) or set(document) != DOCUMENT_KEYS:
        raise _corrupt("the document does not carry exactly the recorded fields")
    if document["format"] != FORMAT_NAME:
        raise _corrupt(f"the format name is {document['format']!r}, not {FORMAT_NAME!r}")
    if not isinstance(document["runs"], list):
        raise _corrupt("the runs field is not a list")
    seen = set()
    for run in document["runs"]:
        _validate_run(run)
        if run["id"] in seen:
            raise _corrupt(f"run {run['id']!r} stands in the state twice")
        seen.add(run["id"])
    return document


def _find_run(document, run_id):
    for run in document["runs"]:
        if run["id"] == run_id:
            return run
    return None


def _missed_proofs(run, now):
    """Whole proof intervals missed at the instant, deadline exact counts zero."""
    base_text = run["last_proof_at"] or run["registered_at"]
    base = _parse_instant(base_text, _corrupt)
    try:
        interval = timedelta(seconds=run["proof_interval_seconds"])
        delta = now - base
        if delta <= interval:
            return 0
        return (delta - timedelta(microseconds=1)) // interval
    except OverflowError:
        raise InvalidRequestError(
            "the missed proof count at this instant stands outside the "
            "range the switch can represent"
        ) from None


def _warrant(run, now):
    """The warranted stage index and the live trigger names at the instant."""
    index = 0
    reasons = []
    hard = run["hard_ceiling"]
    soft = run["soft_ceiling"]
    if hard is not None and run["spend_total"] >= hard:
        index = STAGES.index("kill")
        reasons.append("hard ceiling")
    if soft is not None and run["spend_total"] >= soft:
        index = max(index, STAGES.index("cut budget"))
        reasons.append("soft ceiling")
    missed = _missed_proofs(run, now)
    if missed >= 1:
        reasons.append("missed proofs")
        if missed >= run["missed_proof_limit"]:
            index = max(index, STAGES.index("kill"))
        else:
            warranted = MISSED_COUNT_STAGES[min(missed, 4)]
            index = max(index, STAGES.index(warranted))
    return index, reasons


class Switch:
    """One staged cutoff in one state file, opened fresh on every call."""

    def __init__(self, path):
        self.path = Path(path)
        self.lock_path = self.path.with_name(self.path.name + LOCK_SUFFIX)

    @contextmanager
    def _locked(self, exclusive):
        fd = os.open(self.lock_path, os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
            os.close(fd)

    def _read_document(self):
        """Read and strictly validate the state, or start empty.

        A missing file reads as the empty document. A file that stands
        but cannot be read as recorded raises CorruptStateError and is
        never reset and never written over by a read.
        """
        try:
            raw = self.path.read_bytes()
        except FileNotFoundError:
            return _empty_document()
        except OSError as exc:
            raise _corrupt(f"the state file cannot be read, {exc}") from exc
        try:
            document = json.loads(raw, object_pairs_hook=_object_without_duplicate_keys)
        except _DuplicateKeyError as exc:
            raise _corrupt(f"a key stands twice in the document, {exc}") from exc
        except ValueError as exc:
            raise _corrupt(f"the document does not parse, {exc}") from exc
        return _validate_document(document)

    def _publish(self, document):
        text = json.dumps(document, indent=2) + "\n"
        temp = self.path.with_name(
            f".{self.path.name}.tmp-{os.getpid()}-{secrets.token_hex(4)}"
        )
        try:
            with open(temp, "w", encoding="utf-8") as handle:
                handle.write(text)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temp, self.path)
        except BaseException:
            try:
                temp.unlink()
            except OSError:
                pass
            raise
        try:
            dir_fd = os.open(self.path.parent, os.O_RDONLY)
            try:
                os.fsync(dir_fd)
            finally:
                os.close(dir_fd)
        except OSError:
            pass

    def _mutate(self, now, action):
        _validate_now(now)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._locked(exclusive=True):
            document = self._read_document()
            result = action(document)
            self._publish(document)
            return result

    def _read(self, action):
        if not self.path.exists():
            return action(_empty_document())
        with self._locked(exclusive=False):
            return action(self._read_document())

    @staticmethod
    def _append_record(run, kind, now, stage, reasons):
        run["log"].append(
            {
                "seq": len(run["log"]) + 1,
                "kind": kind,
                "at": format_instant(now),
                "stage": stage,
                "reasons": list(reasons),
                "model_name": None,
                "model_confidence": None,
                "fallback_taken": None,
            }
        )

    def _require_run(self, document, run_id):
        run = _find_run(document, run_id)
        if run is None:
            raise NotFoundError(f"run {run_id!r} does not stand in the state file")
        return run

    def register(
        self,
        run_id,
        now,
        proof_interval_seconds=DEFAULT_PROOF_INTERVAL_SECONDS,
        missed_proof_limit=DEFAULT_MISSED_PROOF_LIMIT,
        soft_ceiling=None,
        hard_ceiling=None,
    ):
        """Open a run record and record the registration."""
        validate_name(run_id)
        _validate_positive_int(proof_interval_seconds, "proof interval")
        if proof_interval_seconds > PROOF_INTERVAL_MAX_SECONDS:
            raise InvalidRequestError(
                f"invalid proof interval {_printable(proof_interval_seconds)}, "
                f"use a positive integer of at most "
                f"{PROOF_INTERVAL_MAX_SECONDS} seconds"
            )
        _validate_positive_int(missed_proof_limit, "missed proof limit")
        _validate_ceiling(soft_ceiling, "soft ceiling")
        _validate_ceiling(hard_ceiling, "hard ceiling")
        if (
            soft_ceiling is not None
            and hard_ceiling is not None
            and soft_ceiling > hard_ceiling
        ):
            raise InvalidRequestError(
                "the soft ceiling stands above the hard ceiling, "
                "declare the soft ceiling at or below the hard ceiling"
            )

        def action(document):
            if _find_run(document, run_id) is not None:
                raise RefusalError(f"run {run_id!r} already stands in the state file")
            run = {
                "id": run_id,
                "registered_at": format_instant(now),
                "proof_interval_seconds": proof_interval_seconds,
                "missed_proof_limit": missed_proof_limit,
                "soft_ceiling": soft_ceiling,
                "hard_ceiling": hard_ceiling,
                "spend_total": 0,
                "last_proof_at": None,
                "stage": "none",
                "clear_streak": 0,
                "killed": False,
                "log": [],
            }
            self._append_record(run, "registered", now, "none", [])
            document["runs"].append(run)
            return None

        return self._mutate(now, action)

    def prove(self, run_id, note, now):
        """Record a progress proof. Content is never judged in this slice."""
        validate_name(run_id)
        _validate_note(note)

        def action(document):
            run = self._require_run(document, run_id)
            if not run["killed"]:
                if run["last_proof_at"] is None or now > _parse_instant(
                    run["last_proof_at"], _corrupt
                ):
                    run["last_proof_at"] = format_instant(now)
            self._append_record(run, "proof", now, run["stage"], [])
            return None

        return self._mutate(now, action)

    def spend(self, run_id, amount, now):
        """Record a spend report and return the accumulated spend."""
        validate_name(run_id)
        if not _is_int(amount) or amount < 0 or amount > SPEND_MAX:
            raise InvalidRequestError(
                f"invalid spend amount {_printable(amount)}, use a "
                f"nonnegative integer of at most {SPEND_MAX}"
            )

        def action(document):
            run = self._require_run(document, run_id)
            if not run["killed"]:
                if run["spend_total"] + amount > SPEND_MAX:
                    raise InvalidRequestError(
                        f"the spend report of {amount} would carry the "
                        f"accumulated spend of run {run_id!r} past {SPEND_MAX}"
                    )
                run["spend_total"] += amount
            self._append_record(run, "spend", now, run["stage"], [])
            return run["spend_total"]

        return self._mutate(now, action)

    def pull(self, run_id, now):
        """Pull the switch. A pull goes straight to kill and latches it."""
        validate_name(run_id)
        _validate_now(now)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._locked(exclusive=True):
            document = self._read_document()
            run = self._require_run(document, run_id)
            if run["killed"]:
                return "kill"
            run["stage"] = "kill"
            run["killed"] = True
            run["clear_streak"] = 0
            self._append_record(run, "pull", now, "kill", ["operator pull"])
            self._publish(document)
            return "kill"

    def stage(self, run_id, now):
        """Evaluate one run at the instant and report its stage and reasons."""
        validate_name(run_id)
        _validate_now(now)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._locked(exclusive=True):
            document = self._read_document()
            run = self._require_run(document, run_id)
            if run["killed"]:
                _, reasons = _warrant(run, now)
                return StageReport(run_id, "kill", reasons)
            warranted, reasons = _warrant(run, now)
            recorded = STAGES.index(run["stage"])
            changed = False
            if warranted > recorded:
                run["stage"] = STAGES[warranted]
                run["clear_streak"] = 0
                if run["stage"] == "kill":
                    run["killed"] = True
                self._append_record(run, "stage", now, run["stage"], reasons)
                changed = True
            elif warranted == recorded:
                if run["clear_streak"] != 0:
                    run["clear_streak"] = 0
                    changed = True
                if recorded != STAGES.index("none"):
                    self._append_record(run, "stage", now, run["stage"], reasons)
                    changed = True
            else:
                run["clear_streak"] += 1
                changed = True
                if run["clear_streak"] >= 2:
                    run["stage"] = STAGES[recorded - 1]
                    run["clear_streak"] = 0
                    self._append_record(run, "stage", now, run["stage"], reasons)
            if changed:
                self._publish(document)
            return StageReport(run_id, run["stage"], reasons)

    def log(self, run_id):
        """Return the recorded decision log of one run, in order."""
        validate_name(run_id)

        def action(document):
            run = self._require_run(document, run_id)
            return [dict(record, reasons=list(record["reasons"])) for record in run["log"]]

        return self._read(action)
