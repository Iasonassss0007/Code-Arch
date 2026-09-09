"""Generate navigation tasks whose answers cannot be read off the file listing.

The first M1 task set measured `grep`: 22 of 24 answers contained a query token
in their own path, and both arms already receive the complete path listing, so
the baseline scored 46/48 with ~1.06 files opened and ~0.04 searches per
episode. There was no exploration to reduce and no headroom to win.

This generator builds `impact` tasks instead: "if this file changes, what else
needs review?" The answer is the set of transitive importers, which no amount
of path matching reveals and which a grep-only agent can only assemble by
walking the chain one round at a time.

Ground truth comes from madge, a third-party dependency-graph tool, so it is
independent of codearch's own resolver. Grading codearch against codearch would
prove nothing.

Three gates run before a task is admitted, none of which costs an LLM call:

  no stem leak     the target's filename stem must not appear in any answer path
  spread           answers must span >=2 directories, so "list the siblings"
                   is not a winning heuristic
  not trivial      the free lexical reference policy must fail the task
                   (applied by gate_tasks_nav.py, which runs after this)

Usage:
    python build_tasks_nav.py --out tasks-nav.json
"""
import argparse
import json
import subprocess
import sys
from collections import deque
from pathlib import Path

ROOT = Path(__file__).resolve().parent

# madge is run against the source subdirectory when one exists, so its paths
# need this prefix to become repository-relative.
BASES = {"hono": "src", "typedi": "src", "commerce": ""}

REPO_URLS = {
    "hono": "https://github.com/honojs/hono.git",
    "typedi": "https://github.com/typestack/typedi.git",
    "commerce": "https://github.com/vercel/commerce.git",
}

TIERS = {"hono": 1, "commerce": 2, "typedi": 3}

# An answer set below the floor is a lookup; above the ceiling it is a recall
# exercise no agent completes and every arm fails identically.
MIN_ANSWER = 2
MAX_ANSWER = 20

# Requiring depth >= 2 is what separates this from a single grep: at least one
# answer is reached only through another file.
MIN_DEPTH = 2

QUERY = (
    "The exported behavior of `{target}` is changing. "
    "List every source file that depends on it, directly or transitively, "
    "and would need review."
)


def load_graph(repo):
    """Return repo-relative forward edges: file -> [files it imports]."""
    raw = json.loads((ROOT / "oracle" / f"{repo}.json").read_text(encoding="utf-8"))
    base = BASES[repo]

    def rel(p):
        p = p.replace("\\", "/")
        return f"{base}/{p}" if base else p

    return {rel(k): [rel(v) for v in vs] for k, vs in raw.items()}


def reverse(graph):
    back = {k: set() for k in graph}
    for src, deps in graph.items():
        for dep in deps:
            back.setdefault(dep, set()).add(src)
    return back


def importers(back, target):
    """Transitive importers of `target`, with the hop count that reached each."""
    seen = {}
    queue = deque((n, 1) for n in back.get(target, ()))
    while queue:
        node, depth = queue.popleft()
        if node == target or node in seen:
            continue
        seen[node] = depth
        for parent in back.get(node, ()):
            if parent not in seen and parent != target:
                queue.append((parent, depth + 1))
    return seen


def stem(path):
    name = path.rsplit("/", 1)[-1]
    return name.split(".", 1)[0].lower()


def directory(path):
    return path.rsplit("/", 1)[0] if "/" in path else ""


def build(repo, existing):
    graph = load_graph(repo)
    back = reverse(graph)
    tasks = []

    for target in sorted(graph):
        if target not in existing:
            continue
        reached = importers(back, target)
        answers = sorted(a for a in reached if a in existing)
        if not (MIN_ANSWER <= len(answers) <= MAX_ANSWER):
            continue
        if max((reached[a] for a in answers), default=0) < MIN_DEPTH:
            continue

        # Triviality is decided by the measured adversary in gate_tasks_nav.py,
        # not by hand-tuned filters here. One principled gate beats three
        # plausible ones -- guessing which tasks are easy is exactly the mistake
        # that shipped the previous task set.
        tasks.append(
            {
                "id": f"{repo}-impact-{stem(target)}-{len(tasks)}",
                "repo": f"repos/{repo}",
                "tier": TIERS[repo],
                "kind": "impact",
                "query": QUERY.format(target=target),
                "target": target,
                "expected_files": answers,
                "oracle": {
                    "tool": "madge",
                    "version": "8",
                    "relation": "transitive_importers",
                },
                "gates": {
                    "max_depth": max(reached[a] for a in answers),
                    "directories": len({directory(a) for a in answers}),
                },
            }
        )
    return tasks


def revision(repo):
    return subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT / "repos" / repo, text=True
    ).strip()


def source_files(repo):
    root = ROOT / "repos" / repo
    return {
        p.relative_to(root).as_posix()
        for p in root.rglob("*")
        if p.is_file()
        and p.suffix in {".ts", ".tsx", ".js", ".jsx"}
        and ".git" not in p.parts
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, default=ROOT / "tasks-nav.json")
    ap.add_argument("--per-repo", type=int, default=8)
    ap.add_argument("--repos", nargs="*", default=["hono", "commerce", "typedi"])
    args = ap.parse_args()

    all_tasks = []
    for repo in args.repos:
        if not (ROOT / "oracle" / f"{repo}.json").exists():
            print(f"skip {repo}: no oracle graph", file=sys.stderr)
            continue
        existing = source_files(repo)
        found = build(repo, existing)
        # Prefer deeper chains: those are the ones a single grep cannot answer.
        found.sort(
            key=lambda t: (-t["gates"]["max_depth"], -len(t["expected_files"]), t["id"])
        )
        chosen = found[: args.per_repo]
        rev = revision(repo)
        for t in chosen:
            t["provenance"] = {
                "url": REPO_URLS[repo],
                "revision": rev,
                "annotation": "Transitive importers from madge 8; gates: no stem leak, >=2 directories, depth>=2.",
            }
        print(f"{repo}: {len(found)} candidates -> {len(chosen)} tasks")
        all_tasks.extend(chosen)

    args.out.write_text(json.dumps(all_tasks, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(all_tasks)} tasks to {args.out}")


if __name__ == "__main__":
    main()
