//! Measures item extraction (the product's real prompt, schema and validation) on the
//! synthetic cases in `crates/ai/eval/items.json`, for the offline mock or a model server on
//! this machine. Nothing is sent off the server: `OpenAiCompat` only accepts loopback http.
//!
//!   cargo run -p procure-ai --example eval                       (the mock)
//!   EVAL_BASE_URL=http://127.0.0.1:11434/v1 EVAL_MODEL=<model> cargo run -p procure-ai --example eval
//!
//! Prints a summary and writes every answer to `reports/ai-eval/<provider>-<unix time>.json`.

use std::time::Instant;

use procure_ai::extract::{self, ItemSuggestion};
use procure_ai::mock::MockProvider;
use procure_ai::provider::{OpenAiCompat, Provider};
use serde_json::{Value, json};

fn norm(s: &str) -> String {
    extract::normalise(s)
}

/// Expected items found among the suggestions: by part number when one is written, else by
/// the key word in the description. Returns (matched, mpn exact, quantity exact) counts.
fn score(expected: &[Value], got: &[ItemSuggestion]) -> (usize, usize, usize) {
    let mut used = vec![false; got.len()];
    let (mut matched, mut mpn_ok, mut qty_ok) = (0, 0, 0);
    for e in expected {
        let mpn = e["mpn"].as_str().unwrap_or("");
        let key = e["key"].as_str().unwrap_or("").to_lowercase();
        let hit = got.iter().enumerate().position(|(i, g)| {
            !used[i]
                && if mpn.is_empty() {
                    g.description.to_lowercase().contains(&key)
                } else {
                    norm(&g.mpn) == norm(mpn)
                }
        });
        let Some(i) = hit else { continue };
        used[i] = true;
        matched += 1;
        if g_mpn_exact(&got[i].mpn, mpn) {
            mpn_ok += 1;
        }
        if u64::from(got[i].quantity) == e["quantity"].as_u64().unwrap_or(0) {
            qty_ok += 1;
        }
    }
    (matched, mpn_ok, qty_ok)
}

fn g_mpn_exact(got: &str, want: &str) -> bool {
    got.trim() == want
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local = std::env::var("EVAL_BASE_URL").ok();
    let provider: Box<dyn Provider> = match &local {
        Some(base) => Box::new(OpenAiCompat::new(
            base,
            &std::env::var("EVAL_MODEL").map_err(|_| "EVAL_MODEL")?,
            None,
        )?),
        None => Box::new(MockProvider),
    };
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(format!(
        "{root}/crates/ai/eval/items.json"
    ))?)?;
    let cases = cases["cases"].as_array().ok_or("cases")?;

    let (mut expected_n, mut got_n, mut matched_n, mut mpn_n, mut qty_n) = (0, 0, 0, 0, 0);
    let (mut errors, mut invented_mpn, mut empty_ok) = (0, 0, 0);
    let mut latencies = Vec::new();
    let mut records = Vec::new();
    for c in cases {
        let text = c["text"].as_str().unwrap_or("");
        let expected = c["items"].as_array().cloned().unwrap_or_default();
        let started = Instant::now();
        let result = extract::extract_items(provider.as_ref(), text).await;
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        latencies.push(ms);
        expected_n += expected.len();
        match result {
            Ok(out) => {
                let got = out.value;
                got_n += got.len();
                let (m, mp, q) = score(&expected, &got);
                matched_n += m;
                mpn_n += mp;
                qty_n += q;
                invented_mpn += got
                    .iter()
                    .filter(|g| g.flags.iter().any(|f| f == "mpn_not_in_source"))
                    .count();
                if expected.is_empty() && got.is_empty() {
                    empty_ok += 1;
                }
                println!(
                    "{:<16} {:>6} ms  expected {} got {} matched {m}",
                    c["id"].as_str().unwrap_or(""),
                    ms,
                    expected.len(),
                    got.len()
                );
                records.push(json!({ "id": c["id"], "ms": ms, "items": got }));
            }
            Err(e) => {
                errors += 1;
                println!(
                    "{:<16} {:>6} ms  error {e:?}",
                    c["id"].as_str().unwrap_or(""),
                    ms
                );
                records.push(json!({ "id": c["id"], "ms": ms, "error": format!("{e:?}") }));
            }
        }
    }
    latencies.sort_unstable();
    let pct = |p: usize| latencies[(latencies.len() - 1) * p / 100];
    let empties = cases
        .iter()
        .filter(|c| c["items"].as_array().is_some_and(Vec::is_empty))
        .count();
    let summary = json!({
        "provider": provider.name(), "model": provider.model(), "cases": cases.len(),
        "expected_items": expected_n, "suggested_items": got_n, "matched_items": matched_n,
        "recall_pct": matched_n * 100 / expected_n.max(1),
        "precision_pct": matched_n * 100 / got_n.max(1),
        "mpn_exact": mpn_n, "quantity_exact": qty_n,
        "mpn_not_in_source_flags": invented_mpn,
        "no_item_cases_answered_empty": format!("{empty_ok}/{empties}"),
        "errors": errors, "latency_ms_p50": pct(50), "latency_ms_p95": pct(95),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    let dir = format!("{root}/reports/ai-eval");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let file = format!(
        "{dir}/{}-{stamp}.json",
        if local.is_some() { "local" } else { "mock" }
    );
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&json!({ "summary": summary, "cases": records }))?,
    )?;
    println!("written {file}");
    Ok(())
}
