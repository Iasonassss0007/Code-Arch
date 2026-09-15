"""Offline M1v2 efficacy proxy: iterative grep vs the on-demand import index.

The paid M1v2 agent run is gated on provider credit. This measures the
question underneath it without any model: for transitive-impact tasks —
"the behavior of X is changing, what else needs review?" — is the answer
cheaper and more accurate to obtain through the artifacts codearch emits
(the map pointing at `.codearch/imports.md`) than through the best-effort
strategy a repository-grep agent executes (walk the import chain a round at
a time, search by module name, open and verify)?

Two deterministic arms, both given the full file inventory and the target:

  grep arm    iterative search-by-stem + open-and-verify. Each round searches
              the current node's stem, opens candidate files, and keeps those
              whose import lines actually reference the stem (false positives
              like symbol usages are discarded on open). Verified importers
              seed the next round. Bounded (15 searches, 40 opens); every
              search result line and opened file is charged.
  index arm   reads the map, then opens `.codearch/imports.md` once (charged)
              and walks importer edges breadth-first from the target.

Both arms answer with every file they verified/reached ("natural") and the
top-N by hop distance where N is the answer's cardinality ("cardinality-
controlled" — the established adversary convention from gate_tasks_nav.py,
isolating information-source quality from stopping-rule quality).

Pre-stated verdict rule, fixed before any run:

  offline-payoff = (index natural F1 >= grep natural F1 + 0.10)
                AND (index mean tokens <= grep mean tokens)

This is an information-access measurement, not an LLM agent run. It cannot
support a claim about coding agents; the paid M1v2 run remains the real
gate. It can falsify the artifact's economics cheaply: if the index does not
beat iterative grep here, serving it to a paid agent cannot be justified by
these tasks.

    python impact_proxy.py [--tasks tasks-nav-gated.json] [--out results-nav-proxy]
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import deque
from pathlib import Path

from build_tasks_nav import importers
from check_import_index import parse_index
from run import ROOT, CRATE, set_f1

ARROW = " ← "

# Action budgets for the grep arm. Generous relative to the 12-action real
# protocol on purpose: this measures the best-effort strategy, not a hurried
# one — the token charge, not the cap, is the finding. Exhaustion is recorded.
MAX_OPENS = 40
MAX_SEARCHES = 15
MAX_HITS = 50

TS_SPEC = re.compile(r"(?:from\s+|require\(\s*|import\(\s*|import\s+)['\"]([^'\"]+)['\"]")
PY_FROM = re.compile(r"^\s*from\s+([\w\.]+)\s+import", re.M)
PY_IMPORT = re.compile(r"^\s*import\s+([\w\.,\s]+)", re.M)
PY_SUFFIXES = {".py"}


def suffix(path):
    return "." + path.rsplit(".", 1)[-1].lower() if "." in path.rsplit("/", 1)[-1] else ""


def stem_of(path):
    name = path.rsplit("/", 1)[-1]
    return name.split(".", 1)[0].lower()


def specifier_refs(path, text):
    """Every module specifier `text` imports, as written."""
    out = []
    if suffix(path) in PY_SUFFIXES:
        out += [m for m in PY_FROM.findall(text)]
        for group in PY_IMPORT.findall(text):
            for name in group.split(","):
                name = name.strip().split(" as ")[0].strip()
                if name:
                    out.append(name)
    else:
        out += TS_SPEC.findall(text)
    return out


def references_stem(path, text, stem):
    """Does `text` import a module whose last path/dot segment equals `stem`?"""
    for spec in specifier_refs(path, text):
        last = spec.split(".")[-1] if suffix(path) in PY_SUFFIXES else spec.split("/")[-1]
        last = last.rsplit(".", 1)[0].lower()
        if last == stem:
            return True
    return False


def rank_by_hop(found):
    """(hop, path) — both arms rank the same way, on their own discovery."""
    return [p for p, _ in sorted(found.items(), key=lambda kv: (kv[1], kv[0]))]


def grep_arm(texts, inventory, target, expected_n):
    """Best-effort iterative grep walk. Returns findings, actions, deliveries."""
    found = {}
    examined = set()
    queue = deque([(target, 0)])
    opens = searches = 0
    delivered = []  # every text the policy consumed, for token accounting

    while queue and opens < MAX_OPENS and searches < MAX_SEARCHES:
        node, hop = queue.popleft()
        stem = stem_of(node)

        searches += 1
        hits = []
        for path in sorted(inventory):
            if path == node:
                continue
            for line_no, line in enumerate(texts[path].splitlines(), 1):
                if stem in line.lower():
                    hits.append({"path": path, "line": line_no, "text": line.strip()})
            if len(hits) >= MAX_HITS:
                break
        hits = hits[:MAX_HITS]
        delivered.append(json.dumps({"hits": hits, "truncated": len(hits) >= MAX_HITS},
                                   ensure_ascii=False))
        delivered.append(json.dumps({"tool": "search", "query": stem}))

        for hit in hits:
            path = hit["path"]
            if path in examined or opens >= MAX_OPENS:
                continue
            examined.add(path)
            opens += 1
            text = texts[path]
            delivered.append(json.dumps({"path": path, "source": text}))
            delivered.append(json.dumps({"tool": "open", "path": path}))
            if references_stem(path, text, stem):
                found[path] = hop + 1
                queue.append((path, hop + 1))

    return {
        "found": found,
        "opens": opens,
        "searches": searches,
        "exhausted": bool(queue) and (opens >= MAX_OPENS or searches >= MAX_SEARCHES),
        "delivered": delivered,
    }


def index_arm(back, target):
    """One index read, then a pure graph walk. No searching at all."""
    found = importers(back, target)
    delivered = [json.dumps({"tool": "open", "path": ".codearch/imports.md"})]
    return {
        "found": found,
        "opens": 1,
        "searches": 0,
        "exhausted": False,
        "delivered": delivered,
    }


def answer_of(found, expected_n, natural):
    ranked = rank_by_hop(found)
    return ranked if natural else ranked[:expected_n]


def score_task(task, grep, index, natural):
    expected = task["expected_files"]
    n = len(expected)
    g_answer = answer_of(grep["found"], n, natural)
    i_answer = answer_of(index["found"], n, natural)
    return {
        "id": task["id"],
        "repo": task["repo"],
        "expected": n,
        "grep_f1": set_f1(g_answer, expected),
        "index_f1": set_f1(i_answer, expected),
        "grep_answer": g_answer,
        "index_answer": i_answer,
        "grep_found": len(grep["found"]),
        "index_found": len(index["found"]),
        "adversary_f1": max(task["gates"]["adversary_f1"].values()),
    }


def load_texts(root):
    return {
        p.relative_to(root).as_posix(): p.read_text(encoding="utf-8", errors="replace")
        for p in sorted(root.rglob("*"))
        if p.is_file() and suffix(p.as_posix()) in {".ts", ".tsx", ".js", ".jsx",
                                                    ".mts", ".cts", ".mjs", ".cjs", ".py"}
        and ".git" not in p.parts
    }


def token_counter_via_helper():
    """cl100k_base over delivered texts, via eval-support (the production
    counter run.py already uses). Falls back to word count when the helper
    is unavailable so smoke runs still work; the report records which."""
    binary = CRATE / "target" / "debug" / ("eval-support.exe" if os.name == "nt" else "eval-support")
    if not binary.is_file():
        return (lambda texts: [len(t.split()) for t in texts]), "word-count fallback"

    def count(texts):
        p = subprocess.run([str(binary)], input=json.dumps({"op": "tokens", "texts": texts}),
                           text=True, capture_output=True, check=True)
        return json.loads(p.stdout)

    return count, "cl100k_base via eval-support"


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--tasks", type=Path, default=ROOT / "tasks-nav-gated.json")
    ap.add_argument("--out", type=Path, default=ROOT / "results-nav-proxy")
    args = ap.parse_args()
    tasks = json.loads(args.tasks.read_text(encoding="utf-8"))
    args.out.mkdir(parents=True, exist_ok=True)

    subprocess.run(["cargo", "build", "--locked", "--bins"], cwd=CRATE, check=True)
    binary = CRATE / "target" / "debug" / ("codearch" + (".exe" if os.name == "nt" else ""))
    count_tokens, counter_name = token_counter_via_helper()

    rows = []
    with tempfile.TemporaryDirectory(prefix="codearch-proxy-") as tmp:
        maps = {}
        indexes = {}
        for repo in sorted({t["repo"] for t in tasks}):
            source = (ROOT / repo).resolve()
            state = Path(tmp) / source.name
            state.mkdir()
            subprocess.run([str(binary), str(source), "--out", str(state / "CODEBASE.md"),
                            "--no-index", "--codearch-dir", str(state)],
                            capture_output=True, check=True)
            maps[repo] = (state / "CODEBASE.md").read_text(encoding="utf-8")
            indexes[repo] = (state / "imports.md").read_text(encoding="utf-8")

        for task in tasks:
            repo = task["repo"]
            source = (ROOT / repo).resolve()
            texts = load_texts(source)
            inventory = sorted(texts)

            grep = grep_arm(texts, inventory, task["target"], len(task["expected_files"]))
            back = parse_index(indexes[repo])
            index = index_arm(back, task["target"])

            for natural in (True, False):
                row = score_task(task, grep, index, natural)
                if not natural:
                    # Token accounting runs once per task, on the natural arm.
                    g_tokens = count_tokens(
                        [json.dumps({"query": task["query"]}), json.dumps({"files": inventory})]
                        + grep["delivered"])
                    i_tokens = count_tokens(
                        [json.dumps({"query": task["query"]}), json.dumps({"files": inventory}),
                         maps[repo], json.dumps({"map": maps[repo]})]
                        + index["delivered"] + [indexes[repo]])
                    row.update({
                        "grep_tokens": sum(g_tokens), "index_tokens": sum(i_tokens),
                        "grep_opens": grep["opens"], "index_opens": index["opens"],
                        "grep_searches": grep["searches"], "index_searches": index["searches"],
                        "grep_exhausted": grep["exhausted"],
                    })
                rows.append(row)

    natural_rows = [r for r in rows if "grep_tokens" in r or True]  # placeholder, split below
    # rows carry each task twice: natural=True scored without tokens, then
    # natural=False with tokens. Split by presence of the token fields.
    natural_rows = [r for r in rows if "grep_tokens" not in r]
    cardinality_rows = [r for r in rows if "grep_tokens" in r]

    def arm_means(rs, key):
        return sum(r[key] for r in rs) / len(rs)

    verdict = {
        "tasks": len(cardinality_rows),
        "counter": counter_name,
        "natural": {
            "grep_f1": arm_means(natural_rows, "grep_f1"),
            "index_f1": arm_means(natural_rows, "index_f1"),
        },
        "cardinality_controlled": {
            "grep_f1": arm_means(cardinality_rows, "grep_f1"),
            "index_f1": arm_means(cardinality_rows, "index_f1"),
        },
        "tokens": {
            "grep_mean": arm_means(cardinality_rows, "grep_tokens"),
            "index_mean": arm_means(cardinality_rows, "index_tokens"),
        },
        "actions": {
            "grep_opens_mean": arm_means(cardinality_rows, "grep_opens"),
            "grep_searches_mean": arm_means(cardinality_rows, "grep_searches"),
            "index_opens_mean": arm_means(cardinality_rows, "index_opens"),
            "index_searches_mean": arm_means(cardinality_rows, "index_searches"),
        },
        "grep_exhausted": sum(r["grep_exhausted"] for r in cardinality_rows),
    }
    verdict["offline_payoff"] = (
        verdict["natural"]["index_f1"] >= verdict["natural"]["grep_f1"] + 0.10
        and verdict["tokens"]["index_mean"] <= verdict["tokens"]["grep_mean"]
    )

    report = {"summary": verdict, "rows_natural": natural_rows, "rows": cardinality_rows}
    (args.out / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    v = verdict
    lines = [
        "# M1v2 offline efficacy proxy — iterative grep vs import index", "",
        "Deterministic information-access measurement, zero provider calls. Not an LLM agent run;",
        "the paid M1v2 run stays the real gate. Pre-stated rule: index natural F1 >= grep + 0.10",
        "AND index mean tokens <= grep mean tokens.", "",
        f"Tasks {v['tasks']} · counter {v['counter']} · grep budget {MAX_SEARCHES} searches / {MAX_OPENS} opens"
        f" (exhausted on {v['grep_exhausted']})", "",
        "| Arm | Natural F1 | Cardinality-controlled F1 | Mean tokens | Mean opens | Mean searches |",
        "|---|---:|---:|---:|---:|---:|",
        f"| Iterative grep | {v['natural']['grep_f1']:.3f} | {v['cardinality_controlled']['grep_f1']:.3f}"
        f" | {v['tokens']['grep_mean']:.0f} | {v['actions']['grep_opens_mean']:.1f}"
        f" | {v['actions']['grep_searches_mean']:.1f} |",
        f"| Map + index | {v['natural']['index_f1']:.3f} | {v['cardinality_controlled']['index_f1']:.3f}"
        f" | {v['tokens']['index_mean']:.0f} | {v['actions']['index_opens_mean']:.1f}"
        f" | {v['actions']['index_searches_mean']:.1f} |",
        "",
        f"Offline payoff: {'yes' if v['offline_payoff'] else 'no'}.", "",
        "Per-task: grep found vs index found vs expected, with the one-shot free-adversary F1",
        "the task was admitted against, so the iterative-vs-one-shot baseline difference is visible.", "",
        "| Task | Repo | Expected | Grep found | Index found | Grep F1 | Index F1 | Adversary F1 |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for r in cardinality_rows:
        repo_name = r["repo"].rsplit("/", 1)[-1]
        lines.append(f"| {r['id']} | {repo_name} | {r['expected']} | {r['grep_found']}"
                     f" | {r['index_found']} | {r['grep_f1']:.3f} | {r['index_f1']:.3f}"
                     f" | {r['adversary_f1']:.3f} |")
    (args.out / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    print("\n".join(lines[:16]))


if __name__ == "__main__":
    main()
