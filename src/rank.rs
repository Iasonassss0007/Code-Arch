//! Stage 7 — Rank.
//!
//! Job    Score importance so the token budget spends itself on what matters.
//! In     CodeGraph, RouteHints
//! Out    importance score per file
//! Fails  Cannot fail.
//!
//! An agent asking "where do I look" is asking about high-fan-in,
//! entry-point-adjacent files. Line count is deliberately not a term: a
//! 2,000-line generated constants file is not important.
//!
//! The churn term is 0.20 of the score and requires git history, which arrives
//! at M2. Until then it contributes zero for every file, so it cancels out of
//! the ordering rather than distorting it.

use crate::graph::CodeGraph;
use crate::types::{FileId, RouteHint};
use std::collections::HashSet;

pub const W_PAGERANK: f64 = 0.35;
pub const W_ENTRY_POINT: f64 = 0.25;
pub const W_CHURN: f64 = 0.20;
pub const W_FAN_IN: f64 = 0.20;

const DAMPING: f64 = 0.85;
const ITERATIONS: usize = 40;

pub fn score(g: &CodeGraph, routes: &[RouteHint]) -> Vec<f64> {
    if g.n == 0 {
        return Vec::new();
    }

    let pr = normalize(&pagerank(g));
    let entries: HashSet<FileId> = routes.iter().map(|r| r.file).collect();
    let fan_in: Vec<f64> = (0..g.n).map(|i| g.fan_in(i) as f64).collect();
    let fan_in = normalize(&fan_in);

    (0..g.n)
        .map(|i| {
            let entry = if entries.contains(&i) { 1.0 } else { 0.0 };
            W_PAGERANK * pr[i]
                + W_ENTRY_POINT * entry
                + W_CHURN * 0.0 // git signals land at M2
                + W_FAN_IN * fan_in[i]
        })
        .collect()
}

/// PageRank over the directed import graph. A file imported by many important
/// files is itself important, which is exactly the property we want.
pub fn pagerank(g: &CodeGraph) -> Vec<f64> {
    let n = g.n;
    if n == 0 {
        return Vec::new();
    }
    let base = 1.0 / n as f64;
    let mut rank = vec![base; n];
    let mut next = vec![0.0; n];

    for _ in 0..ITERATIONS {
        // Rank held by nodes with no outgoing edges is redistributed evenly,
        // otherwise it leaks out of the system each iteration.
        let dangling: f64 = (0..n)
            .filter(|&i| g.out_edges[i].is_empty())
            .map(|i| rank[i])
            .sum();

        let leak = (1.0 - DAMPING) / n as f64 + DAMPING * dangling / n as f64;
        for slot in next.iter_mut() {
            *slot = leak;
        }

        for i in 0..n {
            let outs = &g.out_edges[i];
            if outs.is_empty() {
                continue;
            }
            let share = DAMPING * rank[i] / outs.len() as f64;
            for &j in outs {
                next[j] += share;
            }
        }

        std::mem::swap(&mut rank, &mut next);
    }

    rank
}

fn normalize(v: &[f64]) -> Vec<f64> {
    let max = v.iter().cloned().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|x| x / max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagerank_sums_to_one() {
        let g = CodeGraph::from_edges(4, &[(0, 1, 1.0), (1, 2, 1.0), (2, 0, 1.0), (3, 0, 1.0)]);
        let pr = pagerank(&g);
        let total: f64 = pr.iter().sum();
        assert!((total - 1.0).abs() < 1e-6, "sum was {total}");
    }

    #[test]
    fn widely_imported_file_outranks_a_leaf() {
        // Everyone imports 0; nothing imports 3.
        let g = CodeGraph::from_edges(4, &[(1, 0, 1.0), (2, 0, 1.0), (3, 0, 1.0)]);
        let pr = pagerank(&g);
        assert!(pr[0] > pr[1]);
        assert!(pr[0] > pr[3]);
    }

    #[test]
    fn entry_points_lift_the_score() {
        let g = CodeGraph::from_edges(2, &[(0, 1, 1.0)]);
        let without = score(&g, &[]);
        let with = score(
            &g,
            &[RouteHint {
                file: 0,
                label: "entry: index.ts".into(),
            }],
        );
        assert!(with[0] > without[0]);
    }

    #[test]
    fn empty_graph_scores_nothing() {
        let g = CodeGraph::from_edges(0, &[]);
        assert!(score(&g, &[]).is_empty());
    }
}
