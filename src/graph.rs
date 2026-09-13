//! Stage 5 — Graph.
//!
//! Job    Fuse every available signal into one weighted graph over files.
//! In     Resolution (import edges), Inventory (directory structure)
//! Out    CodeGraph
//! Fails  Cannot fail. With no imports at all it degrades to directory adjacency.
//!
//! M2 carries three of the four planned signals. Embedding similarity (0.35)
//! stays out: M3 showed the Tier-2 gap was a resolver bug, not a missing
//! signal. Cross-language contracts joined at 0.45: an exact normalized-path
//! match is precise but lexical, so below behavioral co-change and above
//! co-location. The fusion point is here so adding a signal touches only
//! this file.

use crate::git::CoChange;
use crate::inventory::Inventory;
use crate::resolve::Resolution;
use std::collections::HashMap;

/// Strong, directional, ground truth.
pub const W_IMPORT: f64 = 1.00;
/// Language-agnostic and noisy alone, which is why it is worth less than an
/// import edge and more than mere co-location.
pub const W_COCHANGE: f64 = 0.55;
/// An exact cross-language contract match: precise but lexical, hence below
/// co-change (behavioral) and above directory (prior).
pub const W_CONTRACT: f64 = 0.45;
/// Weak prior that keeps sibling files from fragmenting when imports are sparse.
pub const W_DIRECTORY: f64 = 0.25;

/// Above this many files in one directory, the all-pairs prior would add more
/// edges than signal, so it is skipped for that directory.
const DIR_CLIQUE_MAX: usize = 30;

pub struct CodeGraph {
    pub n: usize,
    /// Undirected fused adjacency: `adj[i]` holds `(j, weight)`.
    pub adj: Vec<Vec<(usize, f64)>>,
    /// Directed edges, for PageRank, fan-in, flows and cluster coupling:
    /// imports plus API contracts (both directions — the join does not know
    /// consumer from provider, and the coupling is real either way).
    pub out_edges: Vec<Vec<usize>>,
    pub in_edges: Vec<Vec<usize>>,
    /// Co-change pairs that survived stage 4, `(low, high)` and sorted. Kept
    /// apart from the fused weights because the confidence model measures how
    /// far this signal and the import graph agree.
    pub cochange: Vec<(usize, usize)>,
    /// Contract pairs fused from the cross-language join, same shape as
    /// `cochange`. Kept apart so the map can say where the edge came from.
    pub contracts: Vec<(usize, usize)>,
}

impl CodeGraph {
    pub fn degree(&self, i: usize) -> f64 {
        self.adj[i].iter().map(|(_, w)| w).sum()
    }

    /// Sum of undirected edge weights (each edge counted once).
    pub fn total_weight(&self) -> f64 {
        self.adj
            .iter()
            .flat_map(|row| row.iter().map(|(_, w)| *w))
            .sum::<f64>()
            / 2.0
    }

    pub fn edge_count(&self) -> usize {
        self.adj.iter().map(|r| r.len()).sum::<usize>() / 2
    }

    pub fn fan_in(&self, i: usize) -> usize {
        self.in_edges[i].len()
    }

    /// Builds a graph directly from edges. Used by tests and by callers that
    /// already hold an edge list.
    pub fn from_edges(n: usize, edges: &[(usize, usize, f64)]) -> CodeGraph {
        let mut adj = vec![Vec::new(); n];
        let mut out_edges = vec![Vec::new(); n];
        let mut in_edges = vec![Vec::new(); n];
        for &(i, j, w) in edges {
            adj[i].push((j, w));
            adj[j].push((i, w));
            out_edges[i].push(j);
            in_edges[j].push(i);
        }
        CodeGraph {
            n,
            adj,
            out_edges,
            in_edges,
            cochange: Vec::new(),
            contracts: Vec::new(),
        }
    }
}

pub fn build(
    inv: &Inventory,
    res: &Resolution,
    cc: &CoChange,
    contracts: &[(usize, usize)],
) -> CodeGraph {
    let n = inv.len();
    let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
    let mut out_edges = vec![Vec::new(); n];
    let mut in_edges = vec![Vec::new(); n];

    for &(from, to) in &res.edges {
        if from >= n || to >= n || from == to {
            continue;
        }
        out_edges[from].push(to);
        in_edges[to].push(from);
        *weights.entry(key(from, to)).or_insert(0.0) += W_IMPORT;
    }

    // Contracts: the one coupling imports cannot see. Both directions enter
    // the directed edges — flows, fan-in and cluster coupling all traverse
    // them — at a weight below co-change because the evidence is lexical.
    let mut fused_contracts = Vec::with_capacity(contracts.len());
    for &(a, b) in contracts {
        if a >= n || b >= n || a == b {
            continue;
        }
        out_edges[a].push(b);
        out_edges[b].push(a);
        in_edges[a].push(b);
        in_edges[b].push(a);
        *weights.entry(key(a, b)).or_insert(0.0) += W_CONTRACT;
        fused_contracts.push(key(a, b));
    }

    // Co-change: files that keep changing together are coupled whether or not
    // either imports the other. Weights arrive normalized to (0, 1].
    let mut cochange = Vec::with_capacity(cc.pairs.len());
    for &((i, j), w) in &cc.pairs {
        if i >= n || j >= n || i == j {
            continue;
        }
        *weights.entry(key(i, j)).or_insert(0.0) += W_COCHANGE * w;
        cochange.push(key(i, j));
    }

    // Directory adjacency: files that sit together usually belong together,
    // but only weakly, and only where the directory is small enough that
    // co-location actually means something.
    let mut by_dir: HashMap<&str, Vec<usize>> = HashMap::new();
    for f in &inv.files {
        by_dir.entry(f.dir()).or_default().push(f.id);
    }
    for members in by_dir.values() {
        if members.len() < 2 || members.len() > DIR_CLIQUE_MAX {
            continue;
        }
        for (a, &i) in members.iter().enumerate() {
            for &j in &members[a + 1..] {
                *weights.entry(key(i, j)).or_insert(0.0) += W_DIRECTORY;
            }
        }
    }

    let mut adj = vec![Vec::new(); n];
    // Sorted for reproducibility: HashMap iteration order is not stable.
    let mut pairs: Vec<((usize, usize), f64)> = weights.into_iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    for ((i, j), w) in pairs {
        adj[i].push((j, w));
        adj[j].push((i, w));
    }

    for row in &mut out_edges {
        row.sort_unstable();
        row.dedup();
    }
    for row in &mut in_edges {
        row.sort_unstable();
        row.dedup();
    }

    CodeGraph {
        n,
        adj,
        out_edges,
        in_edges,
        cochange,
        contracts: fused_contracts,
    }
}

