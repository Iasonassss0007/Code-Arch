"""Head-to-head: local GGUF labeler vs the derived baseline.

Exit criterion 4 of the M0.5 design. Scores both labelers over the same frozen
20-cluster reference set and writes labels-llm-report.{json,md}.

The derived baseline is re-run here rather than read from labels-real-report.json
so both arms are scored by identical code in a single invocation. This script
never overwrites labels-real-report.*, which stays the frozen derived record.

    python eval/score_llm_labels.py [path/to/model.gguf]

Requires eval-support built with the llm feature:

    cargo build --features llm
"""
import hashlib
import json
import os
import sys
from pathlib import Path

from run import ROOT, CRATE, bridge, evaluate_labels

DEFAULT_MODEL = CRATE / 'models' / 'qwen2.5-coder-1.5b-instruct-q4_k_m.gguf'


def predictions_of(response):
    """eval-support returns {labels, fell_back}; older builds a bare array."""
    if isinstance(response, dict):
        return response['labels'], response.get('fell_back', 0)
    return response, 0


def main():
    model = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_MODEL
    if not model.is_file():
        raise SystemExit(f'Model not found: {model}')

    clusters = json.loads((ROOT / 'clusters-real.json').read_text())
    review = json.loads((ROOT / 'labels-reference-review.json').read_text())
    actual = hashlib.sha256((ROOT / 'clusters-real.json').read_bytes()).hexdigest()
    if review['reference_sha256'] != actual:
        raise ValueError('Reference set changed after blind review')

    helper = CRATE / 'target/debug' / ('eval-support.exe' if os.name == 'nt' else 'eval-support')

    derived, _ = predictions_of(bridge(helper, {'op': 'labels', 'clusters': clusters}))
    generated, fell_back = predictions_of(
        bridge(helper, {'op': 'labels', 'clusters': clusters, 'model': str(model)})
    )

    base = evaluate_labels(clusters, derived)
    cand = evaluate_labels(clusters, generated)

    metrics = ('name_specificity', 'groundedness', 'sibling_collision_rate')
    result = {
        'model': model.name,
        'reference_sha256': actual,
        'human_reviewed': False,
        'blind_model_review': 'labels-reference-review.json',
        'clusters': len(clusters),
        'fell_back': fell_back,
        'derived': {m: base[m] for m in metrics},
        'llm': {m: cand[m] for m in metrics},
        'derived_rows': base['rows'],
        'llm_rows': cand['rows'],
    }
    (ROOT / 'labels-llm-report.json').write_text(json.dumps(result, indent=2) + '\n')

    def pct(x):
        return f'{x:.0%}'

    lines = [
        '# Local model vs derived labeler',
        '',
        f"Model: `{model.name}`. {len(clusters)} frozen reference clusters. "
        f"{fell_back} of {len(clusters)} fell back to the derived name "
        f"(generation failure, ungrounded output, collision, or bad length).",
        '',
        'Higher is better for specificity and groundedness; lower is better for '
        'collisions. References are model-reviewed, not human ground truth, so '
        'these numbers compare two labelers against a fixed standard rather than '
        'establishing absolute quality.',
        '',
        '| Metric | Derived | Local model |',
        '|---|---:|---:|',
        f"| Name specificity | {pct(base['name_specificity'])} | {pct(cand['name_specificity'])} |",
        f"| Lexical groundedness | {pct(base['groundedness'])} | {pct(cand['groundedness'])} |",
        f"| Sibling collisions | {pct(base['sibling_collision_rate'])} | {pct(cand['sibling_collision_rate'])} |",
        '',
        '## Per-cluster names',
        '',
        '| Cluster | Derived | Local model | Changed |',
        '|---|---|---|---|',
    ]
    by_id = {r['id']: r for r in base['rows']}
    for row in cand['rows']:
        d = by_id[row['id']]['name']
        lines.append(
            f"| {row['id']} | {d} | {row['name']} | {'yes' if d != row['name'] else ''} |"
        )

    (ROOT / 'labels-llm-report.md').write_text('\n'.join(lines) + '\n')
    print('\n'.join(lines))


if __name__ == '__main__':
    main()
