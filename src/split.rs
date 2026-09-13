//! Hierarchical split: coarse units for the root map, fine clusters nested.
//!
//! Job    Turn one flat partition into a two-level hierarchy the root budget
//!        can hold: graph-derived full units plus directory buckets for the
//!        disconnected tail, with every fine cluster nested under exactly one
//!        coarse unit by majority.
//! In     CodeGraph, fine Partition, importance scores, rels
//! Out    Vec<CoarseUnit>, parent per fine cluster, routing terms, budgets
//! Fails  Cannot fail. Degenerate inputs yield buckets, never invented links.
//!
//! The tail rule is the whole point: a coarse unit with zero total edge
//! weight is files that share nothing the graph can see. Grouping those by
//! directory claims only co-location, and the low-band machinery renders
//! exactly that. Merging them into named communities would assert
//! relationships the code does not have.

use crate::cluster;
use crate::graph::CodeGraph;
use crate::types::FileId;
use std::collections::{HashMap, HashSet};

/// Resolution for the coarse pass. Low gamma merges connected communities
/// big while edgeless files stay single whatever the value.
pub const COARSE_GAMMA: f64 = 0.5;

/// Floor and cap for per-domain-file token budgets (Decision 2).
pub const DOMAIN_BUDGET_MIN: usize = 300;
pub const DOMAIN_BUDGET_MAX: usize = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Full,
    Bucket,
}

#[derive(Debug, Clone)]
pub struct CoarseUnit {
    pub kind: UnitKind,
    pub files: Vec<FileId>,
    /// Nested fine cluster ids, sorted ascending.
    pub fine: Vec<usize>,
    /// Directory key for buckets (`compiler/packages`), empty for full units.
    pub dir_key: String,
}

/// Coarse partition of the graph: connected communities as full candidates,
/// the edgeless tail as directory buckets. `max_full` caps full units;
/// buckets are unbounded pointers and outside the cap.
pub fn coarse(g: &CodeGraph, rels: &[String], seed: u64, max_full: usize) -> Vec<CoarseUnit> {
    let base = cluster::merge_small(
        g,
        cluster::partition(g, COARSE_GAMMA, seed),
        cluster::MIN_CLUSTER_SIZE,
    );
    let members = base.members();

    // Full means connected TO THE WORLD, not merely internally coherent: an
    // isolated 200-file component is real substructure with nowhere to route
    // from, so it rides a bucket (its fine subsections still render there)
    // rather than occupying a root slot.
    let mut full: Vec<Vec<FileId>> = Vec::new();
    let mut tail: Vec<FileId> = Vec::new();
    for files in members {
        if files.is_empty() {
            continue;
        }
        if external_weight(g, &files) > 0.0 {
            full.push(files);
        } else {
            tail.extend(files);
        }
    }

    let mut units: Vec<CoarseUnit> = merge_full_units(g, full, max_full)
        .into_iter()
        .map(|files| CoarseUnit {
            kind: UnitKind::Full,
            files,
            fine: Vec::new(),
            dir_key: String::new(),
        })
        .collect();
    units.extend(
        buckets_by_rel(&tail, rels)
            .into_iter()
            .map(|(dir_key, files)| CoarseUnit {
                kind: UnitKind::Bucket,
                files,
                fine: Vec::new(),
                dir_key,
            }),
    );
    // Initial order only: full units first, then buckets by key. The runner
    // permutes units into coarse-summary order afterwards (dependency ids
    // align without a translation table), so this order does not survive —
    // what matters here is that it is deterministic.
    units.sort_by(|a, b| match (a.kind, b.kind) {
        (UnitKind::Full, UnitKind::Bucket) => std::cmp::Ordering::Less,
        (UnitKind::Bucket, UnitKind::Full) => std::cmp::Ordering::Greater,
        _ => a.dir_key.cmp(&b.dir_key),
    });
    units
}

/// Total edge weight from member files to files OUTSIDE the member set.
/// Adjacency is undirected, so any cross-boundary edge counts once.
fn external_weight(g: &CodeGraph, files: &[FileId]) -> f64 {
    let member: HashSet<FileId> = files.iter().copied().collect();
    files
        .iter()
        .flat_map(|&i| g.adj[i].iter().map(move |&(j, w)| (j, w)))
        .filter(|(j, _)| !member.contains(j))
        .map(|(_, w)| w)
        .sum()
}

