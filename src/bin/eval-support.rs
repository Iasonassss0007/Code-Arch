//! JSON-only bridge to the production tokenizer and derived labeler for the eval harness.
use codearch::label::{ClusterSummary, DerivedLabeler, Labeler};
use serde_json::{Value, json};
use std::io::{self, Read};

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

fn main() -> anyhow::Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let v: Value = serde_json::from_str(&input)?;
    let result = match v["op"].as_str() {
        Some("tokens") => {
            // Fail instead of silently estimating benchmark tokens.
            let bpe = tiktoken_rs::cl100k_base()?;
            json!(
                strings(&v["texts"])
                    .iter()
                    .map(|s| bpe.encode_with_special_tokens(s).len())
                    .collect::<Vec<_>>()
            )
        }
        Some("labels") => {
            if v.get("model").is_some() {
                anyhow::bail!("model labeling was removed in codearch 0.3 (see tag v0.2-with-llm)");
            }
            let labeler = DerivedLabeler;

            let mut siblings = std::collections::HashMap::<String, Vec<String>>::new();
            let mut labels = Vec::new();
            for c in v["clusters"].as_array().unwrap() {
                let s = ClusterSummary {
                    dirs: strings(&c["dirs"]),
                    top_symbols: strings(&c["top_symbols"]),
                    // Present in the frozen set and used by both the prompt and
                    // the grounding guard. `derive_name` reads only `dirs`, so
                    // passing it does not move the derived baseline. Absent in
                    // older frozen sets (clusters.json), where it defaults to
                    // empty rather than failing the whole benchmark run.
                    entry_points: c
                        .get("entry_points")
                        .map(strings)
                        .unwrap_or_default(),
                    external_deps: strings(&c["external_deps"]),
                    files: (0..c["file_count"].as_u64().unwrap() as usize).collect(),
                    ..Default::default()
                };
                let taken = siblings
                    .entry(c["group"].as_str().unwrap().to_owned())
                    .or_default();
                let label = labeler.label(&s, taken);
                taken.push(label.name.clone());
                labels.push(json!({"id": c["id"], "name": label.name, "summary": label.summary}));
            }
            json!({"labels": labels})
        }
        _ => anyhow::bail!("unknown operation"),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
