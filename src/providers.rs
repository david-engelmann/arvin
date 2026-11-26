//! Pure zero-network wire codecs for System One decision model providers under DM-24.
//!
//! Every transformation in this module is deterministic and offline, holding no
//! network client, no clock, and no credential. Helper processes and callers
//! translate canonical DM-21 payloads into provider wire requests and normalize
//! raw provider replies back into canonical DM-21 reply bodies.

use crate::evaluate::Consultation;
use crate::join::{field_opt, model_number_unit, object_pairs};
use crate::reader::parse_document;
use crate::transport::consultation_from_reply_body;
use crate::value::{Error, JsonInt, Result, Value};
use crate::writer::dump_value;
use crate::{is_valid_model_id, validate_model_id, MODEL_NAME, VERDICT_LABELS};

const CHOICE_INSTRUCTIONS: &str =
    "Choose the stage verdict supported by the picture, or other to abstain.";
const SCORE_INSTRUCTIONS: &str =
    "Rate the likelihood that the run is running away across the ten ordered levels.";
const OTHER_CRITERION: &str = "None of the listed verdicts is supported by the picture.";

/// One of the five supported System One decision model provider dialects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionProvider {
    TypeSafe,
    Google,
    OpenAi,
    Perplexity,
    Cloudflare,
}

impl DecisionProvider {
    /// Canonical lowercase provider identifier.
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionProvider::TypeSafe => "typesafe",
            DecisionProvider::Google => "google",
            DecisionProvider::OpenAi => "openai",
            DecisionProvider::Perplexity => "perplexity",
            DecisionProvider::Cloudflare => "cloudflare",
        }
    }

    /// Parse a canonical lowercase provider identifier.
    pub fn from_name(name: &str) -> Option<DecisionProvider> {
        match name {
            "typesafe" => Some(DecisionProvider::TypeSafe),
            "google" => Some(DecisionProvider::Google),
            "openai" => Some(DecisionProvider::OpenAi),
            "perplexity" => Some(DecisionProvider::Perplexity),
            "cloudflare" => Some(DecisionProvider::Cloudflare),
            _ => None,
        }
    }

    /// Default HTTPS endpoint for this provider family.
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            DecisionProvider::TypeSafe => "https://api.typesafe.ai/v1/systemone",
            DecisionProvider::Google => "https://generativelanguage.googleapis.com/v1beta/decisions",
            DecisionProvider::OpenAi => "https://api.openai.com/v1/decisions",
            DecisionProvider::Perplexity => "https://api.perplexity.ai/v1/decisions",
            DecisionProvider::Cloudflare => {
                "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run/@cf/cloudflare/clef"
            }
        }
    }
}

/// Auto-detect the System One provider dialect from a model identifier.
pub fn detect_provider(model: &str) -> DecisionProvider {
    if model.starts_with("typesafe/") || model.starts_with("jev-") {
        DecisionProvider::TypeSafe
    } else if model.starts_with("google/")
        || model.starts_with("models/gemini-decision-")
        || model.starts_with("gemini-decision-")
    {
        DecisionProvider::Google
    } else if model.starts_with("openai/")
        || model.starts_with("gpt-6-")
        || model.starts_with("gpt-oss-safeguard")
    {
        DecisionProvider::OpenAi
    } else if model.starts_with("perplexity/") || model.starts_with("pplx-decider") {
        DecisionProvider::Perplexity
    } else if model.starts_with("cloudflare/")
        || model.starts_with("@cf/cloudflare/clef")
        || model.starts_with("clef")
    {
        DecisionProvider::Cloudflare
    } else {
        DecisionProvider::TypeSafe
    }
}

/// Resolve the JSON body model identifier sent inside a provider request payload.
pub fn wire_model_name(provider: DecisionProvider, model: &str) -> String {
    match provider {
        DecisionProvider::TypeSafe => model.strip_prefix("typesafe/").unwrap_or(model).to_string(),
        DecisionProvider::Google => {
            let stripped = model.strip_prefix("google/").unwrap_or(model);
            stripped
                .strip_prefix("models/")
                .unwrap_or(stripped)
                .to_string()
        }
        DecisionProvider::OpenAi => model.strip_prefix("openai/").unwrap_or(model).to_string(),
        DecisionProvider::Perplexity => model
            .strip_prefix("perplexity/")
            .unwrap_or(model)
            .to_string(),
        DecisionProvider::Cloudflare => {
            let stripped = model.strip_prefix("cloudflare/").unwrap_or(model);
            match stripped {
                "@cf/cloudflare/clef" => "clef".to_string(),
                "@cf/cloudflare/clef-flash" => "clef-flash".to_string(),
                "@cf/cloudflare/clef-omni" => "clef-omni".to_string(),
                other => other.to_string(),
            }
        }
    }
}

