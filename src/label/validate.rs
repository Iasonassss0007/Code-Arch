//! Stage 9 — generated-label validation.
//!
//! Job    Decide whether a generated name may be trusted, and build the prompt
//!        that produced it.
//! In     ClusterSummary (deterministic evidence), candidate name, sibling names
//! Out    Accept, or reject with a reason the caller turns into a derived label
//! Fails  Never. Every function here is total and pure.
//!
//! This module is compiled unconditionally, without the `llm` feature. The
//! guard is the part of stage 9 most worth testing and least worth hiding
//! behind a build flag: `core_idea.md:445` argues a confident wrong name is
//! worse than no name, because the agent will trust it and be misdirected.

use super::{ClusterSummary, NAME_ALIASES};

/// Longest name the guard will accept.
///
/// The longest name `derive_name` produces across the eval corpus is 20 chars
/// ("Bundle Check Scripts", "Benchmarks Utilities"), so this leaves 2x headroom.
/// The guard must never reject a length the derived path itself emits, or the
/// fallback would be inconsistent with the thing it falls back from.
pub const MAX_NAME_CHARS: usize = 40;

/// Why a generated name was refused. Carried so the run can report counts
/// rather than silently substituting derived output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    Empty,
    TooLong,
    Collision,
    Ungrounded,
}

