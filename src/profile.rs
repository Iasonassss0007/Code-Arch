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

    for (dep, display) in FRAMEWORKS {
        if p.deps.contains(*dep) && !p.frameworks.iter().any(|f| f == display) {
            p.frameworks.push((*display).to_string());
        }
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
    )
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
}