/// Resolve the Cloudflare Workers AI URL route identifier for a model name.
pub fn cloudflare_route_model(model: &str) -> String {
    let stripped = model.strip_prefix("cloudflare/").unwrap_or(model);
    match stripped {
        "clef" => "@cf/cloudflare/clef".to_string(),
        "clef-flash" => "@cf/cloudflare/clef-flash".to_string(),
        "clef-omni" => "@cf/cloudflare/clef-omni".to_string(),
        other => other.to_string(),
    }
}

fn valid_cloudflare_account_id(account_id: &str) -> bool {
    !account_id.is_empty()
        && account_id.len() <= 128
        && account_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_cloudflare_route(route: &str) -> bool {
    if route.is_empty() || route.starts_with('/') || route.ends_with('/') {
        return false;
    }
    for seg in route.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return false;
        }
        if !seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
        {
            return false;
        }
    }
    true
}

/// Build and validate the Cloudflare Workers AI HTTPS endpoint URL for an account and model.
pub fn cloudflare_endpoint(account_id: &str, model: &str) -> Result<String> {
    if !valid_cloudflare_account_id(account_id) {
        return Err(Error::InvalidRequest(
            "the Cloudflare account identifier contains invalid characters or is empty".to_string(),
        ));
    }
    validate_model_id(model)?;
    let route = cloudflare_route_model(model);
    if !valid_cloudflare_route(&route) {
        return Err(Error::InvalidRequest(
            "the Cloudflare model route contains disallowed path or query characters".to_string(),
        ));
    }
    Ok(format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run/{route}"
    ))
}

fn render_json(value: &Value) -> String {
    let mut out = String::new();
    dump_value(value, 0, &mut out);
    out
}

fn contains_non_finite_float(root: &Value) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node {
            Value::Float(n) if !n.is_finite() => return true,
            Value::Array(items) => stack.extend(items.iter()),
            Value::Object(pairs) => stack.extend(pairs.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
    false
}

fn number_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) if number.is_finite() => Some(*number),
        Value::Int(number) => (*number)
            .to_string()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite()),
        Value::BigInt(text) => text.parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

