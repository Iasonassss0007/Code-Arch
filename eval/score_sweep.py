"""Multi-model sweep: derived baseline vs every downloaded registry model.

Scores all arms over the same frozen 20-cluster reference set with identical
v2 code in a single invocation, so no arm is advantaged by a change made
between runs. Models whose GGUF file is absent are reported as skipped with
the exact download command, never as a failure.

    python eval/score_sweep.py [--models ID ...] [--models-dir DIR]

Requires eval-support built with the llm feature for the model arms:

    cargo build --features llm

Writes eval/labels-sweep-report.{json,md}. Never touches
labels-real-report.* (frozen derived record) or labels-llm-report.*
(single-model M6-lite record).
"""
import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from run import ROOT, CRATE, bridge, evaluate_labels

REGISTRY = json.loads((ROOT / 'models-sweep.json').read_text())
METRICS = ('name_specificity', 'groundedness', 'sibling_collision_rate')


def download_command(entry):
    return f"huggingface-cli download {entry['repo']} {entry['file']} --local-dir models"


def predictions_of(response):
    """eval-support returns {labels, fell_back, summary_fell_back}."""
    if isinstance(response, dict):
        return (response['labels'], response.get('fell_back', 0),
                response.get('summary_fell_back', 0))
    return response, 0, 0


def check_references():
    clusters = json.loads((ROOT / 'clusters-real.json').read_text())
    review = json.loads((ROOT / 'labels-reference-review.json').read_text())
    actual = hashlib.sha256((ROOT / 'clusters-real.json').read_bytes()).hexdigest()
    if review['reference_sha256'] != actual:
        raise ValueError('Reference set changed after blind review')
    return clusters, actual


def helper_binary():
    helper = CRATE / 'target/debug' / ('eval-support.exe' if os.name == 'nt' else 'eval-support')
    if not helper.is_file():
        raise SystemExit(
            f'eval-support not found at {helper}. Build it first: '
            'cargo build --features llm')
    return helper


def score_arm(helper, clusters, model_path):
    """Run one arm through eval-support. model_path None means derived."""
    request = {'op': 'labels', 'clusters': clusters}
    if model_path is not None:
        request['model'] = str(model_path)
    started = time.perf_counter()
    response = bridge(helper, request)
    seconds = time.perf_counter() - started
    generated, fell_back, summary_fell_back = predictions_of(response)
    result = evaluate_labels(clusters, generated)
    return generated, result, fell_back, summary_fell_back, seconds


def choose_winner(rows):
    """Stated-before-the-run rule: specificity, then fewer summary fallbacks,
    then smaller file, then id. Operates on report model rows."""
    return min(rows, key=lambda r: (
        -r['name_specificity'], r['summary_fell_back'], r['size_mb'], r['id']))


def pct(x):
    return f'{x:.0%}'


