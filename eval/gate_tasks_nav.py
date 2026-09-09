"""Reject navigation tasks that a cheap heuristic already solves.

The first M1 task set failed because nobody asked whether the answer was
recoverable without doing the work. It was: 22 of 24 answers contained a query
token in their own path, and both arms are handed the complete file listing.

So every task now faces an explicit adversary before admission. The adversary
gets exactly what the agent gets for free -- the query and the inventory of
paths -- and no file contents. Whatever it scores is the floor a real result
has to clear, and a task it already solves measures nothing.

Two adversaries run, both free:

  path_guess   answer with every file sharing a path token with the target.
               This is the "same folder, similar name" heuristic, and it is
               what quietly solved the previous benchmark.

  hub_guess    answer with the most-imported files in the repository.
               Catches tasks whose answer is just "the popular files", which
               would reward a tool for restating PageRank.

A task is admitted only when both adversaries fall below --max-f1.

Usage:
    python gate_tasks_nav.py --tasks tasks-nav.json --out tasks-nav-gated.json
"""
import argparse
import json
import re
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent

# Path segments too common to carry information about which files relate.
STOPWORDS = {
    "src", "lib", "index", "test", "tests", "spec", "ts", "tsx", "js", "jsx",
    "app", "packages", "components", "utils", "types", "d",
}


def tokens(path):
    parts = re.split(r"[/\-_.]", path.lower())
    return {p for p in parts if p and p not in STOPWORDS and len(p) > 2}


def f1(predicted, expected):
    predicted, expected = set(predicted), set(expected)
    if not predicted or not expected:
        return 0.0
    hit = len(predicted & expected)
    if not hit:
        return 0.0
    precision = hit / len(predicted)
    recall = hit / len(expected)
    return 2 * precision * recall / (precision + recall)


def inventory(repo):
    root = ROOT / repo
    return sorted(
        p.relative_to(root).as_posix()
        for p in root.rglob("*")
        if p.is_file()
        and p.suffix in {".ts", ".tsx", ".js", ".jsx"}
        and ".git" not in p.parts
    )


def path_guess(task, paths, budget):
    """The `budget` files whose paths look most like the target's.

    Ranked and cardinality-matched on purpose. An adversary that returns every
    token match scores near zero on precision alone, which flatters the task
    without the heuristic being any weaker -- that mistake is what let the
    previous task set look hard when a one-line rank would have solved it.
    """
    want = tokens(task["target"])
    if not want:
        return []
    scored = [
        (len(tokens(p) & want), p)
        for p in paths
        if p != task["target"] and tokens(p) & want
    ]
    scored.sort(key=lambda x: (-x[0], x[1]))
    return [p for _, p in scored[:budget]]


def hub_guess(task, paths, graph_counts, budget):
    """The most-imported files, i.e. 'answer with the popular ones'."""
    ranked = sorted(paths, key=lambda p: (-graph_counts.get(p, 0), p))
    return [p for p in ranked if p != task["target"]][:budget]


def import_counts(repo):
    """How often each file is imported, straight from the madge oracle."""
    name = Path(repo).name
    raw = json.loads((ROOT / "oracle" / f"{name}.json").read_text(encoding="utf-8"))
    base = {"hono": "src", "typedi": "src", "commerce": ""}.get(name, "")
    counts = Counter()
    for deps in raw.values():
        for d in deps:
            d = d.replace("\\", "/")
            counts[f"{base}/{d}" if base else d] += 1
    return counts


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tasks", type=Path, default=ROOT / "tasks-nav.json")
    ap.add_argument("--out", type=Path, default=ROOT / "tasks-nav-gated.json")
    ap.add_argument("--max-f1", type=float, default=0.35)
    args = ap.parse_args()

    tasks = json.loads(args.tasks.read_text(encoding="utf-8"))
    paths_by_repo, counts_by_repo = {}, {}
    kept, dropped = [], []

    for task in tasks:
        repo = task["repo"]
        if repo not in paths_by_repo:
            paths_by_repo[repo] = inventory(repo)
            counts_by_repo[repo] = import_counts(repo)
        paths = paths_by_repo[repo]
        expected = task["expected_files"]

        # The adversary is handed the answer's size. That is generous, and
        # deliberately so: it is a floor, not a fair opponent.
        budget = len(expected)
        scores = {
            "path_guess": f1(path_guess(task, paths, budget), expected),
            "hub_guess": f1(hub_guess(task, paths, counts_by_repo[repo], budget), expected),
        }
        worst = max(scores.values())
        task.setdefault("gates", {})["adversary_f1"] = {
            k: round(v, 3) for k, v in scores.items()
        }

        if worst <= args.max_f1:
            kept.append(task)
        else:
            dropped.append((task["id"], scores))

    print(f"{len(tasks)} tasks -> {len(kept)} admitted, {len(dropped)} rejected")
    for tid, s in dropped:
        detail = ", ".join(f"{k}={v:.2f}" for k, v in s.items())
        print(f"  reject {tid}: {detail}")
    if kept:
        worst = max(max(t["gates"]["adversary_f1"].values()) for t in kept)
        print(f"admitted tasks: strongest adversary F1 = {worst:.2f} (cap {args.max_f1})")

    args.out.write_text(json.dumps(kept, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(kept)} tasks to {args.out}")


if __name__ == "__main__":
    main()
