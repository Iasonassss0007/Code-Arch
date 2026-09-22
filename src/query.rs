//! Query interface — serve the indexes as lookups.
//!
//! Job    Answer `importers` and `callers` lookups from the index files a
//!        previous `codearch` run wrote (`.codearch/imports.md`,
//!        `.codearch/routes.md`). No analysis, no writing: if the files do
//!        not hold what a lookup needs, the lookup reports a miss rather
//!        than re-analyzing.
//! In     Index file text (read by the caller) plus a normalized target.
//! Out    Parsed maps, transitive walks, and rendered answer strings.
//! Fails  Never on content: unparseable lines are skipped the way the eval's
//!        Python readers skip them (a line without the arrow is not an edge).

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

/// The arrow separating an imported file from its importers. Same literal
/// the writer (`render::import_index`) emits and the eval's `parse_index`
/// splits on.
const ARROW: &str = " ← ";

/// One `routes.md` section: the backend file defining the view, the route
/// strings listed there, and the frontend callers listed under it. A view
/// name may head several sections, so lookups keep every one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    pub file: String,
    pub routes: Vec<String>,
    pub callers: Vec<String>,
}

/// Reverse import index: imported file -> its direct importers, in file
/// order. Parses only the line shape the writer emits (`path ← a, b`);
/// header and prose lines carry no arrow and are skipped, exactly like the
/// eval's `parse_index`.
pub fn parse_imports(markdown: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for line in markdown.lines() {
        if let Some((target, rest)) = line.split_once(ARROW) {
            out.insert(target.to_string(), rest.split(", ").map(str::to_string).collect());
        }
    }
    out
}

/// Route index: view name -> every section naming that view, in file order.
/// Caller lines mirror the eval's `parse_routes` (`- \`path\``); anything
/// else is skipped.
pub fn parse_routes(markdown: &str) -> BTreeMap<String, Vec<RouteEntry>> {
    let mut out: BTreeMap<String, Vec<RouteEntry>> = BTreeMap::new();
    let mut view: Option<String> = None;
    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix("## `") {
            view = None;
            if let Some((name, tail)) = rest.split_once('`') {
                // `## \`View\` — \`backend/file.py\``: the defining file is
                // the second backticked span.
                let file = tail.split('`').nth(1).unwrap_or_default().to_string();
                out.entry(name.to_string()).or_default().push(RouteEntry {
                    file,
                    routes: Vec::new(),
                    callers: Vec::new(),
                });
                view = Some(name.to_string());
            }
            continue;
        }
        let Some(name) = view.as_ref() else {
            continue;
        };
        let Some(entries) = out.get_mut(name) else {
            continue;
        };
        // Sections for one view accumulate; the lines belong to the last.
        let Some(entry) = entries.last_mut() else {
            continue;
        };
        if line.starts_with("Routes:") {
            entry.routes = backticked(line);
        } else if line.starts_with("- `") && line.ends_with('`') && line.len() > 4 {
            entry.callers.push(line[3..line.len() - 1].to_string());
        }
    }
    out
}

/// Every backticked span on a line, in order (`Routes: \`a\`, \`b\``).
fn backticked(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('`') {
            out.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out
}

/// Transitive importers with hop counts, ported from the eval's
/// `build_tasks_nav.importers`: breadth-first over the reverse index, each
/// file recorded once at the smallest hop count, the target itself never
/// listed. `max_depth` caps the walk (the eval's walk is unbounded, i.e.
/// `None`). Sorted by (depth, path) for stable output.
pub fn importers(
    index: &BTreeMap<String, Vec<String>>,
    target: &str,
    max_depth: Option<usize>,
) -> Vec<(String, usize)> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut queue: VecDeque<(String, usize)> = index
        .get(target)
        .map(|v| v.iter().map(|n| (n.clone(), 1)).collect())
        .unwrap_or_default();
    while let Some((node, depth)) = queue.pop_front() {
        if node == target || seen.contains_key(&node) {
            continue;
        }
        if max_depth.is_some_and(|m| depth > m) {
            continue;
        }
        seen.insert(node.clone(), depth);
        if let Some(parents) = index.get(&node) {
            for parent in parents {
                if parent != target && !seen.contains_key(parent) {
                    queue.push_back((parent.clone(), depth + 1));
                }
            }
        }
    }
    let mut out: Vec<(String, usize)> = seen.into_iter().collect();
    out.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    out
}

