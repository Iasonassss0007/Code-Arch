"""Score a codearch routes.md against the xlang oracle (view -> direct URL callers)."""
import argparse, json, re
from pathlib import Path
import build_tasks_xlang as X

def parse(text):
    out = {}
    for sec in text.split('\n## ')[1:]:
        view = re.match(r'`(\w+)`', sec).group(1)
        out[view] = set(re.findall(r'^- `([^`]+)`', sec, re.M))
    return out

def score(pred, gold):
    tp = fp = fn = 0
    rows = []
    for v in sorted(set(gold) | set(pred)):
        g, p = gold.get(v, set()), pred.get(v, set())
        tp += len(g & p); fp += len(p - g); fn += len(g - p)
        rows += [('FP', v, f) for f in sorted(p - g)] + [('FN', v, f) for f in sorted(g - p)]
    return tp, fp, fn, rows

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('routes', type=Path)
    args = ap.parse_args()
    pred = parse(args.routes.read_text(encoding='utf-8'))
    gold, _ = X.callers_by_view(X.routes(), X.frontend())
    gold = {v: f for (v, _), f in gold.items()}
    tp, fp, fn, rows = score(pred, gold)
    for kind, v, f in rows:
        print(kind, v, f)
    tasks = json.loads((X.ROOT / 'tasks-xlang.json').read_text(encoding='utf-8'))
    exact = sum(pred.get(t['view'], set()) == set(t['oracle']['direct_callers']) for t in tasks)
    print(f'P={tp/max(tp+fp,1):.2f} R={tp/max(tp+fn,1):.2f} tp={tp} fp={fp} fn={fn} '
          f'tasks_exact={exact}/{len(tasks)}')

if __name__ == '__main__':
    main()
