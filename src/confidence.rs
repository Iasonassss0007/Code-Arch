//! Confidence model.
//!
//! Job    Say how far each cluster should be trusted, per Principle 2.
//! In     Resolution, CodeGraph, Partition
//! Out    a score and a band per cluster
//! Fails  Cannot fail. A missing input drops out of the weighting instead of
//!        scoring zero — a term nobody could measure is not evidence of doubt.
//!
//! ```text
//! resolution   resolved internal refs / attempted, restricted to members
//! stability    fraction of members that never migrate across seeds
//! agreement    Jaccard of the cluster's import edges and co-change edges
//!
//! confidence = 0.45*resolution + 0.35*stability + 0.20*agreement
//! ```
//!
//! The low band is the point of the exercise. A tool that never emits it on a
//! repository it cannot read is not measuring anything.

use crate::cluster::{self, Partition};
use crate::graph::CodeGraph;
use crate::resolve::Resolution;
use crate::types::FileId;
use std::collections::{HashMap, HashSet};

pub const W_RESOLUTION: f64 = 0.45;
pub const W_STABILITY: f64 = 0.35;
pub const W_AGREEMENT: f64 = 0.20;

/// Band boundaries from the architecture document.
pub const HIGH: f64 = 0.75;
pub const MEDIUM: f64 = 0.45;

/// Alternative seeds run to measure stability. Five partitions total, which is
/// enough to separate a real seam from a tie-break artifact.
const STABILITY_SEEDS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    High,
    Medium,
    Low,
}

