//! Stage 4 — Git signals.
//!
//! Job    Read commit history and emit co-change pairs and per-file churn.
//! In     Repository root, Inventory
//! Out    CoChange { pairs, churn }
//! Fails  No git binary, no `.git`, an empty or unreadable history -> empty
//!        output. The graph proceeds on imports alone and the map says so.
//!
//! Two files that keep changing in the same commit are coupled whether or not
//! either imports the other. That is the whole point: annotation-driven
//! registration, event buses and dependency injection leave the import graph
//! incomplete, and git does not care what language it is looking at.
//!
//! Time is anchored to the newest commit in the log rather than to the wall
//! clock, so the stage is a pure function of the repository. Running it twice
//! on the same checkout produces the same weights tomorrow as today.

use crate::inventory::Inventory;
use crate::types::{FileClass, FileId};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Commits read from the tip. Enough history to see coupling, little enough to
/// stay fast on a repository with a decade behind it.
const LOG_COMMITS: usize = 2_000;

/// Commits older than this contribute nothing. Architecture that changed three
/// years ago is not the architecture being mapped.
const WINDOW_DAYS: f64 = 730.0;

/// Age at which a commit's evidence is worth half of a fresh one's.
const HALF_LIFE_DAYS: f64 = 180.0;

/// A commit touching more mapped files than this is a refactor, a rename sweep
/// or a formatting pass. It says nothing about which files belong together.
const MAX_COMMIT_FILES: usize = 50;

/// A pair seen once is a coincidence.
const MIN_PAIR_COMMITS: usize = 2;

const SECONDS_PER_DAY: f64 = 86_400.0;

/// One commit as the log reports it: when, and which paths it touched.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub seconds: i64,
    pub files: Vec<String>,
}

#[derive(Debug, Default)]
pub struct CoChange {
    /// Normalized weights in `(0, 1]`, keyed by `(low, high)` file id and
    /// sorted by key. Sorted because everything downstream must reproduce.
    pub pairs: Vec<((FileId, FileId), f64)>,
    /// Commits touching each file, densely indexed by `FileId`.
    pub churn: Vec<usize>,
    /// Commits actually read, after the age window. Zero means the stage
    /// degraded and the map should say so.
    pub commits_read: usize,
}

impl CoChange {
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// Whether git history was available at all, regardless of whether any
    /// pair survived filtering.
    pub fn has_history(&self) -> bool {
        self.commits_read > 0
    }
}

/// Runs `git log` at `root` and builds the signal. Every failure path returns
/// an empty result: this stage degrades, it does not abort the run.
pub fn collect(root: &Path, inv: &Inventory) -> CoChange {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "log",
            "--no-merges",
            "--name-only",
            &format!("-n{LOG_COMMITS}"),
            "--pretty=format:%x00%ct",
        ])
        .output();

    let Ok(out) = out else {
        return CoChange::default(); // no git binary on PATH
    };
    if !out.status.success() {
        return CoChange::default(); // not a repository, or no commits yet
    }

    let text = String::from_utf8_lossy(&out.stdout);
    build(&parse_log(&text), inv)
}

/// Splits `git log --name-only` output into commits. Pure, so the test suite
/// needs neither a git binary nor a repository.
pub fn parse_log(text: &str) -> Vec<Commit> {
    let mut commits = Vec::new();
    // The NUL in the pretty format is the only unambiguous record separator:
    // commit messages are excluded, but file paths may contain anything else.
    for chunk in text.split('\0').skip(1) {
        let mut lines = chunk.lines();
        let Some(seconds) = lines.next().and_then(|l| l.trim().parse::<i64>().ok()) else {
            continue;
        };
        let files: Vec<String> = lines
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        if !files.is_empty() {
            commits.push(Commit { seconds, files });
        }
    }
    commits
}