def render_markdown(report):
    d = report['derived']
    lines = [
        '# Multi-model sweep vs derived labeler',
        '',
        f"{report['clusters']} frozen reference clusters, metric "
        f"{REGISTRY['metric']}. References are model-reviewed, not human "
        'ground truth, so these numbers compare labelers against a fixed '
        'standard rather than establishing absolute quality. '
        'Counts sit beside percentages: on 20 clusters one point is one cluster.',
        '',
        '| Model | Specificity | Groundedness | Collisions | Name FB | Summary FB | Time (s) |',
        '|---|---:|---:|---:|---:|---:|---:|',
        f"| derived (baseline) | {pct(d['name_specificity'])} ({d['specific_count']}/20) "
        f"| {pct(d['groundedness'])} | {pct(d['sibling_collision_rate'])} | - | - | {d['seconds']:.1f} |",
    ]
    for m in report['models']:
        lines.append(
            f"| {m['id']} | {pct(m['name_specificity'])} ({m['specific_count']}/20) "
            f"| {pct(m['groundedness'])} | {pct(m['sibling_collision_rate'])} "
            f"| {m['fell_back']}/20 | {m['summary_fell_back']}/20 | {m['seconds']:.0f} |")
    for s in report['skipped']:
        lines.append(f"| {s['id']} | skipped ({s['reason']}) | | | | | |")
    lines += [
        '',
        f"Rule winner: `{report['winner']}`. "
        f"Gap to derived specificity: {report['gap_to_derived_pp']:+.0f} pp.",
        '',
        '## Reproduce',
        '',
        '```powershell',
        'cargo build --features llm',
    ]
    for s in report['skipped']:
        lines.append(s['download'])
    lines += ['python eval/score_sweep.py', '```']
    return '\n'.join(lines) + '\n'


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--models', nargs='*', default=None,
                        help='registry ids to run (default: all)')
    parser.add_argument('--models-dir', type=Path, default=CRATE / 'models')
    args = parser.parse_args()

    wanted = set(args.models) if args.models else None
    entries = [e for e in REGISTRY['models'] if wanted is None or e['id'] in wanted]
    if wanted and len(entries) != len(wanted):
        unknown = wanted - {e['id'] for e in REGISTRY['models']}
        raise SystemExit(f'Unknown model ids: {sorted(unknown)}')

    clusters, reference_sha = check_references()
    helper = helper_binary()

    derived_labels, derived, _, _, derived_seconds = score_arm(helper, clusters, None)
    _ = derived_labels  # baseline rows are re-scored per model below, not stored
    derived_entry = {
        'name_specificity': derived['name_specificity'],
        'groundedness': derived['groundedness'],
        'sibling_collision_rate': derived['sibling_collision_rate'],
        'specific_count': sum(1 for r in derived['rows'] if r['specific']),
        'seconds': derived_seconds,
    }

    models, skipped = [], []
    for entry in entries:
        path = args.models_dir / entry['file']
        if not path.is_file():
            skipped.append({'id': entry['id'], 'reason': f'model file not found: {path.name}',
                            'download': download_command(entry)})
            print(f"skip {entry['id']}: {path.name} not present")
            continue
        print(f'scoring {entry["id"]} ({path.name}) ...')
        try:
            _, result, fell_back, summary_fell_back, seconds = score_arm(helper, clusters, path)
        except subprocess.CalledProcessError as exc:
            skipped.append({'id': entry['id'],
                            'reason': f'eval-support failed: {str(exc)[-200:]}',
                            'download': download_command(entry)})
            print(f'skip {entry["id"]}: eval-support failed')
            continue
        models.append({
            'id': entry['id'], 'file': entry['file'], 'license': entry['license'],
            'size_mb': entry['size_mb'], 'seconds': seconds,
            'fell_back': fell_back, 'summary_fell_back': summary_fell_back,
            'name_specificity': result['name_specificity'],
            'groundedness': result['groundedness'],
            'sibling_collision_rate': result['sibling_collision_rate'],
            'specific_count': sum(1 for r in result['rows'] if r['specific']),
            'rows': result['rows'],
        })
        print(f"  specificity {result['name_specificity']:.0%}, "
              f"name FB {fell_back}/20, summary FB {summary_fell_back}/20, {seconds:.0f}s")

    if models:
        winner = choose_winner(models)
        gap_pp = 100 * (winner['name_specificity'] - derived_entry['name_specificity'])
        winner_id = winner['id']
    else:
        winner_id, gap_pp = None, None

    report = {
        'reference_sha256': reference_sha,
        'clusters': len(clusters),
        'metric': REGISTRY['metric'],
        'human_reviewed': False,
        'blind_model_review': 'labels-reference-review.json',
        'derived': derived_entry,
        'models': models,
        'skipped': skipped,
        'winner': winner_id,
        'gap_to_derived_pp': gap_pp,
    }
    (ROOT / 'labels-sweep-report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    text = render_markdown(report)
    (ROOT / 'labels-sweep-report.md').write_text(text, encoding='utf-8')
    print()
    print(text)


if __name__ == '__main__':
    main()
