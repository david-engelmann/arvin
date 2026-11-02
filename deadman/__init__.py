"""Deadman, the staged kill switch for agent runs.

Hosts register runs, post progress proofs and spend reports, and
poll the switch for the stage of a run and the reasons for it,
then enforce the stage against their own processes.
"""

from .core import (
    FORMAT_NAME,
    STAGES,
    TRIGGERS,
    CorruptStateError,
    DeadmanError,
    InvalidNameError,
    InvalidRequestError,
    NotFoundError,
    RefusalError,
    StageReport,
    Switch,
)

__all__ = [
    "FORMAT_NAME",
    "STAGES",
    "TRIGGERS",
    "CorruptStateError",
    "DeadmanError",
    "InvalidNameError",
    "InvalidRequestError",
    "NotFoundError",
    "RefusalError",
    "StageReport",
    "Switch",
]