/// Turns parsed commits into weighted pairs and churn counts.
///
/// Credit per commit is `1 / (files - 1)`, so a two-file commit is strong
/// evidence and a forty-file commit is nearly none, and it decays with age.
/// Pairs need [`MIN_PAIR_COMMITS`] separate commits to survive, and two test
/// files coupled only to each other are dropped: they have no import basis and
/// would pull a tests blob out of the real subsystems. A test and the file it
/// exercises are kept, because that agreement with the import graph is exactly
/// what the confidence model reads.
pub fn build(commits: &[Commit], inv: &Inventory) -> CoChange {
    let mut churn = vec![0usize; inv.len()];
    if commits.is_empty() {
        return CoChange {
            pairs: Vec::new(),
            churn,
            commits_read: 0,
        };
    }

    let by_path: HashMap<&str, FileId> = inv.files.iter().map(|f| (f.rel.as_str(), f.id)).collect();
    // Anchored on the last commit that touched mapped code, not the last commit
    // of any kind. An archived repository whose recent history is documentation
    // and CI would otherwise have its whole code history fall outside the
    // window, and the signal would vanish exactly where nothing has moved.
    let anchor = commits
        .iter()
        .filter(|c| c.files.iter().any(|p| by_path.contains_key(p.as_str())))
        .map(|c| c.seconds)
        .max()
        .unwrap_or(0);

    let mut totals: HashMap<(FileId, FileId), (f64, usize)> = HashMap::new();
    let mut commits_read = 0usize;

    for commit in commits {
        let age_days = (anchor - commit.seconds).max(0) as f64 / SECONDS_PER_DAY;
        if age_days > WINDOW_DAYS {
            continue;
        }
        let mut ids: Vec<FileId> = commit
            .files
            .iter()
            .filter_map(|p| by_path.get(p.as_str()).copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            continue;
        }
        commits_read += 1;
        for &id in &ids {
            churn[id] += 1;
        }
        if ids.len() < 2 || ids.len() > MAX_COMMIT_FILES {
            continue;
        }

        let decay = 0.5f64.powf(age_days / HALF_LIFE_DAYS);
        let credit = decay / (ids.len() - 1) as f64;
        for (a, &i) in ids.iter().enumerate() {
            for &j in &ids[a + 1..] {
                if inv.get(i).class == FileClass::Test && inv.get(j).class == FileClass::Test {
                    continue;
                }
                let slot = totals.entry((i, j)).or_insert((0.0, 0));
                slot.0 += credit;
                slot.1 += 1;
            }
        }
    }

    let mut pairs: Vec<((FileId, FileId), f64)> = totals
        .into_iter()
        .filter(|(_, (_, n))| *n >= MIN_PAIR_COMMITS)
        .map(|(k, (w, _))| (k, w))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    normalize(&mut pairs);

    CoChange {
        pairs,
        churn,
        commits_read,
    }
}

