"""arvin, the staged kill switch for agent runs with a Rust core.

This package is the thin Python face of the arvin library.
Each function calls the library and returns the facts the
command line reports, as plain dictionaries where the facts
carry names, and a refusal raises a subclass of ArvinError
carrying the same message text the command line prints.
InvalidNameError covers a run id in the wrong shape,
InvalidRequestError a request value that is not usable,
NotFoundError a run missing from the state file, RefusalError
a standing state that refuses the operation, CorruptError a
state file that cannot be read as recorded, and FailureError
a failure of the machine under the state file, mirroring the
error classes of the library itself.

Every operation but the log read takes the current instant
as ISO 8601 text the library parses, exactly as the library
takes it, and no call reads a clock of its own. The register
call returns the stored facts of the run it opens, the stage
call returns the stage name and its reasons, the
stage_with_model call returns the same report for an
evaluation that consulted a model, the spend call returns
the accumulated spend, the pull call returns the stage name,
and the log call returns the decision records with every
field they stand in the state file with, the five model
fields among them, carried exactly as they stand in the
file, null in every record the deterministic operations
write and populated on the stage record of a consultation.
"""

from ._arvin import (
    CorruptError,
    ArvinError,
    FailureError,
    InvalidNameError,
    InvalidRequestError,
    NotFoundError,
    RefusalError,
    log,
    prove,
    pull,
    register,
    spend,
    stage,
    stage_with_model,
)

__all__ = [
    "CorruptError",
    "ArvinError",
    "FailureError",
    "InvalidNameError",
    "InvalidRequestError",
    "NotFoundError",
    "RefusalError",
    "log",
    "prove",
    "pull",
    "register",
    "spend",
    "stage",
    "stage_with_model",
]

__version__ = "0.1.0"
