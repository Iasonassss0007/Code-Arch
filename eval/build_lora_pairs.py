"""Export frozen clusters as LoRA seed pairs for the M6 fine-tune path.

Emits one JSONL record per (cluster, acceptable name):

    {"input": {"dirs": [...], "top_symbols": [...], "entry_points": [...],
               "external_deps": [...], "siblings": [...]},
     "completion": {"name": <acceptable reference name>,
                    "summary": <derived-template sentence>} }

Prompts are stored structured, not rendered: the trainer renders them through
the same `build_prompt` the labeler serves at inference, so prompt wording
cannot drift between train and serve. Summaries are the derived template
(replicated here from `derive_summary` in src/label.rs), because the frozen
references carry acceptable *names* but no gold summaries — summary
supervision needs teacher-model distillation over harvested production
clusters, which is the stated next step, not something this seed pretends to
be. What this file fixes is the format and the name-head seed.

    python eval/build_lora_pairs.py [--out eval/lora-pairs.jsonl]
"""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def derive_summary(cluster):
    """Mirror of derive_summary in src/label.rs. Kept in sync by test."""
    n = cluster['file_count']
    noun = 'file' if n == 1 else 'files'
    dirs = cluster.get('dirs') or []
    where = f"{n} {noun} under `{dirs[0]}`" if dirs and dirs[0] else \
        f"{n} {noun} at the repository root"
    parts = [where]
    if cluster.get('top_symbols'):
        parts.append('key symbols ' + ', '.join(cluster['top_symbols'][:3]))
    if cluster.get('external_deps'):
        parts.append('uses ' + ', '.join(cluster['external_deps'][:2]))
    return '; '.join(parts) + '.'


def build_pairs(clusters):
    first_name = {c['id']: c['acceptable_names'][0] for c in clusters}
    pairs = []
    for c in clusters:
        siblings = [first_name[o['id']] for o in clusters
                    if o['group'] == c['group'] and o['id'] != c['id']]
        for name in c['acceptable_names']:
            pairs.append({
                'id': f"{c['id']}::{name}",
                'input': {
                    'dirs': c['dirs'],
                    'top_symbols': c['top_symbols'][:8],
                    'entry_points': c.get('entry_points', [])[:4],
                    'external_deps': c['external_deps'][:5],
                    'siblings': siblings[:12],
                },
                'completion': {'name': name, 'summary': derive_summary(c)},
            })
    return pairs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=ROOT / 'lora-pairs.jsonl')
    args = parser.parse_args()
    clusters = json.loads((ROOT / 'clusters-real.json').read_text())
    pairs = build_pairs(clusters)
    args.out.write_text('\n'.join(json.dumps(p) for p in pairs) + '\n', encoding='utf-8')
    print(f'{len(pairs)} pairs from {len(clusters)} clusters -> {args.out}')


if __name__ == '__main__':
    main()
