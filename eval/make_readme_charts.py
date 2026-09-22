"""Build the README's "What helps agents" charts from committed result files.

    python eval/make_readme_charts.py

Writes docs/images/agent-quality.svg, agent-context.svg and agent-tokens.svg, and
prints the paired-delta table the README shows under the charts. Every number comes
from a result file named below; nothing is typed in by hand.

Two token measures, as the harness reports them:

  context   every item placed in the agent's context, counted once (task, file
            list, search results, opened files, tool answers), tokenized by
            eval-support's `tokens` op exactly as run_agent.py does. Navigation
            checkpoints store 0 here (the harness fills it at report time), so it
            is recomputed from the recorded traces; needs `cargo build --bins`.
  billed    provider input + output tokens, which re-count the conversation on
            every model call.

Arms are compared only inside one run (or one task set on the same model), so a
chart never pits a number against a baseline from a different run:

  navigation      results-nav-rescored (34 tasks, gemini-3.5-flash-lite)
  cross-language  results-xlang-routes (no help) + results-xlang-routes-callers
                  (lookup tool: route_callers now carries each caller's direct importers),
                  same tasks and model; the no-help prompt is unchanged
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


def nav_context_tokens(task_ids):
    """Mean context tokens per episode by arm, over the rescored navigation task set.

    Same count as run_agent.py's report step: each trace item tokenized once.
    """
    from run import CRATE, bridge
    helper = CRATE / 'target' / 'debug' / ('eval-support.exe' if __import__('os').name == 'nt' else 'eval-support')
    if not helper.exists():
        raise SystemExit(f'{helper} missing; run: cargo build --bins')
    rows = [e for run in ('results-nav-3arm', 'results-nav-importers')
            for e in load(f'{run}/checkpoint.json')['episodes'] if e['task'] in task_ids]
    counts = iter(bridge(helper, {'op': 'tokens', 'texts': [t['content'] for r in rows for t in r['trace']]}))
    by = defaultdict(list)
    for r in rows:
        by[r['arm']].append(sum(next(counts) for _ in r['trace']))
    return {arm: statistics.mean(v) for arm, v in by.items()}


def numbers():
    nav = load('results-nav-rescored/report.json')
    s = nav['summary']
    routes = load('results-xlang-routes/report.json')['episodes']
    fixed = load('results-xlang-routes-callers/report.json')['episodes']
    paid = load('results-xlang-36flash-paid/checkpoint.json')['episodes']
    task_ids = {t['id'] for t in load('tasks-nav-gated.json')}
    ctx = nav_context_tokens(task_ids)

    def context(episodes, arm):
        return statistics.mean(e['tokens'] for e in episodes if e['arm'] == arm)

    # Each arm: (mean F1, billed tokens per task, context tokens per task).
    x_none, x_lookup = per_task(routes, 'without_map'), per_task(fixed, 'routes_tool')
    groups = [
        ('Navigation: which files are affected if this file changes?',
         '34 tasks on Hono, Next.js Commerce, TypeDI · gemini-3.5-flash-lite',
         {'lookup': (s['importers_tool']['mean_f1'], s['importers_tool']['provider_tokens_per_episode'], ctx['importers_tool']),
          'map': (s['with_map']['mean_f1'], s['with_map']['provider_tokens_per_episode'], ctx['with_map']),
          'none': (s['without_map']['mean_f1'], s['without_map']['provider_tokens_per_episode'], ctx['without_map'])}),
        ('Cross-language: a Django view changes, which Angular files are affected?',
         '8 tasks on paperless-ngx · gemini-3.6-flash',
         {'lookup': (statistics.mean(x_lookup.values()), provider_tokens(fixed, 'routes_tool'), context(fixed, 'routes_tool')),
          'none': (statistics.mean(x_none.values()), provider_tokens(routes, 'without_map'), context(routes, 'without_map'))}),
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


def chart(title, subtitle, groups, value, vmax, ticks, fmt, path, label=None):
    """`label(key, arms)` overrides a bar's value text; `fmt` always formats ticks."""
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
            text = label(key, arms) if label else fmt(v)
            body.append(f'<text class="value" x="{left + w + 8:.1f}" y="{y + bar_h / 2 + 4.5:.1f}">{esc(text)}</text>')
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
    # Context: both benchmarks fit one absolute axis, so show real token counts,
    # with each bar's change against its own no-help arm spelled out.
    def change(key, arms):
        v, base = arms[key][2], arms['none'][2]
        text = f'{v / 1000:.1f}k tokens'
        if key != 'none':
            pct = round(100 * (v - base) / base)
            text += f' · {abs(pct)}% {"less" if pct < 0 else "more"}'
        return text
    top = max(a[2] for _, _, arms in groups for a in arms.values())
    vmax = 10000 * (int(top // 10000) + 1)
    chart('Context the agent reads per task (lower is better)',
          'Task, file list, search results, opened files and tool answers, each counted once.',
          groups, lambda a: a[2], vmax, list(range(0, vmax + 1, 10000)),
          lambda v: f'{v / 1000:.0f}k' if v else '0', OUT / 'agent-context.svg', label=change)
    # Billed tokens differ ~15x between benchmarks, so each is indexed to its own
    # no-help arm (= 100%): one axis, no second scale.
    relative = [(g, n, {k: (v[0], v[1] / arms['none'][1]) for k, v in arms.items()})
                for g, n, arms in groups]
    chart('Model tokens billed per task, relative to no help (lower is cheaper)',
          'Input + output tokens across every model call; the conversation is re-sent each step. No help = 100%.',
          relative, lambda a: a[1], 2.0, [0, 0.5, 1.0, 1.5, 2.0],
          lambda v: f'{v * 100:.0f}%', OUT / 'agent-tokens.svg')
    print('| Benchmark | Setup | Change in F1 vs no help | 95% CI | Tasks |')
    print('|---|---|---:|---|---:|')
    for bench, arm, d, ci, n in deltas:
        print(f'| {bench} | {arm} | {d:+.3f} | [{ci[0]:+.2f}, {ci[1]:+.2f}] | {n} |')
    for gtitle, _, arms in groups:
        print(gtitle, {k: (round(v[0], 3), round(v[1]), round(v[2])) for k, v in arms.items()})


if __name__ == '__main__':
    main()