/// Up to 5 indexed view names containing the query (case-insensitive), in
/// index order. Offered when a `callers` lookup misses.
pub fn suggest_views(index: &BTreeMap<String, Vec<RouteEntry>>, query: &str) -> Vec<String> {
    let q = query.to_lowercase();
    index
        .keys()
        .filter(|v| v.to_lowercase().contains(&q))
        .take(5)
        .cloned()
        .collect()
}

/// `<FILE>` arrives in whatever form an agent has (`src/a.ts`, `./src/a.ts`,
/// `src\a.ts`, an absolute path inside the repo). Normalize to the index's
/// form: repo-relative with `/` separators. A path escaping the repo is an
/// error, not an empty result. Purely lexical except for absolute paths,
/// which are resolved against the canonical repo root.
pub fn normalize_target(repo: &Path, raw: &str) -> Result<String, String> {
    let slashed = raw.replace('\\', "/");
    let rel = slashed.strip_prefix("./").unwrap_or(&slashed);
    let p = Path::new(rel);
    if p.is_absolute() {
        let canon_repo = repo
            .canonicalize()
            .map_err(|e| format!("cannot resolve repository '{}': {e}", repo.display()))?;
        let canon_p = Path::new(rel)
            .canonicalize()
            .map_err(|_| format!("cannot resolve path '{raw}': not found or inaccessible"))?;
        return canon_p.strip_prefix(&canon_repo).map_err(|_| format!("path '{raw}' is outside the repository")).map(|r| {
            r.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        });
    }
    lexical_rel(rel).ok_or_else(|| format!("path '{raw}' is outside the repository"))
}

/// Lexically clean a repo-relative path: drop `.`, resolve `..`, reject an
/// escape above the root.
fn lexical_rel(rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(c),
        }
    }
    Some(parts.join("/"))
}

/// Read an index file. A missing file is the "run `codearch` first" error,
/// not an empty index.
pub fn index_text(dir: &Path, file: &str, repo: &str) -> Result<String, String> {
    std::fs::read_to_string(dir.join(file)).map_err(|_| {
        format!(
            "no .codearch/{file} under {}; run codearch {repo} first",
            dir.display()
        )
    })
}

/// Minutes since the index file was written, for the staleness line. `None`
/// when the clock cannot be read; the caller then omits the line.
pub fn index_age_minutes(dir: &Path, file: &str) -> Option<u64> {
    let mtime = std::fs::metadata(dir.join(file)).ok()?.modified().ok()?;
    std::time::SystemTime::now()
        .duration_since(mtime)
        .ok()
        .map(|d| d.as_secs() / 60)
}

/// Plain-text `importers` answer: one `depth  path` line per hit, in
/// (depth, path) order. Empty when there is nothing to say.
pub fn importers_text(hits: &[(String, usize)]) -> String {
    let mut s = String::new();
    for (path, depth) in hits {
        s.push_str(&format!("{depth}  {path}\n"));
    }
    s
}

