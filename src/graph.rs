//! Stage 5 — Graph.
//!
//! Job    Fuse every available signal into one weighted graph over files.
//! In     Resolution (import edges), Inventory (directory structure)
//! Out    CodeGraph
//! Fails  Cannot fail. With no imports at all it degrades to directory adjacency.
//!
//! M0 carries two of the four planned signals. Git co-change (0.55) arrives at
//! M2 and embedding similarity (0.35) at M3; the fusion point is here so adding
//! them touches only this file.

use crate::inventory::Inventory;
use crate::resolve::Resolution;
use std::collections::HashMap;

/// Strong, directional, ground truth.
pub const W_IMPORT: f64 = 1.00;
/// Weak prior that keeps sibling files from fragmenting when imports are sparse.
pub const W_DIRECTORY: f64 = 0.25;

/// Above this many files in one directory, the all-pairs prior would add more
/// edges than signal, so it is skipped for that directory.
const DIR_CLIQUE_MAX: usize = 30;

pub struct CodeGraph {
    pub n: usize,
    /// Undirected fused adjacency: `adj[i]` holds `(j, weight)`.
    pub adj: Vec<Vec<(usize, f64)>>,
    /// Directed import edges, for PageRank and fan-in.
    pub out_edges: Vec<Vec<usize>>,
    pub in_edges: Vec<Vec<usize>>,
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
        }
    }
}

pub fn build(inv: &Inventory, res: &Resolution) -> CodeGraph {
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
