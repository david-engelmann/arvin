# Fixture manifest

The state files in this folder were written by the Python implementation of deadman on main, driven through the public Switch API with injected instants, and frozen here as equivalence ground truth for the Rust port that opens Round 2. The base instant of every production run is the base instant of the Round 1 test suite, 2031-05-04T12:00:00+00:00. Truths for each fixture were driven on copies of the produced file, never on the fixture itself. Continuation fixtures were produced by copying a fixture and applying the recorded continuation operations to the copy under Python, so the Rust suite can check a continuation byte for byte. The Rust fixture suite reads each file, checks its decision log against the lines recorded here, evaluates it at the recorded probe instant where one stands, replays every production operation and compares the written state byte for byte, and continues three of the fixtures against the continuation files byte for byte.

The empty file stands apart from the others. It is the empty document the implementation publishes before any run registers, produced by publishing the empty document value directly, and the Rust side checks it reads as an empty switch and takes a registration.

## Inventory

- continued-killed.json, sha256 0d63bdb471b518351c91987a13f4deefa196d1749a4cc6e7619d7da91a5e5d2d, 1766 bytes
- continued-mid-ladder.json, sha256 6c74595ca2163fedf1c64fcfecc342b1a2de3e283208a9d296bf2a474871233c, 1857 bytes
- continued-warn.json, sha256 9c9e899c7164baccb4ca679a6e20fea9a7c59b267099f312148ac89257d2365a, 1528 bytes
- empty.json, sha256 00189e79df175357140f313d447b17be5fc15394d566d410cb3aa1bbd3ba4c81, 48 bytes
- full-log.json, sha256 dd6b6795475c57ecd27bf21a74c95caa93a1de16ee15d6b58224792c5272b778, 2506 bytes
- killed-by-hard-ceiling.json, sha256 eaffc5ff29992a923995da0d7d809baf97de79de8a086b49608dec735d6bb1f6, 1238 bytes
- killed-by-pull.json, sha256 beaea47112afe73659c3d9856b8d0e10ceafd6e71dfeb84c9b3c8993840be0cf, 1262 bytes
- mid-ladder.json, sha256 b72ef987323b4dd28ed9dd6b27c2e8777ea3ec6e9ae74afe0b1d66d29b39c8a8, 1249 bytes
- registered.json, sha256 eb179b0e60e40b3778ae11713cc06688aa002eeb4678fce56e49d1e079be99c1, 674 bytes
- warn.json, sha256 6d0687d0cee3862a8ffaed18791a3d8acd622f700513f0228e812f633e0ab8d4, 977 bytes

## Productions and truths

registered.json stands for a freshly registered run. Production is a registration at the base instant with the default thresholds. The probe instant is 30 seconds after the base instant, and the evaluation there reports stage none with no reasons. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
```

warn.json stands for a run standing at warn. Production is a registration at the base instant and an evaluation 61 seconds after it. The probe instant is 61 seconds after the base instant, and the evaluation there reports stage warn with the missed proofs reason. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 stage at=2031-05-04T12:01:01+00:00 stage=warn reason=missed proofs
```

mid-ladder.json stands for a run mid ladder with spend recorded. Production is a registration at the base instant with soft ceiling 100 and hard ceiling 1000, a spend of 40 one second after it, and an evaluation 121 seconds after it. The probe instant is 122 seconds after the base instant, and the evaluation there reports stage throttle with the missed proofs reason. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 spend at=2031-05-04T12:00:01+00:00 stage=none
3 stage at=2031-05-04T12:02:01+00:00 stage=throttle reason=missed proofs
```

killed-by-pull.json stands for a run latched at kill by an operator pull. Production is a registration at the base instant, a proof one second after it, and a pull two seconds after it. The probe instant is 500 seconds after the base instant, and the evaluation there reports stage kill with the missed proofs reason. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 proof at=2031-05-04T12:00:01+00:00 stage=none
3 pull at=2031-05-04T12:00:02+00:00 stage=kill reason=operator pull
```

