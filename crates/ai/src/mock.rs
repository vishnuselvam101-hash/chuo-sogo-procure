//! Offline, deterministic stand-in for a model (the default provider). It reads simple
//! line-oriented text so the whole flow works without any AI service; it is not meant to be
//! clever. Its output goes through exactly the same validation as a real model's.

use serde_json::{Value, json};

use crate::extract::{EXTRACT_ITEMS, EXTRACT_QUOTATION};
use crate::listing::{EXTRACT_LISTING, TRANSLATE_LISTING};
use crate::negotiation::{self, NEGOTIATION_BRIEF};
use crate::provider::{AiError, BoxFuture, ModelCall, Provider};

pub struct MockProvider;

fn fenced(user: &str) -> &str {
    let start = user
        .find("<untrusted_input>")
        .map_or(0, |i| i + "<untrusted_input>".len());
    let end = user.rfind("</untrusted_input>").unwrap_or(user.len());
    user.get(start..end.max(start)).unwrap_or("")
}

fn is_part_number(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    t.len() >= 4
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn number(tok: &str) -> Option<i64> {
    tok.trim_matches(|c: char| !c.is_ascii_digit())
        .replace(',', "")
        .parse()
        .ok()
}

/// "100 x NSK 6204ZZ deep groove bearing" / "Linear guide HSR15, 4 set".
fn items(text: &str) -> Value {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some((qi, qty)) = toks
            .iter()
            .enumerate()
            .find(|(_, t)| t.chars().all(|c| c.is_ascii_digit() || c == ','))
            .and_then(|(i, t)| number(t).map(|n| (i, n)))
        else {
            continue;
        };
        let unit = toks
            .get(qi + 1)
            .map(|t| {
                t.trim_matches(|c: char| !c.is_ascii_alphabetic())
                    .to_ascii_lowercase()
            })
            .filter(|u| matches!(u.as_str(), "pcs" | "set" | "sets" | "box" | "m" | "kg"))
            .map(|u| if u == "sets" { "set".to_string() } else { u });
        let mpn_idx = toks.iter().position(|t| is_part_number(t));
        let mpn = mpn_idx.map(|i| {
            toks[i]
                .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        });
        let manufacturer = mpn_idx
            .and_then(|i| i.checked_sub(1))
            .map(|i| toks[i])
            .filter(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_uppercase()))
            .unwrap_or("");
        let description: Vec<&str> = toks
            .iter()
            .enumerate()
            .filter(|(i, t)| *i != qi && !t.eq_ignore_ascii_case("x") && Some(*i) != mpn_idx)
            .map(|(_, t)| *t)
            .collect();
        out.push(json!({
            "description": description.join(" ").trim_matches(|c: char| c == ',' || c == '-').trim(),
            "manufacturer": manufacturer,
            "mpn": mpn.unwrap_or_default(),
            "quantity": qty,
            "unit": unit.unwrap_or_else(|| "pcs".into()),
            "confidence": 60,
        }));
    }
    json!({ "items": out })
}

/// "Line 1: 1,200 x 100, 7 days, part 6204ZZ" plus "Currency: JPY", "Shipping: 2000",
/// "Valid until 2026-12-31", "Ref: Q-123".
fn quotation(text: &str) -> Value {
    let mut currency = String::new();
    let mut shipping = "0".to_string();
    let mut valid_until = String::new();
    let mut reference = String::new();
    let mut lines = Vec::new();
    for line in text.lines().map(str::trim) {
        let lower = line.to_ascii_lowercase();
        let toks: Vec<&str> = line
            .split(|c: char| c.is_whitespace() || c == ':')
            .filter(|t| !t.is_empty())
            .collect();
        if let Some(code) = toks
            .iter()
            .find(|t| matches!(**t, "JPY" | "USD" | "EUR" | "INR" | "CNY" | "KRW"))
        {
            currency = (*code).to_string();
        }
        if lower.starts_with("shipping") {
            if let Some(n) = toks.iter().find_map(|t| number(t)) {
                shipping = n.to_string();
            }
        } else if lower.contains("valid") {
            if let Some(d) = toks
                .iter()
                .find(|t| t.len() == 10 && t.chars().filter(|c| *c == '-').count() == 2)
            {
                valid_until = (*d).to_string();
            }
        } else if lower.starts_with("ref") {
            reference = toks.get(1).copied().unwrap_or("").to_string();
        } else if lower.starts_with("line") {
            let nums: Vec<&str> = toks
                .iter()
                .copied()
                .filter(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .collect();
            let line_no = nums.first().and_then(|t| number(t));
            let price = nums
                .get(1)
                .map(|t| t.trim_end_matches(',').replace(',', ""));
            let qty = toks
                .iter()
                .position(|t| t.eq_ignore_ascii_case("x"))
                .and_then(|i| toks.get(i + 1))
                .and_then(|t| number(t));
            let days = toks
                .iter()
                .position(|t| t.to_ascii_lowercase().starts_with("day"))
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| number(toks[i]));
            let part = toks
                .iter()
                .position(|t| t.eq_ignore_ascii_case("part"))
                .and_then(|i| toks.get(i + 1))
                .map(|t| {
                    t.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                        .to_string()
                });
            if let (Some(line_no), Some(price), Some(qty)) = (line_no, price, qty) {
                lines.push(json!({
                    "line_no": line_no, "unit_price": price, "quantity": qty,
                    "lead_time_days": days, "offered_mpn": part.unwrap_or_default(), "confidence": 60,
                }));
            }
        }
    }
    json!({ "currency": currency, "shipping": shipping, "valid_until": valid_until,
            "supplier_reference": reference, "lines": lines })
}