/// Scales weights into `(0, 1]` against the 99th-percentile pair rather than
/// the maximum: one runaway pair — a file and its generated mirror, say —
/// would otherwise flatten every real signal against it.
fn normalize(pairs: &mut [((FileId, FileId), f64)]) {
    if pairs.is_empty() {
        return;
    }
    let mut sorted: Vec<f64> = pairs.iter().map(|(_, w)| *w).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((sorted.len() as f64 * 0.99).ceil() as usize).min(sorted.len()) - 1;
    let scale = sorted[idx];
    if scale <= 0.0 {
        return;
    }
    for (_, w) in pairs.iter_mut() {
        *w = (*w / scale).min(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileRecord, Language};
    use std::path::PathBuf;

    fn inv(paths: &[(&str, FileClass)]) -> Inventory {
        let files = paths
            .iter()
            .enumerate()
            .map(|(id, (rel, class))| FileRecord {
                id,
                rel: (*rel).into(),
                abs: PathBuf::from(rel),
                language: Language::Ts,
                bytes: 100,
                loc: 10,
                class: *class,
            })
            .collect();
        Inventory {
            root: PathBuf::from("."),
            files,
            skipped: Vec::new(),
            excluded: Default::default(),
        }
    }

    fn src(paths: &[&str]) -> Inventory {
        let v: Vec<(&str, FileClass)> = paths.iter().map(|p| (*p, FileClass::Source)).collect();
        inv(&v)
    }

    fn commit(seconds: i64, files: &[&str]) -> Commit {
        Commit {
            seconds,
            files: files.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    const DAY: i64 = 86_400;

    #[test]
    fn parses_commits_and_their_paths() {
        let log = "\01700000000\nsrc/a.ts\nsrc/b.ts\n\01699000000\nsrc/c.ts\n";
        let commits = parse_log(log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].seconds, 1_700_000_000);
        assert_eq!(commits[0].files, vec!["src/a.ts", "src/b.ts"]);
        assert_eq!(commits[1].files, vec!["src/c.ts"]);
    }

    #[test]
    fn skips_commits_that_touched_no_files_and_malformed_records() {
        // A merge-free log can still contain a commit with no name-only body,
        // and a truncated read can leave a record with no timestamp.
        let commits = parse_log("\01700000000\n\0notanumber\nsrc/a.ts\n\01699000000\nsrc/b.ts\n");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].files, vec!["src/b.ts"]);
    }

    #[test]
    fn empty_log_parses_to_nothing() {
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn pairs_need_two_commits() {
        let inv = src(&["a.ts", "b.ts", "c.ts"]);
        let cc = build(
            &[
                commit(1_000 * DAY, &["a.ts", "b.ts"]),
                commit(1_001 * DAY, &["a.ts", "b.ts"]),
                commit(1_002 * DAY, &["a.ts", "c.ts"]),
            ],
            &inv,
        );
        let keys: Vec<(FileId, FileId)> = cc.pairs.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![(0, 1)]);
    }

    #[test]
    fn small_commits_outweigh_large_ones() {
        let inv = src(&["a.ts", "b.ts", "c.ts", "d.ts", "e.ts"]);
        // (a,b) twice alone; (c,d) twice inside five-file commits.
        let cc = build(
            &[
                commit(1_000 * DAY, &["a.ts", "b.ts"]),
                commit(1_000 * DAY, &["a.ts", "b.ts"]),
                commit(1_000 * DAY, &["c.ts", "d.ts", "e.ts", "a.ts", "b.ts"]),
                commit(1_000 * DAY, &["c.ts", "d.ts", "e.ts", "a.ts", "b.ts"]),
            ],
            &inv,
        );
        let w = |i: FileId, j: FileId| cc.pairs.iter().find(|(k, _)| *k == (i, j)).unwrap().1;
        assert!(w(0, 1) > w(2, 3), "tight pair {} vs {}", w(0, 1), w(2, 3));
    }

    #[test]
    fn giant_commits_contribute_no_pairs() {
        let paths: Vec<String> = (0..60).map(|i| format!("f{i}.ts")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let inv = src(&refs);
        let cc = build(
            &[commit(1_000 * DAY, &refs), commit(1_001 * DAY, &refs)],
            &inv,
        );
        assert!(cc.pairs.is_empty());
        // Churn still counts them: the files did change.
        assert_eq!(cc.churn[0], 2);
    }

    #[test]
    fn recent_coupling_outweighs_old_coupling() {
        let inv = src(&["a.ts", "b.ts", "c.ts", "d.ts"]);
        let cc = build(
            &[
                commit(1_000 * DAY, &["a.ts", "b.ts"]),
                commit(1_000 * DAY, &["a.ts", "b.ts"]),
                commit(640 * DAY, &["c.ts", "d.ts"]),
                commit(640 * DAY, &["c.ts", "d.ts"]),
            ],
            &inv,
        );
        let w = |i: FileId, j: FileId| cc.pairs.iter().find(|(k, _)| *k == (i, j)).unwrap().1;
        assert!(w(0, 1) > w(2, 3));
    }

    #[test]
    fn commits_outside_the_window_are_ignored() {
        let inv = src(&["a.ts", "b.ts", "c.ts", "d.ts"]);
        let cc = build(
            &[
                commit(2_000 * DAY, &["a.ts", "b.ts"]),
                commit(2_000 * DAY, &["a.ts", "b.ts"]),
                commit(1_000 * DAY, &["c.ts", "d.ts"]),
                commit(1_000 * DAY, &["c.ts", "d.ts"]),
            ],
            &inv,
        );
        let keys: Vec<(FileId, FileId)> = cc.pairs.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![(0, 1)]);
        assert_eq!(cc.churn[2], 0, "out-of-window commits do not count as churn");
    }

    #[test]
    fn the_window_hangs_off_the_last_commit_that_touched_code() {
        // An archived repository: the code stopped moving, then documentation
        // and CI kept committing for years. The code history must still count.
        let inv = src(&["a.ts", "b.ts"]);
        let cc = build(
            &[
                commit(3_000 * DAY, &["README.md"]),
                commit(2_900 * DAY, &[".github/workflows/ci.yml"]),
                commit(1_000 * DAY, &["a.ts", "b.ts"]),
                commit(1_001 * DAY, &["a.ts", "b.ts"]),
            ],
            &inv,
        );
        assert_eq!(cc.commits_read, 2);
        assert_eq!(cc.pairs.len(), 1);
    }

    #[test]
    fn test_to_test_pairs_are_dropped_and_source_to_test_kept() {
        let inv = inv(&[
            ("a.ts", FileClass::Source),
            ("a.test.ts", FileClass::Test),
            ("b.test.ts", FileClass::Test),
        ]);
        let cc = build(
            &[
                commit(1_000 * DAY, &["a.ts", "a.test.ts", "b.test.ts"]),
                commit(1_001 * DAY, &["a.ts", "a.test.ts", "b.test.ts"]),
            ],
            &inv,
        );
        let keys: Vec<(FileId, FileId)> = cc.pairs.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![(0, 1), (0, 2)]);
    }

    #[test]
    fn paths_outside_the_inventory_are_ignored() {
        let inv = src(&["a.ts", "b.ts"]);
        let cc = build(
            &[
                commit(1_000 * DAY, &["a.ts", "b.ts", "deno_dist/a.ts", "README.md"]),
                commit(1_001 * DAY, &["a.ts", "b.ts", "deno_dist/a.ts", "README.md"]),
            ],
            &inv,
        );
        assert_eq!(cc.pairs.len(), 1);
        assert_eq!(cc.churn.len(), 2);
    }

    #[test]
    fn weights_are_scaled_against_the_ninety_ninth_percentile() {
        let inv = src(&["a.ts", "b.ts", "c.ts", "d.ts"]);
        let mut commits = Vec::new();
        // One runaway pair, one ordinary pair.
        for i in 0..40 {
            commits.push(commit(1_000 * DAY + i, &["a.ts", "b.ts"]));
        }
        for i in 0..2 {
            commits.push(commit(1_000 * DAY + i, &["c.ts", "d.ts"]));
        }
        let cc = build(&commits, &inv);
        let w = |i: FileId, j: FileId| cc.pairs.iter().find(|(k, _)| *k == (i, j)).unwrap().1;
        assert!((w(0, 1) - 1.0).abs() < 1e-9, "runaway pair clamps to 1.0");
        assert!(w(2, 3) > 0.0 && w(2, 3) <= 1.0);
    }

    #[test]
    fn no_commits_degrades_to_empty() {
        let cc = build(&[], &src(&["a.ts"]));
        assert!(cc.is_empty());
        assert!(!cc.has_history());
        assert_eq!(cc.churn, vec![0]);
    }

    #[test]
    fn a_directory_without_git_degrades_rather_than_failing() {
        let tmp = std::env::temp_dir().join(format!("codearch-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let cc = collect(&tmp, &src(&["a.ts"]));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!cc.has_history());
        assert!(cc.is_empty());
    }
}