killed-by-hard-ceiling.json stands for a run latched at kill by the hard ceiling. Production is a registration at the base instant with hard ceiling 10, a spend of 10 one second after it, and an evaluation two seconds after it. The probe instant is 500 seconds after the base instant, and the evaluation there reports stage kill with the hard ceiling and missed proofs reasons. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 spend at=2031-05-04T12:00:01+00:00 stage=none
3 stage at=2031-05-04T12:00:02+00:00 stage=kill reason=hard ceiling
```

full-log.json stands for a run with a full decision log, every record carrying the reserved model fields standing empty. Production is a registration at the base instant with soft ceiling 10 and hard ceiling 100, a proof 10 seconds after it, a spend of 10 eleven seconds after it, evaluations 12 seconds and 200 seconds and 201 seconds after it, and a pull 202 seconds after it. The probe instant is 500 seconds after the base instant, and the evaluation there reports stage kill with the soft ceiling and missed proofs reasons. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 proof at=2031-05-04T12:00:10+00:00 stage=none
3 spend at=2031-05-04T12:00:11+00:00 stage=none
4 stage at=2031-05-04T12:00:12+00:00 stage=cut budget reason=soft ceiling
5 stage at=2031-05-04T12:03:20+00:00 stage=cut budget reason=soft ceiling reason=missed proofs
6 stage at=2031-05-04T12:03:21+00:00 stage=cut budget reason=soft ceiling reason=missed proofs
7 pull at=2031-05-04T12:03:22+00:00 stage=kill reason=operator pull
```

continued-warn.json is the warn fixture continued under Python with a proof 61 seconds after the base instant and two evaluations at the same instant, walking the run back down to none through the hysteresis. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 stage at=2031-05-04T12:01:01+00:00 stage=warn reason=missed proofs
3 proof at=2031-05-04T12:01:01+00:00 stage=warn
4 stage at=2031-05-04T12:01:01+00:00 stage=none
```

continued-mid-ladder.json is the mid ladder fixture continued under Python with a spend of 60 at 122 seconds after the base instant, carrying the total to the soft ceiling, and an evaluation at 123 seconds. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 spend at=2031-05-04T12:00:01+00:00 stage=none
3 stage at=2031-05-04T12:02:01+00:00 stage=throttle reason=missed proofs
4 spend at=2031-05-04T12:02:02+00:00 stage=throttle
5 stage at=2031-05-04T12:02:03+00:00 stage=cut budget reason=soft ceiling reason=missed proofs
```

continued-killed.json is the killed by hard ceiling fixture continued under Python with a spend of 500 three seconds after the base instant and a proof four seconds after it, both recorded as no operations on the closed run. Its decision log lines are

```text
1 registered at=2031-05-04T12:00:00+00:00 stage=none
2 spend at=2031-05-04T12:00:01+00:00 stage=none
3 stage at=2031-05-04T12:00:02+00:00 stage=kill reason=hard ceiling
4 spend at=2031-05-04T12:00:03+00:00 stage=kill
5 proof at=2031-05-04T12:00:04+00:00 stage=kill
```

## Producing script

The script below produced every fixture above when run against the Python implementation on main. Its paths are recorded in the relative form of the paths the run used, the tree of the Python implementation standing beside it and the fixtures folder of this repo as the destination, with working copies and truth drives in a scratch folder the script makes. The lock files the drives leave beside the working copies are byproducts of the run and are not fixtures.