/// Cap full units by repeatedly folding the smallest into its strongest
/// full-neighbour. Units with no full-neighbour stay: over-cap is reported,
/// links are not invented. Terminates: every fold removes one unit.
fn merge_full_units(g: &CodeGraph, mut full: Vec<Vec<FileId>>, max: usize) -> Vec<Vec<FileId>> {
    while full.len() > max {
        let index_of: HashMap<FileId, usize> = full
            .iter()
            .enumerate()
            .flat_map(|(u, fs)| fs.iter().map(move |&f| (f, u)))
            .collect();
        let mut order: Vec<usize> = (0..full.len()).collect();
        order.sort_by_key(|&u| (full[u].len(), u));
        let mut folded = false;
        for &u in &order {
            let mut pull = vec![0.0f64; full.len()];
            for &i in &full[u] {
                for &(j, w) in &g.adj[i] {
                    if let Some(&v) = index_of.get(&j) {
                        if v != u {
                            pull[v] += w;
                        }
                    }
                }
            }
            let target = pull
                .iter()
                .enumerate()
                .filter(|(v, _)| *v != u)
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .filter(|(_, w)| **w > 0.0)
                .map(|(v, _)| v);
            if let Some(v) = target {
                let mut moved = std::mem::take(&mut full[u]);
                full[v].append(&mut moved);
                full.remove(u);
                folded = true;
                break;
            }
        }
        if !folded {
            break;
        }
    }
    full
}

/// Bucketing with rels available: one bucket per top-level directory, plus
/// `(root)` for top-level files. No subdivision: a bucket is a pointer
/// regardless of size, and splitting `big/a` (201 files, one real directory)
/// into 201 file-pointers would be a listing wearing a hierarchy costume.
/// The domain file behind the pointer is budget-capped and truncates
/// honestly instead.
pub fn buckets_by_rel(files: &[FileId], rels: &[String]) -> Vec<(String, Vec<FileId>)> {
    let mut groups: HashMap<String, Vec<FileId>> = HashMap::new();
    for &f in files {
        groups.entry(top_key(&rels[f], 1)).or_default().push(f);
    }
    let mut out: Vec<(String, Vec<FileId>)> = groups.into_iter().collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, fs) in out.iter_mut() {
        fs.sort_by(|&a, &b| rels[a].cmp(&rels[b]));
    }
    out
}

fn top_key(rel: &str, depth: usize) -> String {
    if !rel.contains('/') {
        return "(root)".to_string();
    }
    let segs: Vec<&str> = rel.split('/').collect();
    if segs.len() <= depth {
        return rel.to_string();
    }
    if depth == 0 {
        return "(root)".to_string();
    }
    segs[..depth].join("/")
}

/// Nest fine clusters under coarse units by majority of member files. Ties
/// go to the smallest unit index. `members` are the fine clusters in
/// whatever order the caller renders them — post-reorder summaries, not
/// pre-reorder partition ids (see `align_by_files`). Returns the parent per
/// member index, in the same order.
pub fn nest(members: &[Vec<FileId>], file_unit: &[usize]) -> Vec<usize> {
    members
        .iter()
        .map(|fs| {
            let mut counts: HashMap<usize, usize> = HashMap::new();
            for &f in fs {
                if let Some(&u) = file_unit.get(f) {
                    *counts.entry(u).or_insert(0) += 1;
                }
            }
            // Sorted, not max_by: HashMap iteration order is random, and a
            // comparator tiebreak cannot fix a random visit order.
            let mut ranked: Vec<(usize, usize)> = counts.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            ranked.into_iter().next().map(|(u, _)| u).unwrap_or(0)
        })
        .collect()
}

/// Align groups to summaries by member file SET. `summarize` reorders
/// clusters by importance and renumbers them, so position `i` in the
/// summaries is generally not partition id `i` — the only honest join key
/// is the file set itself. Partitions cover every file exactly once, so the
/// match is exact and unique; the index fallback is unreachable in practice
/// and keeps the function total.
pub fn align_by_files(groups: &[Vec<FileId>], summaries: &[Vec<FileId>]) -> Vec<usize> {
    let mut key_of: HashMap<Vec<FileId>, usize> = HashMap::new();
    for (i, fs) in summaries.iter().enumerate() {
        let mut sorted = fs.clone();
        sorted.sort_unstable();
        key_of.insert(sorted, i);
    }
    groups
        .iter()
        .enumerate()
        .map(|(u, fs)| {
            let mut sorted = fs.clone();
            sorted.sort_unstable();
            key_of.get(&sorted).copied().unwrap_or_else(|| u.min(summaries.len().saturating_sub(1)))
        })
        .collect()
}

