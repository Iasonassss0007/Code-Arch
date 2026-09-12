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
use crate::types::RouteHint;
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
}

pub fn detect(inv: &Inventory) -> Profile {
    let mut p = Profile::default();

    if let Some(raw) = read_json(&inv.root.join("package.json")) {
        p.has_package_json = true;
        p.project_name = raw
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        p.description = raw
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        for key in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(obj) = raw.get(key).and_then(|v| v.as_object()) {
                for name in obj.keys() {
                    p.deps.insert(name.clone());
                }
            }
        }

        if let Some(main) = raw.get("main").and_then(|v| v.as_str()) {
            p.manifest_entries.push(normalize_rel(main));
        }
        match raw.get("bin") {
            Some(serde_json::Value::String(s)) => p.manifest_entries.push(normalize_rel(s)),
            Some(serde_json::Value::Object(o)) => {
                for v in o.values() {
                    if let Some(s) = v.as_str() {
                        p.manifest_entries.push(normalize_rel(s));
                    }
                }
            }
            _ => {}
        }
    }

    read_pyproject(&inv.root, &mut p);
    read_requirements(&inv.root, &mut p);

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

    // tsconfig.json is very often JSONC, so it cannot go through serde directly.
    for candidate in ["tsconfig.json", "jsconfig.json"] {
        let path = inv.root.join(candidate);
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
            p.mappings.base_url = Some(normalize_rel(b));
        }
        if let Some(paths) = co.get("paths").and_then(|v| v.as_object()) {
            for (alias, targets) in paths {
                let list: Vec<String> = targets
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.as_str())
                            .map(normalize_rel)
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

    p
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
    let manage = inv
        .files
        .iter()
        .chain(inv.skipped.iter())
        .find(|f| f.rel.rsplit('/').next().unwrap_or(&f.rel) == "manage.py");
    let Some(manage) = manage else {
        return false;
    };
    // Path match alone is weak (`manage.py` could be anything); the settings
    // declaration makes it Django's.
    std::fs::read_to_string(&manage.abs)
        .map(|t| t.contains("DJANGO_SETTINGS_MODULE"))
        .unwrap_or(false)
}

/// Entry points derived from framework conventions and manifest fields.
///
/// This is path-based on purpose: a Next.js route is a fact about the file's
/// location, and no parser recovers it.
pub fn route_hints(profile: &Profile, inv: &Inventory) -> Vec<RouteHint> {
    let mut hints = Vec::new();

    for f in &inv.files {
        let rel = f.rel.as_str();
        let trimmed = rel.strip_prefix("src/").unwrap_or(rel);

        if profile.uses("Next.js") {
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
        // Views and models stay targets, not entries. Gated on detection so
        // a stray urls.py in another ecosystem is never mislabeled.
        if profile.uses("Django") {
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
}
