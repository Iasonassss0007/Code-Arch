//! Stage 1 — Profile.
//!
//! Job    Identify ecosystem, frameworks and module resolution rules before parsing.
//! In     Inventory (for entry-point shapes) + manifests on disk
//! Out    Profile { frameworks, deps, mappings, ... }
//! Fails  No recognized manifest -> empty profile, run continues.
//!
//! This stage runs before parsing because it changes how later stages behave:
//! tsconfig path aliases decide what stage 3 can resolve, and framework
//! conventions decide what counts as an entry point.

use crate::inventory::Inventory;
use crate::types::{FileRecord, RouteHint};
use std::collections::BTreeSet;
use std::path::Path;

/// Dependency name -> display name. Order here is the order shown in the map.
const FRAMEWORKS: &[(&str, &str)] = &[
    ("next", "Next.js"),
    ("nuxt", "Nuxt"),
    ("@angular/core", "Angular"),
    ("@nestjs/core", "NestJS"),
    ("react", "React"),
    ("vue", "Vue"),
    ("svelte", "Svelte"),
    ("solid-js", "Solid"),
    ("express", "Express"),
    ("fastify", "Fastify"),
    ("koa", "Koa"),
    ("hono", "Hono"),
    ("@trpc/server", "tRPC"),
    ("graphql", "GraphQL"),
    ("@prisma/client", "Prisma"),
    ("drizzle-orm", "Drizzle"),
    ("typeorm", "TypeORM"),
    ("mongoose", "Mongoose"),
    ("kysely", "Kysely"),
    ("next-auth", "Auth.js"),
    ("@auth/core", "Auth.js"),
    ("passport", "Passport"),
    ("stripe", "Stripe"),
    ("@supabase/supabase-js", "Supabase"),
    ("firebase", "Firebase"),
    ("redis", "Redis"),
    ("ioredis", "Redis"),
    ("bullmq", "BullMQ"),
    ("socket.io", "Socket.IO"),
    ("electron", "Electron"),
    ("react-native", "React Native"),
    ("tailwindcss", "Tailwind CSS"),
    ("vite", "Vite"),
    ("webpack", "webpack"),
    ("vitest", "Vitest"),
    ("jest", "Jest"),
    ("@playwright/test", "Playwright"),
    ("zod", "Zod"),
    // Python (M4). Matched against normalized dependency names from
    // pyproject.toml and requirements files, not against imports.
    ("django", "Django"),
    ("flask", "Flask"),
    ("sqlalchemy", "SQLAlchemy"),
    ("djangorestframework", "DRF"),
];

#[derive(Debug, Default, Clone)]
pub struct PathMappings {
    /// tsconfig `compilerOptions.baseUrl`, repository-relative and slash-normalized.
    pub base_url: Option<String>,
    /// tsconfig `compilerOptions.paths`, alias pattern -> replacement patterns.
    pub paths: Vec<(String, Vec<String>)>,
    /// Package-level baseUrls rebased to repository-relative form (slice 2).
    /// Tried in order after `base_url`; first hit wins.
    pub extra_base_urls: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Profile {
    pub project_name: Option<String>,
    pub description: Option<String>,
    pub frameworks: Vec<String>,
    pub deps: BTreeSet<String>,
    pub mappings: PathMappings,
    pub has_package_json: bool,
    pub has_tsconfig: bool,
    /// Declared entry points from package.json `main`/`bin`.
    pub manifest_entries: Vec<String>,
    /// Workspace package dirs (repository-relative, sorted), from `workspaces`
    /// or `pnpm-workspace.yaml`. Empty for single-package repos: no behavior
    /// change there. Slice 3 will hang per-package resolver roots off this.
    pub package_dirs: Vec<String>,
    /// Frameworks per workspace package dir, same order as `package_dirs`.
    /// The global `frameworks` stays the union (Stack display, Django gate
    /// fallback); `route_hints` consults the scope a file lives in, so a
    /// Django package cannot label a stray `urls.py` in its neighbor.
    pub package_frameworks: Vec<(String, Vec<String>)>,
}

impl Profile {
    /// Confidence that we understood the project's conventions at all.
    pub fn confidence(&self) -> f32 {
        let mut c: f32 = 0.3;
        if self.has_package_json {
            c += 0.3;
        }
        if self.has_tsconfig {
            c += 0.2;
        }
        if !self.frameworks.is_empty() {
            c += 0.2;
        }
        c.min(1.0)
    }

    pub fn uses(&self, framework: &str) -> bool {
        self.frameworks.iter().any(|f| f == framework)
    }

    /// Longest workspace package containing `rel`, else the root: the
    /// scope-relative path plus that scope's frameworks. Empty
    /// `package_dirs` collapses to `(rel, global)` — single-package behavior
    /// is therefore unchanged by construction.
    fn scope_of<'a>(&'a self, rel: &'a str) -> (&'a str, &'a [String]) {
        let mut best: Option<(&str, &[String])> = None;
        for dir in &self.package_dirs {
            let prefix = format!("{dir}/");
            if let Some(rest) = rel.strip_prefix(&prefix) {
                let longer = best.map(|(r, _)| rest.len() < r.len()).unwrap_or(true);
                if longer {
                    let fw = self
                        .package_frameworks
                        .iter()
                        .find(|(d, _)| d == dir)
                        .map(|(_, f)| f.as_slice())
                        .unwrap_or(&[]);
                    best = Some((rest, fw));
                }
            }
        }
        best.unwrap_or((rel, &self.frameworks))
    }
}

