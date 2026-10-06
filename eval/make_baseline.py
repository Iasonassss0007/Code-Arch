"""Rebuild eval/baseline from the pinned clones."""
import hashlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import build_tasks_xlang as X
import score_routes

ROOT = Path(__file__).resolve().parent
CRATE = ROOT.parent
OUT = ROOT / "baseline"
MAX_BYTES = 5 * 1024 * 1024
WARN_BYTES = 1024 * 1024
REPOS = (
    ("hono", "https://github.com/honojs/hono.git", "e7b38ee42bfa41f20194ad20fb949b625346be62"),
    ("commerce", "https://github.com/vercel/commerce.git", "3761e52e60df9c6a316e067dbfd7032e494d3634"),
    ("typedi", "https://github.com/typestack/typedi.git", "7c6ac12a3de8507b1306f761942562f15375ed1f"),
    ("paperless-ngx", "https://github.com/paperless-ngx/paperless-ngx.git", "1f374cd65652ff8991f1d7e1efe49354876ac5c2"),
)


def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True, encoding="utf-8").strip()


def require_pin(name, sha):
    source = ROOT / "repos" / name
    if not (source / ".git").exists():
        raise SystemExit(f"missing clone: {source}")
    head = git(source, "rev-parse", "HEAD")
    if head != sha:
        raise SystemExit(f"{name} is {head}, pin is {sha}")
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=source, text=True, encoding="utf-8")
    if dirty:
        raise SystemExit(f"{name} is dirty:\n{dirty}")
    return source


def source_commit():
    rev = git(CRATE, "rev-parse", "HEAD")
    while True:
        names = subprocess.check_output(
            ["git", "diff-tree", "--no-commit-id", "--name-only", "-r", rev],
            cwd=CRATE, text=True, encoding="utf-8",
        ).splitlines()
        other = [n for n in names if n != "eval/make_baseline.py" and not n.startswith("eval/baseline/")]
        if other or not names:
            break
        parents = git(CRATE, "rev-parse", f"{rev}^@").split()
        if len(parents) != 1:
            break
        rev = parents[0]
    return git(CRATE, "rev-parse", "--short", rev)


def binary():
    subprocess.run(["cargo", "build", "--locked", "--bin", "codearch"], cwd=CRATE, check=True)
    return CRATE / "target" / "debug" / ("codearch" + (".exe" if os.name == "nt" else ""))


