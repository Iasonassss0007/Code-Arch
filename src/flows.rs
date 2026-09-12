//! Stage 8 — Flows.
//!
//! Job    Produce readable execution paths through each subsystem.
//! In     CodeGraph (import out-edges), cluster members, importance scores,
//!        entry points (RouteHints filtered to the cluster, best first)
//! Out    Vec<Flow> per cluster, Flow { entry label, ordered path, leaves }
//! Fails  No entries, untraversable entries, or a non-high band (decided by
//!        the caller) -> no flows. Never a fabricated flow.
//!
//! One chain per entry, not a closure: from the entry, greedily follow the
//! highest-importance unvisited member-internal out-edge to depth 4. Nodes
//! below the member score floor are not stepped onto (the entry itself is
//! exempt). Terminal fan-out to pure leaves collapses into a count. A path
//! of one node is not a flow and is dropped.
//!
//! This stage never sees source text, only file ids and scores, so there is
//! nothing for a model to hallucinate with and no model is involved.

use crate::graph::CodeGraph;
use crate::types::FileId;
use std::collections::HashSet;

/// Import-chain depth cap from the architecture document.
pub const MAX_DEPTH: usize = 4;

/// Flows kept per domain, best entry first.
pub const MAX_FLOWS: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    /// RouteHint label, e.g. "page /search". The human-meaningful part.
    pub entry_label: String,
    /// Ordered file ids, entry first.
    pub path: Vec<FileId>,
    /// Pure-leaf out-neighbors of the terminal node, collapsed, not listed.
    pub leaves: usize,
}

/// One flow per entry, in entry order. The caller filters bands and caps
/// entries at `MAX_FLOWS`; this function only traverses.
pub fn domain_flows(
    members: &[FileId],
    entries: &[(FileId, &str)],
    g: &CodeGraph,
    scores: &[f64],
    rels: &[String],
) -> Vec<Flow> {
    let member_set: HashSet<FileId> = members.iter().copied().collect();
    let floor = score_floor(members, scores);
    entries
        .iter()
        .filter_map(|(entry, label)| walk(*entry, label, &member_set, g, scores, rels, floor))
        .collect()
}

/// Bottom quartile of member scores. Below this a node is not stepped onto.
fn score_floor(members: &[FileId], scores: &[f64]) -> f64 {
    let mut s: Vec<f64> = members.iter().map(|&f| scores[f]).collect();
    if s.is_empty() {
        return f64::INFINITY;
    }
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s[s.len() / 4]
}