/// Per cluster: does it hold at least one internal import edge? Buckets
/// render connected nested clusters as named subsections and bare singletons
/// by directory; this flag is what separates the two. Members arrive in
/// render order (post-reorder summaries), matching every other per-fine
/// array the runner threads.
pub fn fine_connected(g: &CodeGraph, members: &[Vec<FileId>]) -> Vec<bool> {
    members
        .iter()
        .map(|fs| {
            let member: HashSet<FileId> = fs.iter().copied().collect();
            fs.iter().any(|&i| {
                g.out_edges[i]
                    .iter()
                    .any(|j| *j != i && member.contains(j))
            })
        })
        .collect()
}

/// Per-unit token budgets: share proportional to `size × log(1 + churn)`,
/// floored and capped per Decision 2. Without churn history the size term
/// stands alone.
pub fn domain_budgets(units: &[CoarseUnit], churn: &[usize]) -> Vec<usize> {
    let weights: Vec<f64> = units
        .iter()
        .map(|u| {
            // Empty without git history: the size term stands alone.
            let churn_sum: usize =
                u.files.iter().map(|&f| churn.get(f).copied().unwrap_or(0)).sum();
            u.files.len() as f64 * (1.0 + churn_sum as f64).ln_1p()
        })
        .collect();
    let total: f64 = weights.iter().sum::<f64>().max(1.0);
    // The root map holds the overview; domain files share a bounded pool on
    // top. Pool size scales with unit count so shares stay meaningful.
    let pool = (units.len() * DOMAIN_BUDGET_MAX) as f64;
    weights
        .iter()
        .map(|w| {
            let share = if total <= 0.0 {
                0.0
            } else {
                w / total * pool
            };
            (share as usize).clamp(DOMAIN_BUDGET_MIN, DOMAIN_BUDGET_MAX)
        })
        .collect()
}

const ROUTING_STOP: &[&str] = &[
    "src", "lib", "index", "test", "tests", "spec", "app", "main", "init",
    "util", "utils", "common", "core", "shared", "helper", "helpers",
    // Extensions and dotted fragments (`line.js`, `config.build`) are file
    // addresses, not vocabulary: routing on them sends agents to suffixes.
    "ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs", "py",
    "json", "md", "yml", "yaml", "lock",
];

/// Discriminative routing terms per unit: identifier-ish tokens from member
/// paths plus caller-supplied symbol terms, minus anything appearing in more
/// than one unit, top 8 by frequency (alpha tiebreak). Buckets route on
/// directory vocabulary, which is all they honestly have.
pub fn routing_terms(
    units: &[CoarseUnit],
    rels: &[String],
    extra: &[Vec<String>],
) -> Vec<Vec<String>> {
    let per_unit: Vec<Vec<String>> = units
        .iter()
        .enumerate()
        .map(|(u, unit)| {
            let mut terms = Vec::new();
            for &f in &unit.files {
                terms.extend(path_terms(&rels[f]));
            }
            if let Some(syms) = extra.get(u) {
                for s in syms {
                    terms.extend(split_ident(s));
                }
            }
            terms
        })
        .collect();
    let mut doc_of: HashMap<String, usize> = HashMap::new();
    for terms in &per_unit {
        let mut seen = HashSet::new();
        for t in terms {
            if seen.insert(t.as_str()) {
                *doc_of.entry(t.clone()).or_insert(0) += 1;
            }
        }
    }
    per_unit
        .into_iter()
        .map(|terms| {
            let mut freq: HashMap<&str, usize> = HashMap::new();
            for t in &terms {
                if doc_of.get(t).copied().unwrap_or(0) > 1 {
                    continue;
                }
                *freq.entry(t.as_str()).or_insert(0) += 1;
            }
            let mut ranked: Vec<(&str, usize)> = freq.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            ranked.into_iter().take(8).map(|(t, _)| t.to_string()).collect()
        })
        .collect()
}