/// Format a canonical DM-21 payload JSON text into the wire JSON for a System One provider.
pub fn format_provider_request(
    dm21_payload: &str,
    provider: Option<DecisionProvider>,
) -> Result<String> {
    let doc = parse_document(dm21_payload.as_bytes()).map_err(|_| {
        Error::InvalidRequest("the judge payload is not a valid JSON document".to_string())
    })?;
    if contains_non_finite_float(&doc) {
        return Err(Error::InvalidRequest(
            "the judge payload carries a non-finite number".to_string(),
        ));
    }
    let pairs = object_pairs(&doc).ok_or_else(|| {
        Error::InvalidRequest("the judge payload is not a JSON object".to_string())
    })?;
    let model = match field_opt(pairs, "model") {
        Some(Value::Str(text)) => {
            validate_model_id(text)?;
            text.as_str()
        }
        _ => {
            return Err(Error::InvalidRequest(
                "the judge payload carries an invalid model identifier".to_string(),
            ))
        }
    };
    let state = match field_opt(pairs, "state") {
        Some(Value::Str(text)) => text.as_str(),
        _ => {
            return Err(Error::InvalidRequest(
                "the judge payload carries no state text".to_string(),
            ))
        }
    };
    let questions = match field_opt(pairs, "questions") {
        Some(Value::Array(items)) if items.len() == 2 => items,
        _ => {
            return Err(Error::InvalidRequest(
                "the judge payload must carry two question objects".to_string(),
            ))
        }
    };
    let choice_pairs = object_pairs(&questions[0])
        .ok_or_else(|| Error::InvalidRequest("the choice question is not an object".to_string()))?;
    if field_opt(choice_pairs, "kind").and_then(Value::as_str) != Some("choice") {
        return Err(Error::InvalidRequest(
            "the first question does not carry kind choice".to_string(),
        ));
    }
    let labels = match field_opt(choice_pairs, "labels") {
        Some(Value::Array(items)) if items.len() == 3 => items,
        _ => {
            return Err(Error::InvalidRequest(
                "the choice question must carry three label entries".to_string(),
            ))
        }
    };
    let mut criteria_pairs: Vec<(String, Value)> = Vec::with_capacity(4);
    let mut openai_choices: Vec<Value> = Vec::with_capacity(4);
    for (index, expected) in VERDICT_LABELS.iter().enumerate() {
        let lpairs = object_pairs(&labels[index]).ok_or_else(|| {
            Error::InvalidRequest("a choice label entry is not an object".to_string())
        })?;
        let label = field_opt(lpairs, "label")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidRequest("a choice label is missing".to_string()))?;
        if label != *expected {
            return Err(Error::InvalidRequest(
                "the choice labels do not stand in hold, escalate, de-escalate order".to_string(),
            ));
        }
        let criterion = field_opt(lpairs, "criterion")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidRequest("a choice criterion is missing".to_string()))?;
        criteria_pairs.push((label.to_string(), Value::Str(criterion.to_string())));
        openai_choices.push(Value::Object(vec![
            ("value".to_string(), Value::Str(label.to_string())),
            ("description".to_string(), Value::Str(criterion.to_string())),
        ]));
    }
    criteria_pairs.push(("other".to_string(), Value::Str(OTHER_CRITERION.to_string())));
    openai_choices.push(Value::Object(vec![
        ("value".to_string(), Value::Str("other".to_string())),
        (
            "description".to_string(),
            Value::Str(OTHER_CRITERION.to_string()),
        ),
    ]));

    let score_pairs = object_pairs(&questions[1])
        .ok_or_else(|| Error::InvalidRequest("the score question is not an object".to_string()))?;
    if field_opt(score_pairs, "kind").and_then(Value::as_str) != Some("score") {
        return Err(Error::InvalidRequest(
            "the second question does not carry kind score".to_string(),
        ));
    }
    let levels = match field_opt(score_pairs, "levels") {
        Some(Value::Array(items))
            if items.len() == 10 && items.iter().all(|v| v.as_str().is_some()) =>
        {
            items
        }
        _ => {
            return Err(Error::InvalidRequest(
                "the score question must carry ten level strings".to_string(),
            ))
        }
    };

    let resolved = provider.unwrap_or_else(|| detect_provider(model));
    let wire_model = wire_model_name(resolved, model);
    if wire_model.is_empty() || wire_model.starts_with('/') || wire_model.ends_with('/') {
        return Err(Error::InvalidRequest(
            "the judge model identifier resolves to an empty provider model name".to_string(),
        ));
    }

    let verdict_question = Value::Object(vec![
        ("type".to_string(), Value::Str("choice".to_string())),
        (
            "instructions".to_string(),
            Value::Str(CHOICE_INSTRUCTIONS.to_string()),
        ),
        ("criteria".to_string(), Value::Object(criteria_pairs)),
    ]);
    let score_question = Value::Object(vec![
        ("type".to_string(), Value::Str("score".to_string())),
        (
            "instructions".to_string(),
            Value::Str(SCORE_INSTRUCTIONS.to_string()),
        ),
        ("criteria".to_string(), Value::Array(levels.clone())),
    ]);

    let out_doc = match resolved {
        DecisionProvider::TypeSafe
        | DecisionProvider::Perplexity
        | DecisionProvider::Cloudflare => Value::Object(vec![
            ("model".to_string(), Value::Str(wire_model)),
            ("state".to_string(), Value::Str(state.to_string())),
            (
                "questions".to_string(),
                Value::Object(vec![
                    ("verdict".to_string(), verdict_question),
                    ("score".to_string(), score_question),
                ]),
            ),
        ]),
        DecisionProvider::Google => Value::Object(vec![
            ("model".to_string(), Value::Str(wire_model)),
            ("input".to_string(), Value::Str(state.to_string())),
            (
                "questions".to_string(),
                Value::Object(vec![
                    ("verdict".to_string(), verdict_question),
                    ("score".to_string(), score_question),
                ]),
            ),
        ]),
        DecisionProvider::OpenAi => {
            let verdict_item = Value::Object(vec![
                ("name".to_string(), Value::Str("verdict".to_string())),
                ("type".to_string(), Value::Str("choice".to_string())),
                (
                    "instructions".to_string(),
                    Value::Str(CHOICE_INSTRUCTIONS.to_string()),
                ),
                ("choices".to_string(), Value::Array(openai_choices)),
            ]);
            let mut openai_levels = Vec::with_capacity(levels.len());
            for (index, item) in levels.iter().enumerate() {
                let description = item.as_str().unwrap_or("");
                openai_levels.push(Value::Object(vec![
                    ("label".to_string(), Value::Str(index.to_string())),
                    (
                        "description".to_string(),
                        Value::Str(description.to_string()),
                    ),
                ]));
            }
            let score_item = Value::Object(vec![
                ("name".to_string(), Value::Str("score".to_string())),
                ("type".to_string(), Value::Str("score".to_string())),
                (
                    "instructions".to_string(),
                    Value::Str(SCORE_INSTRUCTIONS.to_string()),
                ),
                ("levels".to_string(), Value::Array(openai_levels)),
            ]);
            Value::Object(vec![
                ("model".to_string(), Value::Str(wire_model)),
                ("input".to_string(), Value::Str(state.to_string())),
                (
                    "questions".to_string(),
                    Value::Array(vec![verdict_item, score_item]),
                ),
            ])
        }
    };

    Ok(render_json(&out_doc))
}

