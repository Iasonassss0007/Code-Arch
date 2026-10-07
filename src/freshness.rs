use crate::{cache, inventory, query, Options};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

const MAX_LISTED: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Fresh,
    Stale,
    Unknown,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Fresh => "fresh",
            State::Stale => "stale",
            State::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Freshness {
    pub state: State,
    pub index_age_minutes: Option<u64>,
    pub changed_count: usize,
    pub changed: Vec<String>,
    pub refreshed: bool,
    pub refresh_error: Option<String>,
}

#[derive(serde::Deserialize)]
struct LightStore {
    version: u32,
    files: HashMap<String, LightFile>,
    #[serde(default)]
    configs: HashMap<String, LightFile>,
}

#[derive(serde::Deserialize)]
struct LightFile {
    mtime_ns: u64,
    size: u64,
}

type SigMap = HashMap<String, (u64, u64)>;

fn stored_stats(dir: &Path) -> Option<(SigMap, SigMap)> {
    let text = std::fs::read_to_string(dir.join("cache").join("store.json")).ok()?;
    let store: LightStore = serde_json::from_str(&text).ok()?;
    if store.version != cache::FORMAT || store.files.is_empty() {
        return None;
    }
    let file_sig = |files: HashMap<String, LightFile>| {
        files
            .into_iter()
            .map(|(path, f)| (path, (f.mtime_ns, f.size)))
            .collect()
    };
    Some((file_sig(store.files), file_sig(store.configs)))
}

fn changed_paths(stored: &SigMap, current: &SigMap) -> Vec<String> {
    current
        .iter()
        .filter(|(path, sig)| stored.get(*path) != Some(*sig))
        .map(|(path, _)| path.clone())
        .chain(stored.keys().filter(|p| !current.contains_key(*p)).cloned())
        .collect()
}

pub fn check(repo: &Path, dir: &Path) -> Freshness {
    let mut out = Freshness {
        state: State::Unknown,
        index_age_minutes: query::index_age_minutes(dir, "imports.md"),
        changed_count: 0,
        changed: Vec::new(),
        refreshed: false,
        refresh_error: None,
    };
    let (Some((stored_sources, stored_configs)), Ok(current)) =
        (stored_stats(dir), inventory::stat_snapshot(repo))
    else {
        return out;
    };
    let mut changed = changed_paths(&stored_sources, &current.sources);
    changed.extend(changed_paths(&stored_configs, &current.configs));
    changed.sort();
    out.changed_count = changed.len();
    changed.truncate(MAX_LISTED);
    out.changed = changed;
    out.state = if out.changed_count == 0 { State::Fresh } else { State::Stale };
    out
}

pub fn rebuild(repo: &Path, dir: &Path) -> anyhow::Result<()> {
    let opts = Options {
        root: repo.to_path_buf(),
        codearch_dir: Some(dir.to_path_buf()),
        map: false,
        ..Options::default()
    };
    crate::run(&opts).map(|_| ())
}

pub fn resolve(repo: &Path, dir: &Path, allow_rebuild: bool) -> Freshness {
    let mut found = check(repo, dir);
    if !allow_rebuild || found.state == State::Fresh {
        return found;
    }
    match rebuild(repo, dir) {
        Ok(()) => {
            found = check(repo, dir);
            found.refreshed = true;
        }
        Err(e) => found.refresh_error = Some(format!("{e:#}")),
    }
    found
}

