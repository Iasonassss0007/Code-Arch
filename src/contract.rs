//! Cross-language API contracts.
//!
//! Job    Turn URL-shaped evidence both parsers harvested into file pairs the
//!        graph can fuse: a TS `fetch('/api/users')` and a Python
//!        `@app.route('/api/users')` share a contract neither import graph
//!        sees.
//! In     Inventory (for languages), per-file `urls` from stage 2.
//! Out    Sorted, deduplicated `(FileId, FileId)` pairs, smaller id first.
//! Fails  Cannot fail. No shared normalized path means no pairs.
//!
//! Three deliberate limits, all stated in the Still Open record:
//!
//! ```text
//! Ecosystem divide, not file extension: Ts vs Tsx vs Js share one import
//! graph, so only Python-vs-not pairs join. Same-language coupling is the
//! import graph's job; these edges exist for what imports cannot see.
//! Two-segment minimum   `normalize_url` refuses `/health`-shaped strings,
//!                       so bare health checks never join.
//! One edge per pair     Two files sharing three contracts are one edge:
//!                       the pair is the claim, not its multiplicity.
//! ```

use crate::inventory::Inventory;
use crate::parse::FileParse;
use crate::types::FileId;
use std::collections::HashMap;

/// Join files sharing a normalized contract path across languages.
pub fn join(inv: &Inventory, parsed: &[FileParse]) -> Vec<(FileId, FileId)> {
    let mut by_url: HashMap<&str, Vec<FileId>> = HashMap::new();
    for p in parsed {
        for u in &p.urls {
            by_url.entry(u.as_str()).or_default().push(p.file);
        }
    }

    let mut out: Vec<(FileId, FileId)> = Vec::new();
    let mut urls: Vec<&str> = by_url.keys().copied().collect();
    urls.sort_unstable();
    for u in urls {
        let mut files = by_url[u].clone();
        files.sort_unstable();
        files.dedup();
        // Ecosystem divide, not enum variant: Ts vs Tsx vs Js share one
        // import graph, so only Python-vs-not pairs join. (Caught on hono:
        // variant inequality produced 24 same-ecosystem pairs.)
        for (a, &i) in files.iter().enumerate() {
            for &j in &files[a + 1..] {
                if inv.get(i).language.is_python() != inv.get(j).language.is_python() {
                    out.push((i, j));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Inventory;
    use crate::parse::{Parsers, parse_one};
    use crate::types::{FileClass, FileRecord, Language};

    fn inventory(paths: &[( &str, Language)]) -> Inventory {
        Inventory {
            root: std::path::PathBuf::from("."),
            files: paths
                .iter()
                .enumerate()
                .map(|(id, (rel, lang))| FileRecord {
                    id,
                    rel: (*rel).into(),
                    abs: std::path::PathBuf::from(rel),
                    language: *lang,
                    bytes: 100,
                    loc: 10,
                    class: FileClass::Source,
                })
                .collect(),
            skipped: Vec::new(),
            excluded: Default::default(),
        }
    }

    fn parsed_with_urls(urls: &[(&str, usize)]) -> Vec<FileParse> {
        let mut by_file: HashMap<usize, Vec<String>> = HashMap::new();
        for (u, f) in urls {
            by_file.entry(*f).or_default().push((*u).to_string());
        }
        let mut out: Vec<FileParse> = by_file
            .into_iter()
            .map(|(file, urls)| FileParse {
                file,
                urls,
                ..Default::default()
            })
            .collect();
        out.sort_by_key(|p| p.file);
        out
    }

    fn parse_urls(src: &str, lang: Language) -> Vec<String> {
        let mut p = Parsers::new();
        parse_one(&mut p, 0, lang, src).urls
    }

    #[test]
    fn fetch_and_route_share_a_contract() {
        assert_eq!(
            parse_urls("fetch('/api/users');", Language::Ts),
            vec!["api/users".to_string()]
        );
        assert_eq!(
            parse_urls("@app.route('/api/users')\ndef users(): pass\n", Language::Python),
            vec!["api/users".to_string()]
        );
    }

    #[test]
    fn dynamic_segments_normalize_together() {
        assert_eq!(
            parse_urls("fetch('/api/users/42');", Language::Ts),
            vec!["api/users/{}".to_string()]
        );
        assert_eq!(
            parse_urls("@app.get('/api/users/<int:id>')\ndef u(): pass\n", Language::Python),
            vec!["api/users/{}".to_string()]
        );
        assert_eq!(
            parse_urls("router.get('/api/users/:id', h);", Language::Ts),
            vec!["api/users/{}".to_string()]
        );
    }

    #[test]
    fn single_segment_and_templates_stay_out() {
        assert!(parse_urls("fetch('/health');", Language::Ts).is_empty());
        assert!(parse_urls("fetch(`/api/${t}`);", Language::Ts).is_empty());
        assert!(parse_urls("const x = 'text/html';", Language::Ts).is_empty());
    }

    #[test]
    fn django_path_call_is_harvested() {
        assert_eq!(
            parse_urls("urlpatterns = [path('api/users/', views.u)]\n", Language::Python),
            vec!["api/users".to_string()]
        );
    }

    #[test]
    fn ordinary_calls_contribute_nothing() {
        assert!(parse_urls("include(router.urls)\n", Language::Python).is_empty());
        assert!(parse_urls("print('a/b')\n", Language::Python).is_empty());
    }

    #[test]
    fn normalize_url_rules() {
        use crate::parse::normalize_url;
        assert_eq!(normalize_url("/api/users").as_deref(), Some("api/users"));
        assert_eq!(normalize_url("api/users/").as_deref(), Some("api/users"));
        assert_eq!(
            normalize_url("/api/users?page=2#top").as_deref(),
            Some("api/users")
        );
        assert_eq!(normalize_url("/health"), None);
        assert_eq!(normalize_url("text"), None);
    }

    #[test]
    fn python_and_ts_share_a_contract() {
        let inv = inventory(&[
            ("frontend/api.ts", Language::Ts),
            ("backend/server.py", Language::Python),
        ]);
        let parsed = parsed_with_urls(&[("api/users", 0), ("api/users", 1)]);
        assert_eq!(join(&inv, &parsed), vec![(0, 1)]);
    }

    #[test]
    fn same_ecosystem_variants_do_not_join() {
        // Ts vs Tsx share one import graph: the shared path is that graph's
        // business, not the join's. Found live on hono (24 false pairs).
        let inv = inventory(&[
            ("a/one.ts", Language::Ts),
            ("b/two.tsx", Language::Tsx),
            ("c/three.js", Language::Js),
        ]);
        let parsed = parsed_with_urls(&[("api/users", 0), ("api/users", 1), ("api/users", 2)]);
        assert!(join(&inv, &parsed).is_empty());
    }

    #[test]
    fn python_only_urls_do_not_join() {
        let inv = inventory(&[
            ("a/one.py", Language::Python),
            ("b/two.py", Language::Python),
        ]);
        let parsed = parsed_with_urls(&[("api/users", 0), ("api/users", 1)]);
        assert!(join(&inv, &parsed).is_empty());
    }
}
