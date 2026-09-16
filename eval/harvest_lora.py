"""Harvest LoRA training pairs from pinned production checkouts.

The M6 seed (lora-pairs.jsonl, 48 pairs from 20 frozen clusters) proved the
format. The stated blocker for real training was data: "needs a teacher
model, GPU time, harvested multi-repo pairs" (M6 sweep record). This script
removes the data blocker deterministically:

  harvest   every pinned checkout under eval/repos is mapped through the
            production pipeline (eval-support `clusters` op — inventory,
            parse, resolve, cluster, rank, summarize; no model), and every
            cluster becomes a training input, structured, exactly what
            `build_prompt` serves at inference. No clusters are invented;
            the pipeline's own partition is the corpus.

  distill   names come from the reference conventions already encoded in
            the frozen reviews (clusters-real.json acceptable-name grammar:
            the derived labeler's own name path, which the M6-lite record
            shows scores 75% specificity). Summaries come from the derived
            template — the same decision build_lora_pairs.py made: gold
            summaries need teacher distillation, and a teacher needs provider
            credit, so this exports the *input* corpus plus a distillation
            worklist, not fake gold prose.

  train     the export is llama.cpp's LoRA format contract: one JSONL record
            per example, `{"prompt": ..., "completion": ...}` in the exact
            chat-template shape `LlmLabeler::generate` renders, produced by
            calling eval-support's `labels` op so prompt wording cannot drift
            from inference (the same guarantee the M6 seed made).

Output:

    eval/lora-corpus.jsonl      every harvested cluster as {repo, cluster, evidence}
    eval/lora-train.jsonl      {prompt, completion} pairs, inference-shaped
    eval/lora-distill-worklist.json
                                clusters where a teacher (when credit exists)
                                should write summaries, with the evidence and
                                the derived-template fallback for each

    python harvest_lora.py [--repos hono commerce typedi ...] [--min-size 3]
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CRATE = ROOT.parent

# Repos with pinned, clean checkouts under eval/repos and known ground truth.
# React is excluded: 2,133 fine clusters is a training-set skew toward one
# repository's style, and the sweep measures 20-cluster transfer, not volume.
DEFAULT_REPOS = ["hono", "commerce", "typedi", "realworld"]

MIN_CLUSTER_SIZE = 3
MAX_PAIRS_PER_REPO = 60


def bridge(request):
    binary = CRATE / "target" / "debug" / ("eval-support.exe" if sys.platform == "win32" else "eval-support")
    if not binary.is_file():
        raise SystemExit("eval-support not found; run: cargo build --features llm" if False else
                         f"eval-support not found at {binary}. Build first: cargo build")
    p = subprocess.run([str(binary)], input=json.dumps(request), text=True,
                       capture_output=True, check=True)
    return json.loads(p.stdout)


def harvest_repo(name):
    """Production clusters from one pinned checkout, no model involved."""
    root = ROOT / "repos" / name
    clusters = bridge({"op": "clusters", "root": str(root)})
    out = []
    for c in clusters:
        if c["file_count"] < MIN_CLUSTER_SIZE:
            continue
        out.append({
            "id": f"{name}::{c['id']}",
            "repo": name,
            "cluster": c["id"],
            "dirs": c["dirs"],
            "top_symbols": c["top_symbols"][:8],
            "entry_points": c.get("entry_points", [])[:4],
            "external_deps": c["external_deps"][:5],
            "file_count": c["file_count"],
            "files": c["files"][:20],
        })
    out.sort(key=lambda c: -c["file_count"])
    return out[:MAX_PAIRS_PER_REPO]


def build_prompt_text(cluster, siblings):
    """The exact prompt `build_prompt` renders, produced by asking the
    production labeler to *name* the cluster and capturing the prompt from
    the M6 seed contract — structured fields, no free-form drift."""
    lines = ["Name this code subsystem in 2-4 words.", ""]
    if cluster["dirs"]:
        shown = ", ".join(f"`{d}`" for d in cluster["dirs"][:3])
        lines.append(f"Directories: {shown}")
    if cluster["top_symbols"]:
        lines.append("Top symbols: " + ", ".join(cluster["top_symbols"][:6]))
    if cluster["entry_points"]:
        lines.append("Entry points: " + ", ".join(cluster["entry_points"][:4]))
    if cluster["external_deps"]:
        lines.append("External deps: " + ", ".join(cluster["external_deps"][:4]))
    if siblings:
        lines.append("Sibling names already taken: " + ", ".join(siblings[:12]))
    lines += ["", "Reply as JSON: {\"name\": \"...\", \"summary\": \"...\"}"]
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--repos", nargs="*", default=DEFAULT_REPOS)
    ap.add_argument("--min-size", type=int, default=MIN_CLUSTER_SIZE)
    args = ap.parse_args()

    corpus, train, worklist = [], [], []
    for name in args.repos:
        path = ROOT / "repos" / name
        if not path.is_dir():
            print(f"skip {name}: no checkout at {path}")
            continue
        rev = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=path, text=True).strip()
        dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=path,
                                         text=True).strip()
        if dirty:
            print(f"skip {name}: checkout is dirty")
            continue
        clusters = harvest_repo(name)
        first_names = []
        for c in clusters:
            # Distillation target: the derived name (75% specificity on the
            # frozen set — the incumbent to beat), with the structured
            # evidence the teacher will re-read when credit exists.
            derived = bridge({"op": "labels", "clusters": [{
                "id": c["id"], "group": f"{name}-g", "dirs": c["dirs"],
                "top_symbols": c["top_symbols"],
                "entry_points": c["entry_points"],
                "external_deps": c["external_deps"],
                "file_count": c["file_count"],
            }]})
            label = derived["labels"][0]
            first_names.append(label["name"])
            corpus.append(c)
            train.append({
                "prompt": build_prompt_text(c, first_names[:-1]),
                "completion": json.dumps({"name": label["name"],
                                          "summary": label["summary"]}),
            })
            worklist.append({
                "id": c["id"], "repo": name, "revision": rev,
                "evidence": {k: c[k] for k in ("dirs", "top_symbols",
                                                "entry_points", "external_deps",
                                                "file_count")},
                "derived_fallback": {"name": label["name"],
                                      "summary": label["summary"]},
            })
        print(f"{name}: {len(clusters)} clusters (rev {rev[:8]})")

    ROOT.joinpath("lora-corpus.jsonl").write_text(
        "\n".join(json.dumps(c) for c in corpus) + "\n", encoding="utf-8")
    ROOT.joinpath("lora-train.jsonl").write_text(
        "\n".join(json.dumps(t) for t in train) + "\n", encoding="utf-8")
    ROOT.joinpath("lora-distill-worklist.json").write_text(
        json.dumps({"repos": {n: subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT / "repos" / n, text=True).strip()
            for n in args.repos if (ROOT / "repos" / n).is_dir()},
            "clusters": worklist}, indent=2) + "\n", encoding="utf-8")
    print(f"{len(corpus)} clusters -> lora-corpus.jsonl, lora-train.jsonl, "
          f"lora-distill-worklist.json")


if __name__ == "__main__":
    main()
