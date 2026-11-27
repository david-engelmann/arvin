#!/usr/bin/env python3
"""Standalone standard-library CLI supervisor walkthrough for arvin.

Runs in a temporary directory, driving the arvin binary through registration,
progress proof, missed-proof stage rise, non-mutating run summary inspection,
latched operator pull, and decision log reading.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def resolve_arvin_bin(explicit: str | None) -> str:
    if explicit:
        return explicit
    env_bin = os.environ.get("CARGO_BIN_EXE_arvin")
    if env_bin:
        return env_bin
    repo_root = Path(__file__).resolve().parent.parent
    exe_name = "arvin.exe" if os.name == "nt" else "arvin"
    for profile in ("debug", "release"):
        candidate = repo_root / "target" / profile / exe_name
        if candidate.is_file():
            return str(candidate)
    found = shutil.which("arvin")
    if found:
        return found
    raise RuntimeError(
        "arvin binary not found; build with `cargo build --bin arvin` or pass --arvin-bin"
    )


def run_cli(arvin_bin: str, state_path: Path, *args: str) -> str:
    proc = subprocess.run(
        [arvin_bin, "--state", str(state_path), *args],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"arvin {' '.join(args)} exited {proc.returncode}: {proc.stderr.strip()}"
        )
    return proc.stdout.strip()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="supervisor_loop.py",
        description="Run a standalone CLI supervisor walkthrough with arvin.",
    )
    parser.add_argument(
        "--arvin-bin",
        default=None,
        help="Path to the arvin binary (defaults to CARGO_BIN_EXE_arvin, target/debug, or PATH)",
    )
    ns = parser.parse_args(argv)
    arvin_bin = resolve_arvin_bin(ns.arvin_bin)

    with tempfile.TemporaryDirectory(prefix="arvin-supervisor-") as tmp:
        state_path = Path(tmp) / "state.json"

        reg = run_cli(
            arvin_bin,
            state_path,
            "register",
            "--proof-interval",
            "3",
            "--missed-limit",
            "5",
            "--soft-ceiling",
            "100",
            "--hard-ceiling",
            "200",
            "loop-run",
        )
        assert reg == "registered loop-run", reg
        print(reg)

        proved = run_cli(
            arvin_bin, state_path, "prove", "--note", "unit tests green", "loop-run"
        )
        assert proved == "proved loop-run", proved
        print(proved)

        time.sleep(3.2)
        stage_out = run_cli(arvin_bin, state_path, "stage", "loop-run")
        stage_lines = stage_out.splitlines()
        assert "stage warn" in stage_lines, stage_out
        assert "reason missed proofs" in stage_lines, stage_out
        print(stage_out)

        runs_out = run_cli(arvin_bin, state_path, "runs")
        assert "loop-run stage=warn killed=false" in runs_out, runs_out
        print(runs_out)

        pull_out = run_cli(arvin_bin, state_path, "pull", "loop-run")
        assert pull_out == "stage kill", pull_out
        print(pull_out)

        log_out = run_cli(arvin_bin, state_path, "log", "loop-run")
        log_lines = log_out.splitlines()
        assert len(log_lines) == 4, log_out
        assert log_lines[-1].endswith("stage=kill reason=operator pull"), log_out
        print(f"verified {len(log_lines)} log records")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
