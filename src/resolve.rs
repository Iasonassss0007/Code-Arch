//! Stage 3 — Resolve.
//!
//! Job    Turn raw specifier strings into concrete file targets.
//! In     RawRefs from stage 2, Inventory, tsconfig path mappings
//! Out    Resolution { edges, externals, unresolved, resolution_rate }
//! Fails  Unresolvable specifier -> dropped from the graph and counted. The
//!        unresolved rate is the primary confidence input for the whole run.
//!
//! This is the ecosystem-specific layer and it does not generalize. Getting it
//! right for one ecosystem is worth more than getting it approximately right
//! for five.

use crate::inventory::Inventory;
use crate::parse::FileParse;
use crate::profile::PathMappings;
use crate::types::FileId;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Extension preference when several files share an extensionless path.
const EXT_PRECEDENCE: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Node builtins that carry no `node:` prefix.
const NODE_BUILTINS: &[&str] = &[
    "assert", "buffer", "child_process", "cluster", "console", "crypto", "dgram", "dns", "events",
    "fs", "http", "http2", "https", "net", "os", "path", "perf_hooks", "process", "querystring",
    "readline", "stream", "string_decoder", "timers", "tls", "tty", "url", "util", "v8", "vm",
    "worker_threads", "zlib",
];

#[derive(Debug, Default)]
pub struct Resolution {
    /// Directed, deduplicated import edges (importer -> imported).
    pub edges: Vec<(FileId, FileId)>,
    /// External package name -> reference count.
    pub externals: BTreeMap<String, usize>,
    /// External packages referenced by each file, indexed by `FileId`.
    /// Lets a cluster cite the dependencies its own files actually use.
    pub file_externals: Vec<Vec<String>>,
    /// Specifiers that looked internal but matched no file.
    pub unresolved: Vec<(FileId, String)>,
    /// Internal specifiers resolved / internal specifiers attempted.
    pub resolution_rate: f64,
}

impl Resolution {
    pub fn top_externals(&self, n: usize) -> Vec<(&str, usize)> {
        let mut v: Vec<(&str, usize)> = self
            .externals
            .iter()
            .map(|(k, c)| (k.as_str(), *c))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.truncate(n);
        v
    }
}

/// Lookup table from path-ish key to file id.
struct FileIndex {
    by_key: HashMap<String, FileId>,
}

impl FileIndex {
    fn build(inv: &Inventory) -> FileIndex {
        let mut by_key: HashMap<String, FileId> = HashMap::new();
        let mut rank: HashMap<String, usize> = HashMap::new();

        let ext_rank = |rel: &str| -> usize {
            let ext = rel.rsplit('.').next().unwrap_or("");
            EXT_PRECEDENCE
                .iter()
                .position(|e| *e == ext)
                .unwrap_or(EXT_PRECEDENCE.len())
        };

        for f in &inv.files {
            let rel = f.rel.as_str();
            by_key.insert(rel.to_string(), f.id);

            let stemmed = strip_extension(rel);
            insert_ranked(&mut by_key, &mut rank, stemmed.clone(), f.id, ext_rank(rel));

            // `src/auth/index.ts` also answers to `src/auth`.
            if let Some(dir) = stemmed.strip_suffix("/index") {
                insert_ranked(&mut by_key, &mut rank, dir.to_string(), f.id, ext_rank(rel));
            } else if stemmed == "index" {
                insert_ranked(&mut by_key, &mut rank, String::new(), f.id, ext_rank(rel));
            }
        }

        FileIndex { by_key }
    }

    fn get(&self, key: &str) -> Option<FileId> {
        self.by_key.get(key).copied()
    }
}

fn insert_ranked(
    by_key: &mut HashMap<String, FileId>,
    rank: &mut HashMap<String, usize>,
    key: String,
    id: FileId,
    r: usize,
) {
    match rank.get(&key) {
        Some(existing) if *existing <= r => {}
        _ => {
            rank.insert(key.clone(), r);
            by_key.insert(key, id);
        }
    }
}