/// "Key: value" lines (メーカー / 型番 / 品名 / 状態 or maker / model / name / condition are
/// fields, other keys become specifications); otherwise the first part-number-like word is
/// the model and the first line the title.
fn listing(text: &str) -> Value {
    let mut manufacturer = String::new();
    let mut model = String::new();
    let mut title = String::new();
    let mut condition = String::new();
    let mut specs = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((k, v)) = line.split_once([':', '：']) else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k.to_lowercase().as_str() {
            "メーカー" | "maker" | "manufacturer" => manufacturer = v.to_string(),
            "型番" | "品番" | "model" | "part" | "part number" => model = v.to_string(),
            "品名" | "name" | "product" | "title" => title = v.to_string(),
            "状態" | "condition" => {
                condition = match v {
                    "新品" | "new" | "New" => "new",
                    "中古" | "used" | "Used" => "used",
                    "再生品" | "refurbished" => "refurbished",
                    _ => "",
                }
                .to_string()
            }
            _ if !v.is_empty() && specs.len() < 50 => {
                specs.push(json!({ "key": k, "value": v }));
            }
            _ => {}
        }
    }
    if model.is_empty() {
        let ascii: String = text
            .chars()
            .map(|c| if c.is_ascii() { c } else { ' ' })
            .collect();
        model = ascii
            .split_whitespace()
            .find(|t| is_part_number(t))
            .map(|t| {
                t.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                    .to_string()
            })
            .unwrap_or_default();
    }
    if title.is_empty() {
        // The first plain line (a "key: value" line is a field, not a product name).
        title = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.contains([':', '：']))
            .unwrap_or("")
            .chars()
            .take(80)
            .collect();
    }
    let japanese = title
        .chars()
        .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF));
    let description: String = text.trim().chars().take(500).collect();
    let (title_ja, title_en, desc_ja, desc_en) = if japanese {
        (title, String::new(), description, String::new())
    } else {
        (String::new(), title, String::new(), description)
    };
    json!({ "manufacturer": manufacturer, "model_number": model, "condition": condition,
            "title": title_ja, "title_en": title_en, "description": desc_ja,
            "description_en": desc_en, "specs": specs, "confidence": 60 })
}

/// The mock cannot translate: it returns the text unchanged (still flagged as a draft).
fn translation(text: &str) -> Value {
    let title = text
        .lines()
        .find_map(|l| l.strip_prefix("TITLE: "))
        .unwrap_or("")
        .to_string();
    let description = text
        .split_once("DESCRIPTION: ")
        .map_or("", |(_, d)| d)
        .trim()
        .to_string();
    json!({ "title": title, "description": description })
}

impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn model(&self) -> &str {
        "rules-v1"
    }

    fn complete<'a>(&'a self, call: &'a ModelCall) -> BoxFuture<'a, Result<String, AiError>> {
        Box::pin(async move {
            let text = fenced(&call.user);
            let value = match call.capability {
                EXTRACT_ITEMS => items(text),
                EXTRACT_QUOTATION => quotation(text),
                EXTRACT_LISTING => listing(text),
                TRANSLATE_LISTING => translation(text),
                NEGOTIATION_BRIEF => negotiation::mock_answer(text),
                _ => return Err(AiError::Unavailable),
            };
            Ok(value.to_string())
        })
    }
}
