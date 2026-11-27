"""Type stubs for the arvin Python package."""

from collections.abc import Callable
from os import PathLike
from typing import Any

__version__: str
MODEL_NAME: str
LEGACY_MODEL_NAME: str
JUDGE_MODELS: tuple[str, ...]

class ArvinError(Exception): ...
class InvalidNameError(ArvinError): ...
class InvalidRequestError(ArvinError): ...
class NotFoundError(ArvinError): ...
class RefusalError(ArvinError): ...
class CorruptError(ArvinError): ...
class FailureError(ArvinError): ...

def register(
    state: str | PathLike[str],
    run: str,
    now: str,
    proof_interval: int | None = None,
    missed_limit: int | None = None,
    soft_ceiling: int | None = None,
    hard_ceiling: int | None = None,
) -> dict[str, Any]: ...
def prove(
    state: str | PathLike[str],
    run: str,
    now: str,
    note: str | None = None,
) -> None: ...
def spend(
    state: str | PathLike[str],
    run: str,
    amount: int,
    now: str,
) -> int: ...
def pull(
    state: str | PathLike[str],
    run: str,
    now: str,
) -> str: ...
def stage(
    state: str | PathLike[str],
    run: str,
    now: str,
) -> dict[str, Any]: ...
def stage_with_model(
    state: str | PathLike[str],
    run: str,
    now: str,
    judge: str | None = None,
    judge_args: list[str] | tuple[str, ...] | None = None,
    judge_timeout: int | float | None = None,
    fixture: dict[str, Any] | None = None,
    host_context: str | None = None,
    judge_model: str | None = None,
    transport: Callable[[dict[str, Any]], str] | None = None,
) -> dict[str, Any]: ...
def log(
    state: str | PathLike[str],
    run: str,
) -> list[dict[str, Any]]: ...
def runs(
    state: str | PathLike[str],
) -> list[dict[str, Any]]: ...
def detect_provider(model: str) -> str: ...
def format_provider_request(
    payload: str | dict[str, Any],
    provider: str | None = None,
) -> str: ...
def normalize_provider_reply(
    raw_reply: str,
    provider: str | None = None,
    model: str | None = None,
) -> str: ...

__all__: list[str]
