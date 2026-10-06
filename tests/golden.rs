use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

struct FixtureCase {
    name: &'static str,
    files: &'static [&'static str],
}

const FIXTURES: &[FixtureCase] = &[
    FixtureCase {
        name: "mono",
        files: &["imports.md"],
    },
    FixtureCase {
        name: "xlang",
        files: &["imports.md", "routes.md"],
    },
    FixtureCase {
        name: "tier1",
        files: &["imports.md"],
    },
];

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("codearch-golden-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("create {}: {err}", path.display()));
        Scratch { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn fixture_src(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("eval/fixtures")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|err| panic!("mkdir {}: {err}", to.display()));
    let entries =
        std::fs::read_dir(from).unwrap_or_else(|err| panic!("read {}: {err}", from.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("dirent: {err}"));
        let dest = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .unwrap_or_else(|err| panic!("file type: {err}"));
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest)
                .unwrap_or_else(|err| panic!("copy {}: {err}", dest.display()));
        }
    }
}

fn copy_fixture(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    copy_tree(&fixture_src(name), &scratch.path);
    scratch
}

fn codearch(args: &[std::ffi::OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codearch"))
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("spawn codearch: {err}"))
}

fn index(root: &Path) {
    let out = codearch(&[root.as_os_str().to_os_string()]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "index {} exit\n{}",
        root.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn query(root: &Path, args: &[&str]) -> Output {
    let mut full: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
    full.push("--repo".into());
    full.push(root.as_os_str().into());
    codearch(&full)
}

fn stderr_text(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap_or_else(|err| panic!("stderr utf-8: {err}"))
}

fn substitute_root_dir_name(bytes: &[u8], root: &Path) -> Vec<u8> {
    let name = root
        .file_name()
        .unwrap_or_else(|| panic!("temp dir has no name"))
        .to_string_lossy();
    String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|err| panic!("index utf-8: {err}"))
        .replace(name.as_ref(), "<ROOT>")
        .into_bytes()
}

fn assert_snapshot(root: &Path, name: &str, file: &str) {
    let got = std::fs::read(root.join(".codearch").join(file))
        .unwrap_or_else(|err| panic!("read {name} {file}: {err}"));
    let got = substitute_root_dir_name(&got, root);
    let want = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(name)
            .join(file),
    )
    .unwrap_or_else(|err| panic!("snapshot {name}/{file}: {err}"));
    assert_eq!(got, want, "{name} {file} bytes");
}

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("mkdir {}: {err}", parent.display()));
    }
    std::fs::write(path, text).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

#[test]
fn fixture_indexes_match_golden_snapshots() {
    for case in FIXTURES {
        let scratch = copy_fixture(case.name);
        index(&scratch.path);
        for file in case.files {
            assert_snapshot(&scratch.path, case.name, file);
        }
        if !case.files.contains(&"routes.md") {
            let name = case.name;
            assert!(
                !scratch.path.join(".codearch/routes.md").exists(),
                "{name} routes.md must be absent"
            );
        }
    }
}

