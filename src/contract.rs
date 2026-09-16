//! Cross-language API contracts.
//!
//! Job    Turn evidence both parsers harvested into file pairs the graph can
//!        fuse. URL contracts: a TS `fetch('/api/users')` and a Python
//!        `@app.route('/api/users')` share a path neither import graph sees.
//!        Semantic contracts: a TS caller of `createUser(...)` and a Python
//!        file defining `create_user` share a call shape no import graph sees.
//! In     Inventory (for languages), per-file `urls` and `symbols` from stage 2.
//! Out    Sorted, deduplicated `(FileId, FileId)` pairs, smaller id first.
//! Fails  Cannot fail. No shared evidence means no pairs.
//!
//! Deliberate limits, all stated in the Still Open record:
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
//!
//! Semantic-contract specifics:
//!
//! ```text
//! Definition side       only exported-or-Python symbols join — a Python
//!                       `def create_user` is a callable surface; an
//!                       internal TS `const` is not a contract.
//! Rarity floor          a stem appearing in files of BOTH ecosystems
//!                       everywhere (get, list, run) is vocabulary, not a
//!                       contract; `common_shape_stems` lists the ones to
//!                       refuse outright.
//! One shared stem       is a weak, lexical claim, so the fusion weight is
//!                       below URL contracts (W_CONTRACT_SEMANTIC) and the
//!                       report counts the two join kinds separately.
//! ```

use crate::inventory::Inventory;
use crate::parse::FileParse;
use crate::types::{FileId, SymbolKind};
use std::collections::{HashMap, HashSet};

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

/// Stems that describe a call shape in every codebase. Joining on any of
/// these would assert a relationship between unrelated files that merely
/// share vocabulary.
const COMMON_SHAPE_STEMS: &[&str] = &[
    "get", "set", "list", "find", "run", "init", "main", "call", "exec", "apply",
    "create", "update", "delete", "remove", "add", "handle", "load", "save", "new",
    "index", "test", "setup", "run", "start", "stop", "open", "close", "read",
    "write", "send", "check", "make", "build", "parse", "format", "validate",
    "process", "use", "data", "info", "config", "utils", "helper",
];

/// Join files across languages that share a rare symbol shape: a TS file
/// referencing `createUser` and a Python file defining `create_user` are
/// coupled through a call shape no import graph sees. Mirrors `join`'s
/// discipline: Python-vs-not only, one edge per pair, sorted and deduplicated.
pub fn semantic_join(inv: &Inventory, parsed: &[FileParse]) -> Vec<(FileId, FileId)> {
    let mut pairs: Vec<(FileId, FileId)> = Vec::new();
    let common: HashSet<&str> = COMMON_SHAPE_STEMS.iter().copied().collect();

    // Stems already claimed by a URL contract pair contribute nothing new:
    // that pair has its edge, and the same stem would only double-assert it.
    let url_pairs: HashSet<(FileId, FileId)> = join(inv, parsed).into_iter().collect();

    // File-level stem sets, restricted to the callable surface (functions,
    // classes, methods; Python has no export flag so all its defs qualify,
    // TS-side only exported symbols do — an internal const is not a contract).
    let mut per_file: Vec<HashSet<String>> = Vec::with_capacity(parsed.len());
    for p in parsed {
        let stems: HashSet<String> = p
            .symbols
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    SymbolKind::Function | SymbolKind::Class | SymbolKind::Method
                ) && (inv.get(p.file).language.is_python() || s.exported)
            })
            .map(|s| s.stem_key())
            .filter(|k| !common.contains(k.as_str()) && key_min_len(k))
            .collect();
        per_file.push(stems);
    }

    // Stem -> files, then rare stems only: a stem in many files on both
    // sides is repository vocabulary, not a contract.
    let mut by_stem: HashMap<&str, Vec<usize>> = HashMap::new();
    for (f, stems) in per_file.iter().enumerate() {
        for s in stems {
            by_stem.entry(s.as_str()).or_default().push(f);
        }
    }
    for (_stem, mut files) in by_stem {
        files.sort_unstable();
        files.dedup();
        let py = files
            .iter()
            .filter(|&&f| inv.get(f).language.is_python())
            .count();
        let ts = files.len() - py;
        if py == 0 || ts == 0 || py > 3 || ts > 3 {
            continue; // not cross-ecosystem, or too common to be a claim
        }
        for (a, &i) in files.iter().enumerate() {
            for &j in &files[a + 1..] {
                if inv.get(i).language.is_python() != inv.get(j).language.is_python() {
                    let pair = if i < j { (i, j) } else { (j, i) };
                    if !url_pairs.contains(&pair) {
                        pairs.push(pair);
                    }
                }
            }
        }
    }
    pairs.sort();
    pairs.dedup();
    pairs
}