fn find_unique_named_item<'a>(items: &'a [Value], target: &str) -> Result<Option<&'a Value>> {
    let mut found: Option<&'a Value> = None;
    for item in items {
        let pairs = object_pairs(item).ok_or_else(|| {
            Error::InvalidRequest("an answers array item is not an object".to_string())
        })?;
        if field_opt(pairs, "name").and_then(Value::as_str) == Some(target) {
            if found.is_some() {
                return Err(Error::InvalidRequest(format!(
                    "the provider answers array carries duplicate {target} entries"
                )));
            }
            found = Some(item);
        }
    }
    Ok(found)
}

fn parse_level_index(ipairs: &[(String, Value)]) -> Option<usize> {
    let val_opt = match field_opt(ipairs, "value") {
        None => None,
        Some(v) => {
            let num = number_f64(v)?;
            if num.fract() != 0.0 || !(0.0..=9.0).contains(&num) {
                return None;
            }
            Some(num as usize)
        }
    };
    let lbl_opt = match field_opt(ipairs, "label") {
        None => None,
        Some(Value::Str(s)) => {
            let idx = s.parse::<usize>().ok()?;
            if idx >= 10 || s != &idx.to_string() {
                return None;
            }
            Some(idx)
        }
        Some(_) => return None,
    };
    match (val_opt, lbl_opt) {
        (Some(v), Some(l)) if v == l => Some(v),
        (Some(_), Some(_)) => None,
        (Some(v), None) => Some(v),
        (None, Some(l)) => Some(l),
        (None, None) => None,
    }
}

fn weighted_level_score(probs: &Value) -> Option<f64> {
    if let Value::Array(items) = probs {
        if items.len() != 10 {
            return None;
        }
        let mut weighted = 0.0;
        let mut total = 0.0;
        if object_pairs(&items[0]).is_some() {
            let mut seen = [false; 10];
            for item in items {
                let ipairs = object_pairs(item)?;
                let lvl = parse_level_index(ipairs)?;
                if seen[lvl] {
                    return None;
                }
                seen[lvl] = true;
                let p = field_opt(ipairs, "probability").and_then(model_number_unit)?;
                weighted += (lvl as f64) * p;
                total += p;
            }
        } else {
            for (index, item) in items.iter().enumerate() {
                if object_pairs(item).is_some() {
                    return None;
                }
                let p = model_number_unit(item)?;
                weighted += (index as f64) * p;
                total += p;
            }
        }
        return if (0.99..=1.01).contains(&total) {
            Some((weighted / total).clamp(0.0, 9.0))
        } else {
            None
        };
    }
    if let Some(pairs) = object_pairs(probs) {
        if pairs.len() != 10 {
            return None;
        }
        let mut weighted = 0.0;
        let mut total = 0.0;
        for index in 0..10usize {
            let key = index.to_string();
            let item = field_opt(pairs, &key)?;
            let p = model_number_unit(item)?;
            weighted += (index as f64) * p;
            total += p;
        }
        return if (0.99..=1.01).contains(&total) {
            Some((weighted / total).clamp(0.0, 9.0))
        } else {
            None
        };
    }
    None
}

