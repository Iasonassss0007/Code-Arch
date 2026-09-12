"""Deterministic M1v2 ceiling: can the reverse import index alone answer each impact task?

For every gated task, walk `.codearch/imports.md` breadth-first from the target
and score the reached set against madge's answer. That is what a perfect agent
reading only the index would return -- no model, no network, no cost. It
separates "the index lacks the answer" from "the agent failed to use it", so it
runs before any paid agent episode.

    python check_import_index.py --tasks tasks-nav-gated.json --out results-nav-ceiling
"""
import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

from build_tasks_nav import BASES, importers
from run import CRATE, ROOT, set_f1

ARROW = " ← "


def parse_index(text):
    """imports.md -> {imported file: [importers]}."""
    back = {}
    for line in text.splitlines():
        if ARROW in line:
            target, rest = line.split(ARROW, 1)
            back[target] = rest.split(", ")
    return back


def score_task(back, task, base):
    """Score the index-only answer, raw and inside the directory madge scanned.

    `base` is empty now that madge scans whole checkouts, so both scores are
    equal. The scoped score stays so an oracle limited to a subdirectory can
    still be compared fairly.
    """
    predicted = sorted(importers(back, task["target"]))
    expected = task["expected_files"]
    scoped = [p for p in predicted if not base or p.startswith(base + "/")]
    hit = len(set(predicted) & set(expected))
    return {
        "id": task.get("id"),
        "predicted": predicted,
        "recall": hit / len(expected) if expected else 1.0,
        "f1": set_f1(predicted, expected),
        "f1_scoped": set_f1(scoped, expected),
        "missing": sorted(set(expected) - set(predicted)),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--tasks", type=Path, default=ROOT / "tasks-nav-gated.json")
    p.add_argument("--out", type=Path, default=ROOT / "results-nav-ceiling")
    p.add_argument("--gate", type=float, default=0.80,
                   help="mean recall below this means fix the resolver before paying for agent runs")
    args = p.parse_args()
    tasks = json.loads(args.tasks.read_text(encoding="utf-8"))
    args.out.mkdir(parents=True, exist_ok=True)

    subprocess.run(["cargo", "build", "--locked", "--bins"], cwd=CRATE, check=True)
    binary = CRATE / "target" / "debug" / ("codearch" + (".exe" if os.name == "nt" else ""))

    rows = []
    with tempfile.TemporaryDirectory(prefix="codearch-ceiling-") as tmp:
        for repo in sorted({t["repo"] for t in tasks}):
            source = (ROOT / repo).resolve()
            name = source.name
            revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
            if {t["provenance"]["revision"] for t in tasks if t["repo"] == repo} != {revision}:
                raise ValueError(f"{repo} is not at its pinned revision")
            state = Path(tmp) / name
            state.mkdir()
            subprocess.run([str(binary), str(source), "--out", str(state / "CODEBASE.md"),
                            "--no-index", "--codearch-dir", str(state)], capture_output=True, check=True)
            text = (state / "imports.md").read_text(encoding="utf-8")
            (args.out / f"{name}-imports.md").write_text(text, encoding="utf-8")
            back = parse_index(text)
            for t in tasks:
                if t["repo"] == repo:
                    row = score_task(back, t, BASES[name])
                    row["repo"] = name
                    row["expected"] = len(t["expected_files"])
                    row["adversary_f1"] = max(t["gates"]["adversary_f1"].values())
                    rows.append(row)

    def mean(key):
        return sum(r[key] for r in rows) / len(rows)

    summary = {
        "tasks": len(rows),
        "mean_recall": mean("recall"),
        "mean_f1": mean("f1"),
        "mean_f1_scoped": mean("f1_scoped"),
        "full_recall": sum(r["recall"] == 1.0 for r in rows),
        "mean_adversary_f1": mean("adversary_f1"),
        "gate": args.gate,
        "passes_gate": mean("recall") >= args.gate,
    }
    (args.out / "report.json").write_text(json.dumps({"summary": summary, "rows": rows}, indent=2) + "\n",
                                          encoding="utf-8")

    lines = [
        "# M1v2 deterministic ceiling — reverse import index", "",
        "Transitive importers walked from `.codearch/imports.md` alone, scored against madge. "
        "No model: this bounds what any agent can get from the index.", "",
        f"Tasks {summary['tasks']} · mean recall {summary['mean_recall']:.3f} · "
        f"mean F1 {summary['mean_f1']:.3f} · mean F1 in madge scope {summary['mean_f1_scoped']:.3f} · "
        f"full recall {summary['full_recall']}/{summary['tasks']} · "
        f"free-adversary mean F1 {summary['mean_adversary_f1']:.3f}", "",
        f"Gate (mean recall ≥ {args.gate:.2f}): {'pass' if summary['passes_gate'] else 'FAIL'}", "",
        "| Task | Expected | Reached | Recall | F1 | F1 in scope | Adversary F1 | Missing |",
        "|---|---:|---:|---:|---:|---:|---:|---|",
    ]
    for r in rows:
        missing = ", ".join(f"`{m}`" for m in r["missing"]) or "—"
        lines.append(f"| {r['id']} | {r['expected']} | {len(r['predicted'])} | {r['recall']:.3f} | "
                     f"{r['f1']:.3f} | {r['f1_scoped']:.3f} | {r['adversary_f1']:.3f} | {missing} |")
    (args.out / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines[:7]))


if __name__ == "__main__":
    main()