fn path_terms(rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in rel.split('/') {
        // Every dot-separated part on its own: `runtime.development.js`
        // contributes runtime + development, never the dotted whole.
        for part in seg.split('.') {
            out.extend(split_ident(part));
        }
    }
    out.into_iter()
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() > 2 && !ROUTING_STOP.contains(&s.as_str()))
        .collect()
}

fn split_ident(s: &str) -> Vec<String> {    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
        } else if c.is_ascii_uppercase() && prev_lower {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(c);
            prev_lower = false;
        } else {
            cur.push(c);
            prev_lower = c.is_ascii_lowercase();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| t.len() > 2 && !ROUTING_STOP.contains(&t.as_str()))
        .collect()
}

/// URL-safe slug for a domain filename: lowercase alphanumerics, runs of
/// anything else collapsed to one dash, no leading/trailing dashes.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("domain");
    }
    out
}

/// Slugs for all units, suffixed on collision (`core`, `core-2`). Order is
/// the unit order; collisions resolve deterministically.
pub fn assign_slugs(names: &[String]) -> Vec<String> {
    let mut used = HashSet::new();
    names
        .iter()
        .map(|n| {
            let base = slugify(n);
            let mut slug = base.clone();
            let mut i = 2;
            while !used.insert(slug.clone()) {
                slug = format!("{base}-{i}");
                i += 1;
            }
            slug
        })
        .collect()
}