pub fn resolve_all(inv: &Inventory, parsed: &[FileParse], mappings: &PathMappings) -> Resolution {
    let index = FileIndex::build(inv);
    let mut out = Resolution {
        file_externals: vec![Vec::new(); inv.len()],
        ..Default::default()
    };
    let mut seen: HashSet<(FileId, FileId)> = HashSet::new();
    let mut internal_attempts = 0usize;
    let mut internal_hits = 0usize;

    for p in parsed {
        let from = p.file;
        let from_dir = inv.get(from).dir().to_string();

        for r in &p.refs {
            let spec = r.specifier.as_str();

            // Relative specifiers are always internal attempts. Bare
            // specifiers first try the alias/baseUrl table: a hit means the
            // repository itself answers to that name (the Next.js baseUrl
            // convention), and only a miss falls through to the package
            // heuristic. Without this order, `components/grid` reads as the
            // package `components` and route files lose all out-edges.
            // Accepted collision: an npm name matching a repo file resolves
            // to the repo file. node_modules is excluded from the inventory,
            // so the repo file is the saner reading.
            if !spec.starts_with('.') {
                if let Some(to) = resolve_bare(spec, mappings, &index) {
                    internal_attempts += 1;
                    internal_hits += 1;
                    if to != from && seen.insert((from, to)) {
                        out.edges.push((from, to));
                    }
                    continue;
                }
            }

            if is_external_specifier(spec) {
                let pkg = package_name(spec);
                if from < out.file_externals.len() && !out.file_externals[from].contains(&pkg) {
                    out.file_externals[from].push(pkg.clone());
                }
                *out.externals.entry(pkg).or_insert(0) += 1;
                continue;
            }

            internal_attempts += 1;
            match resolve_one(spec, &from_dir, mappings, &index) {
                Some(to) => {
                    internal_hits += 1;
                    if to != from && seen.insert((from, to)) {
                        out.edges.push((from, to));
                    }
                }
                None => out.unresolved.push((from, spec.to_string())),
            }
        }
    }

    out.resolution_rate = if internal_attempts == 0 {
        1.0
    } else {
        internal_hits as f64 / internal_attempts as f64
    };
    out.edges.sort_unstable();
    out
}

fn resolve_one(
    spec: &str,
    from_dir: &str,
    mappings: &PathMappings,
    index: &FileIndex,
) -> Option<FileId> {
    if spec.starts_with('.') {
        let joined = join_relative(from_dir, spec);
        return lookup(&joined, index);
    }

    resolve_bare(spec, mappings, index)
}

/// Alias table and baseUrl lookup for a non-relative specifier. Called twice
/// for bare specs — once as an internal-attempt probe before the external
/// heuristic, once inside `resolve_one` — so it must stay side-effect free.
fn resolve_bare(spec: &str, mappings: &PathMappings, index: &FileIndex) -> Option<FileId> {
    for (pattern, targets) in &mappings.paths {
        if let Some(tail) = match_alias(pattern, spec) {
            for target in targets {
                let candidate = target.replace('*', &tail);
                if let Some(id) = lookup(&candidate, index) {
                    return Some(id);
                }
            }
        }
    }

    if let Some(base) = &mappings.base_url {
        let candidate = if base == "." || base.is_empty() {
            spec.to_string()
        } else {
            format!("{base}/{spec}")
        };
        if let Some(id) = lookup(&candidate, index) {
            return Some(id);
        }
    }

    None
}

/// Try a path key, then the TypeScript-ESM habit of importing `./foo.js`
/// when the file on disk is `./foo.ts`.
fn lookup(path: &str, index: &FileIndex) -> Option<FileId> {
    if let Some(id) = index.get(path) {
        return Some(id);
    }
    for ext in [".js", ".jsx", ".mjs", ".cjs"] {
        if let Some(stripped) = path.strip_suffix(ext) {
            if let Some(id) = index.get(stripped) {
                return Some(id);
            }
        }
    }
    index.get(&format!("{path}/index"))
}

/// `@/*` against `@/auth/login` yields `auth/login`.
fn match_alias(pattern: &str, spec: &str) -> Option<String> {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            let rest = spec.strip_prefix(prefix)?;
            let rest = if suffix.is_empty() {
                rest
            } else {
                rest.strip_suffix(suffix)?
            };
            Some(rest.to_string())
        }
        None => {
            if pattern == spec {
                Some(String::new())
            } else {
                None
            }
        }
    }
}

fn is_external_specifier(spec: &str) -> bool {
    if spec.starts_with('.') {
        return false;
    }
    if spec.starts_with("node:") || spec.starts_with("bun:") {
        return true;
    }
    if NODE_BUILTINS.contains(&spec) {
        return true;
    }
    // Anything else non-relative may still be an alias, so stay conservative:
    // only a plain package-shaped name counts as external here.
    !spec.starts_with('@') && !spec.starts_with('~') && looks_like_package(spec)
}

/// A bare, lowercase, dash-or-dot name with no path-like leading segment.
fn looks_like_package(spec: &str) -> bool {
    let head = spec.split('/').next().unwrap_or(spec);
    !head.is_empty()
        && head.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.' || c == '_'
        })
}

fn package_name(spec: &str) -> String {
    let spec = spec.strip_prefix("node:").unwrap_or(spec);
    let parts: Vec<&str> = spec.split('/').collect();
    if spec.starts_with('@') && parts.len() >= 2 {
        format!("{}/{}", parts[0], parts[1])
    } else {
        parts[0].to_string()
    }
}

