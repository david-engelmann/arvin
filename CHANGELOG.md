# Changelog

All notable changes to arvin are recorded in this file.

## 0.2.0 on 2026-11-26

- Default decision model advanced to `jev-1.14.0` with backward-compatible reading and replay of recorded `jev-1.13.0` fixtures under `DM-24`.
- First-class provider wire codecs and `JUDGE_MODELS` support added across TypeSafe (`jev-1.14.0`, `jev-1.13.0`), Google (`gemini-decision-1`), OpenAI (`gpt-6-luna`, `gpt-oss-safeguard-120b`, `gpt-oss-safeguard-20b`), Perplexity (`pplx-decider-v1.3`, `pplx-decider-latest`, `pplx-decider-flash-v1.3`, `pplx-decider-flash-latest`), and Cloudflare (`@cf/cloudflare/clef`, `@cf/cloudflare/clef-flash`, `clef`, `clef-flash`), plus standalone HTTPS helper `examples/decision_judge.py`.
- Configurable `--judge-model` on the command line and `judge_model` plus callable `transport` on `arvin.stage_with_model` in Python.
- Read-only `runs` on `Switch`, `arvin --state STATE runs`, and `arvin.runs(state)` summary inspection added alongside top-level `-V` and `--version` flags under `DM-25`.
- Cross-platform lock identity and portability CI added across Linux, macOS, and Windows, with PyO3 `0.30` `abi3-py39` bindings, PEP 561 `py.typed` and `__init__.pyi` type stubs, and automated `v0.2.0` release workflows under `DM-26`.

## 0.1.0 on 2026-11-15

- Deterministic staged escalation engine (`none`, `warn`, `throttle`, `cut tools`, `cut budget`, `kill`) over `deadman-state/1` with missed-proof counting, spend ceilings, two-evaluation step-down hysteresis, and latched operator pull (`DM-1` through `DM-12`).
- Decision model consultation seat (`stage_with_model`), rendered vignette (`DM-14`), confidence bars (`DM-15`), fallback classes (`DM-16`), subprocess and fixture transports (`DM-20`, `DM-21`), and certified step-down ancestry (`DM-22`, `DM-23`).