pub fn detect(inv: &Inventory) -> Profile {
    let mut p = Profile::default();

    let root_raw = read_json(&inv.root.join("package.json"));
    if let Some(raw) = root_raw.as_ref() {
        read_package_json(raw, "", &mut p);
    }
    read_pyproject(&inv.root, &mut p);
    read_requirements(&inv.root, &mut p);
    read_tsconfig(&inv.root, "", &mut p);

    // Slice 2: workspace packages merge into the same profile, rebased to
    // repository-relative form. Root is read first so it wins every
    // first-wins rule (alias patterns, baseUrl, name/description).
    p.package_dirs = discover_packages(&inv.root, root_raw.as_ref());
    for pkg in p.package_dirs.clone() {
        let dir = inv.root.join(&pkg);
        // Snapshot-diff: the readers merge into `p.deps`, and the difference
        // is the package's own dependency set for its framework scope.
        let before = p.deps.clone();
        if let Some(raw) = read_json(&dir.join("package.json")) {
            read_package_json(&raw, &pkg, &mut p);
        }
        read_pyproject(&dir, &mut p);
        read_requirements(&dir, &mut p);
        read_tsconfig(&dir, &pkg, &mut p);
        let pkg_deps: BTreeSet<String> = p.deps.difference(&before).cloned().collect();
        p.package_frameworks.push((pkg.clone(), package_frameworks(inv, &pkg, &pkg_deps)));
    }

    // A standalone app beside a backend (`src-ui/` next to a Django `src/`)
    // is no workspace, yet its tsconfig baseUrl decides how its own imports
    // resolve: paperless-ngx's `'src/app/services/...'` specifiers were all
    // unresolved without it. Only the tsconfig is read; the directory does
    // not become a package, so package scoping is unchanged.
    for app in nested_ts_apps(&inv.root, &p.package_dirs) {
        read_tsconfig(&inv.root.join(&app), &app, &mut p);
    }

    for (dep, display) in FRAMEWORKS {
        if p.deps.contains(*dep) && !p.frameworks.iter().any(|f| f == display) {
            p.frameworks.push((*display).to_string());
        }
    }

    // The FRAMEWORKS loop above already catches a `django` dependency. These
    // catch Django projects whose manifests are absent or minimal: the
    // framework is recognizable from its entry file alone.
    if !p.frameworks.iter().any(|f| f == "Django") && is_django_project(&inv, &p.deps) {
        p.frameworks.push("Django".to_string());
    }

    p
}

/// The root `package.json` block, generalized over directories. `prefix` is
/// the package dir repository-relative, or empty for the root: manifest
/// entries rebase through it, while name/description only fill when the root
/// left them absent.
fn read_package_json(raw: &serde_json::Value, prefix: &str, p: &mut Profile) {
    p.has_package_json = true;
    if p.project_name.is_none() {
        p.project_name = raw
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if p.description.is_none() {
        p.description = raw
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(obj) = raw.get(key).and_then(|v| v.as_object()) {
            for name in obj.keys() {
                p.deps.insert(name.clone());
            }
        }
    }

    if let Some(main) = raw.get("main").and_then(|v| v.as_str()) {
        p.manifest_entries.push(rebase(prefix, main));
    }
    match raw.get("bin") {
        Some(serde_json::Value::String(s)) => p.manifest_entries.push(rebase(prefix, s)),
        Some(serde_json::Value::Object(o)) => {
            for v in o.values() {
                if let Some(s) = v.as_str() {
                    p.manifest_entries.push(rebase(prefix, s));
                }
            }
        }
        _ => {}
    }
}

/// The root tsconfig block, generalized the same way. Package baseUrls cannot
/// share the single `base_url` slot, so they append to `extra_base_urls`;
/// package alias targets rebase to repository-relative form. Duplicate alias
/// patterns keep the first registration (root before packages, packages in
/// sorted order).
fn read_tsconfig(dir: &Path, prefix: &str, p: &mut Profile) {
    // tsconfig.json is very often JSONC, so it cannot go through serde directly.
    for candidate in ["tsconfig.json", "jsconfig.json"] {
        let path = dir.join(candidate);
        if !path.exists() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text)) else {
            continue;
        };
        p.has_tsconfig = true;
        let Some(co) = v.get("compilerOptions") else {
            continue;
        };
        if let Some(b) = co.get("baseUrl").and_then(|v| v.as_str()) {
            // Root keeps its exact historical value ("." stays ".").
            let rebased = if prefix.is_empty() {
                normalize_rel(b)
            } else {
                rebase(prefix, b)
            };
            if prefix.is_empty() {
                p.mappings.base_url = Some(rebased);
            } else if Some(&rebased) != p.mappings.base_url.as_ref()
                && !p.mappings.extra_base_urls.contains(&rebased)
            {
                p.mappings.extra_base_urls.push(rebased);
            }
        }
        if let Some(paths) = co.get("paths").and_then(|v| v.as_object()) {
            for (alias, targets) in paths {
                if p.mappings.paths.iter().any(|(a, _)| a == alias) {
                    continue;
                }
                let list: Vec<String> = targets
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.as_str())
                            .map(|t| rebase(prefix, t))
                            .collect()
                    })
                    .unwrap_or_default();
                if !list.is_empty() {
                    p.mappings.paths.push((alias.clone(), list));
                }
            }
        }
        break;
    }
}