fn walk(
    entry: FileId,
    label: &str,
    members: &HashSet<FileId>,
    g: &CodeGraph,
    scores: &[f64],
    rels: &[String],
    floor: f64,
) -> Option<Flow> {
    let mut path = vec![entry];
    let mut visited: HashSet<FileId> = HashSet::from([entry]);
    let mut leaves = 0usize;

    loop {
        let cur = *path.last().unwrap();
        if path.len() - 1 >= MAX_DEPTH {
            break;
        }
        let mut cands: Vec<FileId> = g.out_edges[cur]
            .iter()
            .copied()
            .filter(|n| members.contains(n) && !visited.contains(n))
            .filter(|n| scores[*n] >= floor)
            .collect();
        // Highest importance first; path breaks ties so runs reproduce.
        cands.sort_by(|&a, &b| {
            scores[b]
                .partial_cmp(&scores[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| rels[a].cmp(&rels[b]))
        });
        if cands.is_empty() {
            break;
        }
        if cands.len() == 1 || path.len() == 1 {
            // A single candidate is a chain link, not fan-out: step onto it.
            // (At the entry this also turns a bare two-node chain into a flow
            // instead of collapsing it. Collapsing starts one step deeper,
            // against multiple terminals.)
            let n = cands[0];
            visited.insert(n);
            path.push(n);
            continue;
        }
        // Deeper stops prefer continuation: step toward nodes that themselves
        // lead somewhere unvisited, and collapse pure terminal fan-out into a
        // count instead of spending the chain on one leaf.
        let cont: Vec<FileId> = cands
            .iter()
            .copied()
            .filter(|n| {
                g.out_edges[*n]
                    .iter()
                    .any(|m| members.contains(m) && !visited.contains(m))
            })
            .collect();
        match cont.into_iter().next() {
            Some(n) => {
                visited.insert(n);
                path.push(n);
            }
            None => {
                leaves = cands.len();
                break;
            }
        }
    }

    if path.len() < 2 {
        return None;
    }

    Some(Flow {
        entry_label: label.to_string(),
        path,
        leaves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rels(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn linear_chain_renders_full_path() {
        let g = CodeGraph::from_edges(3, &[(0, 1, 1.0), (1, 2, 1.0)]);
        let flows = domain_flows(&[0, 1, 2], &[(0, "page /")], &g, &[1.0; 3], &rels(&["a", "b", "c"]));
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].path, vec![0, 1, 2]);
        assert_eq!(flows[0].leaves, 0);
    }

    #[test]
    fn depth_cap_truncates() {
        let g = CodeGraph::from_edges(6, &[(0, 1, 1.0), (1, 2, 1.0), (2, 3, 1.0), (3, 4, 1.0), (4, 5, 1.0)]);
        let flows = domain_flows(&[0, 1, 2, 3, 4, 5], &[(0, "e")], &g, &[1.0; 6], &rels(&["a","b","c","d","e","f"]));
        assert_eq!(flows[0].path, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn importance_floor_prunes() {
        // 0 -> 1 (weak) -> 2, 3: the weak bridge is not stepped onto, so the
        // entry has nowhere to go. Four members so the quartile floor bites.
        let g = CodeGraph::from_edges(4, &[(0, 1, 1.0), (1, 2, 1.0), (1, 3, 1.0)]);
        let scores = [1.0, 0.0, 1.0, 1.0];
        let flows = domain_flows(&[0, 1, 2, 3], &[(0, "e")], &g, &scores, &rels(&["a","b","c","d"]));
        assert!(flows.is_empty());
    }

    #[test]
    fn entry_is_exempt_from_the_floor() {
        // A weak entry still walks into strong neighbors; only stepped-onto
        // nodes face the floor, never the start.
        let g = CodeGraph::from_edges(4, &[(0, 1, 1.0)]);
        let scores = [0.0, 1.0, 1.0, 1.0];
        let flows = domain_flows(&[0, 1, 2, 3], &[(0, "e")], &g, &scores, &rels(&["a","b","c","d"]));
        assert_eq!(flows[0].path, vec![0, 1]);
    }

    #[test]
    fn leaves_collapse() {
        // 0 -> 1, and 1 fans out to pure leaves 2 and 3.
        let g = CodeGraph::from_edges(4, &[(0, 1, 1.0), (1, 2, 1.0), (1, 3, 1.0)]);
        let flows = domain_flows(&[0, 1, 2, 3], &[(0, "e")], &g, &[1.0; 4], &rels(&["a","b","c","d"]));
        assert_eq!(flows[0].path, vec![0, 1]);
        assert_eq!(flows[0].leaves, 2);
    }

    #[test]
    fn cycles_terminate() {
        let g = CodeGraph::from_edges(2, &[(0, 1, 1.0), (1, 0, 1.0)]);
        let flows = domain_flows(&[0, 1], &[(0, "e")], &g, &[1.0; 2], &rels(&["a","b"]));
        assert_eq!(flows[0].path, vec![0, 1]);
    }

    #[test]
    fn cross_cluster_edges_stop_the_path() {
        let g = CodeGraph::from_edges(3, &[(0, 1, 1.0), (1, 2, 1.0)]);
        let flows = domain_flows(&[0, 1], &[(0, "e")], &g, &[1.0; 3], &rels(&["a","b","c"]));
        assert_eq!(flows[0].path, vec![0, 1]);
    }

    #[test]
    fn greedy_picks_strongest_branch_and_ties_by_path() {
        let g = CodeGraph::from_edges(3, &[(0, 1, 1.0), (0, 2, 1.0)]);
        let scores = [1.0, 0.9, 0.5];
        let flows = domain_flows(&[0, 1, 2], &[(0, "e")], &g, &scores, &rels(&["a","b","c"]));
        assert_eq!(flows[0].path, vec![0, 1]);
    }

    #[test]
    fn entry_with_no_internal_edges_has_no_flow() {
        let g = CodeGraph::from_edges(2, &[]);
        let flows = domain_flows(&[0, 1], &[(0, "e")], &g, &[1.0; 2], &rels(&["a","b"]));
        assert!(flows.is_empty());
    }
}