/// JSON `importers` answer, matching the eval tool's observation shape.
pub fn importers_json(target: &str, hits: &[(String, usize)]) -> String {
    serde_json::json!({
        "target": target,
        "importers": hits.iter().map(|(p, d)| serde_json::json!({"path": p, "depth": d})).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Direct importers of each caller: the second half of "what does an API
/// change touch", served with the callers so an agent needs no follow-up
/// lookup per caller. Callers nothing imports are left out.
pub fn caller_importers(
    imports: &BTreeMap<String, Vec<String>>,
    callers: &[String],
) -> BTreeMap<String, Vec<String>> {
    callers
        .iter()
        .filter_map(|c| {
            let direct: Vec<String> = importers(imports, c, Some(1)).into_iter().map(|(p, _)| p).collect();
            (!direct.is_empty()).then(|| (c.clone(), direct))
        })
        .collect()
}

/// Plain-text `callers` answer: one block per matching section, the defining
/// file and its routes first, then each caller indented, followed by the
/// files importing it when an import index is at hand.
pub fn callers_text(matches: &[RouteEntry], imports: Option<&BTreeMap<String, Vec<String>>>) -> String {
    let mut s = String::new();
    for m in matches {
        s.push_str(&format!("{}  (routes: {})\n", m.file, m.routes.join(", ")));
        let by = imports.map(|i| caller_importers(i, &m.callers)).unwrap_or_default();
        for c in &m.callers {
            s.push_str(&format!("  {c}\n"));
            if let Some(list) = by.get(c) {
                s.push_str(&format!("    imported by: {}\n", list.join(", ")));
            }
        }
    }
    s
}

/// JSON `callers` answer: the view plus every matching section. With an
/// import index, a section also maps each caller to its direct importers
/// (`importers`, omitted when empty).
pub fn callers_json(
    view: &str,
    matches: &[RouteEntry],
    imports: Option<&BTreeMap<String, Vec<String>>>,
) -> String {
    serde_json::json!({
        "view": view,
        "matches": matches.iter().map(|m| {
            let mut o = serde_json::json!({
                "file": m.file, "routes": m.routes, "callers": m.callers,
            });
            let by = imports.map(|i| caller_importers(i, &m.callers)).unwrap_or_default();
            if !by.is_empty() {
                o["importers"] = serde_json::json!(by);
            }
            o
        }).collect::<Vec<_>>(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_parse_skips_headers_and_splits_importers() {
        let text = "# Reverse Import Index — t\n\nGenerated by Code Arch.\n\n3 imports into 2 files · x.\n\nsrc/a.ts ← src/b.ts, src/c.ts\nsrc/b.ts ← src/c.ts\n";
        let idx = parse_imports(text);
        assert_eq!(
            idx.get("src/a.ts").unwrap(),
            &vec!["src/b.ts".to_string(), "src/c.ts".to_string()]
        );
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn imports_round_trip_through_the_writer() {
        let paths = ["src/a.ts", "src/b.ts", "src/c.ts"];
        let edges = [vec![1usize], vec![2], vec![]];
        let idx = crate::render::import_index("t", &paths, &edges, 1.0, 0);
        let back = parse_imports(&idx.markdown);
        assert_eq!(back.get("src/a.ts").unwrap(), &vec!["src/b.ts".to_string()]);
        assert_eq!(back.get("src/b.ts").unwrap(), &vec!["src/c.ts".to_string()]);
        assert_eq!(back.len(), 2);
    }

    fn test_inventory() -> crate::inventory::Inventory {
        use crate::types::{FileClass, FileRecord, Language};
        let files = ["src/views.py", "ui/a.ts", "ui/b.ts"];
        crate::inventory::Inventory {
            root: std::path::PathBuf::from("."),
            files: files
                .iter()
                .enumerate()
                .map(|(id, rel)| FileRecord {
                    id,
                    rel: (*rel).into(),
                    abs: std::path::PathBuf::from(rel),
                    language: if rel.ends_with(".py") {
                        Language::Python
                    } else {
                        Language::Ts
                    },
                    bytes: 10,
                    loc: 2,
                    class: FileClass::Source,
                })
                .collect(),
            skipped: Vec::new(),
            excluded: Default::default(),
        }
    }

    #[test]
    fn routes_round_trip_through_the_writer() {
        let inv = test_inventory();
        let links = vec![
            crate::contract::RouteLink {
                view: "TagViewSet".to_string(),
                view_file: 0,
                routes: vec!["tags".to_string()],
                callers: vec![1, 2],
            },
            crate::contract::RouteLink {
                view: "DocView".to_string(),
                view_file: 0,
                routes: vec!["documents".to_string(), "documents/notes".to_string()],
                callers: vec![1],
            },
        ];
        let md = crate::contract::routes_markdown(&inv, &links);
        let parsed = parse_routes(&md);
        assert_eq!(
            parsed.get("TagViewSet").unwrap(),
            &vec![RouteEntry {
                file: "src/views.py".to_string(),
                routes: vec!["tags".to_string()],
                callers: vec!["ui/a.ts".to_string(), "ui/b.ts".to_string()],
            }]
        );
        assert_eq!(
            parsed.get("DocView").unwrap()[0].routes,
            vec!["documents".to_string(), "documents/notes".to_string()]
        );
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn callers_carry_their_direct_importers_when_the_index_is_there() {
        let imports = parse_imports("ui/a.ts ← ui/x.ts, ui/y.ts\nui/x.ts ← ui/deep.ts\n");
        let m = vec![RouteEntry {
            file: "v.py".to_string(),
            routes: vec!["tags".to_string()],
            callers: vec!["ui/a.ts".to_string(), "ui/b.ts".to_string()],
        }];
        let json: serde_json::Value = serde_json::from_str(&callers_json("V", &m, Some(&imports))).unwrap();
        // Direct only; a caller nothing imports is left out.
        assert_eq!(json["matches"][0]["importers"], serde_json::json!({"ui/a.ts": ["ui/x.ts", "ui/y.ts"]}));
        assert_eq!(json["matches"][0]["callers"], serde_json::json!(["ui/a.ts", "ui/b.ts"]));
        let bare: serde_json::Value = serde_json::from_str(&callers_json("V", &m, None)).unwrap();
        assert!(bare["matches"][0].get("importers").is_none(), "no index, no field");
        assert_eq!(
            callers_text(&m, Some(&imports)),
            "v.py  (routes: tags)\n  ui/a.ts\n    imported by: ui/x.ts, ui/y.ts\n  ui/b.ts\n"
        );
        assert_eq!(callers_text(&m, None), "v.py  (routes: tags)\n  ui/a.ts\n  ui/b.ts\n");
    }

    #[test]
    fn routes_parse_keeps_every_section_for_a_view() {
        let md = "# Route callers\n\n## `V` — `a.py`\n\nRoutes: `x`\n\n- `f1.ts`\n\n## `V` — `b.py`\n\nRoutes: `y`\n\n- `f2.ts`\n";
        let parsed = parse_routes(md);
        assert_eq!(parsed.get("V").unwrap().len(), 2);
        assert_eq!(parsed.get("V").unwrap()[1].file, "b.py");
        assert_eq!(parsed.get("V").unwrap()[1].callers, vec!["f2.ts"]);
    }

    #[test]
    fn walk_records_smallest_depth_excludes_target_and_orders() {
        let idx: BTreeMap<String, Vec<String>> = [
            ("a", vec!["b", "c"]),
            ("b", vec!["c", "d"]),
            ("c", vec!["a", "d"]),
            ("d", vec![]),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.into_iter().map(str::to_string).collect()))
        .collect();
        // b:1, c:1 (direct, even though also reached via b), d:2.
        assert_eq!(
            importers(&idx, "a", None),
            vec![
                ("b".to_string(), 1),
                ("c".to_string(), 1),
                ("d".to_string(), 2)
            ]
        );
        assert_eq!(
            importers(&idx, "a", Some(1)),
            vec![("b".to_string(), 1), ("c".to_string(), 1)]
        );
        assert!(importers(&idx, "missing", None).is_empty());
    }

    #[test]
    fn walk_terminates_on_cycles_without_listing_the_target() {
        let idx: BTreeMap<String, Vec<String>> = [("a", vec!["b"]), ("b", vec!["a"])]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.into_iter().map(str::to_string).collect()))
            .collect();
        assert_eq!(importers(&idx, "a", None), vec![("b".to_string(), 1)]);
    }

    #[test]
    fn normalize_handles_agent_shaped_paths() {
        let repo = Path::new(".");
        assert_eq!(normalize_target(repo, "src/a.ts").unwrap(), "src/a.ts");
        assert_eq!(normalize_target(repo, "./src/a.ts").unwrap(), "src/a.ts");
        assert_eq!(normalize_target(repo, r"src\a.ts").unwrap(), "src/a.ts");
        assert!(normalize_target(repo, "../outside.ts").is_err());
        assert!(normalize_target(repo, "src/../../outside.ts").is_err());
    }

    #[test]
    fn normalize_resolves_absolute_paths_inside_the_repo() {
        let tmp = std::env::temp_dir().join(format!("codearch-query-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("src/a.ts"), "x").unwrap();
        let abs = tmp.join("src/a.ts");
        let got = normalize_target(&tmp, abs.to_str().unwrap()).unwrap();
        assert_eq!(got, "src/a.ts");
        let outside = tmp.join("other.ts");
        std::fs::write(&outside, "x").unwrap();
        // Sibling of the repo root, not inside it.
        assert!(normalize_target(&tmp.join("src"), outside.to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
