"""Train a LoRA adapter on the harvested corpus and gate the result.

One command for the whole M6 fine-tune path:

    python train_lora.py --base models/Qwen2.5-Coder-0.5B-Instruct-Q4_K_M.gguf

What it does, and what it refuses to pretend:

  convert    lora-train.jsonl (harvested, inference-shaped pairs) ->
            llama.cpp finetune-train.bin via convert_lora_to_gguf.py --jsonl
  train      llama.cpp `finetune` with the small-model constraint honored:
            CPU-only defaults, lora rank 16, 2 epochs, --seed fixed. GPU
            flags pass through when the user has one; nothing here requires it.
  gate       the trained adapter is applied to the frozen 20-cluster reference
            set through the production labeler path (eval-support `labels`
            with model + lora args) and scored by the same v2 metric. The
            rule is the one the M6 sweep stated before any training spend:
            the adapter must clear the 0.5B base (20% specificity) and the
            decision to adopt requires reaching derived's 75%. Below 75% the
            adapter is recorded, not adopted -- `--labeler derived` stays the
            default regardless.

Prerequisites (stated, not bundled): a llama.cpp checkout at --llamacpp
(default ../llama.cpp or LLAMA_CPP_DIR), Python deps for its scripts (numpy,
torch-based convert script needs no GPU), and the base GGUF in models/.
Everything runs offline.

    python train_lora.py [--base PATH] [--llamacpp DIR] [--epochs 2]
                         [--rank 16] [--out models/codearch-lora] [--dry-run]
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CRATE = ROOT.parent

sys.path.insert(0, str(ROOT))
from run import evaluate_labels  # noqa: E402

DEFAULT_BASE = "Qwen2.5-Coder-0.5B-Instruct-Q4_K_M.gguf"
TRAIN_JSONL = ROOT / "lora-train.jsonl"

# The M6 sweep numbers this gate restates: 0.5B base specificity 20%,
# incumbent 1.5B 60%, derived 75%. Stated before any training spend.
GATE_BEAT_BASE = 0.20
GATE_ADOPT = 0.75


def llamacpp_dir(explicit):
    if explicit:
        return Path(explicit)
    env = os.environ.get("LLAMA_CPP_DIR")
    if env:
        return Path(env)
    for cand in (CRATE.parent / "llama.cpp", CRATE / "llama.cpp",
                 Path.home() / "llama.cpp"):
        if cand.is_dir():
            return cand
    return None


def run(cmd, **kw):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def score_adapter(base, lora, lora_scale):
    """Frozen-set score through the production labeler path."""
    clusters = json.loads((ROOT / "clusters-real.json").read_text())
    binary = CRATE / "target" / "debug" / ("eval-support.exe" if os.name == "nt" else "eval-support")
    request = {"op": "labels", "clusters": clusters,
               "model": str(base)}
    if lora:
        request["lora"] = str(lora)
        request["lora_scale"] = lora_scale
    p = subprocess.run([str(binary)], input=json.dumps(request),
                       text=True, capture_output=True, check=True)
    response = json.loads(p.stdout)
    generated = response["labels"]
    result = evaluate_labels(clusters, generated)
    return response, result


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", type=Path, default=None,
                    help=f"base GGUF (default models/{DEFAULT_BASE})")
    ap.add_argument("--llamacpp", default=None, help="llama.cpp checkout")
    ap.add_argument("--epochs", type=int, default=2)
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--lora-scale", type=float, default=1.0)
    ap.add_argument("--out", type=Path, default=None, help="adapter prefix")
    ap.add_argument("--dry-run", action="store_true",
                    help="validate inputs and print the command sequence, train nothing")
    ap.add_argument("--skip-train", action="store_true",
                    help="gate an existing adapter only")
    ap.add_argument("--extra-train-args", nargs="*", default=[],
                    help="passed through to llama.cpp finetune (e.g. GPU flags)")
    args = ap.parse_args()

    if not TRAIN_JSONL.is_file():
        raise SystemExit("lora-train.jsonl missing; run: python eval/harvest_lora.py")

    base = args.base or (CRATE / "models" / DEFAULT_BASE)
    if not base.is_file():
        raise SystemExit(f"base model not found: {base}")
    lcp = llamacpp_dir(args.llamacpp)
    out = args.out or (CRATE / "models" / "codearch-lora")

    # Stage 0: gate the *untrained* base on the frozen set, so the adapter's
    # delta is measured against its own starting point, not a remembered one.
    print(f"gating untrained base {base.name} on the frozen 20-cluster set ...")
    _, base_result = score_adapter(base, None, 1.0)
    base_spec = base_result["name_specificity"]
    print(f"  base specificity {base_spec:.0%} "
          f"(recorded 0.5B sweep number: {GATE_BEAT_BASE:.0%})")

    if args.dry_run:
        print("dry run: command sequence that would run --")
        print(f"  python {lcp / 'convert_lora_to_gguf.py' if lcp else '<llamacpp>/convert_lora_to_gguf.py'} "
              f"--jsonl {TRAIN_JSONL} --out {out}-train.bin")
        print(f"  {lcp and (lcp / 'build/bin/finetune') or '<finetune>'} "
              f"--lora {base} --lora-out {out}.gguf --lora-rank {args.rank} "
              f"--epochs {args.epochs} --train-file {out}-train.bin --seed 42 "
              f"{' '.join(args.extra_train_args)}")
        print(f"  gate: eval-support labels with lora={out}.gguf, "
              f"adopt rule specificity >= {GATE_ADOPT:.0%}")
        return

    if not args.skip_train:
        if not lcp:
            raise SystemExit("no llama.cpp checkout found; pass --llamacpp or set LLAMA_CPP_DIR")
        converter = lcp / "convert_lora_to_gguf.py"
        finetune = lcp / "build" / "bin" / ("finetune.exe" if os.name == "nt" else "finetune")
        if not converter.is_file():
            raise SystemExit(f"converter missing: {converter} (needs llama.cpp with examples built)")
        if not finetune.is_file():
            raise SystemExit(f"finetune binary missing: {finetune} (build llama.cpp first)")
        if not lcp.joinpath(".git").exists():
            print(f"warning: {lcp} is not a git checkout; pin the commit yourself")

        run(["python", converter, "--jsonl", TRAIN_JSONL, "--out", f"{out}-train.bin"])
        run([finetune, "--lora", base, "--lora-out", f"{out}.gguf",
             "--lora-rank", args.rank, "--epochs", args.epochs,
             "--train-file", f"{out}-train.bin", "--seed", "42",
             *args.extra_train_args])

    # Stage 2: gate the trained adapter.
    adapter = Path(f"{out}.gguf")
    if not adapter.is_file():
        raise SystemExit(f"adapter not found after training: {adapter}")
    print(f"gating adapter {adapter.name} (scale {args.lora_scale}) ...")
    response, result = score_adapter(base, adapter, args.lora_scale)
    spec = result["name_specificity"]

    report = {
        "base": str(base), "adapter": str(adapter),
        "lora_scale": args.lora_scale, "epochs": args.epochs, "rank": args.rank,
        "clusters": 20,
        "base_specificity": base_spec,
        "specificity": spec,
        "groundedness": result["groundedness"],
        "sibling_collision_rate": result["sibling_collision_rate"],
        "name_fell_back": response.get("fell_back"),
        "summary_fell_back": response.get("summary_fell_back"),
        "beats_base": spec > base_spec,
        "adopts": spec >= GATE_ADOPT,
        "gate_rule": f"beats 0.5B base ({GATE_BEAT_BASE:.0%}); adopt needs derived parity ({GATE_ADOPT:.0%})",
        "rows": result["rows"],
    }
    out_md = ROOT / "lora-train-report.md"
    lines = [
        "# LoRA train gate", "",
        f"Base `{Path(report['base']).name}` + adapter `{adapter.name}` · "
        f"rank {args.rank} · epochs {args.epochs} · scale {args.lora_scale}", "",
        f"| Metric | Untrained base | Adapter |",
        f"|---|---:|---:|",
        f"| Name specificity | {base_spec:.0%} | {spec:.0%} |",
        f"| Groundedness | - | {report['groundedness']:.0%} |",
        f"| Sibling collisions | - | {report['sibling_collision_rate']:.0%} |",
        f"| Name fallbacks | - | {report['name_fell_back'] or 0}/20 |",
        f"| Summary fallbacks | - | {report['summary_fell_back'] or 0}/20 |",
        "",
        f"Beats its own base: {'yes' if report['beats_base'] else 'no'}. "
        f"Adopted as default: {'yes' if report['adopts'] else 'no'} — the adopt rule is "
        f"derived parity ({GATE_ADOPT:.0%}), and `derived` stays the default below it.",
        "",
    ]
    (ROOT / "lora-train-report.json").write_text(json.dumps(report, indent=2) + "\n",
                                                  encoding="utf-8")
    out_md.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines[:14]))


if __name__ == "__main__":
    main()