impl Freshness {
    pub fn is_stale(&self) -> bool {
        self.state == State::Stale
    }

    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "state": self.state.as_str(),
            "changed_count": self.changed_count,
            "changed": self.changed,
        });
        if let Some(age) = self.index_age_minutes {
            out["index_age_minutes"] = json!(age);
        }
        if self.refreshed {
            out["refreshed"] = json!(true);
        }
        if let Some(e) = &self.refresh_error {
            out["refresh_error"] = json!(e);
        }
        out
    }

    pub fn summary_line(&self) -> String {
        let mut line = match self.state {
            State::Fresh if self.refreshed => "index: rebuilt just now, fresh".to_string(),
            State::Fresh => "index: fresh".to_string(),
            State::Stale => {
                let noun = if self.changed_count == 1 { "file" } else { "files" };
                let example = self.changed.first().map(|p| format!(", e.g. {p}")).unwrap_or_default();
                format!("index: stale ({} {noun} changed{example})", self.changed_count)
            }
            State::Unknown => "index: freshness unknown".to_string(),
        };
        if !self.refreshed
            && let Some(age) = self.index_age_minutes
        {
            line.push_str(&format!(", written {age} min ago"));
        }
        if self.state != State::Fresh {
            line.push_str("; run codearch");
        }
        if let Some(e) = &self.refresh_error {
            line.push_str(&format!("; rebuild failed: {e}"));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture {
        tmp: PathBuf,
        repo: PathBuf,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let tmp = std::env::temp_dir()
                .join(format!("codearch-fresh-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            let repo = tmp.join("repo");
            std::fs::create_dir_all(repo.join("src")).unwrap();
            std::fs::write(repo.join("src/a.ts"), "export const a = 1;\n").unwrap();
            std::fs::write(
                repo.join("src/b.ts"),
                "import { a } from \"./a\";\nexport const b = a;\n",
            )
            .unwrap();
            let dir = tmp.join("state");
            Fixture { tmp, repo, dir }
        }

        fn index(&self) {
            rebuild(&self.repo, &self.dir).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }

    #[test]
    fn no_cache_is_unknown() {
        let fx = Fixture::new("unknown");
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Unknown);
        assert_eq!(f.changed_count, 0);
    }

    #[test]
    fn unchanged_tree_is_fresh() {
        let fx = Fixture::new("fresh");
        fx.index();
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Fresh);
        assert!(f.changed.is_empty());
        assert!(f.index_age_minutes.is_some());
    }

    #[test]
    fn edited_file_is_stale_and_named() {
        let fx = Fixture::new("edit");
        fx.index();
        std::fs::write(fx.repo.join("src/a.ts"), "export const a = 12345;\n").unwrap();
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Stale);
        assert_eq!(f.changed, vec!["src/a.ts".to_string()]);
        assert_eq!(f.changed_count, 1);
    }

    #[test]
    fn deleted_file_is_stale() {
        let fx = Fixture::new("delete");
        fx.index();
        std::fs::remove_file(fx.repo.join("src/b.ts")).unwrap();
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Stale);
        assert_eq!(f.changed, vec!["src/b.ts".to_string()]);
    }

    #[test]
    fn added_file_is_stale() {
        let fx = Fixture::new("add");
        fx.index();
        std::fs::write(fx.repo.join("src/c.ts"), "export const c = 3;\n").unwrap();
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Stale);
        assert_eq!(f.changed, vec!["src/c.ts".to_string()]);
    }

    #[test]
    fn listing_is_capped_but_count_is_exact() {
        let fx = Fixture::new("cap");
        fx.index();
        for i in 0..8 {
            std::fs::write(fx.repo.join(format!("src/n{i}.ts")), "export {};\n").unwrap();
        }
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.changed_count, 8);
        assert_eq!(f.changed.len(), MAX_LISTED);
    }

    #[test]
    fn without_permission_nothing_is_written() {
        let fx = Fixture::new("readonly");
        let f = resolve(&fx.repo, &fx.dir, false);
        assert_eq!(f.state, State::Unknown);
        assert!(!f.refreshed);
        assert!(!fx.dir.exists());
    }

    #[test]
    fn rebuild_turns_stale_into_fresh() {
        let fx = Fixture::new("rebuild");
        fx.index();
        std::fs::write(fx.repo.join("src/a.ts"), "export const a = 12345;\n").unwrap();
        let f = resolve(&fx.repo, &fx.dir, true);
        assert_eq!(f.state, State::Fresh);
        assert!(f.refreshed);
        assert!(f.refresh_error.is_none());
        assert_eq!(f.to_json()["refreshed"], true);
    }

    #[test]
    fn rebuild_builds_a_missing_index() {
        let fx = Fixture::new("firstbuild");
        let f = resolve(&fx.repo, &fx.dir, true);
        assert_eq!(f.state, State::Fresh);
        assert!(f.refreshed);
        assert!(fx.dir.join("imports.md").is_file());
    }

    #[test]
    fn failed_rebuild_is_reported_and_state_is_kept() {
        let fx = Fixture::new("fail");
        std::fs::remove_file(fx.repo.join("src/a.ts")).unwrap();
        std::fs::remove_file(fx.repo.join("src/b.ts")).unwrap();
        let f = resolve(&fx.repo, &fx.dir, true);
        assert_eq!(f.state, State::Unknown);
        assert!(!f.refreshed);
        assert!(f.refresh_error.as_deref().unwrap().contains("no JavaScript"));
        assert!(f.to_json().get("refreshed").is_none());
        assert!(f.summary_line().contains("rebuild failed"));
    }

    fn write_rel(repo: &Path, rel: &str, text: &str) {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn config_edit_is_stale(name: &str, rel: &str, before: &str, after: &str) {
        let fx = Fixture::new(name);
        write_rel(&fx.repo, rel, before);
        fx.index();
        assert_eq!(check(&fx.repo, &fx.dir).state, State::Fresh, "{rel} indexed");
        write_rel(&fx.repo, rel, after);
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Stale, "{rel}");
        assert!(f.changed.iter().any(|p| p == rel), "{rel} {:?}", f.changed);
    }

    #[test]
    fn tsconfig_paths_change_is_stale_and_refresh_moves_the_edge() {
        let fx = Fixture::new("tsconfig-alias");
        write_rel(
            &fx.repo,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@x/*":["src/lib/*"]}}}"#,
        );
        write_rel(&fx.repo, "src/lib/a.ts", "export const a = 1;\n");
        write_rel(&fx.repo, "src/other/a.ts", "export const a = 2;\n");
        write_rel(&fx.repo, "src/main.ts", "import { a } from \"@x/a\";\n");
        fx.index();
        let before = std::fs::read_to_string(fx.dir.join("imports.md")).unwrap();
        assert!(before.contains("src/lib/a.ts ← src/main.ts"), "{before}");
        write_rel(
            &fx.repo,
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@x/*":["src/other/*"]}}}"#,
        );
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Stale);
        assert!(f.changed.iter().any(|p| p == "tsconfig.json"), "{:?}", f.changed);
        assert!(f.summary_line().contains("index: stale"));
        let held = std::fs::read_to_string(fx.dir.join("imports.md")).unwrap();
        assert!(held.contains("src/lib/a.ts ← src/main.ts"));
        let rebuilt = resolve(&fx.repo, &fx.dir, true);
        assert_eq!(rebuilt.state, State::Fresh);
        let after = std::fs::read_to_string(fx.dir.join("imports.md")).unwrap();
        assert!(after.contains("src/other/a.ts ← src/main.ts"), "{after}");
        assert!(!after.contains("src/lib/a.ts ← src/main.ts"), "{after}");
    }

    #[test]
    fn jsconfig_change_is_stale() {
        config_edit_is_stale(
            "jsconfig",
            "jsconfig.json",
            r#"{"compilerOptions":{"baseUrl":"."}}"#,
            r#"{"compilerOptions":{"baseUrl":"src"}}"#,
        );
    }

    #[test]
    fn package_json_change_is_stale() {
        config_edit_is_stale(
            "pkg",
            "package.json",
            r##"{"name":"app","imports":{"#a":"./src/a.ts"}}"##,
            r##"{"name":"app","imports":{"#a":"./src/b.ts"}}"##,
        );
    }

    #[test]
    fn pnpm_workspace_change_is_stale() {
        config_edit_is_stale(
            "pnpm",
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/*'\n",
            "packages:\n  - 'apps/*'\n",
        );
    }

    #[test]
    fn pyproject_change_is_stale() {
        config_edit_is_stale(
            "pyproject",
            "pyproject.toml",
            "[project]\nname = \"app\"\n",
            "[project]\nname = \"renamed\"\n",
        );
    }

    #[test]
    fn in_repo_tsconfig_extends_change_is_stale() {
        let fx = Fixture::new("extends");
        write_rel(&fx.repo, "tsconfig.json", r#"{ "extends": "./tsconfig.base.json" }"#);
        write_rel(
            &fx.repo,
            "tsconfig.base.json",
            r#"{"compilerOptions":{"paths":{"@x/*":["src/lib/*"]}}}"#,
        );
        write_rel(&fx.repo, "src/lib/a.ts", "export const a = 1;\n");
        write_rel(&fx.repo, "src/main.ts", "import { a } from \"@x/a\";\n");
        fx.index();
        let before = std::fs::read_to_string(fx.dir.join("imports.md")).unwrap();
        assert!(before.contains("src/lib/a.ts ← src/main.ts"), "{before}");
        write_rel(
            &fx.repo,
            "tsconfig.base.json",
            r#"{"compilerOptions":{"paths":{"@x/*":["src/missing/*"]}}}"#,
        );
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Stale);
        assert!(
            f.changed.iter().any(|p| p == "tsconfig.base.json"),
            "{:?}",
            f.changed
        );
    }

    #[test]
    fn node_modules_extends_change_stays_fresh() {
        let fx = Fixture::new("nm-extends");
        write_rel(&fx.repo, "tsconfig.json", r#"{ "extends": "@ext/cfg" }"#);
        write_rel(
            &fx.repo,
            "node_modules/@ext/cfg/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@x/*":["../../../src/lib/*"]}}}"#,
        );
        write_rel(&fx.repo, "src/lib/a.ts", "export const a = 1;\n");
        write_rel(&fx.repo, "src/other/a.ts", "export const a = 2;\n");
        write_rel(&fx.repo, "src/main.ts", "import { a } from \"@x/a\";\n");
        fx.index();
        let before = std::fs::read_to_string(fx.dir.join("imports.md")).unwrap();
        assert!(before.contains("src/lib/a.ts ← src/main.ts"), "{before}");
        write_rel(
            &fx.repo,
            "node_modules/@ext/cfg/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@x/*":["../../../src/other/*"]}}}"#,
        );
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Fresh);
        assert!(f.changed.is_empty(), "{:?}", f.changed);
    }

    #[test]
    fn requirements_txt_change_stays_fresh() {
        let fx = Fixture::new("requirements");
        write_rel(&fx.repo, "requirements.txt", "django==4.2\n");
        fx.index();
        write_rel(&fx.repo, "requirements.txt", "django==5.0\n");
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Fresh);
        assert!(f.changed.is_empty(), "{:?}", f.changed);
    }

    #[test]
    fn old_cache_without_config_stats_is_unknown() {
        let fx = Fixture::new("old-cache");
        fx.index();
        let path = fx.dir.join("cache").join("store.json");
        let text = std::fs::read_to_string(&path).unwrap().replace("\"version\":11", "\"version\":10");
        std::fs::write(&path, text).unwrap();
        let f = check(&fx.repo, &fx.dir);
        assert_eq!(f.state, State::Unknown);
        assert!(f.summary_line().contains("freshness unknown"));
    }

    #[test]
    fn json_shape_and_summary_lines() {
        let stale = Freshness {
            state: State::Stale,
            index_age_minutes: Some(42),
            changed_count: 3,
            changed: vec!["src/a.ts".into()],
            refreshed: false,
            refresh_error: None,
        };
        assert_eq!(
            stale.to_json(),
            json!({"state": "stale", "index_age_minutes": 42, "changed_count": 3,
                   "changed": ["src/a.ts"]})
        );
        assert_eq!(
            stale.summary_line(),
            "index: stale (3 files changed, e.g. src/a.ts), written 42 min ago; run codearch"
        );
        let fresh = Freshness { state: State::Fresh, changed_count: 0, changed: vec![], ..stale.clone() };
        assert_eq!(fresh.summary_line(), "index: fresh, written 42 min ago");
        let unknown = Freshness { state: State::Unknown, index_age_minutes: None, ..fresh };
        assert_eq!(unknown.summary_line(), "index: freshness unknown; run codearch");
    }
}
