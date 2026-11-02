# deadman

A dead man's switch for agents, one staged cutoff with three triggers, a human pull, a budget ceiling, or a missing progress proof.

## What this is

Frameworks log a thinking agent and a wedged agent identically, and a large share of teams cannot stop a runaway agent's spend in real time at all. Deadman is the single cutoff for the fleet, merging the kill switch and the progress switch into one mechanism because an operator does not install two emergency stops. Every agent must keep proving it is making progress, and a budget ceiling watches the same runs from the cost side. When any trigger trips, the cutoff stages down in order, warn, throttle, cut tools, cut budget, kill the process tree, with the reason recorded either way. The boundary with its siblings is one sentence. Prod is the rescue ladder, deadman is the last rung. In plain terms, deadman is the final mechanism that stops a run when every other way of recovering it has already failed.

## Status

The remote seed was verified on 2026-11-02 against the staged seed and its commit is aa48f0fe5d57b8bc0e01db2c687ed128e1e0f2d4, a parentless commit holding exactly README.md and LICENSE and .gitignore. Setup arrives through a pull request and no work lands on main directly.

The contract lives in CONTRACT.md. The full contract landed with Round 1 slice 1 as the amendment to the setup stub, where obligations DM-1 to DM-12 state the stage semantics and the limits this slice keeps. ROADMAP.md stands as it landed and this slice does not change it.

Round 1 slice 1 builds the staged cutoff that decides and records. Hosts register runs, post progress proofs and spend reports, and poll the stage of a run through python -m deadman against one state file in the deadman-state/1 format. The evaluator walks a run down the stages on missed proofs and on spend ceilings, an operator pull goes straight to kill and latches it, and every decision stands in the decision log of the run. This slice signals no process, and hosts enforce the stage against their own processes.

Closed perfection rounds are 1 of 100. A round counts only when a full cycle of work and audit and review and red team and approval has closed, and seeding and setup are not rounds.

The local gate is python -m pytest -q plus python scripts/codepoint_scan.py, run on the exact final head before any push, with the continuous integration workflow running the same commands as a backstop.

## Licence

MIT. See LICENSE.