```python
import hashlib
import json
import shutil
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

TREE = Path("pybase") / "tree"
sys.path.insert(0, str(TREE))

from deadman import core
from deadman.core import Switch

T0 = datetime(2031, 5, 4, 12, 0, 0, tzinfo=timezone.utc)
WORK = Path("fixture-work")
OUT = Path("tests") / "fixtures"


def at(seconds):
    return T0 + timedelta(seconds=seconds)


def log_lines(switch, run_id="run-a"):
    lines = []
    for record in switch.log(run_id):
        line = (
            f"{record['seq']} {record['kind']} at={record['at']} "
            f"stage={record['stage']}"
        )
        for reason in record["reasons"]:
            line += f" reason={reason}"
        lines.append(line)
    return lines


def truth_for(path, probe_instant):
    copy_path = WORK / f"truth-{path.name}"
    shutil.copyfile(path, copy_path)
    switch = Switch(copy_path)
    report = switch.stage("run-a", probe_instant)
    return {
        "stage_at_probe": report.stage,
        "reasons_at_probe": report.reasons,
        "probe_instant": core.format_instant(probe_instant),
        "log_lines": log_lines(Switch(path)),
    }


def produce():
    WORK.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    truths = {}

    empty = Switch(WORK / "empty.json")
    empty._publish(core._empty_document())
    shutil.copyfile(WORK / "empty.json", OUT / "empty.json")
    truths["empty.json"] = {"log_lines": [], "stage_at_probe": None}

    registered = Switch(WORK / "registered.json")
    registered.register("run-a", T0)
    shutil.copyfile(WORK / "registered.json", OUT / "registered.json")
    truths["registered.json"] = truth_for(OUT / "registered.json", at(30))

    warn = Switch(WORK / "warn.json")
    warn.register("run-a", T0)
    warn.stage("run-a", at(61))
    shutil.copyfile(WORK / "warn.json", OUT / "warn.json")
    truths["warn.json"] = truth_for(OUT / "warn.json", at(61))

    mid = Switch(WORK / "mid-ladder.json")
    mid.register("run-a", T0, soft_ceiling=100, hard_ceiling=1000)
    mid.spend("run-a", 40, at(1))
    mid.stage("run-a", at(121))
    shutil.copyfile(WORK / "mid-ladder.json", OUT / "mid-ladder.json")
    truths["mid-ladder.json"] = truth_for(OUT / "mid-ladder.json", at(122))

    pulled = Switch(WORK / "killed-by-pull.json")
    pulled.register("run-a", T0)
    pulled.prove("run-a", "working", at(1))
    pulled.pull("run-a", at(2))
    shutil.copyfile(WORK / "killed-by-pull.json", OUT / "killed-by-pull.json")
    truths["killed-by-pull.json"] = truth_for(OUT / "killed-by-pull.json", at(500))

    ceiling = Switch(WORK / "killed-by-hard-ceiling.json")
    ceiling.register("run-a", T0, hard_ceiling=10)
    ceiling.spend("run-a", 10, at(1))
    ceiling.stage("run-a", at(2))
    shutil.copyfile(WORK / "killed-by-hard-ceiling.json", OUT / "killed-by-hard-ceiling.json")
    truths["killed-by-hard-ceiling.json"] = truth_for(
        OUT / "killed-by-hard-ceiling.json", at(500)
    )

    full = Switch(WORK / "full-log.json")
    full.register("run-a", T0, soft_ceiling=10, hard_ceiling=100)
    full.prove("run-a", "working", at(10))
    full.spend("run-a", 10, at(11))
    full.stage("run-a", at(12))
    full.stage("run-a", at(200))
    full.stage("run-a", at(201))
    full.pull("run-a", at(202))
    shutil.copyfile(WORK / "full-log.json", OUT / "full-log.json")
    truths["full-log.json"] = truth_for(OUT / "full-log.json", at(500))

    cont = WORK / "continued-warn.json"
    shutil.copyfile(OUT / "warn.json", cont)
    switch = Switch(cont)
    switch.prove("run-a", "working", at(61))
    switch.stage("run-a", at(61))
    switch.stage("run-a", at(61))
    shutil.copyfile(cont, OUT / "continued-warn.json")
    truths["continued-warn.json"] = {
        "log_lines": log_lines(Switch(OUT / "continued-warn.json"))
    }

    cont = WORK / "continued-mid-ladder.json"
    shutil.copyfile(OUT / "mid-ladder.json", cont)
    switch = Switch(cont)
    switch.spend("run-a", 60, at(122))
    switch.stage("run-a", at(123))
    shutil.copyfile(cont, OUT / "continued-mid-ladder.json")
    truths["continued-mid-ladder.json"] = {
        "log_lines": log_lines(Switch(OUT / "continued-mid-ladder.json"))
    }

    cont = WORK / "continued-killed.json"
    shutil.copyfile(OUT / "killed-by-hard-ceiling.json", cont)
    switch = Switch(cont)
    switch.spend("run-a", 500, at(3))
    switch.prove("run-a", "late proof", at(4))
    shutil.copyfile(cont, OUT / "continued-killed.json")
    truths["continued-killed.json"] = {
        "log_lines": log_lines(Switch(OUT / "continued-killed.json"))
    }

    inventory = {}
    for path in sorted(OUT.glob("*.json")):
        raw = path.read_bytes()
        inventory[path.name] = {
            "sha256": hashlib.sha256(raw).hexdigest(),
            "bytes": len(raw),
        }
    print(json.dumps({"inventory": inventory, "truths": truths}, indent=2))
```