/// Join a package dir and a manifest-relative target, lexically normalizing
/// `.`/`..`. `rebase("packages/web", "../shared/*")` is
/// `packages/shared/*`; `rebase("", x)` matches `normalize_rel(x)` except
/// that `"."` maps to `""` (the resolver treats both as the root).
/// A `..` that escapes the root collapses to root-relative rather than
/// failing: the lookup that follows simply misses.
fn rebase(pkg: &str, target: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in pkg.split('/').chain(target.split('/')) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    normalize_rel(&parts.join("/"))
}

/// Workspace package dirs, repository-relative and sorted. Sources, in order:
/// root `package.json` `workspaces` (array or `{packages: [...]}`), then
/// `pnpm-workspace.yaml` `packages:` entries (line-scanned, same precedent as
/// `read_requirements`). Only `*` (single segment) globs expand; `!`
/// negations are ignored. A dir counts only if it holds at least one
/// manifest, so a stale glob cannot invent packages. Capped at 64.
fn discover_packages(root: &Path, raw: Option<&serde_json::Value>) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    if let Some(r) = raw {
        match r.get("workspaces") {
            Some(serde_json::Value::Array(a)) => {
                for v in a {
                    if let Some(s) = v.as_str() {
                        patterns.push(s.to_string());
                    }
                }
            }
            Some(serde_json::Value::Object(o)) => {
                if let Some(a) = o.get("packages").and_then(|v| v.as_array()) {
                    for v in a {
                        if let Some(s) = v.as_str() {
                            patterns.push(s.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    patterns.extend(read_pnpm_workspaces(root));

    let mut out: Vec<String> = Vec::new();
    for pat in patterns {
        if pat.starts_with('!') {
            continue;
        }
        if let Some((head, _)) = pat.split_once('*') {
            let base = root.join(head);
            let Ok(entries) = std::fs::read_dir(&base) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() && has_manifest(&path) {
                    out.push(normalize_rel(&rel_of(root, &path)));
                }
            }
        } else {
            let path = root.join(&pat);
            if path.is_dir() && has_manifest(&path) {
                out.push(normalize_rel(&pat));
            }
        }
        if out.len() >= 64 {
            break;
        }
    }
    out.sort();
    out.dedup();
    out.truncate(64);
    out
}

/// Top-level directories with their own tsconfig/jsconfig that no workspace
/// declares, sorted. Hidden and dependency directories are skipped.
fn nested_ts_apps(root: &Path, packages: &[String]) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| !name.starts_with('.') && name != "node_modules")
        .filter(|name| !packages.contains(name))
        .filter(|name| {
            ["tsconfig.json", "jsconfig.json"]
                .iter()
                .any(|f| root.join(name).join(f).is_file())
        })
        .collect();
    out.sort();
    out
}

/// Minimal `pnpm-workspace.yaml` subset: the `packages:` list. Anything else
/// in the file is ignored.
fn read_pnpm_workspaces(root: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t', '-']) {
            in_section = trimmed == "packages:";
            continue;
        }
        if in_section {
            if let Some(pat) = trimmed.strip_prefix("- ") {
                out.push(pat.trim_matches(|c| c == '"' || c == '\'').to_string());
            }
        }
    }
    out
}

/// At least one manifest the profiler can read.
fn has_manifest(dir: &Path) -> bool {
    for f in ["package.json", "pyproject.toml", "tsconfig.json", "jsconfig.json"] {
        if dir.join(f).is_file() {
            return true;
        }
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("requirements") && n.ends_with(".txt"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Repository-relative slash path for a path under root.
fn rel_of(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

/// `pyproject.toml`, parsed as the subset that matters: `[project] name`,
/// `[project] dependencies` (single- or multi-line), `[tool.poetry] name`,
/// and `[tool.poetry.dependencies]` keys. A full TOML parser for two fields
/// and a list is dependency weight without benefit; this rejects nothing
/// valid, it just ignores the rest of the file.
fn read_pyproject(root: &Path, p: &mut Profile) {
    let Ok(text) = std::fs::read_to_string(root.join("pyproject.toml")) else {
        return;
    };
    let mut section = String::new();
    let mut in_deps_array = false;
    let mut deps_buf = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            flush_py_deps(&deps_buf, &mut p.deps);
            deps_buf.clear();
            in_deps_array = false;
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        if in_deps_array {
            deps_buf.push_str(line);
            deps_buf.push('\n');
            if line.contains(']') {
                flush_py_deps(&deps_buf, &mut p.deps);
                deps_buf.clear();
                in_deps_array = false;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match (section.as_str(), key) {
            ("project" | "tool.poetry", "name") => {
                if p.project_name.is_none() {
                    if let Some(name) = quoted(value) {
                        p.project_name = Some(name);
                    }
                }
            }
            ("project", "description") => {
                if p.description.is_none() {
                    if let Some(d) = quoted(value) {
                        p.description = Some(d);
                    }
                }
            }
            ("project", "dependencies") => {
                deps_buf.push_str(value);
                deps_buf.push('\n');
                if value.contains(']') {
                    flush_py_deps(&deps_buf, &mut p.deps);
                    deps_buf.clear();
                } else {
                    in_deps_array = true;
                }
            }
            ("tool.poetry.dependencies", dep) => {
                if dep != "python" && dep != "source" {
                    p.deps.insert(normalize_py_dep(dep));
                }
            }
            _ => {}
        }
    }
    flush_py_deps(&deps_buf, &mut p.deps);
}

fn flush_py_deps(buf: &str, deps: &mut BTreeSet<String>) {
    // Collect quoted segments with their quote character, so apostrophes
    // inside double-quoted strings (and vice versa) never split a name.
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in buf.chars() {
        match quote {
            Some(qc) if c == qc => {
                deps.insert(normalize_py_dep(&cur));
                cur.clear();
                quote = None;
            }
            Some(_) => cur.push(c),
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                }
            }
        }
    }
}

fn quoted(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches(',');
    value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .map(|s| s.to_string())
}

/// PEP 508 name → lowercase distribution key: extras, version specifiers,
/// markers and whitespace stripped.
fn normalize_py_dep(spec: &str) -> String {
    let end = spec
        .find(['<', '>', '=', '!', '~', '[', ';', ' ', '\t'])
        .unwrap_or(spec.len());
    spec[..end].trim().to_ascii_lowercase()
}

/// `requirements.txt` and siblings (`requirements-dev.txt`, …) at the
/// repository root: one dependency per line, `#` comments and installer
/// options skipped. These files are in no supported language, so they never
/// reach the inventory — the root directory is scanned directly. Deeper
/// `requirements/` directories are not followed; that layout is rare and the
/// failure is a missed framework label, not a wrong map.
fn read_requirements(root: &Path, p: &mut Profile) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !(name == "requirements.txt"
            || (name.starts_with("requirements") && name.ends_with(".txt")))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('-') || line.contains("://") {
                continue;
            }
            let name = normalize_py_dep(line);
            if !name.is_empty() {
                p.deps.insert(name);
            }
        }
    }
}

/// Django is recognizable without any manifest: `manage.py` is its entry
/// file, and it declares the settings module. The `django` dependency itself
/// is handled by the FRAMEWORKS table; this covers the other two signals.
fn is_django_project(inv: &Inventory, deps: &BTreeSet<String>) -> bool {
    if deps.contains("django") {
        return true;
    }
    has_django_manage(inv.files.iter().chain(inv.skipped.iter()), None)
}

/// A `manage.py` declaring `DJANGO_SETTINGS_MODULE`, optionally scoped to one
/// package dir. Path match alone is weak (`manage.py` could be anything); the
/// settings declaration makes it Django's.
fn has_django_manage<'a>(
    files: impl Iterator<Item = &'a FileRecord>,
    scope: Option<&str>,
) -> bool {
    files
        .filter(|f| scope.map(|p| f.rel.starts_with(p)).unwrap_or(true))
        .find(|f| f.rel.rsplit('/').next().unwrap_or(&f.rel) == "manage.py")
        .map(|m| {
            std::fs::read_to_string(&m.abs)
                .map(|t| t.contains("DJANGO_SETTINGS_MODULE"))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Frameworks for one workspace package: the FRAMEWORKS table over its own
/// deps only, plus the manage.py scan scoped to its dir. A Django package
/// therefore cannot lend its framework to a neighbor's stray `urls.py`.
fn package_frameworks(inv: &Inventory, pkg: &str, deps: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for (dep, display) in FRAMEWORKS {
        if deps.contains(*dep) && !out.iter().any(|f| f == display) {
            out.push((*display).to_string());
        }
    }
    if !out.iter().any(|f| f == "Django")
        && (deps.contains("django")
            || has_django_manage(
                inv.files.iter().chain(inv.skipped.iter()),
                Some(&format!("{pkg}/")),
            ))
    {
        out.push("Django".to_string());
    }
    out
}

/// Entry points derived from framework conventions and manifest fields.
///
/// This is path-based on purpose: a Next.js route is a fact about the file's
/// location, and no parser recovers it.
pub fn route_hints(profile: &Profile, inv: &Inventory) -> Vec<RouteHint> {
    let mut hints = Vec::new();

    for f in &inv.files {
        let rel = f.rel.as_str();
        // Slice 4: framework rules consult the scope the file lives in and
        // match against the scope-relative path. A Next.js package's `app/`
        // routes fire; the root and its neighbors stay quiet. Generic and
        // manifest entries are scope-agnostic by design (a runnable module
        // is an entry in any package).
        let (scope_rel, scope_fw) = profile.scope_of(rel);
        let uses = |fw: &str| scope_fw.iter().any(|x| x == fw);
        let trimmed = scope_rel.strip_prefix("src/").unwrap_or(scope_rel);

        if uses("Next.js") {
            if let Some(rest) = trimmed.strip_prefix("app/") {
                if let Some(kind) = next_app_kind(rest) {
                    hints.push(RouteHint {
                        file: f.id,
                        label: format!("{kind} {}", next_app_route(rest)),
                    });
                    continue;
                }
            }
            if let Some(rest) = trimmed.strip_prefix("pages/") {
                if !rest.starts_with('_') {
                    let is_api = rest.starts_with("api/");
                    hints.push(RouteHint {
                        file: f.id,
                        label: format!(
                            "{} {}",
                            if is_api { "API" } else { "page" },
                            pages_route(rest)
                        ),
                    });
                    continue;
                }
            }
        }

        if is_generic_entry(trimmed) {
            hints.push(RouteHint {
                file: f.id,
                label: format!("entry: {rel}"),
            });
        }

        // Django: URLconfs are the route table, manage.py the entry script.
        // Views and models stay targets, not entries. Gated on the file's
        // own scope, so a Django package cannot label a stray urls.py in
        // its neighbor — the monorepo case the root-wide gate got wrong.
        if uses("Django") {
            let base = rel.rsplit('/').next().unwrap_or(rel);
            let app = rel
                .rfind('/')
                .map(|i| rel[..i].rsplit('/').next().unwrap_or("project"))
                .unwrap_or("project");
            let kind = match base {
                "urls.py" => Some(format!("urls {app}")),
                "manage.py" => Some("manage".to_string()),
                "wsgi.py" => Some("wsgi".to_string()),
                "asgi.py" => Some("asgi".to_string()),
                _ => None,
            };
            if let Some(label) = kind {
                hints.push(RouteHint { file: f.id, label });
            }
        }
    }

    for entry in &profile.manifest_entries {
        if let Some(f) = inv.files.iter().find(|f| f.rel == *entry) {
            hints.push(RouteHint {
                file: f.id,
                label: format!("manifest entry: {}", f.rel),
            });
        }
    }

    hints
}

fn next_app_kind(rest: &str) -> Option<&'static str> {
    let base = rest.rsplit('/').next().unwrap_or(rest);
    let stem = base.split('.').next().unwrap_or(base);
    match stem {
        "page" => Some("page"),
        "route" => Some("API"),
        "layout" => Some("layout"),
        // Route-adjacent conventions: entries of their route, not standalone
        // pages. Without these, flows start one step late.
        "loading" => Some("loading"),
        "error" => Some("error"),
        "not-found" => Some("not-found"),
        "opengraph-image" => Some("opengraph-image"),
        "twitter-image" => Some("twitter-image"),
        "sitemap" => Some("sitemap"),
        "robots" => Some("robots"),
        "manifest" => Some("manifest"),
        "favicon" => Some("favicon"),
        "icon" => Some("icon"),
        "apple-icon" => Some("apple-icon"),
        _ => None,
    }
}

/// `dashboard/(admin)/[id]/page.tsx` -> `/dashboard/:id`
fn next_app_route(rest: &str) -> String {
    let mut segs: Vec<String> = Vec::new();
    let parts: Vec<&str> = rest.split('/').collect();
    for seg in &parts[..parts.len().saturating_sub(1)] {
        if seg.starts_with('(') && seg.ends_with(')') {
            continue; // route group, not a URL segment
        }
        if seg.starts_with('@') {
            continue; // parallel route slot
        }
        segs.push(dynamic_segment(seg));
    }
    format!("/{}", segs.join("/"))
}

fn pages_route(rest: &str) -> String {
    let without_ext = rest.rsplit_once('.').map(|(a, _)| a).unwrap_or(rest);
    let without_index = without_ext.strip_suffix("/index").unwrap_or(without_ext);
    let without_index = if without_index == "index" {
        ""
    } else {
        without_index
    };
    let segs: Vec<String> = without_index
        .split('/')
        .filter(|s| !s.is_empty())
        .map(dynamic_segment)
        .collect();
    format!("/{}", segs.join("/"))
}

fn dynamic_segment(seg: &str) -> String {
    if seg.starts_with('[') && seg.ends_with(']') {
        let inner = &seg[1..seg.len() - 1];
        let inner = inner.trim_start_matches("...");
        format!(":{inner}")
    } else {
        seg.to_string()
    }
}

fn is_generic_entry(trimmed: &str) -> bool {
    matches!(
        trimmed,
        "index.ts"
            | "index.js"
            | "main.ts"
            | "main.js"
            | "server.ts"
            | "server.js"
            | "app.ts"
            | "app.js"
            | "cli.ts"
            | "cli.js"
            | "middleware.ts"
            | "middleware.js"
            // Python: runnable modules and conventional app files. `manage.py`
            // is Django-gated above; these are ecosystem-generic.
            | "__main__.py"
            | "app.py"
            | "main.py"
            | "cli.py"
    ) || trimmed.ends_with("/__main__.py")
        || trimmed.ends_with("/app.py")
        || trimmed.ends_with("/main.py")
}

fn normalize_rel(s: &str) -> String {
    s.replace('\\', "/")
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remove `//` and `/* */` comments and trailing commas, preserving string
/// contents. tsconfig.json is JSONC in practice and serde_json is not.
pub fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for n in chars.by_ref() {
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                    out.push(' ');
                }
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }

    strip_trailing_commas(&out)
}

fn strip_trailing_commas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut in_string = false;
    let mut escaped = false;

    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == ',' {
            // Look ahead past whitespace for a closing bracket.
            let next = chars[i + 1..].iter().find(|ch| !ch.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let src = r#"{
            // line comment
            "compilerOptions": {
                /* block */
                "baseUrl": ".",
                "paths": { "@/*": ["src/*"], },
            },
        }"#;
        let v: serde_json::Value = serde_json::from_str(&strip_jsonc(src)).unwrap();
        assert_eq!(v["compilerOptions"]["baseUrl"], ".");
    }

    #[test]
    fn keeps_slashes_inside_strings() {
        let src = r#"{"url": "https://example.com/a", "note": "not // a comment"}"#;
        let v: serde_json::Value = serde_json::from_str(&strip_jsonc(src)).unwrap();
        assert_eq!(v["url"], "https://example.com/a");
        assert_eq!(v["note"], "not // a comment");
    }

    #[test]
    fn builds_next_app_router_paths() {
        assert_eq!(
            next_app_route("dashboard/(admin)/[id]/page.tsx"),
            "/dashboard/:id"
        );
        assert_eq!(next_app_route("page.tsx"), "/");
        assert_eq!(next_app_route("api/users/route.ts"), "/api/users");
    }

    #[test]
    fn builds_pages_router_paths() {
        assert_eq!(pages_route("api/users/[id].ts"), "/api/users/:id");
        assert_eq!(pages_route("index.tsx"), "/");
        assert_eq!(pages_route("blog/index.tsx"), "/blog");
    }

    #[test]
    fn normalizes_python_dep_names() {
        assert_eq!(normalize_py_dep("flask>=2.2.5"), "flask");
        assert_eq!(normalize_py_dep("djangorestframework==3.4.4"), "djangorestframework");
        assert_eq!(normalize_py_dep("PyJWT==1.4.2"), "pyjwt");
        assert_eq!(normalize_py_dep("name; python_version>'3.8'"), "name");
        assert_eq!(normalize_py_dep("sqlalchemy[asyncio]>=2.0"), "sqlalchemy");
    }

    #[test]
    fn parses_pyproject_name_and_deps() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("codearch-pyproject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("pyproject.toml")).unwrap();
        write!(
            f,
            "[project]\nname = \"Flask-SQLAlchemy\"\ndescription = \"Desc.\"\ndependencies = [\n\"flask>=2.2.5\",\n'sqlalchemy>=2.0.16',\n]\n"
        )
        .unwrap();
        let mut p = Profile::default();
        read_pyproject(&dir, &mut p);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(p.project_name.as_deref(), Some("Flask-SQLAlchemy"));
        assert_eq!(p.description.as_deref(), Some("Desc."));
        assert!(p.deps.contains("flask"));
        assert!(p.deps.contains("sqlalchemy"));
    }

    #[test]
    fn parses_poetry_deps_and_skips_python() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("codearch-poetry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("pyproject.toml")).unwrap();
        write!(
            f,
            "[tool.poetry]\nname = \"shop\"\n[tool.poetry.dependencies]\npython = \"^3.11\"\ndjango = \"^5.0\"\n"
        )
        .unwrap();
        let mut p = Profile::default();
        read_pyproject(&dir, &mut p);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(p.project_name.as_deref(), Some("shop"));
        assert!(p.deps.contains("django"));
        assert!(!p.deps.contains("python"));
    }

    #[test]
    fn parses_requirements_skipping_comments_and_options() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("codearch-reqs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("requirements.txt")).unwrap();
        write!(
            f,
            "# web\ndjango==1.10.5\ndjangorestframework>=3.0  # api\n-r base.txt\nhttps://example.com/pkg.zip\n\nsix\n"
        )
        .unwrap();
        let mut p = Profile::default();
        read_requirements(&dir, &mut p);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(p.deps.contains("django"));
        assert!(p.deps.contains("djangorestframework"));
        assert!(p.deps.contains("six"));
        assert_eq!(p.deps.len(), 3);
    }

    fn test_inventory(rels: &[&str]) -> Inventory {
        use crate::types::{FileClass, FileRecord, Language};
        Inventory {
            root: std::path::PathBuf::from("."),
            files: rels
                .iter()
                .enumerate()
                .map(|(id, rel)| FileRecord {
                    id,
                    rel: (*rel).into(),
                    abs: std::path::PathBuf::from(rel),
                    language: Language::from_path(std::path::Path::new(rel))
                        .unwrap_or(Language::Ts),
                    bytes: 10,
                    loc: 1,
                    class: FileClass::Source,
                })
                .collect(),
            skipped: Vec::new(),
            excluded: Default::default(),
        }
    }

    #[test]
    fn python_runnable_modules_are_generic_entries() {
        assert!(is_generic_entry("pkg/__main__.py"));
        assert!(is_generic_entry("app.py"));
        assert!(is_generic_entry("service/main.py"));
        assert!(!is_generic_entry("pkg/models.py"));
    }

    #[test]
    fn django_entries_fire_only_under_detection() {
        let inv = test_inventory(&[
            "conduit/urls.py",
            "conduit/apps/articles/urls.py",
            "conduit/apps/articles/views.py",
            "manage.py",
        ]);
        let django = Profile {
            frameworks: vec!["Django".to_string()],
            ..Default::default()
        };
        let labels: Vec<String> = route_hints(&django, &inv)
            .iter()
            .map(|h| h.label.clone())
            .collect();
        assert!(labels.contains(&"urls conduit".to_string()));
        assert!(labels.contains(&"urls articles".to_string()));
        assert!(labels.contains(&"manage".to_string()));
        assert!(!labels.iter().any(|l| l.contains("views")));

        let plain = route_hints(&Profile::default(), &inv);
        assert!(!plain.iter().any(|h| h.label.starts_with("urls ")));
    }

    #[test]
    fn route_adjacent_files_are_entries_of_their_route() {        assert_eq!(next_app_kind("search/loading.tsx"), Some("loading"));
        assert_eq!(next_app_kind("product/[handle]/not-found.tsx"), Some("not-found"));
        assert_eq!(next_app_kind("[page]/opengraph-image.tsx"), Some("opengraph-image"));
        assert_eq!(next_app_kind("sitemap.ts"), Some("sitemap"));
        // Non-convention files still yield nothing.
        assert_eq!(next_app_kind("search/grid.tsx"), None);
    }

    fn test_inventory_at(root: std::path::PathBuf, rels: &[&str]) -> Inventory {
        use crate::types::{FileClass, FileRecord, Language};
        // `abs` joins the root (not the bare rel) so disk-reading helpers —
        // the manage.py settings scan — work on temp-dir trees.
        let files: Vec<FileRecord> = rels
            .iter()
            .enumerate()
            .map(|(id, rel)| FileRecord {
                id,
                rel: (*rel).into(),
                abs: root.join(rel),
                language: Language::from_path(std::path::Path::new(rel))
                    .unwrap_or(Language::Ts),
                bytes: 10,
                loc: 1,
                class: FileClass::Source,
            })
            .collect();
        Inventory {
            root,
            files,
            skipped: Vec::new(),
            excluded: Default::default(),
        }
    }

    fn write_tree(path: &std::path::Path, content: &str) {
        use std::io::Write;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        write!(f, "{content}").unwrap();
    }

    #[test]
    fn rebase_joins_and_normalizes() {
        assert_eq!(rebase("packages/web", "../shared/*"), "packages/shared/*");
        assert_eq!(rebase("packages/web", "."), "packages/web");
        assert_eq!(rebase("", "dist/index.js"), "dist/index.js");
        assert_eq!(rebase("pkg", "./main.js"), "pkg/main.js");
    }

    #[test]
    fn standalone_app_tsconfig_base_url_is_read_without_becoming_a_package() {
        let dir = std::env::temp_dir().join(format!("codearch-nested-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_tree(&dir.join("pyproject.toml"), "[project]
name = \"backend\"
");
        write_tree(&dir.join("src-ui/package.json"), r#"{"name":"ui"}"#);
        write_tree(&dir.join("src-ui/tsconfig.json"), r#"{"compilerOptions":{"baseUrl":"./"}}"#);
        write_tree(&dir.join("node_modules/x/tsconfig.json"), r#"{"compilerOptions":{"baseUrl":"."}}"#);
        let p = detect(&test_inventory_at(dir.clone(), &[]));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(p.package_dirs.is_empty());
        assert_eq!(p.mappings.extra_base_urls, vec!["src-ui".to_string()]);
    }

    #[test]
    fn workspaces_merge_package_manifests() {
        let dir = std::env::temp_dir().join(format!("codearch-mono-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_tree(
            &dir.join("package.json"),
            r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
        );
        write_tree(
            &dir.join("packages/web/package.json"),
            r#"{"name":"web","dependencies":{"react":"18.0.0"}}"#,
        );
        write_tree(
            &dir.join("packages/web/tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@shared/*":["../shared/*"]}}}"#,
        );
        write_tree(
            &dir.join("packages/api/pyproject.toml"),
            "[project]\nname = \"api\"\ndependencies = [\"flask\"]\n",
        );
        let p = detect(&test_inventory_at(dir.clone(), &[]));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            p.package_dirs,
            vec!["packages/api".to_string(), "packages/web".to_string()]
        );
        assert!(p.frameworks.contains(&"React".to_string()));
        assert!(p.frameworks.contains(&"Flask".to_string()));
        assert!(p.mappings.paths.contains(&(
            "@shared/*".to_string(),
            vec!["packages/shared/*".to_string()]
        )));
        assert!(p.mappings.extra_base_urls.contains(&"packages/web".to_string()));
        assert_eq!(p.project_name.as_deref(), Some("mono"));
    }

    #[test]
    fn pnpm_workspaces_discover_packages() {
        let dir =
            std::env::temp_dir().join(format!("codearch-pnpm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_tree(&dir.join("pnpm-workspace.yaml"), "packages:\n  - 'apps/*'\n");
        write_tree(
            &dir.join("apps/blog/package.json"),
            r#"{"name":"blog","dependencies":{"next":"14.0.0"}}"#,
        );
        let p = detect(&test_inventory_at(dir.clone(), &[]));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(p.package_dirs, vec!["apps/blog".to_string()]);
        assert!(p.frameworks.contains(&"Next.js".to_string()));
    }

    #[test]
    fn stale_glob_invents_no_packages() {
        let dir =
            std::env::temp_dir().join(format!("codearch-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_tree(
            &dir.join("package.json"),
            r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
        );
        write_tree(&dir.join("packages/empty/README.md"), "nothing to read here\n");
        let p = detect(&test_inventory_at(dir.clone(), &[]));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(p.package_dirs.is_empty());
        assert!(p.frameworks.is_empty());
    }

    fn scoped_profile() -> (Profile, Inventory) {
        // Global is the union, scopes disagree: the web package is Next.js,
        // the api package is Flask, and neither lends its rules across.
        let profile = Profile {
            frameworks: vec!["Next.js".to_string(), "Flask".to_string()],
            package_dirs: vec!["packages/api".to_string(), "packages/web".to_string()],
            package_frameworks: vec![
                ("packages/api".to_string(), vec!["Flask".to_string()]),
                ("packages/web".to_string(), vec!["Next.js".to_string()]),
            ],
            ..Default::default()
        };
        let inv = test_inventory_at(
            std::path::PathBuf::from("."),
            &[
                "packages/web/app/page.tsx",
                "packages/api/app/page.tsx",
                "packages/api/app.py",
            ],
        );
        (profile, inv)
    }

    #[test]
    fn next_routes_fire_only_in_the_next_package() {
        let (profile, inv) = scoped_profile();
        let labels: Vec<String> = route_hints(&profile, &inv)
            .iter()
            .map(|h| h.label.clone())
            .collect();
        assert!(labels.contains(&"page /".to_string()));
        // The Flask package's identically-shaped path stays quiet, and its
        // runnable module is still an entry by the scope-agnostic rule.
        assert_eq!(labels.iter().filter(|l| l.starts_with("page ")).count(), 1);
        assert!(labels.contains(&"entry: packages/api/app.py".to_string()));
    }

    #[test]
    fn django_gate_is_per_scope() {
        let profile = Profile {
            frameworks: vec!["Django".to_string()],
            package_dirs: vec!["blog".to_string(), "shop".to_string()],
            package_frameworks: vec![
                ("blog".to_string(), Vec::new()),
                ("shop".to_string(), vec!["Django".to_string()]),
            ],
            ..Default::default()
        };
        let inv = test_inventory_at(
            std::path::PathBuf::from("."),
            &["shop/urls.py", "blog/urls.py"],
        );
        let labels: Vec<String> = route_hints(&profile, &inv)
            .iter()
            .map(|h| h.label.clone())
            .collect();
        assert!(labels.contains(&"urls shop".to_string()));
        assert!(!labels.iter().any(|l| l == "urls blog"));
    }

    #[test]
    fn detect_records_per_package_frameworks() {
        let dir =
            std::env::temp_dir().join(format!("codearch-scopes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_tree(
            &dir.join("package.json"),
            r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
        );
        write_tree(
            &dir.join("packages/shop/package.json"),
            r#"{"name":"shop","dependencies":{"next":"14.0.0"}}"#,
        );
        write_tree(
            &dir.join("packages/api/pyproject.toml"),
            "[project]\nname = \"api\"\ndependencies = [\"django\"]\n",
        );
        let p = detect(&test_inventory_at(dir.clone(), &[]));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            p.package_frameworks,
            vec![
                ("packages/api".to_string(), vec!["Django".to_string()]),
                ("packages/shop".to_string(), vec!["Next.js".to_string()]),
            ]
        );
    }

    #[test]
    fn manage_py_scoped_to_its_package() {
        let dir =
            std::env::temp_dir().join(format!("codearch-manage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_tree(
            &dir.join("package.json"),
            r#"{"name":"mono","private":true,"workspaces":["packages/*"]}"#,
        );
        write_tree(&dir.join("packages/shop/package.json"), r#"{"name":"shop"}"#);
        write_tree(
            &dir.join("packages/api/manage.py"),
            "import os\nos.environ.setdefault('DJANGO_SETTINGS_MODULE', 'api.settings')\n",
        );
        write_tree(&dir.join("packages/api/pyproject.toml"), "[project]\nname = \"api\"\n");
        let p = detect(&test_inventory_at(dir.clone(), &["packages/api/manage.py"]));
        let _ = std::fs::remove_dir_all(&dir);
        let fw = |d: &str| {
            p.package_frameworks
                .iter()
                .find(|(x, _)| x == d)
                .map(|(_, f)| f.clone())
                .unwrap_or_default()
        };
        assert!(fw("packages/api").contains(&"Django".to_string()));
        assert!(!fw("packages/shop").contains(&"Django".to_string()));
    }
}
