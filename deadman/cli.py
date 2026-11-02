"""Command line for deadman, a thin shell over deadman.core.

Every command names the state file first. The clock the command
line uses is the wall clock in UTC, and no flag overrides it, so
the evaluation a caller sees is the evaluation the machine clock
gives. Errors the switch raises are printed as one line on stderr
with the exit code the error carries, and no traceback is ever
printed for them.
"""

import argparse
import sys
from datetime import datetime, timezone

from .core import DeadmanError, Switch


def _now():
    return datetime.now(timezone.utc)


def build_parser():
    parser = argparse.ArgumentParser(
        prog="deadman",
        description=(
            "Watch agent runs and walk them down a staged cutoff "
            "as the evidence of a runaway grows."
        ),
    )
    parser.add_argument(
        "--state",
        required=True,
        help="the state file holding the runs, created by the first mutation",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_register = sub.add_parser("register", help="open a run record")
    p_register.add_argument("run")
    p_register.add_argument(
        "--proof-interval",
        type=int,
        default=60,
        help="the proof interval in seconds, default 60",
    )
    p_register.add_argument(
        "--missed-limit",
        type=int,
        default=5,
        help="the missed proof limit that warrants kill, default 5",
    )
    p_register.add_argument(
        "--soft-ceiling",
        type=int,
        default=None,
        help="the spend at or above which cut budget is warranted",
    )
    p_register.add_argument(
        "--hard-ceiling",
        type=int,
        default=None,
        help="the spend at or above which kill is warranted",
    )

    p_prove = sub.add_parser("prove", help="record a progress proof for a run")
    p_prove.add_argument("run")
    p_prove.add_argument(
        "--note", default="", help="a short progress note, never judged"
    )

    p_spend = sub.add_parser("spend", help="record a spend report for a run")
    p_spend.add_argument("run")
    p_spend.add_argument(
        "--amount", required=True, type=int, help="the spend to add, in host units"
    )

    p_pull = sub.add_parser("pull", help="pull the switch and kill the run")
    p_pull.add_argument("run")

    p_stage = sub.add_parser("stage", help="evaluate a run and report its stage")
    p_stage.add_argument("run")

    p_log = sub.add_parser("log", help="show the recorded decision log of a run")
    p_log.add_argument("run")
    return parser


def _log_line(record):
    line = (
        f"{record['seq']} {record['kind']} at={record['at']} "
        f"stage={record['stage']}"
    )
    for reason in record["reasons"]:
        line += f" reason={reason}"
    return line


def main(argv=None):
    args = build_parser().parse_args(argv)
    switch = Switch(args.state)
    try:
        if args.command == "register":
            switch.register(
                args.run,
                _now(),
                proof_interval_seconds=args.proof_interval,
                missed_proof_limit=args.missed_limit,
                soft_ceiling=args.soft_ceiling,
                hard_ceiling=args.hard_ceiling,
            )
            print(f"registered {args.run}")
        elif args.command == "prove":
            switch.prove(args.run, args.note, _now())
            print(f"proved {args.run}")
        elif args.command == "spend":
            total = switch.spend(args.run, args.amount, _now())
            print(f"spend_total {total}")
        elif args.command == "pull":
            stage = switch.pull(args.run, _now())
            print(f"stage {stage}")
        elif args.command == "stage":
            report = switch.stage(args.run, _now())
            print(f"stage {report.stage}")
            for reason in report.reasons:
                print(f"reason {reason}")
        elif args.command == "log":
            for record in switch.log(args.run):
                print(_log_line(record))
    except DeadmanError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return exc.exit_code
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