impl Band {
    pub fn of(score: f64) -> Band {
        if score >= HIGH {
            Band::High
        } else if score >= MEDIUM {
            Band::Medium
        } else {
            Band::Low
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Band::High => "high",
            Band::Medium => "medium",
            Band::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClusterConfidence {
    pub score: f64,
    pub resolution: f64,
    pub stability: f64,
    /// `None` when there is no co-change signal, or when the cluster has no
    /// internal edges of either kind for the two to agree about.
    pub agreement: Option<f64>,
}

impl ClusterConfidence {
    pub fn band(&self) -> Band {
        Band::of(self.score)
    }
}

/// How often each file stays with the same neighbours when the clustering is
/// re-run under a different tie-break order.
///
/// Clusters are matched between runs by maximum overlap, so a cluster that is
/// merely renumbered counts as stable and one whose members scatter does not.
/// This measures boundary cases, not correctness: a confidently wrong partition
/// is stable too.
pub fn file_stability(
    g: &CodeGraph,
    reference: &Partition,
    max_domains: usize,
    seed: u64,
) -> Vec<f64> {
    let n = reference.membership.len();
    if n == 0 || g.edge_count() == 0 {
        return vec![0.0; n];
    }

    let mut stable = vec![0usize; n];
    for k in 1..=STABILITY_SEEDS {
        let other = cluster::partition_targeting(g, max_domains, seed ^ (k as u64));
        let matched = match_clusters(reference, &other);
        for i in 0..n {
            if matched.get(&reference.membership[i]) == Some(&other.membership[i]) {
                stable[i] += 1;
            }
        }
    }

    stable
        .into_iter()
        .map(|s| s as f64 / STABILITY_SEEDS as f64)
        .collect()
}

/// Maps each reference cluster to the cluster it overlaps most in `other`.
fn match_clusters(reference: &Partition, other: &Partition) -> HashMap<usize, usize> {
    let mut overlap: HashMap<(usize, usize), usize> = HashMap::new();
    for (i, &c) in reference.membership.iter().enumerate() {
        *overlap.entry((c, other.membership[i])).or_insert(0) += 1;
    }
    let mut best: HashMap<usize, (usize, usize)> = HashMap::new();
    // Sorted so a tie between two equally overlapping clusters resolves the
    // same way on every run.
    let mut counts: Vec<((usize, usize), usize)> = overlap.into_iter().collect();
    counts.sort_by(|a, b| a.0.cmp(&b.0));
    for ((c, o), count) in counts {
        let slot = best.entry(c).or_insert((o, 0));
        if count > slot.1 {
            *slot = (o, count);
        }
    }
    best.into_iter().map(|(c, (o, _))| (c, o)).collect()
}

/// Confidence for one cluster, given the files it holds.
///
/// `has_cochange` distinguishes "the signals disagree" from "there was no
/// second signal". With no git history the agreement term is dropped and the
/// remaining two are renormalized, because scoring an unmeasurable term as
/// zero manufactures doubt out of a missing input.
pub fn for_cluster(
    files: &[FileId],
    res: &Resolution,
    g: &CodeGraph,
    stability: &[f64],
    has_cochange: bool,
) -> ClusterConfidence {
    let members: HashSet<FileId> = files.iter().copied().collect();

    let resolution = cluster_resolution(&members, res);
    let stab = if files.is_empty() {
        0.0
    } else {
        files
            .iter()
            .map(|&f| stability.get(f).copied().unwrap_or(0.0))
            .sum::<f64>()
            / files.len() as f64
    };
    let agreement = has_cochange
        .then(|| cluster_agreement(&members, g))
        .flatten();

    let score = match agreement {
        Some(a) => W_RESOLUTION * resolution + W_STABILITY * stab + W_AGREEMENT * a,
        None => {
            let total = W_RESOLUTION + W_STABILITY;
            (W_RESOLUTION * resolution + W_STABILITY * stab) / total
        }
    };

    ClusterConfidence {
        score,
        resolution,
        stability: stab,
        agreement,
    }
}

/// Resolved internal references over attempted ones, for members only. A
/// cluster that attempted none inherits the run's global rate rather than a
/// free 1.0: nothing was demonstrated either way.
fn cluster_resolution(members: &HashSet<FileId>, res: &Resolution) -> f64 {
    let resolved = res.edges.iter().filter(|(f, _)| members.contains(f)).count();
    let unresolved = res
        .unresolved
        .iter()
        .filter(|(f, _)| members.contains(f))
        .count();
    match resolved + unresolved {
        0 => res.resolution_rate,
        attempts => resolved as f64 / attempts as f64,
    }
}

/// How much of the cluster's co-change evidence the import graph confirms:
/// co-change pairs that are also import edges, over co-change pairs.
///
/// Not the Jaccard of the two sets. An import edge between two files nobody
/// edited during the window is absent evidence, not a contradiction, and
/// dividing by the union would let a quiet cluster's own imports vote against
/// it. `None` when the cluster has no co-change pairs at all — then there is
/// no second signal to agree with, and the term is dropped instead of scored.
fn cluster_agreement(members: &HashSet<FileId>, g: &CodeGraph) -> Option<f64> {
    let mut imports: HashSet<(usize, usize)> = HashSet::new();
    for &i in members {
        for &j in g.out_edges.get(i).into_iter().flatten() {
            if i != j && members.contains(&j) {
                imports.insert(if i < j { (i, j) } else { (j, i) });
            }
        }
    }
    let cochange: HashSet<(usize, usize)> = g
        .cochange
        .iter()
        .filter(|(i, j)| members.contains(i) && members.contains(j))
        .copied()
        .collect();

    if cochange.is_empty() {
        return None;
    }
    let confirmed = imports.intersection(&cochange).count();
    Some(confirmed as f64 / cochange.len() as f64)
}

/// One number for the map header, weighted by cluster size so a two-file
/// outlier cannot drag the whole repository's reading down.
pub fn global(confidences: &[ClusterConfidence], sizes: &[usize]) -> f64 {
    let total: usize = sizes.iter().sum();
    if total == 0 || confidences.is_empty() {
        return 0.0;
    }
    confidences
        .iter()
        .zip(sizes)
        .map(|(c, &n)| c.score * n as f64)
        .sum::<f64>()
        / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolution_of(edges: &[(usize, usize)], unresolved: usize) -> Resolution {
        let mut res = Resolution {
            edges: edges.to_vec(),
            resolution_rate: 0.5,
            ..Default::default()
        };
        for i in 0..unresolved {
            res.unresolved.push((0, format!("./missing{i}")));
        }
        res
    }

    #[test]
    fn bands_split_at_the_documented_boundaries() {
        assert_eq!(Band::of(0.75), Band::High);
        assert_eq!(Band::of(0.7499), Band::Medium);
        assert_eq!(Band::of(0.45), Band::Medium);
        assert_eq!(Band::of(0.4499), Band::Low);
    }

    #[test]
    fn every_term_at_full_marks_scores_one() {
        let mut g = CodeGraph::from_edges(2, &[(0, 1, 1.0)]);
        g.cochange = vec![(0, 1)];
        let res = resolution_of(&[(0, 1)], 0);
        let c = for_cluster(&[0, 1], &res, &g, &[1.0, 1.0], true);
        assert!((c.resolution - 1.0).abs() < 1e-9);
        assert!((c.stability - 1.0).abs() < 1e-9);
        assert_eq!(c.agreement, Some(1.0));
        assert!((c.score - 1.0).abs() < 1e-9);
        assert_eq!(c.band(), Band::High);
    }

    #[test]
    fn unresolved_imports_pull_a_cluster_down() {
        let g = CodeGraph::from_edges(2, &[(0, 1, 1.0)]);
        let res = resolution_of(&[(0, 1)], 3);
        let c = for_cluster(&[0, 1], &res, &g, &[1.0, 1.0], false);
        assert!((c.resolution - 0.25).abs() < 1e-9);
        assert!(c.score < 1.0);
    }

    #[test]
    fn a_cluster_that_attempted_no_imports_inherits_the_global_rate() {
        let g = CodeGraph::from_edges(2, &[]);
        let res = resolution_of(&[], 0);
        let c = for_cluster(&[0, 1], &res, &g, &[1.0, 1.0], false);
        assert!((c.resolution - 0.5).abs() < 1e-9);
    }

    #[test]
    fn disagreeing_signals_score_below_agreeing_ones() {
        let mut agree = CodeGraph::from_edges(3, &[(0, 1, 1.0), (1, 2, 1.0)]);
        agree.cochange = vec![(0, 1), (1, 2)];
        let mut disagree = CodeGraph::from_edges(3, &[(0, 1, 1.0), (1, 2, 1.0)]);
        disagree.cochange = vec![(0, 2)];
        let res = resolution_of(&[(0, 1), (1, 2)], 0);
        let hi = for_cluster(&[0, 1, 2], &res, &agree, &[1.0; 3], true);
        let lo = for_cluster(&[0, 1, 2], &res, &disagree, &[1.0; 3], true);
        assert_eq!(hi.agreement, Some(1.0));
        assert!(lo.agreement.unwrap() < 0.4);
        assert!(hi.score > lo.score);
    }

    #[test]
    fn without_git_the_remaining_terms_are_renormalized() {
        let g = CodeGraph::from_edges(2, &[(0, 1, 1.0)]);
        let res = resolution_of(&[(0, 1)], 0);
        let c = for_cluster(&[0, 1], &res, &g, &[1.0, 1.0], false);
        // Perfect resolution and perfect stability must still read as high,
        // not as 0.80 with a fifth of the score withheld for a missing input.
        assert_eq!(c.agreement, None);
        assert!((c.score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_cluster_with_no_co_change_has_nothing_to_agree_about() {
        // Imports but no co-change: the files were simply not edited inside the
        // window. That is absent evidence, and must not score as disagreement.
        let mut g = CodeGraph::from_edges(4, &[(0, 1, 1.0), (2, 3, 1.0)]);
        g.cochange = vec![(0, 1)];
        let res = resolution_of(&[(0, 1), (2, 3)], 0);
        let quiet = for_cluster(&[2, 3], &res, &g, &[1.0; 4], true);
        assert_eq!(quiet.agreement, None);
        assert!((quiet.score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn imports_nobody_edited_do_not_dilute_agreement() {
        // One co-change pair, confirmed by an import, inside a cluster with
        // many other import edges. Agreement is about the co-change evidence.
        let mut g = CodeGraph::from_edges(4, &[(0, 1, 1.0), (1, 2, 1.0), (2, 3, 1.0)]);
        g.cochange = vec![(0, 1)];
        let res = resolution_of(&[(0, 1), (1, 2), (2, 3)], 0);
        let c = for_cluster(&[0, 1, 2, 3], &res, &g, &[1.0; 4], true);
        assert_eq!(c.agreement, Some(1.0));
    }

    #[test]
    fn a_forced_split_is_perfectly_stable() {
        // Two dense triangles joined by nothing: no seed can move a file.
        let g = CodeGraph::from_edges(
            6,
            &[
                (0, 1, 1.0),
                (1, 2, 1.0),
                (0, 2, 1.0),
                (3, 4, 1.0),
                (4, 5, 1.0),
                (3, 5, 1.0),
            ],
        );
        let reference = cluster::partition_targeting(&g, 12, 0x5EED);
        let stability = file_stability(&g, &reference, 12, 0x5EED);
        assert!(stability.iter().all(|&s| s == 1.0), "{stability:?}");
    }

    #[test]
    fn an_edgeless_graph_is_not_stable_at_all() {
        let g = CodeGraph::from_edges(3, &[]);
        let reference = Partition {
            membership: vec![0, 0, 0],
            count: 1,
        };
        assert_eq!(file_stability(&g, &reference, 12, 1), vec![0.0; 3]);
    }

    #[test]
    fn global_confidence_weights_by_cluster_size() {
        let big = ClusterConfidence {
            score: 0.9,
            resolution: 1.0,
            stability: 1.0,
            agreement: None,
        };
        let small = ClusterConfidence { score: 0.1, ..big };
        let g = global(&[big, small], &[99, 1]);
        assert!(g > 0.85, "{g}");
        assert_eq!(global(&[], &[]), 0.0);
    }
}