def write_bytes(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_file() and path.read_bytes() == data:
        return
    path.write_bytes(data)


def publish(directory, filename, data, sums):
    dest = directory / filename
    size = len(data)
    if size > MAX_BYTES:
        lines = data.count(b"\n") + (0 if not data or data.endswith(b"\n") else 1)
        digest = hashlib.sha256(data).hexdigest()
        sums.append(f"{digest}  {lines}  {size}  {filename}")
        if dest.exists():
            dest.unlink()
        print(f"{directory.name}/{filename} is {size} bytes. Wrote a sum instead.", file=sys.stderr)
        return
    if size > WARN_BYTES:
        print(f"{directory.name}/{filename} is {size} bytes.", file=sys.stderr)
    write_bytes(dest, data)


def normalize_report(text, state, name):
    stable = f"eval/baseline/{name}"
    variants = {str(state), str(state.resolve())}
    variants |= {v.replace("\\", "/") for v in list(variants)}
    for variant in sorted(variants, key=len, reverse=True):
        text = text.replace(variant, stable)
    text = text.replace("\\", "/")
    if ":/" in text.split("Wrote:", 1)[-1]:
        raise SystemExit(f"{name} report still contains an absolute path")
    return text.replace("\r\n", "\n").rstrip("\n") + "\n"


def index_repo(exe, name, sha):
    source = require_pin(name, sha)
    dest = OUT / name
    dest.mkdir(parents=True, exist_ok=True)
    sums = []
    with tempfile.TemporaryDirectory(prefix="codearch-baseline-") as tmp:
        state = Path(tmp) / name
        state.mkdir()
        proc = subprocess.run(
            [str(exe), str(source), "--codearch-dir", str(state)],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )
        if proc.returncode != 0:
            sys.stderr.write(proc.stderr)
            raise SystemExit(f"codearch failed on {name} with exit {proc.returncode}")
        require_pin(name, sha)
        publish(dest, "imports.md", (state / "imports.md").read_bytes(), sums)
        routes = state / "routes.md"
        if routes.is_file():
            publish(dest, "routes.md", routes.read_bytes(), sums)
        elif (dest / "routes.md").exists():
            (dest / "routes.md").unlink()
        publish(dest, "report.txt", normalize_report(proc.stdout, state, name).encode("utf-8"), sums)
    if sums:
        write_bytes(dest / "SUMS.txt", ("\n".join(sums) + "\n").encode("utf-8"))
    elif (dest / "SUMS.txt").exists():
        (dest / "SUMS.txt").unlink()
    keep = {"imports.md", "report.txt", "routes.md", "SUMS.txt"}
    for path in dest.iterdir():
        if path.is_file() and path.name not in keep:
            path.unlink()
    return dest


def route_score(routes_path):
    proc = subprocess.run(
        [sys.executable, str(ROOT / "score_routes.py"), str(routes_path)],
        cwd=CRATE, capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"score_routes.py exited {proc.returncode}")
    pred = score_routes.parse(routes_path.read_text(encoding="utf-8"))
    gold, _ = X.callers_by_view(X.routes(), X.frontend())
    gold = {view: files for (view, _), files in gold.items()}
    lines = [proc.stdout.rstrip("\n"), ""]
    for view in sorted(set(gold) | set(pred)):
        got, want = pred.get(view, set()), gold.get(view, set())
        lines.append(f"## {view}")
        lines.append(f"tp={len(want & got)} fp={len(got - want)} fn={len(want - got)}")
        for kind, files in (("TP", sorted(want & got)), ("FP", sorted(got - want)), ("FN", sorted(want - got))):
            lines.extend(f"{kind} {view} {path}" for path in files)
        lines.append("")
    return "\n".join(lines).rstrip("\n") + "\n", proc.returncode


def run_check(args):
    proc = subprocess.run(
        [sys.executable, *args],
        cwd=CRATE, capture_output=True, text=True, encoding="utf-8", errors="replace",
    )
    if proc.stderr.strip():
        sys.stderr.write(proc.stderr)
        if not proc.stderr.endswith("\n"):
            sys.stderr.write("\n")
    return proc


def readme(commit, parity, ceiling):
    pins = "\n".join(f"- {name} `{sha}`" for name, _url, sha in REPOS)
    clones = "\n".join(
        f"git clone --filter=blob:none {url} eval/repos/{name}\n"
        f"git -C eval/repos/{name} checkout {sha}"
        for name, url, sha in REPOS
    )
    return f"""# Parity baseline

This directory is the index-only output of codearch `{commit}` on four pinned repositories.
Diff a new index against these files to see what a later change moved.

The recorded commit is `git rev-parse --short HEAD` before the baseline commit.
The script keeps that hash when a later commit only adds `eval/baseline/` or `eval/make_baseline.py`.

## Pins

{pins}

The clones live in `eval/repos/` and are gitignored.
Each clone must be clean and at the pin above.

## Files

Each repository directory has `imports.md` and `report.txt`.
`routes.md` is present when codearch writes one.
`report.txt` is the index stdout.
The temporary `--codearch-dir` path in that stdout is rewritten to `eval/baseline/<repo>`.
`routes-score.md` is the paperless route score.

## Regenerate

From the repository root, clone each pin if `eval/repos/<name>` is missing.

```
{clones}
```

Then rebuild this directory.

```
python eval/make_baseline.py
```

The script exits if a clone is missing, dirty, or on the wrong commit.
The script indexes with `codearch eval/repos/<repo> --codearch-dir <tmp>`.
It does not write into the clone.
The import-index check writes its report to a temporary directory.
`eval/results-nav-ceiling` stays unchanged.

## Compare a later index

Diff the new index against the recorded file.

```
git diff --no-index eval/baseline/<repo>/imports.md <new>
```

Replace `<repo>` with one pin name.
Do the same for `routes.md` when that repository has one.

## Query parity

```
{parity.rstrip()}
```

## Import index

```
{ceiling.rstrip()}
```
"""


def main():
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    for name, _url, sha in REPOS:
        require_pin(name, sha)
    exe = binary()
    OUT.mkdir(parents=True, exist_ok=True)
    for name, _url, sha in REPOS:
        print(f"indexing {name}", flush=True)
        index_repo(exe, name, sha)
    routes = OUT / "paperless-ngx" / "routes.md"
    if not routes.is_file():
        raise SystemExit("paperless-ngx produced no routes.md")
    print("scoring routes", flush=True)
    score_text, _ = route_score(routes)
    write_bytes(OUT / "routes-score.md", score_text.encode("utf-8"))
    print("checking query parity", flush=True)
    parity = run_check([str(ROOT / "check_query_parity.py")])
    with tempfile.TemporaryDirectory(prefix="codearch-ceiling-") as tmp:
        print("checking import index", flush=True)
        ceiling = run_check([
            str(ROOT / "check_import_index.py"),
            "--tasks", str(ROOT / "tasks-nav-gated.json"),
            "--out", tmp,
        ])
    write_bytes(OUT / "README.md", readme(source_commit(), parity.stdout, ceiling.stdout).encode("utf-8"))
    for name, _url, sha in REPOS:
        require_pin(name, sha)
    if parity.returncode != 0:
        raise SystemExit(parity.returncode)
    if ceiling.returncode != 0:
        raise SystemExit(ceiling.returncode)


if __name__ == "__main__":
    main()
