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
use crate::parse::{DJANGO_INCLUDE_MARKER, FileParse};
use crate::profile::{PathMappings, PathScope, WorkspacePackage};
use crate::types::FileId;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Extension preference when several files share an extensionless path.
/// `py` rides last: it only decides same-stem ties, and TS behavior is
/// exactly what it was before M4 added it here (it must be listed at all so
/// `strip_extension` stems `.py` files for the index).
const EXT_PRECEDENCE: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "py"];

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
    pub asset_refs: usize,
    pub excluded_refs: usize,
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

            // `pkg/__init__.py` also answers to `pkg`: `from . import x` and
            // `import pkg` both target the package itself. Worst rank, so a
            // same-stem module file always wins the key.
            if let Some(dir) = stemmed.strip_suffix("/__init__") {
                insert_ranked(
                    &mut by_key,
                    &mut rank,
                    dir.to_string(),
                    f.id,
                    EXT_PRECEDENCE.len(),
                );
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

pub fn resolve_all(
    inv: &Inventory,
    parsed: &[FileParse],
    mappings: &PathMappings,
    package_dirs: &[String],
) -> Resolution {
    let index = FileIndex::build(inv);
    let mut out = Resolution {
        file_externals: vec![Vec::new(); inv.len()],
        ..Default::default()
    };
    let mut seen: HashSet<(FileId, FileId)> = HashSet::new();
    let mut internal_attempts = 0usize;
    let mut internal_hits = 0usize;
    let py_roots = python_roots(inv, package_dirs);
    let py_local = python_packages(inv);

    for p in parsed {
        let from = p.file;
        let from_dir = inv.get(from).dir().to_string();
        // TS and Python resolution are disjoint halves: dotted specs never
        // reach the bare-specifier probe and vice versa.
        let is_py = inv.get(from).language.is_python();

        for r in &p.refs {
            let spec = r.specifier.as_str();

            if is_py {
                resolve_py(
                    spec,
                    from,
                    &from_dir,
                    &py_roots,
                    &py_local,
                    &index,
                    &mut out,
                    &mut seen,
                    &mut internal_attempts,
                    &mut internal_hits,
                );
                continue;
            }

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
                if let Some(to) = resolve_bare(spec, &from_dir, mappings, &index) {
                    internal_attempts += 1;
                    internal_hits += 1;
                    if to != from && seen.insert((from, to)) {
                        out.edges.push((from, to));
                    }
                    continue;
                }
            }

            // A stylesheet or data file is not code the inventory holds: it
            // can never resolve, and a miss says nothing about the resolver.
            if spec.starts_with('.') && is_asset(spec) {
                out.asset_refs += 1;
                continue;
            }

            if !spec.starts_with('.') && mappings.local_packages.contains(&package_name(spec)) {
                internal_attempts += 1;
                out.unresolved.push((from, spec.to_string()));
                continue;
            }

            if is_external_specifier(spec) || is_unaliased_scoped(spec, mappings) {
                let pkg = package_name(spec);
                if from < out.file_externals.len() && !out.file_externals[from].contains(&pkg) {
                    out.file_externals[from].push(pkg.clone());
                }
                *out.externals.entry(pkg).or_insert(0) += 1;
                continue;
            }

            match resolve_one(spec, &from_dir, mappings, &index) {
                Some(to) => {
                    internal_attempts += 1;
                    internal_hits += 1;
                    if to != from && seen.insert((from, to)) {
                        out.edges.push((from, to));
                    }
                }
                None if spec.starts_with('.')
                    && on_disk(&inv.root, &join_relative(&from_dir, spec)) =>
                {
                    out.excluded_refs += 1;
                }
                None => {
                    internal_attempts += 1;
                    out.unresolved.push((from, spec.to_string()));
                }
            }
        }

        // `from . import views`: the module ref above reached the package;
        // a name that is itself a module file earns its own edge.
        for m in &p.py_members {
            let spec = m.specifier.as_str();
            let to = if spec.starts_with('.') {
                resolve_py_relative(spec, &from_dir, &index)
            } else {
                resolve_dotted(spec, &py_roots, &index)
            };
            if let Some(to) = to {
                push_edge(&mut out, &mut seen, from, to);
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

    resolve_bare(spec, from_dir, mappings, index)
}

/// Alias table and baseUrl lookup for a non-relative specifier. Called twice
/// for bare specs — once as an internal-attempt probe before the external
/// heuristic, once inside `resolve_one` — so it must stay side-effect free.
fn resolve_bare(
    spec: &str,
    from_dir: &str,
    mappings: &PathMappings,
    index: &FileIndex,
) -> Option<FileId> {
    let chosen = nearest_scope(&mappings.scopes, from_dir).filter(|s| !s.dir.is_empty());
    if let Some(scope) = chosen {
        if let Some(id) = paths_hit(&scope.paths, spec, index, &[]) {
            return Some(id);
        }
    }
    let shadowed: Vec<&str> = chosen
        .map(|s| s.paths.iter().map(|(p, _)| p.as_str()).collect())
        .unwrap_or_default();
    if let Some(id) = paths_hit(&mappings.paths, spec, index, &shadowed) {
        return Some(id);
    }

    let local_base = chosen.and_then(|s| s.base_url.as_deref());
    if let Some(base) = local_base {
        if let Some(id) = base_hit(base, spec, index) {
            return Some(id);
        }
    }

    if let Some(base) = &mappings.base_url {
        if Some(base.as_str()) != local_base {
            if let Some(id) = base_hit(base, spec, index) {
                return Some(id);
            }
        }
    }

    // Workspace packages each carry their own baseUrl (slice 2). Tried in
    // order after the root's; the profile keeps them sorted and deduplicated.
    for base in &mappings.extra_base_urls {
        if Some(base.as_str()) == local_base {
            continue;
        }
        if let Some(id) = base_hit(base, spec, index) {
            return Some(id);
        }
    }

    resolve_workspace(spec, &mappings.workspace_packages, index)
}

fn nearest_scope<'a>(scopes: &'a [PathScope], from_dir: &str) -> Option<&'a PathScope> {
    let mut best: Option<&PathScope> = None;
    for scope in scopes {
        if !scope_covers(&scope.dir, from_dir) {
            continue;
        }
        if best.is_none_or(|cur| scope.dir.len() > cur.dir.len()) {
            best = Some(scope);
        }
    }
    best
}

fn scope_covers(dir: &str, from_dir: &str) -> bool {
    dir.is_empty()
        || from_dir == dir
        || from_dir.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))
}

