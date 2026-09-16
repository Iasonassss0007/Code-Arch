//! Stage 6 — Cluster.
//!
//! Job    Partition the graph into subsystems.
//! In     CodeGraph
//! Out    Partition { membership, count }
//! Fails  Graph too sparse to cluster -> caller falls back to directory
//!        structure via `directory_partition`.
//!
//! What this implements, precisely: multilevel modularity optimization by
//! local moving, refinement, and aggregation (Leiden), plus the guarantee
//! that made the architecture pick Leiden over Louvain in the first place —
//! no community is ever internally disconnected, enforced at every level.
//!
//! The refinement phase follows Traag–Waltman–van Eck §3.2 in structure:
//! after the local-moving pass settles communities, each community is
//! re-partitioned from singletons with merges restricted to stay inside the
//! parent community, and aggregation runs on the refined partition so a
//! badly-connected group separates one level sooner. Two deliberate
//! deviations: the merge choice is greedy-best rather than
//! randomness-proportional (determinism first — a run on unchanged input
//! reproduces byte for byte), and refined parts pass through the same
//! connectivity split as coarse ones.
//!
//! Node visit order is shuffled with a seeded PRNG rather than a system one,
//! so a run on unchanged input reproduces byte for byte.

use crate::graph::CodeGraph;
use crate::inventory::Inventory;
use std::collections::VecDeque;

/// Clusters smaller than this are folded into their strongest neighbour.
pub const MIN_CLUSTER_SIZE: usize = 3;

/// Bound on the adaptive resolution search, so the loop always terminates.
#[allow(dead_code)]
const MAX_RESOLUTION_ROUNDS: usize = 5;

#[derive(Debug, Clone)]
pub struct Partition {
    pub membership: Vec<usize>,
    pub count: usize,
}

impl Partition {
    pub fn members(&self) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.count];
        for (node, &c) in self.membership.iter().enumerate() {
            if c < self.count {
                out[c].push(node);
            }
        }
        out
    }

    pub fn sizes(&self) -> Vec<usize> {
        let mut out = vec![0; self.count];
        for &c in &self.membership {
            if c < self.count {
                out[c] += 1;
            }
        }
        out
    }
}

/// Working graph for one level of the hierarchy. Aggregated levels carry
/// self-loops representing edges internal to a merged community.
struct WGraph {
    n: usize,
    adj: Vec<Vec<(usize, f64)>>,
    self_loops: Vec<f64>,
}

impl WGraph {
    fn from_code_graph(g: &CodeGraph) -> WGraph {
        WGraph {
            n: g.n,
            adj: g.adj.clone(),
            self_loops: vec![0.0; g.n],
        }
    }

    fn degree(&self, i: usize) -> f64 {
        self.adj[i].iter().map(|(_, w)| w).sum::<f64>() + 2.0 * self.self_loops[i]
    }

    fn total(&self) -> f64 {
        let off: f64 = self
            .adj
            .iter()
            .flat_map(|r| r.iter().map(|(_, w)| *w))
            .sum::<f64>()
            / 2.0;
        off + self.self_loops.iter().sum::<f64>()
    }
}

/// xorshift64*, so ordering is reproducible without pulling in a rand crate.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn shuffle(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

/// Cluster at a fixed resolution.
pub fn partition(g: &CodeGraph, gamma: f64, seed: u64) -> Partition {
    if g.n == 0 {
        return Partition {
            membership: Vec::new(),
            count: 0,
        };
    }

    let mut wg = WGraph::from_code_graph(g);
    let mut node_comm: Vec<usize> = (0..g.n).collect();
    let mut rng = Rng::new(seed);

    loop {
        let mut comm: Vec<usize> = (0..wg.n).collect();
        let improved = local_move(&wg, &mut comm, gamma, &mut rng);
        let mut comm = split_disconnected(&wg, &comm);
        compact(&mut comm);

        // Refinement: sub-partition each community from singletons, merging
        // only within the parent community. Aggregation runs on the refined
        // partition, not the coarse one.
        let mut refined = refine_partition(&wg, &comm, gamma, &mut rng);
        let k = compact(&mut refined);

        for c in node_comm.iter_mut() {
            *c = refined[*c];
        }

        if !improved || k == wg.n || k <= 1 {
            return Partition {
                membership: node_comm,
                count: k.max(1),
            };
        }

        wg = aggregate(&wg, &refined, k);
    }
}

