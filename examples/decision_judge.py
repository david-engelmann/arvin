#!/usr/bin/env python3
"""Standalone standard-library System One judge helper for arvin under DM-21 and DM-24.

Reads the canonical DM-21 payload file named by --raw-payload, formats it for the
selected System One provider, posts it over HTTPS, and writes a canonical DM-21
reply JSON body to standard output. Uses zero third-party dependencies.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_MODEL = "jev-1.14.0"
VERDICT_LABELS = ("hold", "escalate", "de-escalate")
CHOICE_INSTRUCTIONS = "Choose the stage verdict supported by the picture, or other to abstain."
SCORE_INSTRUCTIONS = (
    "Rate the likelihood that the run is running away across the ten ordered levels."
)
OTHER_CRITERION = "None of the listed verdicts is supported by the picture."
MAX_REPLY_BYTES = 65_536

ENDPOINTS = {
    "typesafe": "https://api.typesafe.ai/v1/systemone",
    "google": "https://generativelanguage.googleapis.com/v1beta/decisions",
    "openai": "https://api.openai.com/v1/decisions",
    "perplexity": "https://api.perplexity.ai/v1/decisions",
}

ACCOUNT_ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,128}$")
ROUTE_SEG_RE = re.compile(r"^[A-Za-z0-9._@-]+$")


class EnvelopeFailureError(ValueError):
    """Raised when an upstream provider envelope explicitly reports failure."""


class MalformedReplyError(ValueError):
    """Raised when a completed provider response fails DM-21 or provider grammar."""


def _reject_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
    out: dict[str, object] = {}
    for key, val in pairs:
        if key in out:
            raise MalformedReplyError(f"duplicate JSON key {key}")
        out[key] = val
    return out


def _reject_nonfinite(token: str) -> object:
    raise MalformedReplyError(f"non-finite JSON constant {token}")


def _parse_finite_float(token: str) -> float:
    num = float(token)
    if not math.isfinite(num):
        raise MalformedReplyError("overflowing float literal in JSON")
    return num


def strict_loads(text: str) -> object:
    try:
        return json.loads(
            text,
            object_pairs_hook=_reject_duplicates,
            parse_constant=_reject_nonfinite,
            parse_float=_parse_finite_float,
        )
    except (json.JSONDecodeError, ValueError, RecursionError) as exc:
        raise MalformedReplyError(str(exc)) from exc


def is_valid_model_id(name: object) -> bool:
    if not isinstance(name, str) or not (1 <= len(name) <= 128):
        return False
    return all(0x21 <= ord(ch) <= 0x7E for ch in name)


def detect_provider(model: str) -> str:
    if model.startswith("typesafe/") or model.startswith("jev-"):
        return "typesafe"
    if (
        model.startswith("google/")
        or model.startswith("models/gemini-decision-")
        or model.startswith("gemini-decision-")
    ):
        return "google"
    if (
        model.startswith("openai/")
        or model.startswith("gpt-6-")
        or model.startswith("gpt-oss-safeguard")
    ):
        return "openai"
    if model.startswith("perplexity/") or model.startswith("pplx-decider"):
        return "perplexity"
    if (
        model.startswith("cloudflare/")
        or model.startswith("@cf/cloudflare/clef")
        or model.startswith("clef")
    ):
        return "cloudflare"
    return "typesafe"


def wire_model_name(provider: str, model: str) -> str:
    if provider == "typesafe":
        return model.removeprefix("typesafe/")
    if provider == "google":
        stripped = model.removeprefix("google/")
        return stripped.removeprefix("models/")
    if provider == "openai":
        return model.removeprefix("openai/")
    if provider == "perplexity":
        return model.removeprefix("perplexity/")
    if provider == "cloudflare":
        stripped = model.removeprefix("cloudflare/")
        if stripped == "@cf/cloudflare/clef":
            return "clef"
        if stripped == "@cf/cloudflare/clef-flash":
            return "clef-flash"
        if stripped == "@cf/cloudflare/clef-omni":
            return "clef-omni"
        return stripped
    raise ValueError(f"unknown provider {provider}")


def cloudflare_route_model(model: str) -> str:
    stripped = model.removeprefix("cloudflare/")
    if stripped == "clef":
        return "@cf/cloudflare/clef"
    if stripped == "clef-flash":
        return "@cf/cloudflare/clef-flash"
    if stripped == "clef-omni":
        return "@cf/cloudflare/clef-omni"
    return stripped


def validate_https_url(url: str) -> str:
    parsed = urllib.parse.urlsplit(url)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
    ):
        raise ValueError("endpoint URL must use https with an explicit host and no userinfo")
    try:
        port = parsed.port
    except ValueError as exc:
        raise ValueError("endpoint URL carries an invalid port") from exc
    if port is not None and not (1 <= port <= 65535):
        raise ValueError("endpoint URL port stands outside 1 through 65535")
    if any(ord(ch) <= 0x20 or ord(ch) >= 0x7F for ch in url):
        raise ValueError("endpoint URL contains whitespace or control characters")
    return url


def cloudflare_endpoint(account_id: str, model: str) -> str:
    if not ACCOUNT_ID_RE.match(account_id):
        raise ValueError("invalid Cloudflare account identifier")
    if not is_valid_model_id(model):
        raise ValueError("invalid Cloudflare model identifier")
    route = cloudflare_route_model(model)
    if not route or route.startswith("/") or route.endswith("/"):
        raise ValueError("invalid Cloudflare model route")
    for seg in route.split("/"):
        if not seg or seg in (".", "..") or not ROUTE_SEG_RE.match(seg):
            raise ValueError("disallowed Cloudflare route segment")
    return f"https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run/{route}"


def resolve_endpoint(provider: str, model: str, explicit_url: str | None) -> str:
    override = explicit_url or os.environ.get("DECISION_API_URL")
    if override:
        return validate_https_url(override)
    if provider == "cloudflare":
        account_id = os.environ.get("CLOUDFLARE_ACCOUNT_ID", "").strip()
        if not account_id:
            raise ValueError("CLOUDFLARE_ACCOUNT_ID is required for the Cloudflare provider")
        return cloudflare_endpoint(account_id, model)
    return ENDPOINTS[provider]


def format_provider_request(doc: dict[str, object], provider: str, model: str) -> dict[str, object]:
    if not is_valid_model_id(model):
        raise ValueError("invalid model identifier")
    state = doc.get("state")
    questions = doc.get("questions")
    if not isinstance(state, str) or not isinstance(questions, list) or len(questions) != 2:
        raise ValueError("DM-21 payload must carry state string and two question objects")
    q_choice, q_score = questions
    if not isinstance(q_choice, dict) or not isinstance(q_score, dict):
        raise ValueError("DM-21 questions must be objects")
    labels = q_choice.get("labels")
    levels = q_score.get("levels")
    if (
        q_choice.get("kind") != "choice"
        or not isinstance(labels, list)
        or len(labels) != 3
        or q_score.get("kind") != "score"
        or not isinstance(levels, list)
        or len(levels) != 10
        or not all(isinstance(item, str) for item in levels)
    ):
        raise ValueError("DM-21 choice or score question shape is invalid")

    criteria_map: dict[str, str] = {}
    openai_choices: list[dict[str, str]] = []
    for idx, expected in enumerate(VERDICT_LABELS):
        entry = labels[idx]
        if (
            not isinstance(entry, dict)
            or entry.get("label") != expected
            or not isinstance(entry.get("criterion"), str)
        ):
            raise ValueError("DM-21 choice labels must stand in hold, escalate, de-escalate order")
        crit = str(entry["criterion"])
        criteria_map[expected] = crit
        openai_choices.append({"value": expected, "description": crit})
    criteria_map["other"] = OTHER_CRITERION
    openai_choices.append({"value": "other", "description": OTHER_CRITERION})

    wire_model = wire_model_name(provider, model)
    if not wire_model or wire_model.startswith("/") or wire_model.endswith("/"):
        raise ValueError("model identifier resolves to an empty provider model name")

    verdict_q = {
        "type": "choice",
        "instructions": CHOICE_INSTRUCTIONS,
        "criteria": criteria_map,
    }
    score_q = {
        "type": "score",
        "instructions": SCORE_INSTRUCTIONS,
        "criteria": list(levels),
    }

    if provider in ("typesafe", "perplexity", "cloudflare"):
        return {
            "model": wire_model,
            "state": state,
            "questions": {"verdict": verdict_q, "score": score_q},
        }
    if provider == "google":
        return {
            "model": wire_model,
            "input": state,
            "questions": {"verdict": verdict_q, "score": score_q},
        }
    if provider == "openai":
        openai_levels = [
            {"label": str(idx), "description": str(text)}
            for idx, text in enumerate(levels)
        ]
        return {
            "model": wire_model,
            "input": state,
            "questions": [
                {
                    "name": "verdict",
                    "type": "choice",
                    "instructions": CHOICE_INSTRUCTIONS,
                    "choices": openai_choices,
                },
                {
                    "name": "score",
                    "type": "score",
                    "instructions": SCORE_INSTRUCTIONS,
                    "levels": openai_levels,
                },
            ],
        }
    raise ValueError(f"unknown provider {provider}")


def _finite_float(val: object) -> float | None:
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        return None
    try:
        num = float(val)
    except OverflowError:
        return None
    if not math.isfinite(num):
        return None
    return num


def _unit_prob(val: object) -> float | None:
    num = _finite_float(val)
    if num is None or not (0.0 <= num <= 1.0):
        return None
    return num


def _score_to_level(raw_zero_to_nine: float) -> int:
    # Half-away-from-zero tie rounding on 0.0..=9.0 to match Rust f64::round().
    rounded = int(math.floor(raw_zero_to_nine + 0.5))
    return max(1, min(10, rounded + 1))


def _parse_level_index(item: dict[str, object]) -> int | None:
    val_opt: int | None = None
    if "value" in item:
        num = _finite_float(item["value"])
        if num is None or not num.is_integer() or not (0.0 <= num <= 9.0):
            return None
        val_opt = int(num)
    lbl_opt: int | None = None
    if "label" in item:
        lbl = item["label"]
        if not isinstance(lbl, str) or lbl not in {str(i) for i in range(10)}:
            return None
        lbl_opt = int(lbl)
    if val_opt is not None and lbl_opt is not None:
        return val_opt if val_opt == lbl_opt else None
    return val_opt if val_opt is not None else lbl_opt


def _weighted_score(probs: object) -> float | None:
    if isinstance(probs, list):
        if len(probs) != 10:
            return None
        weighted = 0.0
        total = 0.0
        if isinstance(probs[0], dict):
            seen = [False] * 10
            for item in probs:
                if not isinstance(item, dict):
                    return None
                lvl = _parse_level_index(item)
                if lvl is None or seen[lvl]:
                    return None
                seen[lvl] = True
                p = _unit_prob(item.get("probability"))
                if p is None:
                    return None
                weighted += lvl * p
                total += p
        else:
            for idx, item in enumerate(probs):
                if isinstance(item, dict):
                    return None
                p = _unit_prob(item)
                if p is None:
                    return None
                weighted += idx * p
                total += p
        return max(0.0, min(9.0, weighted / total)) if 0.99 <= total <= 1.01 else None
    if isinstance(probs, dict):
        if len(probs) != 10:
            return None
        weighted = 0.0
        total = 0.0
        for idx in range(10):
            key = str(idx)
            if key not in probs:
                return None
            p = _unit_prob(probs[key])
            if p is None:
                return None
            weighted += idx * p
            total += p
        return max(0.0, min(9.0, weighted / total)) if 0.99 <= total <= 1.01 else None
    return None


def _extract_optional_level(entry: object) -> int | None:
    if not isinstance(entry, dict):
        raise MalformedReplyError("score entry is not an object")
    for key in ("score", "expected_score", "expected"):
        if key in entry and entry[key] is not None:
            raw = _finite_float(entry[key])
            if raw is None or not (0.0 <= raw <= 9.0):
                raise MalformedReplyError("score stands outside zero through nine")
            return _score_to_level(raw)
    if "probabilities" in entry or "distribution" in entry:
        probs = entry.get("probabilities", entry.get("distribution"))
        if probs is None or probs == {} or probs == []:
            return None
        raw = _weighted_score(probs)
        if raw is None:
            raise MalformedReplyError("score probabilities do not form a ten-level distribution")
        return _score_to_level(raw)
    return None


def _find_unique_named(items: list[object], target: str) -> dict[str, object] | None:
    found: dict[str, object] | None = None
    for item in items:
        if not isinstance(item, dict):
            raise MalformedReplyError("answers array entry is not an object")
        if item.get("name") == target:
            if found is not None:
                raise MalformedReplyError(f"duplicate {target} entry in answers array")
            found = item
    return found


def _parse_choice_probs(raw: object) -> dict[str, float] | None:
    if raw is None:
        return None
    if isinstance(raw, dict):
        out: dict[str, float] = {}
        for key, val in raw.items():
            p = _unit_prob(val)
            if p is None:
                raise MalformedReplyError("choice probability stands outside 0 to 1")
            out[str(key)] = p
        return out
    if isinstance(raw, list):
        out = {}
        for item in raw:
            if not isinstance(item, dict):
                raise MalformedReplyError("choice probability item is not an object")
            label = item.get("value") if isinstance(item.get("value"), str) else item.get("label")
            if not isinstance(label, str) or label in out:
                raise MalformedReplyError("choice probability item has invalid or duplicate label")
            p = _unit_prob(item.get("probability"))
            if p is None:
                raise MalformedReplyError("choice probability stands outside 0 to 1")
            out[label] = p
        return out
    raise MalformedReplyError("choice probabilities must be an object or array")


def normalize_provider_reply(raw_text: str, provider: str, pinned_model: str) -> str:
    if not is_valid_model_id(pinned_model):
        raise ValueError("pinned model identifier is invalid")
    if not raw_text.strip():
        return ""
    root = strict_loads(raw_text)
    if not isinstance(root, dict):
        raise MalformedReplyError("provider reply is not a JSON object")

    if "success" in root:
        if root["success"] is not True:
            raise EnvelopeFailureError("provider envelope reported unsuccessful call")
        if not isinstance(root.get("result"), dict):
            raise EnvelopeFailureError("provider envelope carries no result object")
        payload = root["result"]
    elif provider == "cloudflare" and "result" in root:
        if not isinstance(root["result"], dict):
            raise EnvelopeFailureError("provider envelope carries non-object result")
        payload = root["result"]
    else:
        payload = root

    if (
        isinstance(payload.get("choice"), dict)
        and "answers" not in payload
        and "decisions" not in payload
        and "output" not in payload
    ):
        dm21_model = payload.get("model")
        if dm21_model != pinned_model:
            raise MalformedReplyError("DM-21 reply names a foreign model")
        choice_obj = payload["choice"]
        assert isinstance(choice_obj, dict)
        label = choice_obj.get("label")
        if label is not None and label not in VERDICT_LABELS:
            raise MalformedReplyError("DM-21 reply carries unknown label")
        clean_choice: dict[str, object] = {"label": label}
        if "confidence" in choice_obj:
            conf_val = _unit_prob(choice_obj["confidence"])
            if conf_val is None:
                raise MalformedReplyError("DM-21 reply carries invalid confidence")
            clean_choice["confidence"] = conf_val
        out_dm21: dict[str, object] = {"model": dm21_model, "choice": clean_choice}
        if label is not None:
            if "confidence" not in clean_choice:
                raise MalformedReplyError("DM-21 verdict reply lacks confidence")
            score_obj = payload.get("score")
            if not isinstance(score_obj, dict):
                raise MalformedReplyError("DM-21 verdict reply lacks score")
            lvl = score_obj.get("level")
            if isinstance(lvl, bool) or not isinstance(lvl, int) or not (1 <= lvl <= 10):
                raise MalformedReplyError("DM-21 verdict level outside 1 to 10")
            out_dm21["score"] = {"level": lvl}
        elif "score" in payload:
            score_obj = payload["score"]
            if not isinstance(score_obj, dict):
                raise MalformedReplyError("DM-21 abstention score is not an object")
            lvl = score_obj.get("level")
            if isinstance(lvl, bool) or not isinstance(lvl, int) or not (1 <= lvl <= 10):
                raise MalformedReplyError("DM-21 abstention level outside 1 to 10")
            out_dm21["score"] = {"level": lvl}
        return json.dumps(out_dm21, allow_nan=False)

    container_found = False
    container: object = None
    for key in ("answers", "decisions", "output"):
        if key in payload:
            container = payload[key]
            container_found = True
            break
    if not container_found:
        raise MalformedReplyError("provider reply carries no answers container")

    if isinstance(container, dict):
        verdict_raw = container.get("verdict")
        score_raw = container["score"] if "score" in container else container.get("risk")
    elif isinstance(container, list):
        verdict_raw = _find_unique_named(container, "verdict")
        s_item = _find_unique_named(container, "score")
        r_item = _find_unique_named(container, "risk")
        if s_item is not None and r_item is not None:
            raise MalformedReplyError("answers array carries both score and risk")
        score_raw = s_item if s_item is not None else r_item
    else:
        raise MalformedReplyError("answers container is neither object nor array")

    if not isinstance(verdict_raw, dict):
        raise MalformedReplyError("verdict entry is missing or not an object")
    if "choice" in verdict_raw:
        choice = verdict_raw["choice"]
    elif "selected" in verdict_raw:
        choice = verdict_raw["selected"]
    else:
        raise MalformedReplyError("verdict entry carries no choice")

    probs = _parse_choice_probs(
        verdict_raw.get("probabilities", verdict_raw.get("distribution"))
    )

    if choice is None:
        out: dict[str, object] = {"model": pinned_model, "choice": {"label": None}}
        if score_raw is not None:
            opt_lvl = _extract_optional_level(score_raw)
            if opt_lvl is not None:
                out["score"] = {"level": opt_lvl}
        return json.dumps(out, allow_nan=False)

    if choice == "other":
        choice_out: dict[str, object] = {"label": None}
        if probs is not None and "other" in probs:
            choice_out["confidence"] = probs["other"]
        out = {"model": pinned_model, "choice": choice_out}
        if score_raw is not None:
            opt_lvl = _extract_optional_level(score_raw)
            if opt_lvl is not None:
                out["score"] = {"level": opt_lvl}
        return json.dumps(out, allow_nan=False)

    if isinstance(choice, str) and choice in VERDICT_LABELS:
        if probs is None or choice not in probs:
            raise MalformedReplyError("verdict probabilities lack chosen label")
        if score_raw is None:
            raise MalformedReplyError("verdict reply lacks score")
        opt_lvl = _extract_optional_level(score_raw)
        if opt_lvl is None:
            raise MalformedReplyError("verdict reply lacks numeric score")
        out = {
            "model": pinned_model,
            "choice": {"label": choice, "confidence": probs[choice]},
            "score": {"level": opt_lvl},
        }
        return json.dumps(out, allow_nan=False)

    raise MalformedReplyError("verdict choice names an unknown label")


def _clean_secret(env_names: tuple[str, ...], label: str) -> str:
    for name in env_names:
        val = os.environ.get(name, "").strip()
        if val:
            if any(ord(ch) < 0x20 or ord(ch) == 0x7F for ch in val):
                raise ValueError(f"{name} contains control characters")
            return val
    raise ValueError(f"missing API key for {label}")


def auth_headers(provider: str) -> dict[str, str]:
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    if provider == "typesafe":
        key = _clean_secret(("TYPESAFE_API_KEY", "DECISION_API_KEY"), "typesafe")
        headers["Authorization"] = f"Bearer {key}"
    elif provider == "google":
        key = _clean_secret(("GEMINI_API_KEY", "GOOGLE_API_KEY", "DECISION_API_KEY"), "google")
        headers["x-goog-api-key"] = key
    elif provider == "openai":
        key = _clean_secret(("OPENAI_API_KEY", "DECISION_API_KEY"), "openai")
        headers["Authorization"] = f"Bearer {key}"
    elif provider == "perplexity":
        key = _clean_secret(("PERPLEXITY_API_KEY", "DECISION_API_KEY"), "perplexity")
        headers["Authorization"] = f"Bearer {key}"
    elif provider == "cloudflare":
        key = _clean_secret(("CLOUDFLARE_API_TOKEN", "DECISION_API_KEY"), "cloudflare")
        headers["Authorization"] = f"Bearer {key}"
    return headers


class _HttpsSameHostRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if code not in (307, 308):
            raise urllib.error.URLError(f"refusing status {code} redirect on POST consultation")
        validate_https_url(newurl)
        orig = urllib.parse.urlsplit(req.full_url)
        dest = urllib.parse.urlsplit(newurl)
        if dest.scheme != "https" or dest.netloc != orig.netloc:
            raise urllib.error.URLError("refusing cross-host or non-HTTPS redirect")
        return urllib.request.Request(
            newurl,
            data=req.data,
            headers=dict(req.header_items()),
            method="POST",
        )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--provider",
        choices=("typesafe", "google", "openai", "perplexity", "cloudflare"),
        help="Override auto-detected System One provider dialect",
    )
    parser.add_argument("--model", help="Override model identifier sent to the provider")
    parser.add_argument("--api-url", help="Override HTTPS endpoint URL")
    parser.add_argument("--timeout", type=float, default=9.0, help="HTTP timeout in seconds")
    parser.add_argument("--offline-reply", help="Read raw provider response from file without network")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print formatted provider request metadata without making a network call",
    )
    parser.add_argument(
        "--raw-payload",
        required=True,
        help="Path to the temporary DM-21 payload file supplied by arvin",
    )
    args = parser.parse_args(argv)

    try:
        with open(args.raw_payload, encoding="utf-8") as handle:
            doc = strict_loads(handle.read())
        if not isinstance(doc, dict):
            raise ValueError("DM-21 payload must be a JSON object")
        pinned_model = doc.get("model")
        if not is_valid_model_id(pinned_model):
            raise ValueError("DM-21 payload model is invalid")
        assert isinstance(pinned_model, str)
        request_model = args.model or pinned_model
        if not is_valid_model_id(request_model):
            raise ValueError("override model is invalid")
        provider = args.provider or detect_provider(request_model)
        wire_req = format_provider_request(doc, provider, request_model)
        body_bytes = json.dumps(wire_req, allow_nan=False).encode("utf-8")

        if args.dry_run:
            endpoint = resolve_endpoint(provider, request_model, args.api_url)
            sys.stdout.write(
                json.dumps(
                    {
                        "provider": provider,
                        "endpoint": endpoint,
                        "body": wire_req,
                    },
                    allow_nan=False,
                )
                + "\n"
            )
            return 0

        reply_model = request_model if args.model else pinned_model
        if args.offline_reply:
            with open(args.offline_reply, encoding="utf-8") as r_handle:
                raw_reply = r_handle.read(MAX_REPLY_BYTES + 1)
            if len(raw_reply.encode("utf-8")) > MAX_REPLY_BYTES:
                sys.stdout.write("malformed oversized provider reply\n")
                return 0
        else:
            endpoint = resolve_endpoint(provider, request_model, args.api_url)
            headers = auth_headers(provider)
            opener = urllib.request.build_opener(_HttpsSameHostRedirectHandler)
            http_req = urllib.request.Request(endpoint, data=body_bytes, headers=headers, method="POST")
            with opener.open(http_req, timeout=args.timeout) as resp:
                raw_bytes = resp.read(MAX_REPLY_BYTES + 1)
            if len(raw_bytes) > MAX_REPLY_BYTES:
                sys.stdout.write("malformed oversized provider reply\n")
                return 0
            try:
                raw_reply = raw_bytes.decode("utf-8")
            except UnicodeDecodeError:
                sys.stdout.write("malformed non-utf8 provider reply\n")
                return 0

        try:
            normalized = normalize_provider_reply(raw_reply, provider, reply_model)
        except MalformedReplyError:
            sys.stdout.write("malformed provider reply\n")
            return 0
        except EnvelopeFailureError as exc:
            sys.stderr.write(f"decision_judge envelope error {exc}\n")
            return 1

        sys.stdout.write(normalized + ("\n" if normalized else ""))
        return 0
    except urllib.error.URLError as exc:
        sys.stderr.write(f"decision_judge transport error {exc}\n")
        return 1
    except Exception as exc:
        sys.stderr.write(f"decision_judge error {exc}\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
