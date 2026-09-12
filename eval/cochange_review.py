"""Blind A/B review of with-git vs without-git clusterings.

Both maps are rendered, cluster provenance (which arm produced which domain)
is stripped, and a model judges per-cluster coherence blind -- the same shape
as `labels-reference-review.json`. The deterministic delta report
(`cochange_delta.py`) says what changed; this says whether the change is an
improvement.

    python cochange_review.py --fixture --out results-cochange-review
    python cochange_review.py --live --out results-cochange-review

--fixture runs end to end with no network against the recorded fixture
(`cochange-review-fixture.json`); that is M2 exit criterion 5. --live renders
all three checkouts twice and calls OpenRouter; it spends provider money, so
it is gated the same way M1v2 criterion 4 is (OpenRouter returned HTTP 402 on
2026-09-11).
"""
import argparse
import hashlib
import json
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

from run import CRATE, ROOT

try:
    from cochange_delta import REPOS, render
except ImportError:  # pragma: no cover
    REPOS = ("hono", "commerce", "typedi")
    render = None

SEED = 24301
MODEL = "openai/gpt-4.1-mini"  # same reviewer as labels-reference-review.json
BATCH = 12

SYSTEM = """Review codebase-map domains before the two clustering variants are compared. You see a domain name, its one-line summary, and the full sorted member file list. Assess whether these files cohere as one subsystem: shared directory, shared concern, plausible dependency closure. Flag grab-bag groupings (unrelated directories with no connecting concern) and explain why. Do not invent source facts beyond the names shown. Return JSON {"reviews":[{"id":"...","verdict":"accept" or "review","reason":"..."}]}, one entry per item, in any order. This is an automated annotation review, not human ground truth."""

FIXTURE = ROOT / "cochange-review-fixture.json"


def blind_items(index_without, index_with, repo):
    """Clusters from both arms as uniform review units, provenance attached.

    Presentation is identical for both arms (name, summary, sorted files), so
    nothing in the item text identifies the arm. The arm lives only in the
    manifest, which is never sent to the model.
    """
    units = []
    for arm, index in (("without", index_without), ("with", index_with)):
        for cluster in index["clusters"]:
            units.append({
                "repo": repo,
                "arm": arm,
                "cluster": cluster["id"],
                "name": cluster["name"],
                "summary": cluster["summary"],
                "files": sorted(cluster["files"]),
            })
    return units


def blind(units, seed=SEED):
    """Shuffle units under blind ids. Returns (items, manifest)."""
    order = list(units)
    random.Random(seed).shuffle(order)
    items, manifest = [], {}
    for i, unit in enumerate(order):
        blind_id = f"item-{i:03d}"
        items.append({
            "id": blind_id,
            "repo": unit["repo"],
            "name": unit["name"],
            "summary": unit["summary"],
            "files": unit["files"],
        })
        manifest[blind_id] = {
            "repo": unit["repo"],
            "arm": unit["arm"],
            "cluster": unit["cluster"],
        }
    return items, manifest


def item_text(item):
    lines = [f"Item {item['id']} (repo: {item['repo']}):",
             f"Name: {item['name']}",
             f"Summary: {item['summary']}",
             "Files:"]
    lines += [f"- {f}" for f in item["files"]]
    return "\n".join(lines)


def fixture_backend(items):
    """Deterministic replay for the recorded fixture: no network, no render."""
    recorded = {r["id"]: r for r in json.loads(FIXTURE.read_text(encoding="utf-8"))["reviews"]}
    reviews = []
    for item in items:
        if item["id"] not in recorded:
            raise ValueError(f"Fixture has no recorded review for {item['id']}")
        reviews.append(recorded[item["id"]])
    usage = {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0, "cost": 0.0}
    return reviews, {"model": "fixture", "usage": usage}


def live_backend(items, model=MODEL):
    """Batched OpenRouter review. Raises RuntimeError on provider failure."""
    import urllib.request
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise RuntimeError("OPENROUTER_API_KEY is not configured; the live run is gated on provider credit")
    reviews, total_usage = [], {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0, "cost": 0.0}
    for start in range(0, len(items), BATCH):
        batch = items[start:start + BATCH]
        body = {"model": model, "temperature": 0, "seed": SEED, "max_tokens": 3500,
                "response_format": {"type": "json_object"},
                "messages": [{"role": "system", "content": SYSTEM},
                             {"role": "user", "content": "\n\n".join(item_text(i) for i in batch)}]}
        req = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions",
                                     data=json.dumps(body).encode(),
                                     headers={"Authorization": "Bearer " + key, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=120) as response:
                result = json.load(response)
        except Exception as exc:
            raise RuntimeError(f"OpenRouter review failed: {type(exc).__name__} {exc}") from None
        u = result.get("usage") or {}
        for k in total_usage:
            total_usage[k] += u.get(k, 0) or 0
        try:
            got = json.loads(result["choices"][0]["message"]["content"])["reviews"]
        except (KeyError, IndexError, TypeError, ValueError) as exc:
            raise RuntimeError(f"Reviewer did not return a reviews list: {exc}") from None
        seen = {r["id"] for r in got}
        want = {i["id"] for i in batch}
        if seen != want or any(r.get("verdict") not in ("accept", "review") for r in got):
            raise ValueError(f"Invalid review batch: got {sorted(seen)}, want {sorted(want)}")
        reviews.extend(got)
    return reviews, {"model": model, "usage": total_usage}