fn extract_optional_zero_based_score(entry: &Value) -> Result<Option<f64>> {
    let pairs = object_pairs(entry).ok_or_else(|| {
        Error::InvalidRequest("the provider score entry is not an object".to_string())
    })?;
    for key in ["score", "expected_score", "expected"] {
        if let Some(val) = field_opt(pairs, key) {
            if *val == Value::Null {
                continue;
            }
            let raw = number_f64(val).ok_or_else(|| {
                Error::InvalidRequest("the provider score is not a finite number".to_string())
            })?;
            if (0.0..=9.0).contains(&raw) {
                return Ok(Some(raw));
            }
            return Err(Error::InvalidRequest(
                "the provider score stands outside the zero through nine range".to_string(),
            ));
        }
    }
    if let Some(probs) =
        field_opt(pairs, "probabilities").or_else(|| field_opt(pairs, "distribution"))
    {
        let is_empty = match probs {
            Value::Null => true,
            Value::Array(items) => items.is_empty(),
            Value::Object(entries) => entries.is_empty(),
            _ => false,
        };
        if is_empty {
            return Ok(None);
        }
        let raw = weighted_level_score(probs).ok_or_else(|| {
            Error::InvalidRequest(
                "the provider score probabilities do not form a valid ten-level distribution"
                    .to_string(),
            )
        })?;
        return Ok(Some(raw));
    }
    Ok(None)
}

fn score_to_level(raw_zero_to_nine: f64) -> i128 {
    ((raw_zero_to_nine.round() as i128) + 1).clamp(1, 10)
}

fn parse_choice_probabilities(raw: Option<&Value>) -> Result<Option<Vec<(String, f64)>>> {
    let Some(raw_val) = raw else {
        return Ok(None);
    };
    if *raw_val == Value::Null {
        return Ok(None);
    }
    if let Some(pairs) = object_pairs(raw_val) {
        let mut out = Vec::with_capacity(pairs.len());
        for (key, val) in pairs {
            let p = model_number_unit(val).ok_or_else(|| {
                Error::InvalidRequest(
                    "the provider verdict probabilities carry a value outside 0 to 1".to_string(),
                )
            })?;
            out.push((key.clone(), p));
        }
        return Ok(Some(out));
    }
    if let Value::Array(items) = raw_val {
        let mut out: Vec<(String, f64)> = Vec::with_capacity(items.len());
        for item in items {
            let ipairs = object_pairs(item).ok_or_else(|| {
                Error::InvalidRequest(
                    "a provider choice probability entry is not an object".to_string(),
                )
            })?;
            let label = field_opt(ipairs, "value")
                .and_then(Value::as_str)
                .or_else(|| field_opt(ipairs, "label").and_then(Value::as_str))
                .ok_or_else(|| {
                    Error::InvalidRequest(
                        "a provider choice probability entry carries no value label".to_string(),
                    )
                })?;
            if out.iter().any(|(existing, _)| existing == label) {
                return Err(Error::InvalidRequest(
                    "the provider choice probabilities repeat a label".to_string(),
                ));
            }
            let prob_val = field_opt(ipairs, "probability").ok_or_else(|| {
                Error::InvalidRequest(
                    "a provider choice probability entry carries no probability".to_string(),
                )
            })?;
            let p = model_number_unit(prob_val).ok_or_else(|| {
                Error::InvalidRequest(
                    "the provider verdict probabilities carry a value outside 0 to 1".to_string(),
                )
            })?;
            out.push((label.to_string(), p));
        }
        return Ok(Some(out));
    }
    Err(Error::InvalidRequest(
        "the provider verdict probabilities stand as neither an object nor an array".to_string(),
    ))
}