fn strip_extension(rel: &str) -> String {
    match rel.rsplit_once('.') {
        Some((head, ext)) if EXT_PRECEDENCE.contains(&ext) => head.to_string(),
        _ => rel.to_string(),
    }
}

/// Join a relative specifier onto a directory, collapsing `.` and `..`.
fn join_relative(from_dir: &str, spec: &str) -> String {
    let mut segs: Vec<&str> = if from_dir.is_empty() {
        Vec::new()
    } else {
        from_dir.split('/').collect()
    };
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            other => segs.push(other),
        }
    }
    segs.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_relative_specifiers() {
        assert_eq!(join_relative("src/auth", "./login"), "src/auth/login");
        assert_eq!(join_relative("src/auth", "../users/repo"), "src/users/repo");
        assert_eq!(join_relative("src/a/b", "../../c"), "src/c");
        assert_eq!(join_relative("", "./index"), "index");
    }

    #[test]
    fn matches_tsconfig_aliases() {
        assert_eq!(
            match_alias("@/*", "@/auth/login").as_deref(),
            Some("auth/login")
        );
        assert_eq!(match_alias("~lib/*", "~lib/db").as_deref(), Some("db"));
        assert_eq!(match_alias("@/*", "next/router"), None);
        assert_eq!(match_alias("exact", "exact").as_deref(), Some(""));
    }

    #[test]
    fn extracts_package_names() {
        assert_eq!(package_name("react"), "react");
        assert_eq!(package_name("@scope/pkg/sub"), "@scope/pkg");
        assert_eq!(package_name("node:fs/promises"), "fs");
    }

    #[test]
    fn classifies_external_specifiers() {
        assert!(is_external_specifier("react"));
        assert!(is_external_specifier("node:fs"));
        assert!(is_external_specifier("fs"));
        assert!(!is_external_specifier("./local"));
        // Aliased specifiers must reach the alias table, not be called external.
        assert!(!is_external_specifier("@/auth"));
        assert!(!is_external_specifier("~lib/db"));
    }

    #[test]
    fn strips_only_known_extensions() {
        assert_eq!(strip_extension("src/a.ts"), "src/a");
        assert_eq!(strip_extension("src/a.config"), "src/a.config");
    }

    fn inventory(paths: &[&str]) -> Inventory {
        use crate::types::{FileClass, FileRecord, Language};
        Inventory {
            root: std::path::PathBuf::from("."),
            files: paths
                .iter()
                .enumerate()
                .map(|(id, rel)| FileRecord {
                    id,
                    rel: (*rel).into(),
                    abs: std::path::PathBuf::from(rel),
                    language: Language::Ts,
                    bytes: 100,
                    loc: 10,
                    class: FileClass::Source,
                })
                .collect(),
            skipped: Vec::new(),
            excluded: crate::inventory::ExcludeStats {
                generated: 0,
                config: 0,
                declarations: 0,
                too_large: 0,
                unreadable: 0,
                non_js_ts: 0,
            },
        }
    }

    fn parsed(file: usize, specs: &[&str]) -> FileParse {
        use crate::types::RawRef;
        FileParse {
            file,
            symbols: Vec::new(),
            refs: specs
                .iter()
                .map(|s| RawRef {
                    specifier: (*s).into(),
                    line: 1,
                })
                .collect(),
            partial: false,
        }
    }

    fn base_url_mappings() -> PathMappings {
        PathMappings {
            base_url: Some(".".into()),
            paths: Vec::new(),
        }
    }

    #[test]
    fn baseurl_bare_import_beats_the_package_heuristic() {
        // `components/grid` is package-shaped (lowercase head) and used to be
        // filed as external before the alias/baseUrl table was consulted.
        let inv = inventory(&["app/search/page.tsx", "components/grid/index.tsx"]);
        let res = resolve_all(&inv, &[parsed(0, &["components/grid"])], &base_url_mappings());
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
        assert!((res.resolution_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn missing_bare_import_is_still_external_not_unresolved() {
        let inv = inventory(&["app/search/page.tsx"]);
        let res = resolve_all(&inv, &[parsed(0, &["react"])], &base_url_mappings());
        assert!(res.edges.is_empty());
        assert!(res.unresolved.is_empty());
        assert_eq!(res.externals.get("react"), Some(&1));
    }

    #[test]
    fn genuinely_internal_miss_stays_unresolved() {
        // Mixed-case heads never looked like packages; a miss is still a miss.
        let inv = inventory(&["app/search/page.tsx"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["Components/Missing"])],
            &base_url_mappings(),
        );
        assert_eq!(res.unresolved.len(), 1);
    }

    #[test]
    fn relative_specs_bypass_the_probe_unchanged() {
        let inv = inventory(&["src/a.ts", "src/b.ts"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["./b"])],
            &PathMappings::default(),
        );
        assert_eq!(res.edges, vec![(0, 1)]);
    }
}
