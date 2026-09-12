"""What the git co-change signal changed, measured rather than assumed.

Renders every benchmark checkout twice -- once with `--no-git`, once with the
signal -- and reports the difference: how domains moved, which file pairs were
newly grouped or newly separated, how many of those groupings the import graph
already explained, and where the confidence bands landed.

This makes no quality claim. It says what changed; whether the change is an
improvement is what the blind A/B review (`cochange_review.py`) is for.

    python cochange_delta.py --out results-cochange
"""
import argparse
import itertools
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from check_import_index import parse_index
from run import CRATE, ROOT

REPOS = ("hono", "commerce", "typedi")
SIGNAL = re.compile(r"Git co-change:\s+(\d+) pairs from (\d+) commits")
EXAMPLES = 5


def render(binary, source, state, no_git):
    """One codearch run. Returns (index.json, imports.md text, stdout)."""
    state.mkdir(parents=True, exist_ok=True)
    cmd = [str(binary), str(source), "--out", str(state / "CODEBASE.md"),
           "--codearch-dir", str(state)]
    if no_git:
        cmd.append("--no-git")
    out = subprocess.run(cmd, capture_output=True, text=True, check=True).stdout
    index = json.loads((state / "index.json").read_text(encoding="utf-8"))
    return index, (state / "imports.md").read_text(encoding="utf-8"), out


def colocated(index):
    """Every unordered pair of files that share a domain."""
    pairs = set()
    for cluster in index["clusters"]:
        for a, b in itertools.combinations(sorted(cluster["files"]), 2):
            pairs.add((a, b))
    return pairs


def import_pairs(imports_text):
    """Unordered file pairs joined by a resolved import edge."""
    pairs = set()
    for target, importers in parse_index(imports_text).items():
        for importer in importers:
            pairs.add(tuple(sorted((target, importer))))
    return pairs


def bands(index):
    counts = {"high": 0, "medium": 0, "low": 0}
    for cluster in index["clusters"]:
        conf = cluster.get("confidence")
        if conf:
            counts[conf["band"]] += 1
    return counts


def compare(name, without, with_git):
    before, before_imports, _ = without
    after, after_imports, stdout = with_git
    signal = SIGNAL.search(stdout)
    before_pairs, after_pairs = colocated(before), colocated(after)
    grouped = after_pairs - before_pairs
    separated = before_pairs - after_pairs
    # The import index is identical in both arms; either copy answers the
    # question of whether a new grouping was already explained by structure.
    edges = import_pairs(after_imports)
    hidden = sorted(p for p in grouped if p not in edges)

    return {
        "repo": name,
        "cochange_pairs": int(signal.group(1)) if signal else 0,
        "commits_read": int(signal.group(2)) if signal else 0,
        "domains_without": len(before["clusters"]),
        "domains_with": len(after["clusters"]),
        "confidence_without": before["confidence"],
        "confidence_with": after["confidence"],
        "bands_without": bands(before),
        "bands_with": bands(after),
        "pairs_grouped": len(grouped),
        "pairs_separated": len(separated),
        "grouped_with_import_edge": len(grouped) - len(hidden),
        "grouped_without_import_edge": len(hidden),
        "hidden_examples": hidden[:EXAMPLES],
        "map_tokens_without": before["map_tokens"],
        "map_tokens_with": after["map_tokens"],
    }


def report_markdown(rows):
    lines = [
        "# Git co-change — what it changed",
        "",
        "Each checkout rendered twice, `--no-git` against the default. Pairs are",
        "unordered file pairs sharing a domain. A grouped pair with no import edge",
        "is coupling the import graph never saw; whether that is signal or noise is",
        "not decided here.",
        "",
        "| Repo | Commits | Pairs | Domains | Confidence | Grouped | Separated | Grouped w/o import |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        lines.append(
            f"| {r['repo']} | {r['commits_read']} | {r['cochange_pairs']} | "
            f"{r['domains_without']} → {r['domains_with']} | "
            f"{r['confidence_without']:.2f} → {r['confidence_with']:.2f} | "
            f"{r['pairs_grouped']} | {r['pairs_separated']} | {r['grouped_without_import_edge']} |"
        )
    lines += ["", "## Confidence bands", "", "| Repo | Without git | With git |", "|---|---|---|"]
    for r in rows:
        def fmt(b):
            return f"{b['high']} high, {b['medium']} medium, {b['low']} low"
        lines.append(f"| {r['repo']} | {fmt(r['bands_without'])} | {fmt(r['bands_with'])} |")

    lines += ["", "## Groupings the import graph does not explain", ""]
    for r in rows:
        lines.append(f"**{r['repo']}** — {r['grouped_without_import_edge']} pairs, first {EXAMPLES}:")
        lines.append("")
        if not r["hidden_examples"]:
            lines.append("- none")
        for a, b in r["hidden_examples"]:
            lines.append(f"- `{a}` ↔ `{b}`")
        lines.append("")
    return "\n".join(lines) + "\n"


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--out", type=Path, default=ROOT / "results-cochange")
    p.add_argument("--repos", nargs="*", default=list(REPOS))
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    subprocess.run(["cargo", "build", "--locked", "--release", "--bins"], cwd=CRATE, check=True)
    binary = CRATE / "target" / "release" / ("codearch" + (".exe" if os.name == "nt" else ""))

    rows = []
    with tempfile.TemporaryDirectory(prefix="codearch-cochange-") as tmp:
        for name in args.repos:
            source = (ROOT / "repos" / name).resolve()
            states = {arm: Path(tmp) / name / arm for arm in ("nogit", "git")}
            without = render(binary, source, states["nogit"], no_git=True)
            with_git = render(binary, source, states["git"], no_git=False)
            rows.append(compare(name, without, with_git))
            # Kept for the blind A/B review, which must read identical files.
            for arm, state in states.items():
                (args.out / f"{name}-{arm}-CODEBASE.md").write_text(
                    (state / "CODEBASE.md").read_text(encoding="utf-8"), encoding="utf-8")

    (args.out / "report.json").write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    text = report_markdown(rows)
    (args.out / "report.md").write_text(text, encoding="utf-8")
    # The report is UTF-8; a Windows console defaulting to cp1252 must not be
    # the reason a measurement run fails after it has already written its files.
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    print(text)


if __name__ == "__main__":
    main()