/// Minimum stem length after underscore removal: 3 is a real identifier.
fn key_min_len(k: &str) -> bool {
    k.len() >= 3
}

/// A backend view and the frontend files whose request strings name its route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteLink {
    pub view: String,
    pub view_file: FileId,
    /// Route segments after the mount prefix, joined with `/`.
    pub routes: Vec<String>,
    pub callers: Vec<FileId>,
}

/// Route contracts: the edge `contract::join` misses when neither side
/// writes the full URL as one literal. Paperless registers
/// `router.register("tags", TagViewSet)` and builds requests as
/// `` `${this.baseUrl}${this.resourceName}/` `` with `resourceName = 'tags'`.
///
/// ```text
/// Backend    every Python route with its include() prefix, bound to the
///            file defining the view (class or function). A view name
///            defined in several files is ambiguous and skipped.
/// Mount      a first segment shared by more than half of nested routes
///            (`api`) is where the API is mounted, not what a caller names;
///            it is dropped. Frontends read it from config (`apiBaseUrl`).
/// Frontend   non-test TS files that send HTTP themselves, or extend (transitively)
///            a class defined in one that does. Services inherit the client.
/// Match      every remaining route segment appears among the file's
///            literal segments. Segment sets, not order: a template's
///            pieces are spread over fields and constructor assignments.
/// ```
///
/// Deliberate limits: single common-word routes (`tasks`, `status`) match
/// UI files that use the word as a router path; measured on paperless-ngx
/// against the xlang oracle, precision 0.78 and recall 1.00 over 43 views.
pub fn route_join(inv: &Inventory, parsed: &[FileParse]) -> Vec<RouteLink> {
    let routes: Vec<(FileId, &crate::parse::Route)> = parsed
        .iter()
        .filter(|p| inv.get(p.file).language.is_python())
        .flat_map(|p| p.routes.iter().map(move |r| (p.file, r)))
        .filter(|(_, r)| !r.segments.is_empty())
        .collect();
    if routes.is_empty() {
        return Vec::new();
    }

    // Counted over nested routes only: router registrations (`tags`) carry
    // no prefix and would outvote the mount on a DRF project.
    let mut first: HashMap<&str, usize> = HashMap::new();
    let nested = routes.iter().filter(|(_, r)| r.segments.len() > 1).count();
    for (_, r) in routes.iter().filter(|(_, r)| r.segments.len() > 1) {
        *first.entry(r.segments[0].as_str()).or_default() += 1;
    }
    let mount: HashSet<&str> = first
        .into_iter()
        .filter(|&(_, n)| n * 2 > nested)
        .map(|(s, _)| s)
        .collect();

    // View name -> defining Python files.
    let mut defs: HashMap<&str, Vec<FileId>> = HashMap::new();
    for p in parsed.iter().filter(|p| inv.get(p.file).language.is_python()) {
        for s in &p.symbols {
            if matches!(s.kind, SymbolKind::Class | SymbolKind::Function) {
                defs.entry(s.name.as_str()).or_default().push(p.file);
            }
        }
    }

    // HTTP-sending TS files, closed over `extends`. Tests assert on request
    // URLs without sending them; they are not callers.
    let ts: Vec<&FileParse> = parsed
        .iter()
        .filter(|p| {
            let f = inv.get(p.file);
            !f.language.is_python() && f.class != crate::types::FileClass::Test
        })
        .collect();
    let mut owner: HashMap<&str, Vec<FileId>> = HashMap::new();
    for p in &ts {
        for s in p.symbols.iter().filter(|s| s.kind == SymbolKind::Class) {
            owner.entry(s.name.as_str()).or_default().push(p.file);
        }
    }
    let mut http: HashSet<FileId> = ts.iter().filter(|p| p.http).map(|p| p.file).collect();
    loop {
        let before = http.len();
        for p in &ts {
            if !http.contains(&p.file)
                && p.extends.iter().any(|b| {
                    owner.get(b.as_str()).is_some_and(|fs| fs.len() == 1 && http.contains(&fs[0]))
                })
            {
                http.insert(p.file);
            }
        }
        if http.len() == before {
            break;
        }
    }
    let callers: Vec<(FileId, HashSet<&str>)> = ts
        .iter()
        .filter(|p| http.contains(&p.file))
        .map(|p| (p.file, p.url_segments.iter().map(String::as_str).collect()))
        .collect();

    let mut links: HashMap<(String, FileId), (Vec<String>, Vec<FileId>)> = HashMap::new();
    for (route_file, r) in routes {
        let key: Vec<&str> = r
            .segments
            .iter()
            .enumerate()
            .filter(|&(i, s)| !(i == 0 && mount.contains(s.as_str())))
            .map(|(_, s)| s.as_str())
            .collect();
        if key.is_empty() {
            continue;
        }
        let view_file = match defs.get(r.view.as_str()).map(Vec::as_slice) {
            Some([only]) => *only,
            Some(many) if many.contains(&route_file) => route_file,
            _ => continue,
        };
        let entry = links.entry((r.view.clone(), view_file)).or_default();
        entry.0.push(key.join("/"));
        for (file, segs) in &callers {
            if key.iter().all(|k| segs.contains(k)) {
                entry.1.push(*file);
            }
        }
    }

    let mut out: Vec<RouteLink> = links
        .into_iter()
        .filter(|(_, (_, callers))| !callers.is_empty())
        .map(|((view, view_file), (mut routes, mut callers))| {
            routes.sort();
            routes.dedup();
            callers.sort_unstable();
            callers.dedup();
            RouteLink { view, view_file, routes, callers }
        })
        .collect();
    out.sort_by(|a, b| (a.view_file, &a.view).cmp(&(b.view_file, &b.view)));
    out
}

