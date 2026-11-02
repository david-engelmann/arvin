# deadman

A dead man's switch for agents, one staged cutoff with three triggers, a human pull, a budget ceiling, or a missing progress proof.

## What this is

Frameworks log a thinking agent and a wedged agent identically, and a large share of teams cannot stop a runaway agent's spend in real time at all. Deadman is the single cutoff for the fleet, merging the kill switch and the progress switch into one mechanism because an operator does not install two emergency stops. Every agent must keep proving it is making progress, and a budget ceiling watches the same runs from the cost side. When any trigger trips, the cutoff stages down in order, warn, throttle, cut tools, cut budget, kill the process tree, with the reason recorded either way. The boundary with its siblings is one sentence. Prod is the rescue ladder, deadman is the last rung.

## Status

The remote seed was verified on 2026-11-02 against the staged seed and its commit is aa48f0fe5d57b8bc0e01db2c687ed128e1e0f2d4, a parentless commit holding exactly README.md and LICENSE and .gitignore. Setup arrives through a pull request and no work lands on main directly.

The contract lives in CONTRACT.md. Setup obligations S1 to S5 are the contract state until the full contract lands as the first build act of this repo, and this setup slice claims no feature behaviour for deadman. ROADMAP.md stands as it landed ahead of setup and this slice does not change it.

Closed perfection rounds are 0 of 100. A round counts only when a full cycle of work and audit and review and red team and approval has closed, and seeding and setup are not rounds.

The local gate is python -m pytest -q plus python scripts/codepoint_scan.py, run on the exact final head before any push, with the continuous integration workflow running the same commands as a backstop.

## Licence

MIT. See LICENSE.
