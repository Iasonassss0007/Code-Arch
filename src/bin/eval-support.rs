//! JSON-only bridge to the production tokenizer and labeler for M1.
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
        Some("clusters") => {
            // Production stages, frozen before any labels are generated.
            let inv =
                codearch::inventory::collect(std::path::Path::new(v["root"].as_str().unwrap()))?;
            let profile = codearch::profile::detect(&inv);
            let routes = codearch::profile::route_hints(&profile, &inv);
            let parsed = codearch::parse::parse_all(&inv);
            let res = codearch::resolve::resolve_all(&inv, &parsed, &profile.mappings);
            let graph = codearch::graph::build(&inv, &res, &codearch::git::CoChange::default());
            let part = if graph.edge_count() == 0 {
                codearch::cluster::directory_partition(&inv)
            } else {
                codearch::cluster::partition_targeting(
                    &graph,
                    codearch::DEFAULT_MAX_DOMAINS,
                    0x5EED,
                )
            };
            let scores = codearch::rank::score(&graph, &routes, &[]);
            let summaries =
                codearch::label::summarize(&inv, &parsed, &res, &graph, &part, &scores, &routes);
            json!(
                summaries
                    .iter()
                    .map(|s| json!({
                        "id": s.id, "dirs": s.dirs, "top_symbols": s.top_symbols,
                        "external_deps": s.external_deps, "entry_points": s.entry_points,
                        "depends_on": s.depends_on, "depended_on_by": s.depended_on_by,
                        "file_count": s.size(),
                        "files": s.files.iter().map(|&f| &inv.get(f).rel).collect::<Vec<_>>()
                    }))
                    .collect::<Vec<_>>()
            )
        }
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
            // `model` selects the labeler under test. Absent means the derived
            // baseline, so existing scoring runs are unchanged.
            let labeler: Box<dyn Labeler> = match v["model"].as_str() {
                None => Box::new(DerivedLabeler),
                #[cfg(feature = "llm")]
                Some(path) => {
                    let threads = std::thread::available_parallelism()
                        .map_or(4, |n| n.get() as i32);
                    Box::new(codearch::label::llm::LlmLabeler::load(
                        std::path::Path::new(path),
                        threads,
                    )?)
                }
                #[cfg(not(feature = "llm"))]
                Some(_) => anyhow::bail!(
                    "eval-support was built without the `llm` feature.
Rebuild with: cargo build --features llm"
                ),
            };

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
            json!({"labels": labels, "fell_back": labeler.fell_back(), "summary_fell_back": labeler.summary_fell_back()})
        }
        _ => anyhow::bail!("unknown operation"),
    };
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