/// `.codearch/routes.md`: one section per view, its routes, its callers.
pub fn routes_markdown(inv: &Inventory, links: &[RouteLink]) -> String {
    let mut s = String::from(
        "# Route callers\n\nBackend views and the frontend files whose request strings name \
every segment of the view's route (mount prefix such as `api/` dropped). Static evidence: \
string-built URLs can be missed or over-matched.\n",
    );
    for l in links {
        s.push_str(&format!("\n## `{}` — `{}`\n\n", l.view, inv.get(l.view_file).rel));
        s.push_str(&format!(
            "Routes: {}\n\n",
            l.routes.iter().map(|r| format!("`{r}`")).collect::<Vec<_>>().join(", ")
        ));
        for c in &l.callers {
            s.push_str(&format!("- `{}`\n", inv.get(*c).rel));
        }
    }
    s
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

    fn parse_symbols(src: &str, lang: Language) -> Vec<crate::types::Symbol> {
        let mut p = Parsers::new();
        parse_one(&mut p, 0, lang, src).symbols
    }

    fn parsed_with_symbols(
        syms: &[(&[(&str, crate::types::SymbolKind, bool)], usize)],
    ) -> Vec<FileParse> {
        syms.iter()
            .map(|(list, file)| FileParse {
                file: *file,
                symbols: list
                    .iter()
                    .map(|&(name, kind, exported)| crate::types::Symbol {
                        name: name.to_string(),
                        kind,
                        exported,
                        line: 1,
                    })
                    .collect(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn shared_rare_shape_joins_across_languages() {
        let inv = inventory(&[
            ("frontend/api.ts", Language::Ts),
            ("backend/service.py", Language::Python),
        ]);
        let parsed = parsed_with_symbols(&[
            (
                &[("createUser", crate::types::SymbolKind::Function, true)],
                0,
            ),
            (&[("create_user", crate::types::SymbolKind::Function, false)],
             1),
        ]);
        assert_eq!(semantic_join(&inv, &parsed), vec![(0, 1)]);
    }

    #[test]
    fn common_shape_stems_never_join() {
        let inv = inventory(&[
            ("frontend/api.ts", Language::Ts),
            ("backend/service.py", Language::Python),
        ]);
        // Bare `run` is vocabulary in every codebase, not a contract.
        let parsed = parsed_with_symbols(&[
            (&[("run", crate::types::SymbolKind::Function, true)], 0),
            (&[("run", crate::types::SymbolKind::Function, false)], 1),
        ]);
        assert!(semantic_join(&inv, &parsed).is_empty());
    }

    #[test]
    fn unexported_ts_symbols_do_not_join() {
        // An internal TS const is not a callable surface; Python defs always
        // are (no export keyword exists there).
        let inv = inventory(&[
            ("frontend/api.ts", Language::Ts),
            ("backend/service.py", Language::Python),
        ]);
        let parsed = parsed_with_symbols(&[
            (
                &[("internalThing", crate::types::SymbolKind::Function, false)],
                0,
            ),
            (
                &[("internal_thing", crate::types::SymbolKind::Function, false)],
                1,
            ),
        ]);
        assert!(semantic_join(&inv, &parsed).is_empty());
    }

    #[test]
    fn url_pairs_are_not_double_asserted_semantically() {
        let inv = inventory(&[
            ("frontend/api.ts", Language::Ts),
            ("backend/service.py", Language::Python),
        ]);
        let mut parsed = parsed_with_symbols(&[
            (&[("syncCart", crate::types::SymbolKind::Function, true)], 0),
            (&[("sync_cart", crate::types::SymbolKind::Function, false)], 1),
        ]);
        parsed[0].urls = vec!["api/users".to_string()];
        parsed[1].urls = vec!["api/users".to_string()];
        // The URL contract already pairs them; semantic must not re-add.
        assert_eq!(join(&inv, &parsed), vec![(0, 1)]);
        assert!(semantic_join(&inv, &parsed).is_empty());
    }

    #[test]
    fn same_ecosystem_shapes_do_not_join() {
        let inv = inventory(&[
            ("a/one.ts", Language::Ts),
            ("b/two.tsx", Language::Tsx),
        ]);
        let parsed = parsed_with_symbols(&[
            (&[("chargeOrder", crate::types::SymbolKind::Function, true)], 0),
            (&[("charge_order", crate::types::SymbolKind::Function, true)], 1),
        ]);
        assert!(semantic_join(&inv, &parsed).is_empty());
    }

    #[test]
    fn stems_of_length_two_do_not_join() {
        let inv = inventory(&[
            ("frontend/api.ts", Language::Ts),
            ("backend/service.py", Language::Python),
        ]);
        let parsed = parsed_with_symbols(&[
            (&[("ab", crate::types::SymbolKind::Function, true)], 0),
            (&[("ab", crate::types::SymbolKind::Function, false)], 1),
        ]);
        assert!(semantic_join(&inv, &parsed).is_empty());
    }

    #[test]
    fn symbol_parse_streams_export_flag_through() {
        let ts = parse_symbols(
            "export function syncCart() {}\nfunction internalHelper() {}\n",
            Language::Ts,
        );
        assert!(ts.iter().find(|s| s.name == "syncCart").unwrap().exported);
        assert!(!ts
            .iter()
            .find(|s| s.name == "internalHelper")
            .unwrap()
            .exported);
        let py = parse_symbols("def sync_cart(): ...\n", Language::Python);
        assert!(!py[0].exported);
    }

    #[test]
    fn symbol_parse_produces_joinable_stems() {
        let mut p = Parsers::new();
        let ts = parse_one(
            &mut p,
            0,
            Language::Ts,
            "export function createUser() {}\n",
        );
        let py = parse_one(
            &mut p,
            1,
            Language::Python,
            "def create_user(): ...\n",
        );
        let ts_stems: Vec<String> = ts.symbols.iter().map(|s| s.stem_key()).collect();
        let py_stems: Vec<String> = py.symbols.iter().map(|s| s.stem_key()).collect();
        assert_eq!(ts_stems, vec!["createuser".to_string()]);
        assert_eq!(py_stems, vec!["createuser".to_string()]);
    }

    fn parse_files(files: &[(&str, Language, &str)]) -> (Inventory, Vec<FileParse>) {
        let inv = inventory(&files.iter().map(|(p, l, _)| (*p, *l)).collect::<Vec<_>>());
        let mut p = Parsers::new();
        let parsed = files
            .iter()
            .enumerate()
            .map(|(i, (_, l, src))| parse_one(&mut p, i, *l, src))
            .collect();
        (inv, parsed)
    }

    const URLS_PY: &str = "router.register(r'tags', TagViewSet)
urlpatterns = [
    re_path(r'^api/', include([
        re_path('^documents/', include([
            re_path('^bulk_edit/', BulkEditView.as_view()),
            re_path('^merge/', MergeView.as_view()),
        ])),
        re_path('^profile/', ProfileView.as_view()),
    ])),
]
";
    const VIEWS_PY: &str = "class TagViewSet: ...
class BulkEditView: ...
class MergeView: ...
class ProfileView: ...
";

    #[test]
    fn route_join_links_views_to_inheriting_services_through_the_mount() {
        let (inv, parsed) = parse_files(&[
            ("src/urls.py", Language::Python, URLS_PY),
            ("src/views.py", Language::Python, VIEWS_PY),
            ("ui/abstract.service.ts", Language::Ts,
             "export abstract class AbstractService { constructor(private http: HttpClient) {} url = `${environment.apiBaseUrl}${this.resourceName}/` }
"),
            ("ui/tag.service.ts", Language::Ts,
             "export class TagService extends AbstractService { resourceName = 'tags' }
"),
            ("ui/document.service.ts", Language::Ts,
             "export class DocumentService extends AbstractService { resourceName = 'documents'; edit() { return this.url + 'bulk_edit/' } }
"),
            ("ui/tags.component.ts", Language::Ts,
             "export class TagsComponent { route = 'tags' }
"),
        ]);
        let links = route_join(&inv, &parsed);
        let callers = |view: &str| -> Vec<String> {
            links.iter().find(|l| l.view == view).map(|l| {
                l.callers.iter().map(|c| inv.get(*c).rel.clone()).collect()
            }).unwrap_or_default()
        };
        assert_eq!(callers("TagViewSet"), vec!["ui/tag.service.ts"], "component without HTTP is not a caller");
        assert_eq!(callers("BulkEditView"), vec!["ui/document.service.ts"], "api/ mount dropped, both segments required");
        assert!(callers("MergeView").is_empty());
        assert!(callers("ProfileView").is_empty());
        let tag = links.iter().find(|l| l.view == "TagViewSet").unwrap();
        assert_eq!(inv.get(tag.view_file).rel, "src/views.py");
        let md = routes_markdown(&inv, &links);
        assert!(md.contains("## `BulkEditView` — `src/views.py`"));
        assert!(md.contains("Routes: `documents/bulk_edit`"));
    }

    #[test]
    fn route_join_ignores_tests_and_ambiguous_views() {
        let (mut inv, parsed) = parse_files(&[
            ("src/urls.py", Language::Python, "urlpatterns = [path('api/tags/', TagView.as_view()), path('api/exports/', ExportView.as_view())]
"),
            ("src/a.py", Language::Python, "class TagView: ...
"),
            ("src/b.py", Language::Python, "class TagView: ...
class ExportView: ...
"),
            ("ui/tag.service.spec.ts", Language::Ts, "http.expectOne(`${environment.apiBaseUrl}tags/`)
"),
            ("ui/export.service.ts", Language::Ts, "fetch('/api/exports/')
"),
        ]);
        inv.files[3].class = FileClass::Test;
        let links = route_join(&inv, &parsed);
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].view, "ExportView");
        assert!(route_join(&inv, &parsed[3..]).is_empty(), "no Python routes, no links");
    }
}
