"""Build the README's "What helps agents" charts from committed result files.

    python eval/make_readme_charts.py

Writes docs/images/agent-quality.svg and docs/images/agent-tokens.svg, and prints
the paired-delta table the README shows under the charts. Every number comes from
a result file named below; nothing is typed in by hand.

Arms are compared only inside one run (or one task set on the same model), so a
chart never pits a number against a baseline from a different run:

  navigation      results-nav-rescored (34 tasks, gemini-3.5-flash-lite)
  cross-language  results-xlang-routes (no help) + results-xlang-routes-fixed
                  (lookup tool), same tasks/model/prompt, same day
  cross-language map-in-prompt: only as a paired delta against its own run's
                  no-help arm (results-xlang-36flash-paid), never as a bar

Colors: the dataviz reference palette's first three categorical slots, validated
light and dark (scripts/validate_palette.js: all checks pass; light aqua is below
3:1 contrast, so every bar carries a visible value label and the README repeats
the numbers as a table).
"""
import json
import random
import statistics
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OUT = ROOT.parent / 'docs' / 'images'

# Entity -> fixed color slot. Order never changes between charts.
ARMS = [('lookup', 'Lookup tool'), ('map', 'Map in the prompt'), ('none', 'No help')]
LIGHT = {'lookup': '#2a78d6', 'map': '#eb6834', 'none': '#1baf7a'}
DARK = {'lookup': '#3987e5', 'map': '#d95926', 'none': '#199e70'}


def load(path):
    return json.loads((ROOT / path).read_text(encoding='utf-8'))


def per_task(episodes, arm, trials=None):
    """Task -> mean F1. With `trials`, only tasks where this arm finished that many."""
    by = defaultdict(list)
    for e in episodes:
        if e['arm'] == arm:
            by[e['task']].append(e['f1'])
    return {t: statistics.mean(v) for t, v in by.items() if trials is None or len(v) == trials}


def provider_tokens(episodes, arm):
    eps = [e for e in episodes if e['arm'] == arm]
    return statistics.mean(sum(c['usage']['prompt_tokens'] + c['usage']['completion_tokens']
                               for c in e['calls']) for e in eps)


def paired(a, b, seed=0, n=10000):
    """Mean per-task delta a-b over shared tasks, 95% bootstrap CI (same method as the analyses)."""
    tasks = sorted(set(a) & set(b))
    d = [a[t] - b[t] for t in tasks]
    rng = random.Random(seed)
    boot = sorted(statistics.mean(rng.choices(d, k=len(d))) for _ in range(n))
    return statistics.mean(d), (boot[int(0.025 * n)], boot[int(0.975 * n)]), len(tasks)


def numbers():
    nav = load('results-nav-rescored/report.json')
    s = nav['summary']
    routes = load('results-xlang-routes/report.json')['episodes']
    fixed = load('results-xlang-routes-fixed/report.json')['episodes']
    paid = load('results-xlang-36flash-paid/checkpoint.json')['episodes']

    x_none, x_lookup = per_task(routes, 'without_map'), per_task(fixed, 'routes_tool')
    groups = [
        ('Navigation: which files are affected if this file changes?',
         '34 tasks on Hono, Next.js Commerce, TypeDI · gemini-3.5-flash-lite',
         {'lookup': (s['importers_tool']['mean_f1'], s['importers_tool']['provider_tokens_per_episode']),
          'map': (s['with_map']['mean_f1'], s['with_map']['provider_tokens_per_episode']),
          'none': (s['without_map']['mean_f1'], s['without_map']['provider_tokens_per_episode'])}),
        ('Cross-language: a Django view changes, which Angular files are affected?',
         '8 tasks on paperless-ngx · gemini-3.6-flash',
         {'lookup': (statistics.mean(x_lookup.values()), provider_tokens(fixed, 'routes_tool')),
          'none': (statistics.mean(x_none.values()), provider_tokens(routes, 'without_map'))}),
    ]
    p = nav['paired']
    deltas = [
        ('Navigation', 'Lookup tool', p['importers_tool']['all']['delta'], p['importers_tool']['all']['ci95'], p['importers_tool']['all']['tasks']),
        ('Navigation', 'Map in the prompt', p['with_map']['all']['delta'], p['with_map']['all']['ci95'], p['with_map']['all']['tasks']),
        ('Cross-language', 'Lookup tool', *paired(x_lookup, x_none)),
        # The paid run stopped at 45/48; like its ANALYSIS.md, pair only tasks
        # where both arms finished both trials.
        ('Cross-language', 'Map in the prompt', *paired(per_task(paid, 'with_map', 2), per_task(paid, 'without_map', 2))),
    ]
    return groups, deltas


STYLE = """<style>
  svg {{ --surface:#fcfcfb; --text-1:#0b0b0b; --text-2:#52514e; --grid:#e4e3df;
         {light} }}
  @media (prefers-color-scheme: dark) {{
    svg {{ --surface:#1a1a19; --text-1:#ffffff; --text-2:#c3c2b7; --grid:#383835;
           {dark} }}
  }}
  text {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; }}
  .title {{ font-size:17px; font-weight:600; fill:var(--text-1); }}
  .group {{ font-size:14px; font-weight:600; fill:var(--text-1); }}
  .note {{ font-size:12px; fill:var(--text-2); }}
  .label {{ font-size:13px; fill:var(--text-1); }}
  .value {{ font-size:13px; font-weight:600; fill:var(--text-1); }}
  .tick {{ font-size:11px; fill:var(--text-2); }}
</style>"""