def score(items, manifest, reviews):
    """Accept rate per arm, overall and per repo. Pure function of the inputs."""
    by_id = {r["id"]: r for r in reviews}
    if set(by_id) != set(manifest):
        raise ValueError("Reviews do not cover exactly the blind items")
    arms = {}
    for blind_id, prov in manifest.items():
        key = (prov["arm"], prov["repo"])
        cell = arms.setdefault(key, {"items": 0, "accept": 0})
        cell["items"] += 1
        if by_id[blind_id]["verdict"] == "accept":
            cell["accept"] += 1
    rows = []
    for (arm, repo), cell in sorted(arms.items()):
        rows.append({"arm": arm, "repo": repo, "items": cell["items"],
                     "accept": cell["accept"], "accept_rate": cell["accept"] / cell["items"]})
    overall = {}
    for arm in ("without", "with"):
        cells = [c for (a, _), c in arms.items() if a == arm]
        n = sum(c["items"] for c in cells)
        a = sum(c["accept"] for c in cells)
        overall[arm] = {"items": n, "accept": a, "accept_rate": a / n if n else 1.0}
    return {"by_arm_repo": rows, "overall": overall,
            "delta_pp": 100 * (overall["with"]["accept_rate"] - overall["without"]["accept_rate"])}


def report_markdown(scored):
    lines = ["# Co-change blind A/B — per-cluster coherence", "",
             "Reviewer sees domain name, summary and member files only. Arm provenance is",
             "stripped before review; the manifest restores it for scoring.", "",
             "| Arm | Items | Accept | Rate |", "|---|---|---:|---:|"]
    for arm in ("without", "with"):
        o = scored["overall"][arm]
        lines.append(f"| {arm} | {o['items']} | {o['accept']} | {o['accept_rate']:.3f} |")
    lines += ["", f"Delta (with − without): {scored['delta_pp']:+.2f} pp.", "",
              "| Repo | Without | With |", "|---|---:|---:|"]
    repos = sorted({r["repo"] for r in scored["by_arm_repo"]})
    lookup = {(r["arm"], r["repo"]): r for r in scored["by_arm_repo"]}
    for repo in repos:
        def fmt(arm):
            c = lookup.get((arm, repo))
            return f"{c['accept']}/{c['items']}" if c else "—"
        lines.append(f"| {repo} | {fmt('without')} | {fmt('with')} |")
    return "\n".join(lines) + "\n"


def write_review(out, items, manifest, reviews, meta):
    reference = hashlib.sha256(json.dumps(items, sort_keys=True).encode()).hexdigest()
    review = {"reference_sha256": reference, "review_model": meta["model"],
              "usage": meta["usage"], "reviews": reviews,
              "human_reviewed": False, "candidate_outputs_visible_to_reviewer": False}
    (out / "items.json").write_text(json.dumps(items, indent=2) + "\n", encoding="utf-8")
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (out / "review.json").write_text(json.dumps(review, indent=2) + "\n", encoding="utf-8")
    scored = score(items, manifest, reviews)
    (out / "report.json").write_text(json.dumps(scored, indent=2) + "\n", encoding="utf-8")
    text = report_markdown(scored)
    (out / "report.md").write_text(text, encoding="utf-8")
    return scored, text


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--out", type=Path, default=ROOT / "results-cochange-review")
    p.add_argument("--repos", nargs="*", default=list(REPOS))
    mode = p.add_mutually_exclusive_group(required=True)
    mode.add_argument("--fixture", action="store_true", help="replay the recorded fixture, no network")
    mode.add_argument("--live", action="store_true", help="render both arms and call OpenRouter (spends credit)")
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    if args.fixture:
        data = json.loads(FIXTURE.read_text(encoding="utf-8"))
        items, manifest = data["items"], data["manifest"]
        reviews, meta = fixture_backend(items)
        scored, text = write_review(args.out, items, manifest, reviews, meta)
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        print(text)
        return

    if render is None:
        raise RuntimeError("cochange_delta could not be imported; the live run needs its render()")
    subprocess.run(["cargo", "build", "--locked", "--release", "--bins"], cwd=CRATE, check=True)
    binary = CRATE / "target" / "release" / ("codearch" + (".exe" if os.name == "nt" else ""))
    units = []
    with tempfile.TemporaryDirectory(prefix="codearch-coreview-") as tmp:
        for name in args.repos:
            source = (ROOT / "repos" / name).resolve()
            states = {arm: Path(tmp) / name / arm for arm in ("nogit", "git")}
            without = render(binary, source, states["nogit"], no_git=True)
            with_git = render(binary, source, states["git"], no_git=False)
            units.extend(blind_items(without[0], with_git[0], name))
    items, manifest = blind(units)
    try:
        reviews, meta = live_backend(items)
    except (RuntimeError, ValueError) as exc:
        # A provider failure is not a measurement. Say so in one line and stop
        # rather than freezing a traceback or a partial review into the output.
        print(f"Live run gated: {exc}", file=sys.stderr)
        raise SystemExit(1)
    _, text = write_review(args.out, items, manifest, reviews, meta)
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    print(text)


if __name__ == "__main__":
    main()