fn key(a: usize, b: usize) -> (usize, usize) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileClass, FileRecord, Language};
    use std::path::PathBuf;

    fn inventory(paths: &[&str]) -> Inventory {
        Inventory {
            root: PathBuf::from("."),
            files: paths
                .iter()
                .enumerate()
                .map(|(id, rel)| FileRecord {
                    id,
                    rel: (*rel).into(),
                    abs: PathBuf::from(rel),
                    language: Language::Ts,
                    bytes: 100,
                    loc: 10,
                    class: FileClass::Source,
                })
                .collect(),
            skipped: Vec::new(),
            excluded: Default::default(),
        }
    }

    fn weight(g: &CodeGraph, i: usize, j: usize) -> f64 {
        g.adj[i]
            .iter()
            .find(|(k, _)| *k == j)
            .map(|(_, w)| *w)
            .unwrap_or(0.0)
    }

    #[test]
    fn co_change_adds_an_edge_where_no_import_exists() {
        // Separate directories, so directory adjacency cannot explain the edge.
        let inv = inventory(&["a/one.ts", "b/two.ts"]);
        let res = Resolution::default();
        let cc = CoChange {
            pairs: vec![((0, 1), 1.0)],
            churn: vec![3, 3],
            commits_read: 3,
        };
        let g = build(&inv, &res, &cc, &[]);
        assert!((weight(&g, 0, 1) - W_COCHANGE).abs() < 1e-9);
        assert_eq!(g.cochange, vec![(0, 1)]);
        // It is not an import edge: PageRank and fan-in must not see it.
        assert_eq!(g.fan_in(1), 0);
    }

    #[test]
    fn co_change_and_imports_sum_on_the_same_pair() {
        let inv = inventory(&["a/one.ts", "b/two.ts"]);
        let mut res = Resolution::default();
        res.edges.push((0, 1));
        let cc = CoChange {
            pairs: vec![((0, 1), 0.5)],
            churn: vec![1, 1],
            commits_read: 1,
        };
        let g = build(&inv, &res, &cc, &[]);
        assert!((weight(&g, 0, 1) - (W_IMPORT + W_COCHANGE * 0.5)).abs() < 1e-9);
    }

    #[test]
    fn without_git_the_graph_is_unchanged() {
        let inv = inventory(&["a/one.ts", "b/two.ts"]);
        let mut res = Resolution::default();
        res.edges.push((0, 1));
        let with_empty = build(&inv, &res, &CoChange::default(), &[]);
        assert!((weight(&with_empty, 0, 1) - W_IMPORT).abs() < 1e-9);
        assert!(with_empty.cochange.is_empty());
    }

    #[test]
    fn contract_adds_a_sub_import_edge_visible_to_flows() {
        // Separate directories, so directory adjacency cannot explain it.
        let inv = inventory(&["a/one.ts", "b/two.py"]);
        let g = build(&inv, &Resolution::default(), &CoChange::default(), &[(0, 1)]);
        assert!((weight(&g, 0, 1) - W_CONTRACT).abs() < 1e-9);
        assert_eq!(g.contracts, vec![(0, 1)]);
        // Unlike co-change: the coupling is traversable, so flows, fan-in
        // and cluster coupling all see it — that is the point of the edge.
        assert!(g.out_edges[0].contains(&1));
        assert!(g.out_edges[1].contains(&0));
        assert_eq!(g.fan_in(1), 1);
    }

    #[test]
    fn contract_and_import_sum_on_the_same_pair() {
        let inv = inventory(&["a/one.ts", "b/two.py"]);
        let mut res = Resolution::default();
        res.edges.push((0, 1));
        let g = build(&inv, &res, &CoChange::default(), &[(0, 1)]);
        assert!((weight(&g, 0, 1) - (W_IMPORT + W_CONTRACT)).abs() < 1e-9);
    }

    #[test]
    fn totals_each_undirected_edge_once() {
        let g = CodeGraph::from_edges(3, &[(0, 1, 1.0), (1, 2, 0.5)]);
        assert!((g.total_weight() - 1.5).abs() < 1e-9);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn degree_sums_incident_weights() {
        let g = CodeGraph::from_edges(3, &[(0, 1, 1.0), (0, 2, 0.25)]);
        assert!((g.degree(0) - 1.25).abs() < 1e-9);
        assert!((g.degree(1) - 1.0).abs() < 1e-9);
    }
}