def esc(t):
    return t.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


def bar(x, y, w, h, fill):
    """Square at the baseline, 4px rounded data-end."""
    r = min(4, w / 2, h / 2)
    if w <= 0:
        return ''
    return (f'<path d="M{x:.1f},{y:.1f} H{x + w - r:.1f} Q{x + w:.1f},{y:.1f} {x + w:.1f},{y + r:.1f} '
            f'V{y + h - r:.1f} Q{x + w:.1f},{y + h:.1f} {x + w - r:.1f},{y + h:.1f} H{x:.1f} Z" '
            f'fill="var(--{fill})"/>')


def chart(title, subtitle, groups, value, vmax, ticks, fmt, path):
    width, left, right = 760, 170, 70
    plot = width - left - right
    bar_h, bar_gap, group_gap = 22, 6, 30
    light = ' '.join(f'--{k}:{v};' for k, v in LIGHT.items())
    dark = ' '.join(f'--{k}:{v};' for k, v in DARK.items())

    body = []
    y = 28
    body.append(f'<text class="title" x="16" y="{y}">{esc(title)}</text>')
    y += 20
    body.append(f'<text class="note" x="16" y="{y}">{esc(subtitle)}</text>')
    # Legend: always present for 3 series; identity is never color alone.
    y += 24
    lx = 16
    for key, name in ARMS:
        body.append(f'<rect x="{lx}" y="{y - 10}" width="12" height="12" rx="3" fill="var(--{key})"/>')
        body.append(f'<text class="label" x="{lx + 18}" y="{y}">{esc(name)}</text>')
        lx += 18 + 8 * len(name) + 28
    y += 22

    for gtitle, gnote, arms in groups:
        y += 14
        body.append(f'<text class="group" x="16" y="{y}">{esc(gtitle)}</text>')
        y += 18
        body.append(f'<text class="note" x="16" y="{y}">{esc(gnote)}</text>')
        y += 12
        top = y
        rows = [(k, n) for k, n in ARMS if k in arms]
        height = len(rows) * (bar_h + bar_gap) - bar_gap
        for t in ticks:  # recessive hairline grid behind the bars
            gx = left + plot * t / vmax
            body.append(f'<line x1="{gx:.1f}" y1="{top - 4}" x2="{gx:.1f}" y2="{top + height + 4}" '
                        f'stroke="var(--grid)" stroke-width="1"/>')
        for key, name in rows:
            v = value(arms[key])
            w = plot * v / vmax
            body.append(f'<text class="label" x="{left - 10}" y="{y + bar_h / 2 + 4.5:.1f}" text-anchor="end">{esc(name)}</text>')
            body.append(bar(left, y, w, bar_h, key))
            body.append(f'<text class="value" x="{left + w + 8:.1f}" y="{y + bar_h / 2 + 4.5:.1f}">{fmt(v)}</text>')
            y += bar_h + bar_gap
        y += group_gap - bar_gap
    # Shared axis ticks under the last group.
    for t in ticks:
        gx = left + plot * t / vmax
        body.append(f'<text class="tick" x="{gx:.1f}" y="{y - 8}" text-anchor="middle">{fmt(t)}</text>')
    y += 8

    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{y}" viewBox="0 0 {width} {y}" '
           f'role="img" aria-label="{esc(title)}">\n'
           f'<title>{esc(title)}</title>\n{STYLE.format(light=light, dark=dark)}\n'
           f'<rect width="100%" height="100%" rx="8" fill="var(--surface)"/>\n' + '\n'.join(body) + '\n</svg>\n')
    path.write_text(svg, encoding='utf-8')


def main():
    groups, deltas = numbers()
    OUT.mkdir(parents=True, exist_ok=True)
    chart('Answer quality (F1, higher is better)',
          'Share of the affected files found, balanced against wrong files listed.',
          groups, lambda a: a[0], 1.0, [0, 0.25, 0.5, 0.75, 1.0],
          lambda v: f'{v * 100:.0f}%', OUT / 'agent-quality.svg')
    # Benchmarks differ ~15x in absolute tokens, so each is indexed to its own
    # no-help arm (= 100%): one axis, no second scale.
    relative = [(g, n, {k: (v[0], v[1] / arms['none'][1]) for k, v in arms.items()})
                for g, n, arms in groups]
    chart('Tokens used per task, relative to no help (lower is cheaper)',
          'Model input + output tokens, averaged over all attempts. No help = 100%.',
          relative, lambda a: a[1], 2.0, [0, 0.5, 1.0, 1.5, 2.0],
          lambda v: f'{v * 100:.0f}%', OUT / 'agent-tokens.svg')
    print('| Benchmark | Setup | Change in F1 vs no help | 95% CI | Tasks |')
    print('|---|---|---:|---|---:|')
    for bench, arm, d, ci, n in deltas:
        print(f'| {bench} | {arm} | {d:+.3f} | [{ci[0]:+.2f}, {ci[1]:+.2f}] | {n} |')
    for gtitle, _, arms in groups:
        print(gtitle, {k: (round(v[0], 3), round(v[1])) for k, v in arms.items()})


if __name__ == '__main__':
    main()
