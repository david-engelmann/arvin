# arvin

A staged cutoff for agent runs, where a run keeps its tools, budget, and sandbox only while it keeps proving progress. Three triggers move the stage, an operator pull, a budget ceiling, and missed proofs.

Arvin is a staged cutoff carried by a Rust library with a command line and a thin Python binding. A host registers a run against one state file, posts progress proofs and spend reports while the run works, and polls the stage of the run. Arvin walks the stage down its ladder as the triggers trip, and every evaluation and decision stands in the decision log of the run in the same file. Arvin signals no process itself. The host reads the stage and enforces it against its own processes.

[![CI](https://github.com/david-engelmann/arvin/actions/workflows/ci.yml/badge.svg)](https://github.com/david-engelmann/arvin/actions/workflows/ci.yml)

```
$ arvin --state state.json register --proof-interval 3 --missed-limit 5 --soft-ceiling 100 --hard-ceiling 200 demo-run
registered demo-run
$ arvin --state state.json prove --note "first proof posted" demo-run
proved demo-run
$ sleep 4
$ arvin --state state.json stage demo-run
stage warn
reason missed proofs
```

## Install

Install from a clean checkout with Cargo. The checkout ships two binaries, and the install names the arvin binary on its own because a shared scanner binary of the same name from a sibling repo blocks a plain install of every binary at once.

```
git clone https://github.com/david-engelmann/arvin.git
cd arvin
cargo install --path . --bin arvin
```

## Quickstart

The smallest complete run registers a run with a three second proof interval, posts a proof, lets the interval lapse so the stage rises on missed proofs, then posts proofs again so two clean evaluations walk the stage back down.

```
$ arvin --state state.json register --proof-interval 3 --missed-limit 5 --soft-ceiling 100 --hard-ceiling 200 demo-run
registered demo-run
$ arvin --state state.json prove --note "first proof posted" demo-run
proved demo-run
$ sleep 4
$ arvin --state state.json stage demo-run
stage warn
reason missed proofs
$ arvin --state state.json prove --note "back on track" demo-run
proved demo-run
$ arvin --state state.json stage demo-run
stage warn
$ arvin --state state.json prove --note "still on track" demo-run
proved demo-run
$ arvin --state state.json stage demo-run
stage none
```

## Usage

### Register, prove, and read the stage

Every command names the state file first. A run is registered once, with the ceilings its spend is judged against. A proof records an instant. Its optional note is validated but not stored or judged.

```
$ arvin --state usage.json register --soft-ceiling 100 --hard-ceiling 200 usage-run
registered usage-run
$ arvin --state usage.json prove --note "parser landed" usage-run
proved usage-run
$ arvin --state usage.json stage usage-run
stage none
```

### Spend against the ceilings

A spend report adds to the total of the run. The next evaluation reads the total against the ceilings declared at registration.

```
$ arvin --state usage.json spend --amount 150 usage-run
spend_total 150
$ arvin --state usage.json stage usage-run
stage cut budget
reason soft ceiling
```

### The decision log and the pull

The log shows every record of a run in order. A pull is the operator trigger. It moves the run straight to kill and latches it there.

```
$ arvin --state usage.json log usage-run
1 registered at=2026-11-07T23:21:10.287132+00:00 stage=none
2 proof at=2026-11-07T23:21:10.381314+00:00 stage=none
3 spend at=2026-11-07T23:21:10.480126+00:00 stage=none
4 stage at=2026-11-07T23:21:10.636543+00:00 stage=cut budget reason=soft ceiling
$ arvin --state usage.json pull usage-run
stage kill
$ arvin --state usage.json log usage-run
1 registered at=2026-11-07T23:21:10.287132+00:00 stage=none
2 proof at=2026-11-07T23:21:10.381314+00:00 stage=none
3 spend at=2026-11-07T23:21:10.480126+00:00 stage=none
4 stage at=2026-11-07T23:21:10.636543+00:00 stage=cut budget reason=soft ceiling
5 pull at=2026-11-07T23:21:10.839467+00:00 stage=kill reason=operator pull
```

### The model seat

Between the middle stages the signals can conflict, and the library carries one model consulting entry point, stage_with_model, for those seats. The model advises inside a deterministic envelope and never performs the action. An escalation it recommends is applied only at or above a confidence bar of 0.60, a de-escalation only at or above 0.85 with the signals that raised the stage checked clear by ordinary code, and an abstention or a failed call holds the stage. Round 5 carried the entry point to the command line and the Python binding on the same terms, and both surfaces stand below beside the library form.

The run below is offline. It replays recorded case DM-E01 from tests/fixtures/escalation through the fixture transport the test suite uses, with the recorded consultation of the case, a verdict of escalate from jev-1.13.0 at confidence 0.77. The state is built by the landed operations from the construction the corpus records, a spend of 41 against a soft ceiling of 100 and a hard ceiling of 200, and one deterministic evaluation that rises to warn on missed proofs. The consultation then steps the run to throttle, and the record it appends carries the model fields.

```
stage before consultation: warn
stage after consultation: throttle
reason missed proofs
record 4 stage stage=throttle
model_name jev-1.13.0
model_verdict escalate
model_confidence 0.77
model_score 0.5555555555555556
fallback_taken null
```

The command line names a judge in one of two forms. The --judge form names a program the transport runs with the arguments the caller names followed by `--raw-payload` and the path of a temporary file holding the consultation document, and the program prints the reply body to its standard output, and the --judge-fixture form names a file holding a reply body that is read through the same grammar with no process spawned. A judge that cannot be started, or answers past its timeout, or answers outside the grammar lands in the fallback classes the library records, and the printed lines name the class. The two runs below stand at warn with the held seat open. Each was registered through the binding at named instants with an hour proof interval and raised by one missed proof, the construction the command line cannot stage itself because it reads the wall clock alone. The helper prints the recorded reply body of case DM-E01, and reply.json holds a copy of the same body, the original standing in the repo at tests/fixtures/escalation/replies/DM-E01.json.

```
$ cat judge.sh
#!/bin/sh
printf '%s' '{"model": "jev-1.13.0", "choice": {"label": "escalate", "confidence": 0.77}, "score": {"level": 6}}'
$ arvin --state model.json stage model-run --judge ./judge.sh
stage throttle
reason missed proofs
model jev-1.13.0
verdict escalate
confidence 0.77
score 0.5555555555555556
$ arvin --state model.json stage fixture-run --judge-fixture reply.json
stage throttle
reason missed proofs
model jev-1.13.0
verdict escalate
confidence 0.77
score 0.5555555555555556
```

### Python binding

The binding drives the same lifecycle at instants the caller names, and a refusal raises an error carrying the message text of the command line.

```python
import arvin

state = "state.json"

opened = arvin.register(
    state,
    "binding-run",
    "2031-05-04T12:00:00+00:00",
    soft_ceiling=100,
    hard_ceiling=200,
)
print("registered", opened["id"], "stage", opened["stage"])

arvin.prove(
    state, "binding-run", "2031-05-04T12:00:30+00:00", note="halfway there"
)
total = arvin.spend(state, "binding-run", 150, "2031-05-04T12:00:40+00:00")
print("spend_total", total)

report = arvin.stage(state, "binding-run", "2031-05-04T12:00:40+00:00")
print("stage", report["stage"], "reasons", report["reasons"])

print("pull", arvin.pull(state, "binding-run", "2031-05-04T12:00:50+00:00"))

for entry in arvin.log(state, "binding-run"):
    print("log", entry["seq"], entry["kind"], entry["stage"])
```

```
registered binding-run stage none
spend_total 150
stage cut budget reasons ['soft ceiling']
pull kill
log 1 registered none
log 2 proof none
log 3 spend none
log 4 stage cut budget
log 5 pull kill
```

The binding carries the same model entry point as stage_with_model. A caller names a judge program or a fixture dictionary standing in for the consultation, and the call returns the stage report of the evaluation it ran. The fixture dictionary below carries the same consultation the command line examples above drive, an escalate verdict from jev-1.13.0 at confidence 0.77, and the log read shows the fields the consultation record carries.

```python
import arvin

state = "state.json"

arvin.register(state, "model-run", "2031-05-04T12:00:00+00:00")
report = arvin.stage(state, "model-run", "2031-05-04T12:01:01+00:00")
print("stage", report["stage"], "reasons", report["reasons"])

report = arvin.stage_with_model(
    state,
    "model-run",
    "2031-05-04T12:01:01+00:00",
    fixture={
        "model_name": "jev-1.13.0",
        "verdict": "escalate",
        "confidence": 0.77,
        "score": 5 / 9,
    },
)
print("stage", report["stage"], "reasons", report["reasons"])

entry = arvin.log(state, "model-run")[-1]
print("verdict", entry["model_verdict"], "confidence", entry["model_confidence"])
```

```
stage warn reasons ['missed proofs']
stage throttle reasons ['missed proofs']
verdict escalate confidence 0.77
```

## Status

The staged cutoff works end to end through the command line, the library, and the Python binding against one state file in the deadman-state/1 format. Registration, proofs, spend reports, stage evaluation, the operator pull, and the decision log all stand landed and tested. The evaluator walks a run down the ladder none, warn, throttle, cut tools, cut budget, kill on missed proofs and on spend ceilings, and a pull goes straight to kill and latches it. Arvin signals no process itself, and hosts enforce the stage against their own processes.

The model seat stands landed in the library on the terms above and stands carried to the command line and the Python binding on the same terms. In the Round 4 landing measurement the model reaches the fall seat and recommends de-escalation there, and no endorsement has landed because its confidence stays under the bar. No live model call runs in the gate, and the recorded fixture corpus is what the gate replays.

What is not built. Arvin has no daemon and no watcher, so a stage moves only when a host calls an evaluation. It has no network service, and its state is one file on one machine for callers sharing the filesystem.

Closed perfection rounds are 16 of 100. A round counts only when a full cycle of work and audit and review and red team and approval has closed, and seeding and setup are not rounds.

The contract lives in CONTRACT.md, where obligations DM-1 to DM-23 state the stage semantics, the limits, and the behaviour of the model seat and its records, and ROADMAP.md records the decision model doctrine the seat implements.

The remote seed was verified on 2026-11-02 against the staged seed and its commit is aa48f0fe5d57b8bc0e01db2c687ed128e1e0f2d4, a parentless commit holding exactly README.md and LICENSE and .gitignore.

## License

MIT. See [LICENSE](LICENSE).