/// Normalize a raw System One provider response body into a canonical DM-21 reply JSON text.
pub fn normalize_provider_reply(
    raw_reply: &str,
    provider: Option<DecisionProvider>,
    pinned_model: Option<&str>,
) -> Result<String> {
    if let Some(name) = pinned_model {
        validate_model_id(name)?;
    }
    if raw_reply.trim().is_empty() {
        return Ok(String::new());
    }
    let root = parse_document(raw_reply.as_bytes()).map_err(|_| {
        Error::InvalidRequest("the provider reply is not a valid JSON document".to_string())
    })?;
    if contains_non_finite_float(&root) {
        return Err(Error::InvalidRequest(
            "the provider reply carries a non-finite number".to_string(),
        ));
    }
    let root_pairs = object_pairs(&root).ok_or_else(|| {
        Error::InvalidRequest("the provider reply is not a JSON object".to_string())
    })?;

    let root_model = field_opt(root_pairs, "model").and_then(Value::as_str);
    let effective_provider = provider.or_else(|| {
        pinned_model
            .or(root_model)
            .filter(|m| is_valid_model_id(m))
            .map(detect_provider)
    });

    if let Some(success_val) = field_opt(root_pairs, "success") {
        if *success_val != Value::Bool(true) {
            return Err(Error::InvalidRequest(
                "the provider envelope reported an unsuccessful call".to_string(),
            ));
        }
        if field_opt(root_pairs, "result")
            .and_then(object_pairs)
            .is_none()
        {
            return Err(Error::InvalidRequest(
                "the provider envelope carries no result object".to_string(),
            ));
        }
    }

    let payload = if field_opt(root_pairs, "success") == Some(&Value::Bool(true)) {
        field_opt(root_pairs, "result").expect("result object verified above")
    } else if effective_provider == Some(DecisionProvider::Cloudflare) {
        match field_opt(root_pairs, "result") {
            Some(result) if object_pairs(result).is_some() => result,
            Some(_) => {
                return Err(Error::InvalidRequest(
                    "the provider envelope carries a non-object result".to_string(),
                ))
            }
            None => &root,
        }
    } else {
        &root
    };
    let payload_pairs = object_pairs(payload).expect("payload is an object");

    let out_model = match pinned_model {
        Some(name) => name.to_string(),
        None => field_opt(payload_pairs, "model")
            .and_then(Value::as_str)
            .or(root_model)
            .filter(|name| is_valid_model_id(name))
            .unwrap_or(MODEL_NAME)
            .to_string(),
    };

    // Validate and canonicalize a DM-21 reply object without overwriting foreign models or carrying extra bloat.
    if let Some(choice_val) = field_opt(payload_pairs, "choice") {
        if let Some(choice_pairs) = object_pairs(choice_val) {
            if field_opt(payload_pairs, "answers").is_none()
                && field_opt(payload_pairs, "decisions").is_none()
                && field_opt(payload_pairs, "output").is_none()
            {
                let dm21_model = field_opt(payload_pairs, "model")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        Error::InvalidRequest("the DM-21 reply carries no model string".to_string())
                    })?;
                if let Some(expected) = pinned_model {
                    if dm21_model != expected {
                        return Err(Error::InvalidRequest(
                            "the DM-21 reply names a foreign model".to_string(),
                        ));
                    }
                }
                validate_model_id(dm21_model)?;
                let parsed_dm21 = consultation_from_reply_body(&render_json(payload), dm21_model);
                if parsed_dm21 == Consultation::MalformedReply {
                    return Err(Error::InvalidRequest(
                        "the DM-21 reply fails the reply grammar".to_string(),
                    ));
                }
                let clean_label = match field_opt(choice_pairs, "label") {
                    Some(Value::Str(s)) => Value::Str(s.clone()),
                    _ => Value::Null,
                };
                let mut clean_choice = vec![("label".to_string(), clean_label)];
                if let Some(conf) =
                    field_opt(choice_pairs, "confidence").and_then(model_number_unit)
                {
                    clean_choice.push(("confidence".to_string(), Value::Float(conf)));
                }
                let mut out_pairs = vec![
                    ("model".to_string(), Value::Str(dm21_model.to_string())),
                    ("choice".to_string(), Value::Object(clean_choice)),
                ];
                if let Some(JsonInt::Small(lvl)) = field_opt(payload_pairs, "score")
                    .and_then(object_pairs)
                    .and_then(|sp| field_opt(sp, "level"))
                    .and_then(Value::as_json_int)
                {
                    out_pairs.push((
                        "score".to_string(),
                        Value::Object(vec![("level".to_string(), Value::Int(lvl))]),
                    ));
                }
                return Ok(render_json(&Value::Object(out_pairs)));
            }
        }
    }

    let container = field_opt(payload_pairs, "answers")
        .or_else(|| field_opt(payload_pairs, "decisions"))
        .or_else(|| field_opt(payload_pairs, "output"))
        .ok_or_else(|| {
            Error::InvalidRequest(
                "the provider reply carries no answers, decisions, or output container".to_string(),
            )
        })?;

    let (verdict_opt, score_opt) = if let Some(cpairs) = object_pairs(container) {
        (
            field_opt(cpairs, "verdict"),
            field_opt(cpairs, "score").or_else(|| field_opt(cpairs, "risk")),
        )
    } else if let Value::Array(items) = container {
        let v = find_unique_named_item(items, "verdict")?;
        let s = find_unique_named_item(items, "score")?;
        let r = find_unique_named_item(items, "risk")?;
        if s.is_some() && r.is_some() {
            return Err(Error::InvalidRequest(
                "the provider answers array carries both score and risk entries".to_string(),
            ));
        }
        (v, s.or(r))
    } else {
        return Err(Error::InvalidRequest(
            "the provider answers container is neither an object nor an array".to_string(),
        ));
    };

    let verdict_entry = verdict_opt.ok_or_else(|| {
        Error::InvalidRequest("the provider reply carries no verdict answer".to_string())
    })?;
    let verdict_pairs = object_pairs(verdict_entry).ok_or_else(|| {
        Error::InvalidRequest("the provider verdict entry is not an object".to_string())
    })?;

    let choice_field = field_opt(verdict_pairs, "choice")
        .or_else(|| field_opt(verdict_pairs, "selected"))
        .ok_or_else(|| {
            Error::InvalidRequest("the provider verdict entry carries no choice".to_string())
        })?;

    let probs = parse_choice_probabilities(
        field_opt(verdict_pairs, "probabilities")
            .or_else(|| field_opt(verdict_pairs, "distribution")),
    )?;
    let prob_lookup = |target: &str| -> Option<f64> {
        probs.as_ref().and_then(|entries| {
            entries
                .iter()
                .find(|(label, _)| label == target)
                .map(|(_, val)| *val)
        })
    };

    match choice_field {
        Value::Null => {
            let mut out_pairs = vec![
                ("model".to_string(), Value::Str(out_model)),
                (
                    "choice".to_string(),
                    Value::Object(vec![("label".to_string(), Value::Null)]),
                ),
            ];
            if let Some(score_raw) = score_opt {
                if let Some(raw_score) = extract_optional_zero_based_score(score_raw)? {
                    out_pairs.push((
                        "score".to_string(),
                        Value::Object(vec![(
                            "level".to_string(),
                            Value::Int(score_to_level(raw_score)),
                        )]),
                    ));
                }
            }
            Ok(render_json(&Value::Object(out_pairs)))
        }
        Value::Str(label) if label == "other" => {
            let mut choice_pairs = vec![("label".to_string(), Value::Null)];
            if let Some(conf) = prob_lookup("other") {
                choice_pairs.push(("confidence".to_string(), Value::Float(conf)));
            }
            let mut out_pairs = vec![
                ("model".to_string(), Value::Str(out_model)),
                ("choice".to_string(), Value::Object(choice_pairs)),
            ];
            if let Some(score_raw) = score_opt {
                if let Some(raw_score) = extract_optional_zero_based_score(score_raw)? {
                    out_pairs.push((
                        "score".to_string(),
                        Value::Object(vec![(
                            "level".to_string(),
                            Value::Int(score_to_level(raw_score)),
                        )]),
                    ));
                }
            }
            Ok(render_json(&Value::Object(out_pairs)))
        }
        Value::Str(label) if VERDICT_LABELS.contains(&label.as_str()) => {
            let confidence = prob_lookup(label).ok_or_else(|| {
                Error::InvalidRequest(
                    "the provider verdict probabilities carry no value for the chosen label"
                        .to_string(),
                )
            })?;
            let score_raw = score_opt.ok_or_else(|| {
                Error::InvalidRequest("the provider verdict reply carries no score".to_string())
            })?;
            let raw_score = extract_optional_zero_based_score(score_raw)?.ok_or_else(|| {
                Error::InvalidRequest("the provider verdict reply carries no score".to_string())
            })?;
            let out_doc = Value::Object(vec![
                ("model".to_string(), Value::Str(out_model)),
                (
                    "choice".to_string(),
                    Value::Object(vec![
                        ("label".to_string(), Value::Str(label.clone())),
                        ("confidence".to_string(), Value::Float(confidence)),
                    ]),
                ),
                (
                    "score".to_string(),
                    Value::Object(vec![(
                        "level".to_string(),
                        Value::Int(score_to_level(raw_score)),
                    )]),
                ),
            ]);
            Ok(render_json(&out_doc))
        }
        _ => Err(Error::InvalidRequest(
            "the provider verdict choice names an unknown label".to_string(),
        )),
    }
}
