"""Transitive-importer recall and precision of `codearch importers` against a ground-truth graph.

usage: python eval/recall_vs_ground_truth.py TRUTH.json REPO TARGET... [--codearch-bin BIN] [--codearch-dir DIR] [--out OUT.json]
TRUTH.json comes from eval/ts_ground_truth.mjs.
"""
import argparse, collections, json, subprocess
from pathlib import Path


def truth_importers(edges, target):
    back = collections.defaultdict(set)
    for src, dst, _ in edges:
        if src != dst:
            back[dst].add(src)
    seen, stack = set(), [target]
    while stack:
        for imp in back[stack.pop()]:
            if imp not in seen and imp != target:
                seen.add(imp)
                stack.append(imp)
    return seen


def codearch_importers(bin_, repo, ca_dir, target):
    cmd = [bin_, "importers", target, "--repo", str(repo), "--json"]
    if ca_dir:
        cmd += ["--codearch-dir", str(ca_dir)]
    out = subprocess.run(cmd, text=True, capture_output=True, check=True).stdout
    return {i["path"] for i in json.loads(out)["importers"]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("truth", type=Path)
    ap.add_argument("repo", type=Path)
    ap.add_argument("targets", nargs="+")
    ap.add_argument("--codearch-bin", default="codearch")
    ap.add_argument("--codearch-dir", type=Path)
    ap.add_argument("--out", type=Path)
    args = ap.parse_args()
    bin_path = Path(args.codearch_bin)
    bin_ = str(bin_path.resolve()) if bin_path.exists() else args.codearch_bin
    truth = json.loads(args.truth.read_text(encoding="utf-8"))
    rows = []
    for target in args.targets:
        t = truth_importers(truth["edges"], target)
        c = codearch_importers(bin_, args.repo, args.codearch_dir, target)
        hit = t & c
        rows.append({
            "target": target,
            "truth": len(t),
            "codearch": len(c),
            "both": len(hit),
            "recall": len(hit) / len(t) if t else 1.0,
            "precision": len(hit) / len(c) if c else 1.0,
            "missed": sorted(t - c),
            "extra": sorted(c - t),
        })
        r = rows[-1]
        print(f"{target}: truth {r['truth']} codearch {r['codearch']} recall {r['recall']:.4f} precision {r['precision']:.4f} missed {len(r['missed'])} extra {len(r['extra'])}")
    if args.out:
        args.out.write_text(json.dumps(rows, indent=1), encoding="utf-8")


if __name__ == "__main__":
    main()
