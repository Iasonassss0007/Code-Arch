"""Rescore recorded navigation checkpoints against the current task set.

Answers are fixed once an episode ran; only the oracle can change. When
expected_files is corrected (e.g. Commerce re-oracled with madge --ts-config),
this recomputes F1/exact for every recorded answer, keeps only tasks still in
the gated set, and reports per-arm means plus a paired bootstrap vs a baseline
arm. No provider calls.

    python rescore_nav.py results-nav-3arm results-nav-importers --out results-nav-rescored
"""
import argparse
import json
import random
import statistics
from collections import defaultdict
from pathlib import Path

from run import ROOT, set_f1


def rescore(rows, tasks):
    """Rows re-graded against `tasks`; rows for dropped tasks are removed."""
    by_id = {t['id']: t for t in tasks}
    out = []
    for r in rows:
        t = by_id.get(r['task'])
        if not t:
            continue
        expected = sorted(set(t['expected_files']))
        ok = not r['error']
        out.append({**r, 'expected': expected,
                    'f1': set_f1(r['answer'], expected) if ok else 0.0,
                    'correct': ok and r['answer'] == expected})
    return out


def paired(rows, arm, base, tiers=None, samples=5000, seed=0):
    """Per-task mean-F1 delta (arm - base) with a percentile bootstrap CI."""
    per = defaultdict(lambda: defaultdict(list))
    for r in rows:
        if tiers is None or r['tier'] in tiers:
            per[r['task']][r['arm']].append(r['f1'])
    ds = [statistics.mean(v[arm]) - statistics.mean(v[base]) for v in per.values() if v[arm] and v[base]]
    if not ds:
        return None
    rng = random.Random(seed)
    boot = sorted(statistics.mean(rng.choices(ds, k=len(ds))) for _ in range(samples))
    return {'tasks': len(ds), 'delta': statistics.mean(ds),
            'ci95': [boot[int(samples * 0.025)], boot[int(samples * 0.975) - 1]],
            'better': sum(d > 1e-9 for d in ds), 'worse': sum(d < -1e-9 for d in ds)}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('runs', nargs='+', type=Path)
    ap.add_argument('--tasks', type=Path, default=ROOT / 'tasks-nav-gated.json')
    ap.add_argument('--base', default='without_map')
    ap.add_argument('--out', type=Path, default=ROOT / 'results-nav-rescored')
    args = ap.parse_args()
    tasks = json.loads(args.tasks.read_text(encoding='utf-8'))
    rows, models = [], set()
    for run in args.runs:
        data = json.loads((ROOT / run / 'checkpoint.json').read_text(encoding='utf-8'))
        models.add(data['config']['model'])
        rows += rescore(data['episodes'], tasks)
    if len(models) != 1:
        raise ValueError(f'Runs mix models: {sorted(models)}')
    if len({(r['task'], r['trial'], r['arm']) for r in rows}) != len(rows):
        raise ValueError('Runs overlap on (task, trial, arm)')
    arms = sorted({r['arm'] for r in rows}, key=lambda a: (a != args.base, a))
    summary = {}
    for arm in arms:
        sub = [r for r in rows if r['arm'] == arm]
        summary[arm] = {'episodes': len(sub), 'exact': sum(r['correct'] for r in sub),
                        'mean_f1': statistics.mean(r['f1'] for r in sub),
                        'errors': sum(bool(r['error']) for r in sub),
                        'provider_tokens_per_episode': statistics.mean(
                            r['usage']['prompt_tokens'] + r['usage']['completion_tokens'] for r in sub)}
    tier_ids = sorted({r['tier'] for r in rows})
    deltas = {arm: {'all': paired(rows, arm, args.base), **{f'T{t}': paired(rows, arm, args.base, {t}) for t in tier_ids}}
              for arm in arms if arm != args.base}
    args.out.mkdir(parents=True, exist_ok=True)
    report = {'model': models.pop(), 'runs': [str(r) for r in args.runs], 'tasks': len(tasks),
              'base': args.base, 'summary': summary, 'paired': deltas}
    (args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    lines = [f"# Navigation runs rescored on the current task set", '',
             f"Model `{report['model']}`, {len(tasks)} tasks, runs: {', '.join(report['runs'])}.", '',
             '| Arm | Mean F1 | Exact | Provider tokens/ep | Errors |', '|---|---:|---:|---:|---:|']
    for arm, s in summary.items():
        lines.append(f"| {arm} | {s['mean_f1']:.3f} | {s['exact']}/{s['episodes']} | {s['provider_tokens_per_episode']:,.0f} | {s['errors']} |")
    lines += ['', f'Paired per-task F1 delta vs {args.base} (5000-sample bootstrap):', '',
              '| Arm | Slice | Tasks | Delta F1 | 95% CI | better/worse |', '|---|---|---:|---:|---|---:|']
    for arm, slices in deltas.items():
        for name, d in slices.items():
            if d:
                lines.append(f"| {arm} | {name} | {d['tasks']} | {d['delta']:+.3f} | [{d['ci95'][0]:+.3f}, {d['ci95'][1]:+.3f}] | {d['better']}/{d['worse']} |")
    (args.out / 'report.md').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    print('\n'.join(lines))


if __name__ == '__main__':
    main()
