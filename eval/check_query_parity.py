"""Gate 2 for the query interface: the shipped CLI must agree with the measured lookups.

For every repo used by `tasks-nav-gated.json` and `tasks-xlang.json`:

1. Run `codearch <repo> --codearch-dir <tmp>` (indexes only; outside the pinned
   checkout; afterwards `git -C <repo> status --porcelain` must print nothing).
2. For each nav task target, compare `codearch importers <target> --json`
   with `build_tasks_nav.importers(parse_index(imports.md), target)`.
3. For each xlang task view, compare `codearch callers <view> --json`
   with `run_agent.parse_routes(routes.md)[view]`.

Prints mismatches and a summary; exits 1 on any mismatch.
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import build_tasks_nav as B
from check_import_index import parse_index
from run import CRATE, ROOT
from run_agent import parse_routes


def binary():
    subprocess.run(["cargo", "build", "--locked", "--bins"], cwd=CRATE, check=True)
    return CRATE / "target" / "debug" / ("codearch" + (".exe" if os.name == "nt" else ""))


def query(exe, *args):
    out = subprocess.run([str(exe), *args], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def importers_equal(cli, expected):
    """CLI --json answer vs the eval walk's {path: depth} dict."""
    got = sorted((i["path"], i["depth"]) for i in cli["importers"])
    return got == sorted(expected.items())


def callers_equal(cli, expected):
    """CLI --json answer vs the eval's [caller paths] list."""
    got = sorted({c for m in cli["matches"] for c in m["callers"]})
    return got == sorted(expected)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--repos", nargs="*", default=None,
                    help="only check these repo dirs (default: every repo in the task files)")
    args = ap.parse_args()
    nav = json.loads((ROOT / "tasks-nav-gated.json").read_text(encoding="utf-8"))
    xlang = json.loads((ROOT / "tasks-xlang.json").read_text(encoding="utf-8"))
    exe = binary()
    checked = mismatches = 0
    rows = []
    with tempfile.TemporaryDirectory(prefix="codearch-parity-") as tmp:
        repos = args.repos or sorted({t["repo"] for t in nav} | {t["repo"] for t in xlang})
        for repo in repos:
            source = (ROOT / repo).resolve()
            state = Path(tmp) / source.name
            state.mkdir()
            subprocess.run([str(exe), str(source), "--codearch-dir", str(state)],
                           capture_output=True, check=True)
            dirty = subprocess.check_output(["git", "status", "--porcelain"],
                                            cwd=source, text=True)
            assert not dirty, f"{repo} dirty after indexed run:\n{dirty}"
            back = parse_index((state / "imports.md").read_text(encoding="utf-8"))
            routes_file = state / "routes.md"
            routes = parse_routes(routes_file.read_text(encoding="utf-8")) if routes_file.exists() else {}
            for t in [t for t in nav if t["repo"] == repo]:
                cli = query(exe, "importers", t["target"], "--json",
                            "--repo", str(source), "--codearch-dir", str(state))
                checked += 1
                if cli.get("target") != t["target"] or not importers_equal(cli, B.importers(back, t["target"])):
                    mismatches += 1
                    rows.append(f"MISMATCH importers {t['id']} {t['target']}")
            for t in [t for t in xlang if t["repo"] == repo]:
                cli = query(exe, "callers", t["view"], "--json",
                            "--repo", str(source), "--codearch-dir", str(state))
                checked += 1
                if cli.get("view") != t["view"] or not callers_equal(cli, routes.get(t["view"], [])):
                    mismatches += 1
                    rows.append(f"MISMATCH callers {t['id']} {t['view']}")
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    for r in rows:
        print(r)
    print(f"parity: {checked - mismatches}/{checked} lookups match "
          f"({mismatches} mismatches)")
    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