/// Additive hierarchy value for `index.json`: coarse units with their nested
/// fine ids, plus the parent array. Flat `clusters` are untouched, so eval
/// tooling keeps reading what it always read.
pub fn hierarchy_json(
    units: &[CoarseUnit],
    names: &[String],
    parent: &[usize],
) -> serde_json::Value {
    let coarse: Vec<serde_json::Value> = units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            serde_json::json!({
                "id": i,
                "name": names.get(i).cloned().unwrap_or_default(),
                "kind": match u.kind {
                    UnitKind::Full => "full",
                    UnitKind::Bucket => "bucket",
                },
                "files": u.files.len(),
                "fine": u.fine,
            })
        })
        .collect();
    serde_json::json!({ "coarse": coarse, "parent": parent })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tail_files_bucket_by_directory() {
        // The triple is internally connected but isolated from the world:
        // Full means connected outward, so it rides a bucket with its fine
        // subsections rather than occupying a root slot.
        let g = CodeGraph::from_edges(7, &[(0, 1, 1.0), (1, 2, 1.0)]);
        let r = rels(&["a/x.ts", "a/y.ts", "a/z.ts", "d1/p.ts", "d1/q.ts", "d2/r.ts", "d2/s.ts"]);
        let units = coarse(&g, &r, 0x5EED, 12);
        assert!(units.iter().all(|u| u.kind == UnitKind::Bucket));
        assert_eq!(units.len(), 3);
        let keys: Vec<&str> = units.iter().map(|u| u.dir_key.as_str()).collect();
        assert!(keys.contains(&"a") && keys.contains(&"d1") && keys.contains(&"d2"));
    }

    #[test]
    fn isolated_blobs_bucket_despite_internal_edges() {
        // One connected blob, no outside world: Full means connected
        // outward, so even a real community rides buckets (its fine
        // subsections still render in the domain file).
        let g = CodeGraph::from_edges(
            6,
            &[(0, 1, 1.0), (1, 2, 1.0), (3, 4, 1.0), (4, 5, 1.0), (2, 3, 1.0)],
        );
        let r = rels(&["a/0.ts", "a/1.ts", "a/2.ts", "b/3.ts", "b/4.ts", "b/5.ts"]);
        let units = coarse(&g, &r, 0x5EED, 12);
        assert!(units.iter().all(|u| u.kind == UnitKind::Bucket));
    }

    #[test]
    fn external_weight_sees_only_outward_edges() {
        use crate::graph::CodeGraph;
        let g = CodeGraph::from_edges(3, &[(0, 1, 2.0), (1, 2, 3.0)]);
        assert!((external_weight(&g, &[0, 1]) - 3.0).abs() < 1e-9);
        assert!((external_weight(&g, &[0, 1, 2]) - 0.0).abs() < 1e-9);
        assert!((external_weight(&g, &[2]) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn buckets_group_by_top_level_dir() {
        // One bucket per top-level dir however large: subdividing big/a
        // (201 files, one real directory) into file-pointers would be a
        // listing wearing a hierarchy costume.
        let r: Vec<String> = (0..201).map(|i| format!("big/a/{i}.ts")).collect();
        let files: Vec<usize> = (0..201).collect();
        let out = buckets_by_rel(&files, &r);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "big");
        assert_eq!(out[0].1.len(), 201);
        // Deterministic across runs (HashMap iteration is not).
        assert_eq!(buckets_by_rel(&files, &r), out);
    }

    #[test]
    fn root_level_files_bucket_together() {
        let r = rels(&["a.ts", "b.ts"]);
        let out = buckets_by_rel(&[0, 1], &r);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "(root)");
    }

    #[test]
    fn nesting_is_majority_with_deterministic_ties() {
        // Both clusters straddle two units equally: smallest index wins, and
        // identically on every run despite HashMap iteration order.
        // Members arrive in render order, not partition-id order.
        let members = vec![vec![0, 1], vec![2, 3]];
        let parent = nest(&members, &[0, 1, 1, 2]);
        assert_eq!(parent, vec![0, 1]);
        assert_eq!(nest(&members, &[0, 1, 1, 2]), parent);
    }

    #[test]
    fn align_joins_by_file_set_not_position() {
        // Reordered summaries: group 0's files sit at summary position 1.
        let groups = vec![vec![2, 3], vec![0, 1]];
        let summaries = vec![vec![0, 1], vec![3, 2]];
        assert_eq!(align_by_files(&groups, &summaries), vec![1, 0]);
    }

    #[test]
    fn routing_terms_are_discriminative() {
        let units = vec![
            CoarseUnit { kind: UnitKind::Full, files: vec![0, 1], fine: vec![], dir_key: String::new() },
            CoarseUnit { kind: UnitKind::Full, files: vec![2], fine: vec![], dir_key: String::new() },
        ];
        let r = rels(&["billing/checkout.ts", "billing/stripe.ts", "auth/login.ts"]);
        let terms = routing_terms(&units, &r, &[]);
        assert!(terms[0].contains(&"billing".to_string()));
        assert!(terms[0].contains(&"checkout".to_string()) || terms[0].contains(&"stripe".to_string()));
        assert!(terms[1].contains(&"login".to_string()));
        // Shared terms never route.
        for t in terms[0].iter().chain(terms[1].iter()) {
            assert_ne!(t, "ts");
        }
    }

    #[test]
    fn routing_terms_never_carry_dots_or_extensions() {
        // `runtime.development.js` routes runtime + development, never the
        // dotted whole or the suffix — file addresses are not vocabulary.
        let units = vec![
            CoarseUnit { kind: UnitKind::Full, files: vec![0], fine: vec![], dir_key: String::new() },
        ];
        let r = rels(&["src/runtime.development.js"]);
        let terms = routing_terms(&units, &r, &[]);
        assert!(!terms[0].iter().any(|t| t.contains('.')));
        assert!(!terms[0].contains(&"js".to_string()));
        assert!(terms[0].contains(&"runtime".to_string()));
        assert!(terms[0].contains(&"development".to_string()));
    }

    #[test]
    fn budgets_floor_cap_and_favor_churn() {
        let units = vec![
            CoarseUnit { kind: UnitKind::Full, files: vec![0; 100], fine: vec![], dir_key: String::new() },
            CoarseUnit { kind: UnitKind::Full, files: vec![1; 2], fine: vec![], dir_key: String::new() },
        ];
        let churn = vec![10usize, 0, 0];
        let b = domain_budgets(&units, &churn);
        assert!(b[0] >= b[1]);
        assert!(b.iter().all(|&x| x >= DOMAIN_BUDGET_MIN && x <= DOMAIN_BUDGET_MAX));
    }

    #[test]
    fn slugs_are_safe_and_unique() {
        assert_eq!(slugify("Reg Exp Router"), "reg-exp-router");
        assert_eq!(slugify("compiler/packages"), "compiler-packages");
        assert_eq!(slugify("..."), "domain");
        assert_eq!(
            assign_slugs(&["Core".to_string(), "Core 2".to_string(), "Core".to_string()]),
            vec!["core", "core-2", "core-3"]
        );
    }
}