#[test]
fn mono_importers_json_names_the_importer() {
    let scratch = copy_fixture("mono");
    index(&scratch.path);
    let out = query(
        &scratch.path,
        &["importers", "packages/shared/util.ts", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "mono importers json exit");
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("mono importers json");
    assert_eq!(
        value["target"], "packages/shared/util.ts",
        "mono importers json target"
    );
    assert_eq!(
        value["importers"],
        serde_json::json!([{"depth": 1, "path": "packages/web/src/index.ts"}]),
        "mono importers json importers"
    );
    assert_eq!(
        value["freshness"]["state"], "fresh",
        "mono importers json freshness"
    );
}

#[test]
fn mono_importers_text_is_exact() {
    let scratch = copy_fixture("mono");
    index(&scratch.path);
    let out = query(&scratch.path, &["importers", "packages/shared/util.ts"]);
    assert_eq!(out.status.code(), Some(0), "mono importers text exit");
    assert_eq!(
        out.stdout, b"1  packages/web/src/index.ts\n",
        "mono importers text stdout"
    );
    let stderr = stderr_text(&out);
    assert!(
        stderr.starts_with("index: fresh"),
        "fresh index stderr: {stderr}"
    );
}

#[test]
fn mono_importers_miss_is_empty_stdout() {
    let scratch = copy_fixture("mono");
    index(&scratch.path);
    let out = query(&scratch.path, &["importers", "packages/web/src/index.ts"]);
    assert_eq!(out.status.code(), Some(0), "mono importers miss exit");
    assert_eq!(out.stdout, b"", "mono importers miss stdout");
    let stderr = stderr_text(&out);
    assert!(
        stderr.contains(
            "no importers for 'packages/web/src/index.ts': nothing imports it, or it was not analyzed"
        ),
        "mono importers miss stderr: {stderr}"
    );
}

#[test]
fn mono_callers_without_routes_exits_2() {
    let scratch = copy_fixture("mono");
    index(&scratch.path);
    let out = query(&scratch.path, &["callers", "TagView"]);
    assert_eq!(out.status.code(), Some(2), "mono callers exit");
    let stderr = stderr_text(&out);
    let expected = format!(
        "the index under {} has no Django route declarations; codearch callers needs path(), re_path(), url(), or register() in Python",
        scratch.path.join(".codearch").display()
    );
    assert!(
        stderr.contains(&expected),
        "mono callers stderr: {stderr}"
    );
}

#[test]
fn refresh_on_an_empty_repo_reports_the_rebuild_failure() {
    let scratch = Scratch::new("empty-refresh");
    let out = query(&scratch.path, &["importers", "x.ts", "--refresh"]);
    assert_eq!(out.status.code(), Some(2), "empty refresh exit");
    let stderr = stderr_text(&out);
    assert!(
        stderr.contains("rebuild failed"),
        "empty refresh stderr: {stderr}"
    );
    assert!(
        stderr.contains("no JavaScript, TypeScript or Python"),
        "empty refresh stderr: {stderr}"
    );
}

#[test]
fn changed_file_marks_index_stale_without_rewriting_imports() {
    let scratch = copy_fixture("mono");
    index(&scratch.path);
    let imports = scratch.path.join(".codearch/imports.md");
    let before = std::fs::read(&imports).expect("imports.md before");
    let src = scratch.path.join("packages/web/src/index.ts");
    let mut body = std::fs::read(&src).expect("index.ts");
    let size_before = body.len() as u64;
    body.extend_from_slice(b"\nexport const touched = 1;\n");
    std::fs::write(&src, &body).expect("append to index.ts");
    let size_after = std::fs::metadata(&src).expect("index.ts metadata").len();
    assert!(size_after > size_before, "appended line must grow index.ts");
    let out = query(&scratch.path, &["importers", "packages/shared/util.ts"]);
    assert_eq!(out.status.code(), Some(0), "stale importers exit");
    let stderr = stderr_text(&out);
    assert!(
        stderr.contains("index: stale (1 file changed"),
        "stale stderr: {stderr}"
    );
    let after = std::fs::read(&imports).expect("imports.md after");
    assert_eq!(after, before, "stale importers must not rewrite imports.md");
}

#[test]
fn xlang_callers_hit_the_frontend_fetch() {
    let scratch = copy_fixture("xlang");
    index(&scratch.path);
    let by_view = query(&scratch.path, &["callers", "users", "--json"]);
    assert_eq!(
        by_view.status.code(),
        Some(0),
        "xlang callers exit\n{}",
        stderr_text(&by_view)
    );
    let view_value: serde_json::Value =
        serde_json::from_slice(&by_view.stdout).expect("xlang callers json");
    assert!(
        json_callers(&view_value).iter().any(|c| c == "frontend/api.ts"),
        "xlang callers users: {view_value}"
    );
    let by_path = query(
        &scratch.path,
        &["callers", "--path", "api/users", "--json"],
    );
    assert_eq!(
        by_path.status.code(),
        Some(0),
        "xlang path exit\n{}",
        stderr_text(&by_path)
    );
    let path_value: serde_json::Value =
        serde_json::from_slice(&by_path.stdout).expect("xlang path json");
    assert!(
        json_callers(&path_value).iter().any(|c| c == "frontend/api.ts"),
        "xlang callers path: {path_value}"
    );
}

fn json_callers(value: &serde_json::Value) -> Vec<String> {
    value["matches"]
        .as_array()
        .unwrap_or_else(|| panic!("matches array: {value}"))
        .iter()
        .flat_map(|m| {
            m["callers"]
                .as_array()
                .unwrap_or_else(|| panic!("callers array: {m}"))
                .iter()
                .filter_map(|c| c.as_str().map(str::to_string))
        })
        .collect()
}

#[test]
fn django_routes_snapshot_and_tagview_callers() {
    let scratch = Scratch::new("django");
    write_file(
        &scratch.path.join("urls.py"),
        "urlpatterns = [\n    path('tags/', TagView.as_view()),\n]\n",
    );
    write_file(&scratch.path.join("views.py"), "class TagView:\n    pass\n");
    write_file(
        &scratch.path.join("ui/api.ts"),
        "export function load() { return fetch('tags/') }\n",
    );
    index(&scratch.path);
    assert_snapshot(&scratch.path, "django", "routes.md");
    let out = query(&scratch.path, &["callers", "TagView", "--json"]);
    assert_eq!(out.status.code(), Some(0), "django callers exit");
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("django callers json");
    assert_eq!(value["view"], "TagView", "django callers view");
    assert_eq!(
        value["matches"],
        serde_json::json!([{
            "file": "views.py",
            "routes": ["tags"],
            "callers": ["ui/api.ts"]
        }]),
        "django callers matches"
    );
    assert_eq!(
        value["freshness"]["state"], "fresh",
        "django callers freshness"
    );
    let by_path = query(&scratch.path, &["callers", "--path", "tags", "--json"]);
    assert_eq!(by_path.status.code(), Some(0), "django path json exit");
    let path_value: serde_json::Value =
        serde_json::from_slice(&by_path.stdout).expect("django path json");
    assert_eq!(path_value["path"], "tags", "django path key");
    assert!(path_value.get("view").is_none(), "path json has no top-level view");
    assert_eq!(
        path_value["matches"],
        serde_json::json!([{
            "view": "TagView",
            "file": "views.py",
            "routes": ["tags"],
            "callers": ["ui/api.ts"]
        }]),
        "django path matches"
    );
    let slashed = query(&scratch.path, &["callers", "--path", "/tags/"]);
    assert_eq!(slashed.status.code(), Some(0), "django slashed path exit");
    assert_eq!(
        slashed.stdout,
        b"TagView  views.py  (routes: tags)\n  ui/api.ts\n",
        "django slashed path text"
    );
}

#[test]
fn unresolved_flag_lists_the_broken_import() {
    let scratch = Scratch::new("unresolved");
    write_file(
        &scratch.path.join("src/app.ts"),
        "import './missing';\n",
    );
    let with_flag = codearch(&[
        std::ffi::OsString::from("--unresolved"),
        scratch.path.as_os_str().to_os_string(),
    ]);
    assert_eq!(with_flag.status.code(), Some(0), "unresolved flag exit");
    let stdout = String::from_utf8(with_flag.stdout.clone()).expect("unresolved stdout utf-8");
    assert!(stdout.contains("Unresolved:"), "unresolved header: {stdout}");
    assert!(stdout.contains("src/app.ts: ./missing"), "unresolved pair: {stdout}");
    let without = codearch(&[scratch.path.as_os_str().to_os_string()]);
    assert_eq!(without.status.code(), Some(0), "plain index exit");
    let plain = String::from_utf8(without.stdout).expect("plain stdout utf-8");
    assert!(!plain.contains("Unresolved:"), "plain index printed unresolved: {plain}");
}