/// Split a string into lowercase word tokens, breaking on path separators,
/// punctuation, and camelCase boundaries.
pub fn tokenize(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;

    for ch in s.chars() {
        if ch.is_alphanumeric() {
            // A lowercase-to-uppercase step is a word boundary inside an
            // identifier: applyCors -> apply, cors.
            if ch.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.extend(ch.to_lowercase());
            prev_lower = ch.is_lowercase() || ch.is_numeric();
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            prev_lower = false;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Every token the cluster's own deterministic evidence can vouch for,
/// including alias expansions so an expanded name still traces to its
/// abbreviated directory.
///
/// Deliberately excludes `entry_points`, matching the groundedness metric in
/// `eval/run.py:127`, which allows only dirs, symbols and external deps. A
/// guard more permissive than the metric scoring it would pass names the
/// scorer then counts as ungrounded. Entry points still reach the model
/// through the prompt; they just cannot vouch for a name by themselves.
pub fn evidence_tokens(s: &ClusterSummary) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    let sources = s
        .dirs
        .iter()
        .chain(s.top_symbols.iter())
        .chain(s.external_deps.iter());

    for src in sources {
        for t in tokenize(src) {
            // `derive_name` expands abbreviations, so the expansion has to be
            // vouched for by the abbreviation the cluster actually contains.
            if let Some((_, full)) = NAME_ALIASES.iter().find(|(short, _)| *short == t) {
                out.extend(tokenize(full));
            }
            out.push(t);
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Does any token of `name` trace back to the cluster's own evidence?
pub fn is_grounded(name: &str, s: &ClusterSummary) -> bool {
    let evidence = evidence_tokens(s);
    tokenize(name).iter().any(|t| evidence.contains(t))
}

/// The full guard. `Ok(())` means the name may be used as generated.
pub fn check(name: &str, s: &ClusterSummary, siblings: &[String]) -> Result<(), Rejection> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Rejection::Empty);
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(Rejection::TooLong);
    }
    if siblings.iter().any(|s| s.eq_ignore_ascii_case(trimmed)) {
        return Err(Rejection::Collision);
    }
    if !is_grounded(trimmed, s) {
        return Err(Rejection::Ungrounded);
    }
    Ok(())
}

/// The prompt for one cluster. Structured evidence only — never source code.
pub fn build_prompt(s: &ClusterSummary, siblings: &[String]) -> String {
    fn list(items: &[String], limit: usize) -> String {
        if items.is_empty() {
            return "(none)".to_string();
        }
        items
            .iter()
            .take(limit)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }

    format!(
        "Name one subsystem of a software repository.\n\
         \n\
         Directories: {}\n\
         Key symbols: {}\n\
         Entry points: {}\n\
         External dependencies: {}\n\
         Names already used: {}\n\
         \n\
         Reply with JSON: {{\"name\": ..., \"summary\": ...}}\n\
         \n\
         Name rules:\n\
         - At most three words. Every word must appear in the evidence above.\n\
         - Choose the most specific evidence, not the most general. In a directory path the final segment identifies this subsystem; the segments before it only name the broader category it sits in. For app/service/scheduler the name is Scheduler, not Service.\n\
         - Keep the word that separates this subsystem from the names already used. Drop that word and the name stops distinguishing anything.\n\
         - If the evidence lists several items of one kind, name the kind rather than picking one member of it.\n\
         \n\
         Summary rules:\n\
         - One sentence, built from words that appear in the evidence above.\n\
         - Do not mention components, technologies or behaviour that is not listed.",
        list(&s.dirs, 5),
        list(&s.top_symbols, 8),
        list(&s.entry_points, 4),
        list(&s.external_deps, 5),
        list(siblings, 12),
    )
}

/// Pull `name` and `summary` out of the model's reply.
///
/// GBNF constrains generation to this shape, so a failure here means the
/// grammar was bypassed or the model stopped early. Returns `None` rather than
/// erroring: the caller's answer to every failure is the same derived label.
pub fn parse_response(raw: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let name = v.get("name")?.as_str()?.trim().to_string();
    let summary = v.get("summary")?.as_str()?.trim().to_string();
    if name.is_empty() || summary.is_empty() {
        return None;
    }
    Some((name, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> ClusterSummary {
        ClusterSummary {
            dirs: vec!["src/middleware/cors".to_string()],
            top_symbols: vec!["applyCors".to_string(), "CorsOptions".to_string()],
            entry_points: vec!["GET /health".to_string()],
            external_deps: vec!["hono".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn tokenize_splits_paths_and_camel_case() {
        assert_eq!(tokenize("src/middleware"), vec!["src", "middleware"]);
        assert_eq!(tokenize("applyCors"), vec!["apply", "cors"]);
    }

    #[test]
    fn name_matching_a_directory_is_grounded() {
        assert!(is_grounded("Middleware", &summary()));
    }

    #[test]
    fn name_matching_a_symbol_is_grounded() {
        assert!(is_grounded("Cors Options", &summary()));
    }

    #[test]
    fn unrelated_name_is_not_grounded() {
        assert!(!is_grounded("Telemetry", &summary()));
    }

    /// `eval/run.py:127` computes groundedness from dirs, symbols and external
    /// deps only. The guard must not be more permissive than the metric that
    /// scores it, or it would pass names the scorer counts as ungrounded.
    /// Entry points still go in the prompt as context; they just cannot vouch
    /// for a name on their own.
    #[test]
    fn entry_points_are_context_not_grounding_evidence() {
        let s = ClusterSummary {
            entry_points: vec!["GET /health".to_string()],
            dirs: vec!["src/middleware".to_string()],
            ..Default::default()
        };
        assert!(!is_grounded("Health", &s));
        assert!(is_grounded("Middleware", &s));
    }

    /// `derive_name` expands abbreviations through NAME_ALIASES, so the guard
    /// must accept the expansion of a directory it can see. Without this the
    /// guard would reject exactly the names the derived path produces.
    #[test]
    fn expanded_alias_traces_to_abbreviated_directory() {
        let s = ClusterSummary {
            dirs: vec!["src/auth".to_string()],
            ..Default::default()
        };
        assert!(is_grounded("Authentication", &s));
    }

    #[test]
    fn generic_name_with_no_evidence_is_rejected() {
        assert_eq!(check("Core", &summary(), &[]), Err(Rejection::Ungrounded));
    }

    #[test]
    fn empty_name_is_rejected() {
        assert_eq!(check("", &summary(), &[]), Err(Rejection::Empty));
    }

    #[test]
    fn whitespace_only_name_is_rejected() {
        assert_eq!(check("   ", &summary(), &[]), Err(Rejection::Empty));
    }

    #[test]
    fn overlong_name_is_rejected() {
        let long = "Middleware ".repeat(12);
        assert_eq!(check(&long, &summary(), &[]), Err(Rejection::TooLong));
    }

    #[test]
    fn name_colliding_with_a_sibling_is_rejected() {
        let taken = vec!["Middleware".to_string()];
        assert_eq!(check("Middleware", &summary(), &taken), Err(Rejection::Collision));
    }

    #[test]
    fn collision_check_ignores_case() {
        let taken = vec!["Middleware".to_string()];
        assert_eq!(check("middleware", &summary(), &taken), Err(Rejection::Collision));
    }

    #[test]
    fn grounded_unique_name_is_accepted() {
        assert_eq!(check("Cors Middleware", &summary(), &[]), Ok(()));
    }

    #[test]
    fn prompt_carries_evidence_and_siblings() {
        let taken = vec!["Routing".to_string()];
        let p = build_prompt(&summary(), &taken);
        assert!(p.contains("src/middleware/cors"));
        assert!(p.contains("applyCors"));
        assert!(p.contains("hono"));
        assert!(p.contains("Routing"));
    }

    #[test]
    fn parses_well_formed_reply() {
        let raw = r#"{"name": "Cors Middleware", "summary": "Handles CORS headers."}"#;
        let (n, s) = parse_response(raw).expect("should parse");
        assert_eq!(n, "Cors Middleware");
        assert_eq!(s, "Handles CORS headers.");
    }

    #[test]
    fn tolerates_surrounding_whitespace_and_newlines() {
        let raw = "\n  {\"name\": \"Routing\", \"summary\": \"Dispatches requests.\"}  \n";
        let (n, _) = parse_response(raw).expect("should parse");
        assert_eq!(n, "Routing");
    }

    /// A model that stops mid-object produces truncated JSON. The caller must
    /// get None and fall back, not a panic.
    #[test]
    fn truncated_reply_yields_none() {
        assert!(parse_response(r#"{"name": "Cors"#).is_none());
    }

    #[test]
    fn reply_missing_summary_yields_none() {
        assert!(parse_response(r#"{"name": "Cors"}"#).is_none());
    }

    #[test]
    fn empty_reply_yields_none() {
        assert!(parse_response("").is_none());
    }
}