fn paths_hit(
    paths: &[(String, Vec<String>)],
    spec: &str,
    index: &FileIndex,
    skip: &[&str],
) -> Option<FileId> {
    for (pattern, targets) in paths {
        if skip.contains(&pattern.as_str()) {
            continue;
        }
        let Some(tail) = match_alias(pattern, spec) else {
            continue;
        };
        for target in targets {
            let candidate = target.replace('*', &tail);
            if let Some(id) = lookup(&candidate, index) {
                return Some(id);
            }
        }
    }
    None
}

fn base_hit(base: &str, spec: &str, index: &FileIndex) -> Option<FileId> {
    let candidate = if base == "." || base.is_empty() {
        spec.to_string()
    } else {
        format!("{base}/{spec}")
    };
    lookup(&candidate, index)
}

/// `shared/x` or `@acme/ui` against the monorepo's own packages. The bare
/// name tries the manifest entries, then `index`, then `src/index`; a
/// subpath tries the package dir, then its `src/`. Longest name wins, so
/// `@acme/ui-kit` never answers for `@acme/ui`.
fn resolve_workspace(spec: &str, pkgs: &[WorkspacePackage], index: &FileIndex) -> Option<FileId> {
    let pkg = pkgs
        .iter()
        .filter(|p| {
            spec.strip_prefix(p.name.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .max_by_key(|p| p.name.len())?;
    let rest = spec[pkg.name.len()..].trim_start_matches('/');
    if rest.is_empty() {
        pkg.entries
            .iter()
            .find_map(|e| lookup(e, index))
            .or_else(|| lookup(&pkg.dir, index))
            .or_else(|| lookup(&format!("{}/src/index", pkg.dir), index))
    } else {
        lookup(&format!("{}/{rest}", pkg.dir), index)
            .or_else(|| lookup(&format!("{}/src/{rest}", pkg.dir), index))
    }
}

/// Relative imports of files the inventory never holds.
const ASSET_EXTS: &[&str] = &[
    "css", "scss", "sass", "less", "styl", "json", "json5", "svg", "png", "jpg", "jpeg", "gif",
    "webp", "avif", "ico", "woff", "woff2", "ttf", "otf", "html", "md", "mdx", "txt", "yaml",
    "yml", "graphql", "gql", "wasm",
];

fn is_asset(spec: &str) -> bool {
    let file = spec.rsplit('/').next().unwrap_or(spec);
    let file = file.split(['?', '#']).next().unwrap_or(file);
    file.rsplit_once('.')
        .is_some_and(|(_, ext)| ASSET_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// `@angular/core`: a scoped npm package, unless a tsconfig alias claims the
/// specifier. `@/x` has no scope, so it stays with the alias table and a
/// miss there is still the resolver's.
fn is_unaliased_scoped(spec: &str, mappings: &PathMappings) -> bool {
    let Some(rest) = spec.strip_prefix('@') else {
        return false;
    };
    let Some((scope, name)) = rest.split_once('/') else {
        return false;
    };
    let name = name.split('/').next().unwrap_or(name);
    looks_like_package(scope)
        && looks_like_package(name)
        && !mappings.paths.iter().any(|(p, _)| match_alias(p, spec).is_some())
}

/// Import roots for absolute dotted specs: always the project root, plus
/// `src/` when a src-layout package is detected (`src/*/__init__.py`
/// exists). A stray `src/` dir with no packages adds a harmless extra root:
/// lookups miss and fall through to external.
/// Slice 3: workspace package dirs join the roots when they hold Python —
/// in a monorepo the package dir, not the repository root, is the import
/// anchor, and without it same-package absolute imports file as third-party.
/// TS package dirs never qualify (no `.py` underneath); extra roots only add
/// miss-and-fall-through lookups, and a repo file still beats the package
/// heuristic under the accepted-collision rule.
fn python_roots(inv: &Inventory, package_dirs: &[String]) -> Vec<String> {
    let mut roots = vec![String::new()];
    if inv
        .files
        .iter()
        .any(|f| f.rel.starts_with("src/") && f.rel.ends_with("/__init__.py"))
    {
        roots.push("src".to_string());
    }
    for pkg in package_dirs {
        let prefix = format!("{pkg}/");
        if inv.files.iter().any(|f| f.rel.starts_with(&prefix) && f.rel.ends_with(".py"))
            && !roots.contains(pkg)
        {
            roots.push(pkg.clone());
        }
    }
    for top in top_python_packages(inv) {
        let parent = top.rsplit_once('/').map_or("", |(p, _)| p).to_string();
        if !roots.contains(&parent) {
            roots.push(parent);
        }
    }
    roots
}

/// One Python ref, dispatched by shape. Attempt counting is asymmetric on
/// purpose: an absolute miss goes external without touching the rate unless
/// its top-level name is a package in the repository, while a relative miss
/// is always the tool's failure and counts. `__future__` is external by
/// definition.
#[allow(clippy::too_many_arguments)]
fn resolve_py(
    spec: &str,
    from: FileId,
    from_dir: &str,
    roots: &[String],
    local: &HashSet<String>,
    index: &FileIndex,
    out: &mut Resolution,
    seen: &mut HashSet<(FileId, FileId)>,
    attempts: &mut usize,
    hits: &mut usize,
) {
    if let Some(dotted) = spec.strip_prefix(DJANGO_INCLUDE_MARKER) {
        // URL wiring names an in-project module; a miss is a root gap, not a
        // package, so it stays unresolved and visible.
        *attempts += 1;
        match resolve_dotted(dotted, roots, index) {
            Some(to) => {
                *hits += 1;
                push_edge(out, seen, from, to);
            }
            None => out.unresolved.push((from, spec.to_string())),
        }
        return;
    }

    if spec == "__future__" {
        // A compiler directive, not a dependency: neither an edge nor an
        // external. Recording it would put `__future__` in the Stack section
        // next to real packages.
        return;
    }

    if spec.starts_with('.') {
        *attempts += 1;
        match resolve_py_relative(spec, from_dir, index) {
            Some(to) => {
                *hits += 1;
                push_edge(out, seen, from, to);
            }
            None => out.unresolved.push((from, spec.to_string())),
        }
        return;
    }

    match resolve_dotted(spec, roots, index) {
        Some(to) => {
            *attempts += 1;
            *hits += 1;
            push_edge(out, seen, from, to);
        }
        None if local.contains(spec.split('.').next().unwrap_or(spec)) => {
            *attempts += 1;
            out.unresolved.push((from, spec.to_string()));
        }
        None => push_external(out, from, spec.split('.').next().unwrap_or(spec)),
    }
}

fn python_packages(inv: &Inventory) -> HashSet<String> {
    top_python_packages(inv)
        .into_iter()
        .map(|d| d.rsplit('/').next().unwrap_or(d).to_string())
        .collect()
}

fn top_python_packages(inv: &Inventory) -> Vec<&str> {
    let inits: HashSet<&str> = inv
        .files
        .iter()
        .filter_map(|f| f.rel.strip_suffix("/__init__.py"))
        .collect();
    let mut tops: Vec<&str> = inits
        .iter()
        .copied()
        .filter(|d| d.rsplit_once('/').is_none_or(|(parent, _)| !inits.contains(parent)))
        .collect();
    tops.sort_unstable();
    tops
}

fn on_disk(root: &std::path::Path, path: &str) -> bool {
    std::iter::once(path)
        .chain([".js", ".jsx", ".mjs", ".cjs"].iter().filter_map(|e| path.strip_suffix(e)))
        .any(|base| {
            root.join(base).is_file()
                || EXT_PRECEDENCE.iter().any(|e| {
                    root.join(format!("{base}.{e}")).is_file()
                        || root.join(format!("{base}/index.{e}")).is_file()
                })
        })
}

fn push_edge(out: &mut Resolution, seen: &mut HashSet<(FileId, FileId)>, from: FileId, to: FileId) {
    if to != from && seen.insert((from, to)) {
        out.edges.push((from, to));
    }
}

fn push_external(out: &mut Resolution, from: FileId, pkg: &str) {
    let pkg = pkg.to_string();
    if from < out.file_externals.len() && !out.file_externals[from].contains(&pkg) {
        out.file_externals[from].push(pkg.clone());
    }
    *out.externals.entry(pkg).or_insert(0) += 1;
}

/// `a.b.c` → `<root>/a/b/c` through the stemmed/dir keys (`a/b/c.py`,
/// `a/b/c/__init__.py`). First root wins; inside a root the index's
/// extension ranking breaks same-stem ties.
fn resolve_dotted(dotted: &str, roots: &[String], index: &FileIndex) -> Option<FileId> {
    if dotted.is_empty() || dotted.starts_with('.') {
        return None;
    }
    let path = dotted.replace('.', "/");
    for root in roots {
        let candidate = if root.is_empty() {
            path.clone()
        } else {
            format!("{root}/{path}")
        };
        if let Some(id) = lookup(&candidate, index) {
            return Some(id);
        }
    }
    None
}

/// `.x` / `..pkg` / `.` against the importer's directory, walked up one
/// level per extra dot. No `__init__.py` requirement (namespace packages
/// resolve by path existence); walking past the root is unresolved.
fn resolve_py_relative(spec: &str, from_dir: &str, index: &FileIndex) -> Option<FileId> {
    let dots = spec.len() - spec.trim_start_matches('.').len();
    if dots == 0 {
        return None;
    }
    let mut segs: Vec<&str> = if from_dir.is_empty() {
        Vec::new()
    } else {
        from_dir.split('/').collect()
    };
    for _ in 1..dots {
        segs.pop()?;
    }
    let rest = &spec[dots..];
    let candidate = if rest.is_empty() {
        segs.join("/")
    } else {
        let mut all = segs;
        all.extend(rest.split('.'));
        all.join("/")
    };
    if candidate.is_empty() {
        return None;
    }
    lookup(&candidate, index)
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
    if ["node:", "bun:", "npm:", "jsr:"].iter().any(|p| spec.starts_with(p)) {
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
    let spec = ["node:", "npm:", "jsr:"]
        .iter()
        .find_map(|p| spec.strip_prefix(p))
        .unwrap_or(spec);
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
                    language: Language::from_path(std::path::Path::new(rel))
                        .unwrap_or(Language::Ts),
                    bytes: 100,
                    loc: 10,
                    class: FileClass::Source,
                })
                .collect(),
            skipped: Vec::new(),
            manifests: Vec::new(),
            excluded: crate::inventory::ExcludeStats {
                generated: 0,
                config: 0,
                declarations: 0,
                too_large: 0,
                unreadable: 0,
                non_supported: 0,
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
            ..Default::default()
        }
    }

    fn base_url_mappings() -> PathMappings {
        PathMappings {
            base_url: Some(".".into()),
            paths: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn baseurl_bare_import_beats_the_package_heuristic() {
        // `components/grid` is package-shaped (lowercase head) and used to be
        // filed as external before the alias/baseUrl table was consulted.
        let inv = inventory(&["app/search/page.tsx", "components/grid/index.tsx"]);
        let res = resolve_all(&inv, &[parsed(0, &["components/grid"])], &base_url_mappings(), &[]);
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
        assert!((res.resolution_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn missing_bare_import_is_still_external_not_unresolved() {
        let inv = inventory(&["app/search/page.tsx"]);
        let res = resolve_all(&inv, &[parsed(0, &["react"])], &base_url_mappings(), &[]);
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
            &[],
        );
        assert_eq!(res.unresolved.len(), 1);
    }

    #[test]
    fn scoped_first_party_package_is_not_external() {
        let inv = inventory(&["apps/web/a.ts"]);
        let m = PathMappings {
            local_packages: ["@acme/ui".to_string()].into(),
            ..Default::default()
        };
        let res = resolve_all(&inv, &[parsed(0, &["@acme/ui/button", "@other/pkg"])], &m, &[]);
        assert_eq!(res.unresolved, vec![(0, "@acme/ui/button".to_string())]);
        assert!(!res.externals.contains_key("@acme/ui"));
        assert_eq!(res.externals.get("@other/pkg"), Some(&1));
        assert!(res.resolution_rate.abs() < 1e-9);
    }

    #[test]
    fn assets_and_excluded_targets_are_counted_apart_from_misses() {
        let dir = std::env::temp_dir().join(format!("codearch-excluded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/catalog.ts"), "// @generated\n").unwrap();
        let mut inv = inventory(&["src/a.ts"]);
        inv.root = dir.clone();
        let specs = ["./catalog.ts", "./a.module.css", "./missing"];
        let res = resolve_all(&inv, &[parsed(0, &specs)], &PathMappings::default(), &[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((res.excluded_refs, res.asset_refs), (1, 1));
        assert_eq!(res.unresolved, vec![(0, "./missing".to_string())]);
        assert!(res.resolution_rate.abs() < 1e-9);
    }

    #[test]
    fn alias_import_of_a_generated_file_is_unresolved_relative_is_excluded() {
        let dir = std::env::temp_dir().join(format!(
            "codearch-gen-alias-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/app.ts"),
            "import { A } from \"@gen/catalog\";\nimport { B } from \"./catalog\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/catalog.ts"),
            "// @generated\nexport const A = 1;\n",
        )
        .unwrap();
        let inv = crate::inventory::collect(&dir).unwrap();
        assert_eq!(inv.excluded.generated, 1);
        assert_eq!(inv.files.len(), 1);
        assert_eq!(inv.files[0].rel, "src/app.ts");
        assert_eq!(inv.skipped.len(), 1);
        assert_eq!(inv.skipped[0].rel, "src/catalog.ts");
        assert_eq!(inv.skipped[0].class, crate::types::FileClass::Generated);
        let src = std::fs::read_to_string(&inv.files[0].abs).unwrap();
        let mut parsers = crate::parse::Parsers::new();
        let parsed = crate::parse::parse_one(
            &mut parsers,
            inv.files[0].id,
            inv.files[0].language,
            &src,
        );
        let mappings = PathMappings {
            paths: vec![("@gen/*".into(), vec!["src/*".into()])],
            ..Default::default()
        };
        let res = resolve_all(&inv, &[parsed], &mappings, &[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(res.unresolved, vec![(0, "@gen/catalog".to_string())]);
        assert_eq!(res.excluded_refs, 1);
        assert!(res.edges.is_empty());
    }

    #[test]
    fn python_first_party_miss_is_unresolved_not_external() {
        let inv = inventory(&["sdk/src/harness/__init__.py", "app.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(1, &["harness.errors", "requests"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.unresolved, vec![(1, "harness.errors".to_string())]);
        assert_eq!(res.externals.get("requests"), Some(&1));
    }

    #[test]
    fn python_source_root_below_the_repo_root_resolves() {
        let inv = inventory(&[
            "python/sdk/src/harness/__init__.py",
            "python/sdk/src/harness/errors.py",
            "examples/run.py",
        ]);
        let res = resolve_all(
            &inv,
            &[parsed(2, &["harness.errors", "harness"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(2, 0), (2, 1)]);
        assert!(res.unresolved.is_empty());
        assert!((res.resolution_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn extra_base_url_resolves_package_bare_imports() {
        let inv = inventory(&["packages/web/src/a.ts", "packages/web/src/b.ts"]);
        let m = PathMappings {
            base_url: None,
            paths: Vec::new(),
            extra_base_urls: vec!["packages/web".into()],
            ..Default::default()
        };
        let res = resolve_all(&inv, &[parsed(0, &["src/b"])], &m, &[]);
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
    }

    #[test]
    fn rebased_alias_target_resolves_across_packages() {
        let inv = inventory(&["packages/web/src/index.ts", "packages/shared/util.ts"]);
        let m = PathMappings {
            base_url: None,
            paths: vec![("@shared/*".into(), vec!["packages/shared/*".into()])],
            ..Default::default()
        };
        let res = resolve_all(&inv, &[parsed(0, &["@shared/util"])], &m, &[]);
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
    }

    #[test]
    fn nearest_tsconfig_paths_resolve_inside_each_app() {
        let dir = std::env::temp_dir().join(format!(
            "codearch-nearest-paths-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, content: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        write(
            "package.json",
            r#"{"name":"mono","private":true,"workspaces":["apps/*"]}"#,
        );
        write(
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["lib/*"]}}}"#,
        );
        write(
            "apps/web/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        );
        write(
            "apps/api/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        );
        write("apps/web/src/page.ts", "import { x } from \"@/util\";\n");
        write("apps/api/src/main.ts", "import { x } from \"@/util\";\n");
        write("apps/web/src/util.ts", "export const x = 1;\n");
        write("apps/api/src/util.ts", "export const x = 1;\n");
        write("src/root.ts", "import { x } from \"@/util\";\n");
        write("lib/util.ts", "export const x = 1;\n");
        let inv = crate::inventory::collect(&dir).unwrap();
        let profile = crate::profile::detect(&inv);
        let mut parsers = crate::parse::Parsers::new();
        let parsed: Vec<_> = inv
            .files
            .iter()
            .map(|f| {
                let src = std::fs::read_to_string(&f.abs).unwrap();
                crate::parse::parse_one(&mut parsers, f.id, f.language, &src)
            })
            .collect();
        let res = resolve_all(&inv, &parsed, &profile.mappings, &profile.package_dirs);
        let _ = std::fs::remove_dir_all(&dir);
        let id = |rel: &str| inv.files.iter().find(|f| f.rel == rel).unwrap().id;
        let has = |from: &str, to: &str| res.edges.contains(&(id(from), id(to)));
        assert!(has("apps/web/src/page.ts", "apps/web/src/util.ts"));
        assert!(!has("apps/web/src/page.ts", "apps/api/src/util.ts"));
        assert!(has("apps/api/src/main.ts", "apps/api/src/util.ts"));
        assert!(!has("apps/api/src/main.ts", "apps/web/src/util.ts"));
        assert!(has("src/root.ts", "lib/util.ts"));
        assert!(!has("src/root.ts", "apps/web/src/util.ts"));
        assert!(!has("src/root.ts", "apps/api/src/util.ts"));
        let paths_of = |dir_name: &str| {
            profile
                .mappings
                .scopes
                .iter()
                .find(|s| s.dir == dir_name)
                .unwrap()
                .paths
                .clone()
        };
        assert_eq!(
            paths_of(""),
            vec![("@/*".to_string(), vec!["lib/*".to_string()])]
        );
        assert_eq!(
            paths_of("apps/web"),
            vec![("@/*".to_string(), vec!["apps/web/src/*".to_string()])]
        );
        assert_eq!(
            paths_of("apps/api"),
            vec![("@/*".to_string(), vec!["apps/api/src/*".to_string()])]
        );
    }

    #[test]
    fn relative_specs_bypass_the_probe_unchanged() {
        let inv = inventory(&["src/a.ts", "src/b.ts"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["./b"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(0, 1)]);
    }

    #[test]
    fn python_relative_import_resolves_within_package() {
        let inv = inventory(&["pkg/__init__.py", "pkg/models.py", "pkg/views.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(2, &[".models"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(2, 1)]);
        assert!((res.resolution_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn python_bare_from_dot_import_targets_the_package_init() {
        let inv = inventory(&["pkg/__init__.py", "pkg/views.py"]);
        let res = resolve_all(&inv, &[parsed(1, &["."])], &PathMappings::default(), &[]);
        assert_eq!(res.edges, vec![(1, 0)]);
    }

    #[test]
    fn python_parent_level_walks_up() {
        let inv = inventory(&["pkg/__init__.py", "pkg/sub/views.py", "pkg/shared.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(1, &["..shared"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(1, 2)]);
    }

    #[test]
    fn python_absolute_internal_beats_third_party_shape() {
        // `flask_sqlalchemy.model` is lowercase-dotted like a deep package
        // import; the repo file wins over the external reading.
        let inv = inventory(&["pkg/__init__.py", "pkg/mod.py", "pkg/other.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(2, &["pkg.mod"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(2, 1)]);
    }

    #[test]
    fn python_src_layout_roots() {
        let inv = inventory(&["src/pkg/__init__.py", "src/pkg/mod.py", "src/pkg/main.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(2, &["pkg.mod"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(2, 1)]);
    }

    #[test]
    fn python_namespace_package_needs_no_init() {
        let inv = inventory(&["pkg/mod.py", "pkg/other.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(1, &[".mod"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(1, 0)]);
    }

    #[test]
    fn python_stdlib_is_external_and_future_is_neither() {
        let inv = inventory(&["pkg/mod.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["os", "sqlalchemy.orm", "__future__"])],
            &PathMappings::default(),
            &[],
        );
        assert!(res.edges.is_empty());
        assert!(res.unresolved.is_empty());
        assert_eq!(res.externals.get("os"), Some(&1));
        // Dotted externals file under their head segment.
        assert_eq!(res.externals.get("sqlalchemy"), Some(&1));
        // A compiler directive, not a dependency: recorded nowhere.
        assert!(!res.externals.contains_key("__future__"));
    }

    #[test]
    fn python_relative_miss_stays_unresolved() {
        let inv = inventory(&["pkg/mod.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &[".missing"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.unresolved.len(), 1);
        assert!(res.resolution_rate < 1.0);
    }

    #[test]
    fn python_django_include_marker_resolves() {
        let inv = inventory(&["proj/urls.py", "app/urls.py", "app/views.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["django-include:app.urls"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.edges, vec![(0, 1)]);
    }

    #[test]
    fn python_django_include_miss_stays_unresolved() {
        let inv = inventory(&["proj/urls.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["django-include:blog.urls"])],
            &PathMappings::default(),
            &[],
        );
        assert_eq!(res.unresolved.len(), 1);
    }

    #[test]
    fn python_package_dir_is_an_import_root() {
        // Monorepo slice 3: `models` from `packages/api/app.py` is an
        // absolute same-package import, not the third-party `models`.
        let inv = inventory(&["packages/api/app.py", "packages/api/models.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["models"])],
            &PathMappings::default(),
            &["packages/api".to_string()],
        );
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
        assert!(res.externals.is_empty());
    }

    fn workspace(name: &str, dir: &str, entries: &[&str]) -> WorkspacePackage {
        WorkspacePackage {
            name: name.into(),
            dir: dir.into(),
            entries: entries.iter().map(|e| (*e).into()).collect(),
        }
    }

    #[test]
    fn workspace_package_names_resolve_to_their_dirs() {
        let inv = inventory(&[
            "packages/app/src/a.ts",
            "packages/shared/index.js",
            "packages/shared/src/flags.ts",
            "packages/ui/src/index.ts",
            "packages/ui-kit/src/index.ts",
        ]);
        let m = PathMappings {
            workspace_packages: vec![
                workspace("shared", "packages/shared", &["packages/shared/dist/main.js"]),
                workspace("@acme/ui", "packages/ui", &[]),
                workspace("@acme/ui-kit", "packages/ui-kit", &[]),
            ],
            ..Default::default()
        };
        let specs = ["shared", "shared/src/flags", "shared/flags", "@acme/ui", "@acme/ui-kit", "react"];
        let res = resolve_all(&inv, &[parsed(0, &specs)], &m, &[]);
        // Missing built entry falls back to index; subpaths try the dir, then src/.
        assert_eq!(res.edges, vec![(0, 1), (0, 2), (0, 3), (0, 4)]);
        assert_eq!(res.externals.get("react"), Some(&1));
        assert!(!res.externals.contains_key("shared"));
        assert!(res.unresolved.is_empty());
    }

    #[test]
    fn scoped_packages_are_external_unless_an_alias_claims_them() {
        let inv = inventory(&["src/a.ts"]);
        let m = PathMappings {
            paths: vec![("@app/*".into(), vec!["src/*".into()]), ("@/*".into(), vec!["src/*".into()])],
            ..Default::default()
        };
        let specs = ["@angular/core", "@angular/common/http", "npm:@medley/router", "jsr:@std/assert", "@app/missing", "@/missing"];
        let res = resolve_all(&inv, &[parsed(0, &specs)], &m, &[]);
        for pkg in ["@angular/core", "@angular/common", "@medley/router", "@std/assert"] {
            assert_eq!(res.externals.get(pkg), Some(&1), "{pkg}");
        }
        // Alias misses are still the resolver's to report.
        let missed: Vec<&str> = res.unresolved.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(missed, vec!["@app/missing", "@/missing"]);
    }

    #[test]
    fn relative_asset_imports_neither_resolve_nor_count() {
        let inv = inventory(&["app/layout.tsx", "app/page.tsx"]);
        let specs = ["./globals.css", "../data/keys.json", "./logo.svg?url", "./page"];
        let res = resolve_all(&inv, &[parsed(0, &specs)], &PathMappings::default(), &[]);
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
        assert!((res.resolution_rate - 1.0).abs() < 1e-9);
    }

    fn parsed_py(file: usize, src: &str) -> FileParse {
        let mut p = crate::parse::Parsers::new();
        crate::parse::parse_one(&mut p, file, crate::types::Language::Python, src)
    }

    #[test]
    fn python_from_import_reaches_submodules() {
        let inv = inventory(&[
            "app/__init__.py",
            "app/models.py",
            "app/sub/__init__.py",
            "app/urls.py",
            "app/views.py",
        ]);
        let src = "from . import views, sub as s\nfrom app import models, helper\nfrom .views import home\nfrom . import *\n";
        let res = resolve_all(&inv, &[parsed_py(3, src)], &PathMappings::default(), &[]);
        // Package inits stay (the old edge), and each submodule joins; a
        // plain attribute (`helper`, `home`) adds nothing and costs nothing.
        assert_eq!(res.edges, vec![(3, 0), (3, 1), (3, 2), (3, 4)]);
        assert!(res.unresolved.is_empty());
        assert!((res.resolution_rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn type_checking_import_resolves_to_an_edge() {
        let inv = inventory(&["pkg/app.py", "pkg/models.py"]);
        let src = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from .models import User\n";
        let parsed = parsed_py(0, src);
        assert_eq!(
            parsed
                .refs
                .iter()
                .map(|r| r.specifier.as_str())
                .collect::<Vec<_>>(),
            vec!["typing", ".models"]
        );
        let res = resolve_all(&inv, &[parsed], &PathMappings::default(), &[]);
        assert_eq!(res.edges, vec![(0, 1)]);
        assert!(res.unresolved.is_empty());
    }

    #[test]
    fn python_package_root_ignored_without_package_python() {
        // A TS-only package dir adds no root: `models` still files external.
        let inv = inventory(&["packages/api/app.py", "packages/api/models.py"]);
        let res = resolve_all(
            &inv,
            &[parsed(0, &["models"])],
            &PathMappings::default(),
            &["packages/web".to_string()],
        );
        assert!(res.edges.is_empty());
        assert_eq!(res.externals.get("models"), Some(&1));
    }
}