/// Resolutions tried when searching for a legible partition. Fixed and ordered
/// so the search is bounded and reproducible.
const GAMMA_LADDER: &[f64] = &[0.5, 0.8, 1.0, 1.4, 2.0, 2.8, 4.0];

/// No single domain may hold more than this share of the repository. A map
/// whose first domain is 72% of the files has not decomposed anything.
const MAX_DOMAIN_SHARE: f64 = 0.40;

/// Cluster at whichever resolution yields a legible map.
///
/// Two failure modes bracket this search, and optimizing against only one of
/// them produces the other: too many domains is a listing rather than a map,
/// and too few means one domain swallows the repository. So the search scans a
/// fixed resolution ladder and picks the partition that fits the domain cap
/// while leaving the largest domain smallest.
pub fn partition_targeting(g: &CodeGraph, target_max: usize, seed: u64) -> Partition {
    let mut candidates: Vec<Partition> = Vec::with_capacity(GAMMA_LADDER.len());
    for &gamma in GAMMA_LADDER {
        candidates.push(merge_small(g, partition(g, gamma, seed), MIN_CLUSTER_SIZE));
    }

    let share = |p: &Partition| -> f64 {
        let total: usize = p.membership.len().max(1);
        p.sizes().into_iter().max().unwrap_or(0) as f64 / total as f64
    };

    // Prefer a partition that fits the cap and spreads the files.
    let fitting: Vec<&Partition> = candidates
        .iter()
        .filter(|p| p.count <= target_max && p.count > 1)
        .collect();

    if let Some(best) = fitting
        .iter()
        .filter(|p| share(p) <= MAX_DOMAIN_SHARE)
        .min_by(|a, b| {
            share(a)
                .partial_cmp(&share(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        return (*best).clone();
    }

    // Nothing satisfied the share limit; take the flattest partition that fits.
    if let Some(best) = fitting.iter().min_by(|a, b| {
        share(a)
            .partial_cmp(&share(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        return (*best).clone();
    }

    // Nothing fit the cap at any resolution, so enforce it directly.
    let flattest = candidates
        .into_iter()
        .min_by_key(|p| p.count)
        .unwrap_or_else(|| partition(g, 1.0, seed));
    merge_until_at_most(g, flattest, target_max)
}

/// Fold the smallest clusters into their strongest neighbour until the count
/// fits. Returns early when the remaining small clusters have no neighbour to
/// merge into — reporting an over-cap count is better than inventing a link.
pub fn merge_until_at_most(g: &CodeGraph, p: Partition, max: usize) -> Partition {
    let mut current = p;
    while current.count > max {
        let sizes = current.sizes();
        let Some(smallest) = (0..current.count)
            .filter(|&c| sizes[c] > 0)
            .min_by_key(|&c| (sizes[c], c))
        else {
            break;
        };

        let mut pull = vec![0.0f64; current.count];
        for node in 0..g.n {
            if current.membership[node] != smallest {
                continue;
            }
            for &(j, w) in &g.adj[node] {
                let cj = current.membership[j];
                if cj != smallest {
                    pull[cj] += w;
                }
            }
        }

        let target = pull
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .filter(|(_, w)| **w > 0.0)
            .map(|(c, _)| c);

        let Some(target) = target else { break };

        let mut membership = current.membership;
        for m in membership.iter_mut() {
            if *m == smallest {
                *m = target;
            }
        }
        let count = compact(&mut membership);
        current = Partition { membership, count };
    }
    current
}

/// Fold clusters below `min_size` into the neighbour they are most connected to.
pub fn merge_small(g: &CodeGraph, p: Partition, min_size: usize) -> Partition {
    if p.count <= 1 {
        return p;
    }
    let mut membership = p.membership;
    let mut count = p.count;

    // Bounded: each round strictly reduces the number of small clusters, and
    // the cap stops a pathological graph from cycling.
    for _ in 0..10 {
        let mut sizes = vec![0usize; count];
        for &c in &membership {
            sizes[c] += 1;
        }
        let small: Vec<usize> = (0..count)
            .filter(|&c| sizes[c] > 0 && sizes[c] < min_size)
            .collect();
        if small.is_empty() {
            break;
        }

        let mut moved = false;
        for &c in &small {
            if sizes[c] == 0 {
                continue; // already absorbed this round
            }
            // Total edge weight from this cluster to each other cluster.
            let mut pull = vec![0.0f64; count];
            for node in 0..g.n {
                if membership[node] != c {
                    continue;
                }
                for &(j, w) in &g.adj[node] {
                    let cj = membership[j];
                    if cj != c {
                        pull[cj] += w;
                    }
                }
            }
            let best = pull
                .iter()
                .enumerate()
                .filter(|(other, _)| sizes[*other] > 0)
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .filter(|(_, w)| **w > 0.0)
                .map(|(other, _)| other);

            if let Some(target) = best {
                for m in membership.iter_mut() {
                    if *m == c {
                        *m = target;
                    }
                }
                sizes[target] += sizes[c];
                sizes[c] = 0;
                moved = true;
            }
        }
        if !moved {
            break; // isolated small clusters have nowhere to go
        }
        count = compact(&mut membership);
    }

    Partition { membership, count }
}

/// Stage 6's declared failure path: group by directory when the graph is too
/// sparse to say anything. Every cluster is then a directory, which is at
/// least honest about where the information came from.
pub fn directory_partition(inv: &Inventory) -> Partition {
    let mut keys: Vec<String> = inv.files.iter().map(|f| f.dir().to_string()).collect();
    keys.sort();
    keys.dedup();

    let membership: Vec<usize> = inv
        .files
        .iter()
        .map(|f| {
            keys.binary_search_by(|k| k.as_str().cmp(f.dir()))
                .unwrap_or(0)
        })
        .collect();

    Partition {
        count: keys.len().max(1),
        membership,
    }
}

fn local_move(g: &WGraph, comm: &mut [usize], gamma: f64, rng: &mut Rng) -> bool {
    let two_m = 2.0 * g.total();
    if two_m <= 0.0 {
        return false;
    }

    let mut tot = vec![0.0f64; g.n];
    for i in 0..g.n {
        tot[comm[i]] += g.degree(i);
    }

    let mut order: Vec<usize> = (0..g.n).collect();
    rng.shuffle(&mut order);
    let mut queue: VecDeque<usize> = order.into_iter().collect();
    let mut queued = vec![true; g.n];

    // Scratch buffer instead of a HashMap: iteration order must be stable.
    let mut wbuf = vec![0.0f64; g.n];
    let mut touched: Vec<usize> = Vec::new();
    let mut improved = false;

    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        let ci = comm[i];
        let ki = g.degree(i);
        tot[ci] -= ki;

        touched.clear();
        touched.push(ci);
        for &(j, w) in &g.adj[i] {
            if j == i {
                continue;
            }
            let cj = comm[j];
            if wbuf[cj] == 0.0 && cj != ci {
                touched.push(cj);
            }
            wbuf[cj] += w;
        }

        let mut best = ci;
        let mut best_gain = wbuf[ci] - gamma * ki * tot[ci] / two_m;
        for &c in &touched {
            let gain = wbuf[c] - gamma * ki * tot[c] / two_m;
            if gain > best_gain + 1e-12 {
                best_gain = gain;
                best = c;
            }
        }

        for &c in &touched {
            wbuf[c] = 0.0;
        }

        tot[best] += ki;
        if best != ci {
            comm[i] = best;
            improved = true;
            for &(j, _) in &g.adj[i] {
                if comm[j] != best && !queued[j] {
                    queued[j] = true;
                    queue.push_back(j);
                }
            }
        }
    }

    improved
}

/// Leiden's refinement phase (greedy deterministic variant): re-partition
/// each community of `comm` starting from singletons, admitting only merges
/// whose target lies inside the same parent community. A move is taken only
/// when it strictly improves the resolution-weighted modularity gain, so the
/// refined partition never scores worse than the singleton start and can only
/// separate groups the coarse pass lumped together — never join across
/// communities. The result is compacted and connectivity-split by the caller.
fn refine_partition(g: &WGraph, comm: &[usize], gamma: f64, rng: &mut Rng) -> Vec<usize> {
    let mut refined: Vec<usize> = (0..g.n).collect();
    let two_m = 2.0 * g.total();
    if two_m <= 0.0 {
        return refined;
    }

    let mut tot = vec![0.0f64; g.n];
    for i in 0..g.n {
        tot[refined[i]] += g.degree(i);
    }

    let mut order: Vec<usize> = (0..g.n).collect();
    rng.shuffle(&mut order);

    // Scratch buffer instead of a HashMap: iteration order must be stable.
    let mut wbuf = vec![0.0f64; g.n];
    let mut touched: Vec<usize> = Vec::new();

    for &i in &order {
        let parent = comm[i];
        let ki = g.degree(i);
        let cur = refined[i];
        tot[cur] -= ki;

        touched.clear();
        touched.push(cur);
        for &(j, w) in &g.adj[i] {
            if j == i || comm[j] != parent {
                continue;
            }
            let cj = refined[j];
            if wbuf[cj] == 0.0 && cj != cur {
                touched.push(cj);
            }
            wbuf[cj] += w;
        }

        let mut best = cur;
        let mut best_gain = wbuf[cur] - gamma * ki * tot[cur] / two_m;
        for &c in &touched {
            let gain = wbuf[c] - gamma * ki * tot[c] / two_m;
            if gain > best_gain + 1e-12 {
                best_gain = gain;
                best = c;
            }
        }

        for &c in &touched {
            wbuf[c] = 0.0;
        }

        tot[best] += ki;
        refined[i] = best;
    }

    split_disconnected(g, &refined)
}

/// Leiden's guarantee: a community whose induced subgraph is disconnected is
/// split into its connected components. Without this, a "subsystem" can be two
/// unrelated groups that merely improved a global score.
fn split_disconnected(g: &WGraph, comm: &[usize]) -> Vec<usize> {
    let mut out = vec![usize::MAX; g.n];
    let mut next = 0usize;

    for start in 0..g.n {
        if out[start] != usize::MAX {
            continue;
        }
        let c = comm[start];
        let id = next;
        next += 1;
        let mut stack = vec![start];
        out[start] = id;
        while let Some(node) = stack.pop() {
            for &(j, _) in &g.adj[node] {
                if comm[j] == c && out[j] == usize::MAX {
                    out[j] = id;
                    stack.push(j);
                }
            }
        }
    }

    out
}

fn aggregate(g: &WGraph, comm: &[usize], k: usize) -> WGraph {
    let mut rows: Vec<Vec<(usize, f64)>> = vec![Vec::new(); k];
    let mut self_loops = vec![0.0f64; k];
    let mut acc: Vec<f64> = vec![0.0; k];

    for i in 0..g.n {
        self_loops[comm[i]] += g.self_loops[i];
    }

    for i in 0..g.n {
        let ci = comm[i];
        let mut touched: Vec<usize> = Vec::new();
        for &(j, w) in &g.adj[i] {
            let cj = comm[j];
            if cj == ci {
                // The pair is visited from both endpoints, so halve.
                self_loops[ci] += w / 2.0;
            } else {
                if acc[cj] == 0.0 {
                    touched.push(cj);
                }
                acc[cj] += w;
            }
        }
        for &cj in &touched {
            rows[ci].push((cj, acc[cj]));
            acc[cj] = 0.0;
        }
    }

    // Merge duplicate entries produced by the per-node accumulation.
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); k];
    for (ci, row) in rows.into_iter().enumerate() {
        let mut sorted = row;
        sorted.sort_by_key(|(j, _)| *j);
        let mut sums: Vec<(usize, f64)> = Vec::new();
        for (j, w) in sorted {
            match sums.last_mut() {
                Some((lj, lw)) if *lj == j => *lw += w,
                _ => sums.push((j, w)),
            }
        }
        adj[ci] = sums;
    }

    WGraph {
        n: k,
        adj,
        self_loops,
    }
}

/// Renumber community ids to a dense `0..k` range, preserving first-seen order.
fn compact(comm: &mut [usize]) -> usize {
    let max = comm.iter().copied().max().unwrap_or(0);
    let mut map: Vec<usize> = vec![usize::MAX; max + 1];
    let mut next = 0usize;
    for c in comm.iter_mut() {
        if map[*c] == usize::MAX {
            map[*c] = next;
            next += 1;
        }
        *c = map[*c];
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_cliques() -> CodeGraph {
        // 0-1-2 fully connected, 3-4-5 fully connected, one weak bridge.
        CodeGraph::from_edges(
            6,
            &[
                (0, 1, 1.0),
                (0, 2, 1.0),
                (1, 2, 1.0),
                (3, 4, 1.0),
                (3, 5, 1.0),
                (4, 5, 1.0),
                (2, 3, 0.05),
            ],
        )
    }

    #[test]
    fn separates_two_cliques() {
        let p = partition(&two_cliques(), 1.0, 42);
        assert_eq!(p.count, 2);
        assert_eq!(p.membership[0], p.membership[1]);
        assert_eq!(p.membership[0], p.membership[2]);
        assert_eq!(p.membership[3], p.membership[4]);
        assert_ne!(p.membership[0], p.membership[3]);
    }

    #[test]
    fn is_deterministic_across_runs() {
        let g = two_cliques();
        let a = partition(&g, 1.0, 7);
        let b = partition(&g, 1.0, 7);
        assert_eq!(a.membership, b.membership);
    }

    #[test]
    fn never_leaves_a_community_internally_disconnected() {
        let g = CodeGraph::from_edges(4, &[(0, 1, 1.0), (2, 3, 1.0)]);
        // Force a partition that puts two disconnected pairs together.
        let bad = vec![0, 0, 0, 0];
        let wg = WGraph::from_code_graph(&g);
        let fixed = split_disconnected(&wg, &bad);
        assert_ne!(fixed[0], fixed[2]);
        assert_eq!(fixed[0], fixed[1]);
        assert_eq!(fixed[2], fixed[3]);
    }

    #[test]
    fn disconnected_components_never_merge() {
        let g =
            CodeGraph::from_edges(6, &[(0, 1, 1.0), (1, 2, 1.0), (3, 4, 1.0), (4, 5, 1.0)]);
        let p = partition(&g, 1.0, 3);
        assert_ne!(p.membership[0], p.membership[3]);
    }

    #[test]
    fn merges_clusters_below_minimum_size() {
        // 0..3 a dense block, 4 hanging off it as its own cluster.
        let g = CodeGraph::from_edges(
            5,
            &[
                (0, 1, 1.0),
                (0, 2, 1.0),
                (1, 2, 1.0),
                (2, 3, 1.0),
                (0, 3, 1.0),
                (3, 4, 0.4),
            ],
        );
        let forced = Partition {
            membership: vec![0, 0, 0, 0, 1],
            count: 2,
        };
        let merged = merge_small(&g, forced, 3);
        assert_eq!(merged.count, 1);
        assert_eq!(merged.membership[4], merged.membership[0]);
    }

    #[test]
    fn avoids_one_domain_swallowing_the_repository() {
        // Three dense blocks joined by thin bridges. At a low resolution these
        // collapse into one community; the search must reject that.
        let mut edges = Vec::new();
        for block in 0..3 {
            let base = block * 6;
            for i in 0..6 {
                for j in (i + 1)..6 {
                    edges.push((base + i, base + j, 1.0));
                }
            }
        }
        edges.push((5, 6, 0.05));
        edges.push((11, 12, 0.05));
        let g = CodeGraph::from_edges(18, &edges);

        let p = partition_targeting(&g, 12, 42);
        let largest = p.sizes().into_iter().max().unwrap_or(0);
        assert!(p.count >= 3, "expected the blocks to stay separate");
        assert!(
            largest as f64 / 18.0 <= MAX_DOMAIN_SHARE,
            "largest domain held {largest}/18 files"
        );
    }

    #[test]
    fn enforces_a_hard_domain_cap() {
        // A chain of five nodes clusters into more groups than we allow.
        let g = CodeGraph::from_edges(
            5,
            &[(0, 1, 1.0), (1, 2, 0.2), (2, 3, 0.2), (3, 4, 1.0)],
        );
        let p = Partition {
            membership: vec![0, 1, 2, 3, 4],
            count: 5,
        };
        let capped = merge_until_at_most(&g, p, 2);
        assert!(capped.count <= 2, "count was {}", capped.count);
    }

    #[test]
    fn cap_stops_rather_than_inventing_a_link() {
        // Three isolated nodes: no edges means no defensible merge.
        let g = CodeGraph::from_edges(3, &[]);
        let p = Partition {
            membership: vec![0, 1, 2],
            count: 3,
        };
        let capped = merge_until_at_most(&g, p, 1);
        assert_eq!(capped.count, 3);
    }

    #[test]
    fn refinement_stays_within_parent_communities() {
        // Three dense blocks; the coarse pass is forced to lump the first
        // two, refinement may split them but must never leak into the third.
        let mut edges = Vec::new();
        for block in 0..3 {
            let base = block * 5;
            for i in 0..5 {
                for j in (i + 1)..5 {
                    edges.push((base + i, base + j, 1.0));
                }
            }
        }
        edges.push((4, 5, 0.1));
        let g = CodeGraph::from_edges(15, &edges);
        let wg = WGraph::from_code_graph(&g);
        let coarse = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1];
        let mut rng = Rng::new(11);
        let mut refined = refine_partition(&wg, &coarse, 1.0, &mut rng);
        let k = compact(&mut refined);
        assert!(k >= 2, "refinement split nothing");
        // Every refined part is a subset of one coarse community.
        for part in 0..k {
            let parents: Vec<usize> = (0..15)
                .filter(|&i| refined[i] == part)
                .map(|i| coarse[i])
                .collect();
            assert!(
                parents.iter().all(|&p| p == parents[0]),
                "refined part {part} spans parent communities"
            );
        }
    }

    #[test]
    fn refinement_is_deterministic() {
        let g = two_cliques();
        let wg = WGraph::from_code_graph(&g);
        let coarse = vec![0, 0, 0, 0, 0, 0];
        let mut a = refine_partition(&wg, &coarse, 1.0, &mut Rng::new(7));
        let mut b = refine_partition(&wg, &coarse, 1.0, &mut Rng::new(7));
        compact(&mut a);
        compact(&mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn refined_parts_are_internally_connected() {
        // A barbell: two triangles joined by one edge, lumped together.
        let g = CodeGraph::from_edges(
            6,
            &[
                (0, 1, 1.0),
                (0, 2, 1.0),
                (1, 2, 1.0),
                (3, 4, 1.0),
                (3, 5, 1.0),
                (4, 5, 1.0),
                (2, 3, 0.05),
            ],
        );
        let wg = WGraph::from_code_graph(&g);
        let coarse = vec![0, 0, 0, 0, 0, 0];
        let refined = refine_partition(&wg, &coarse, 1.0, &mut Rng::new(3));
        // Each refined part must be connected in the induced subgraph.
        let mut parts: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, &c) in refined.iter().enumerate() {
            parts.entry(c).or_default().push(i);
        }
        for members in parts.values() {
            let mut seen = vec![members[0]];
            let mut stack = vec![members[0]];
            while let Some(n) = stack.pop() {
                for &(j, _) in &wg.adj[n] {
                    if members.contains(&j) && !seen.contains(&j) {
                        seen.push(j);
                        stack.push(j);
                    }
                }
            }
            assert_eq!(seen.len(), members.len(), "refined part is disconnected");
        }
    }

    #[test]
    fn compacts_sparse_ids() {
        let mut c = vec![5, 5, 9, 2];
        let k = compact(&mut c);
        assert_eq!(k, 3);
        assert_eq!(c, vec![0, 0, 1, 2]);
    }

    #[test]
    fn handles_empty_graph() {
        let g = CodeGraph::from_edges(0, &[]);
        let p = partition(&g, 1.0, 1);
        assert_eq!(p.count, 0);
    }
}
