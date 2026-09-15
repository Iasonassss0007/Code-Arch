"""Cross-language impact tasks: a Django view changes, which Angular files are affected?

The M1v2 import tasks were answered by one `importers(path)` lookup, so they
measure the import index, not navigation. Here the answer starts on the other
side of a language boundary: no import connects `src/documents/views.py` to
`tag.service.ts`. Only the URL does.

Ground truth, built without codearch:

  backend    Django's route table read with Python's `ast` from
             src/paperless/urls.py (Django itself pulls torch/ocrmypdf, too
             heavy to boot). Matching emulates Django: patterns in order,
             nested includes concatenate, first match wins, DRF router last.
  frontend   URLs each Angular file sends, harvested from its string
             constants (`resourceName`, `endpoint`, `baseUrl`, apiBaseUrl
             templates, `getResourceUrl(id, action)`). Review with --table.
  affected   files that send a request to the view, plus every file that
             imports one of them directly (madge 8 with tsconfig). Transitive
             closure was measured first and rejected: Angular's routing and
             app modules pull 23-166 files into every view, past any cap.

Gates follow build_tasks_nav/gate_tasks_nav: 2-20 answers, >=2 directories,
at least one answer reached only through an import (the URL hop is the step
grep cannot make, so one import hop suffices), both free adversaries below
0.35 F1 (path_guess gets the view class name too). Views whose answer set
equals an earlier view's are dropped: they are the same task twice.

    python build_tasks_xlang.py --table          # print view -> callers for review
    python build_tasks_xlang.py --out tasks-xlang.json
"""
import argparse
import ast
import json
import re
import subprocess
from collections import Counter, defaultdict
from pathlib import Path

import build_tasks_nav as B
import gate_tasks_nav as G

ROOT = Path(__file__).resolve().parent
REPO = ROOT / 'repos' / 'paperless-ngx'
URLS = REPO / 'src' / 'paperless' / 'urls.py'
UI = 'src-ui/src/'
QUERY = ('The HTTP API served by `{view}` in `{file}` is changing. List every frontend '
         'source file that depends on it: files that send requests to it, and files that '
         'directly import one of those.')


# ---------------------------------------------------------------- backend
def _converter(pattern):
    """Django path() converters -> regex; re_path patterns pass through."""
    return re.sub(r'<(?:\w+:)?\w+>', '[^/]+', pattern)


def routes(source=None):
    """[(regex, view class, python file)] in Django resolution order."""
    tree = ast.parse(source or URLS.read_text(encoding='utf-8'))
    modules = {}
    for node in tree.body:
        if isinstance(node, ast.ImportFrom) and node.module:
            for alias in node.names:
                modules[alias.asname or alias.name] = node.module
    router = []
    for node in ast.walk(tree):
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                and node.func.attr == 'register' and len(node.args) >= 2):
            router.append((node.args[0].value, node.args[1].id))

    def file_of(name):
        mod = modules.get(name)
        return f"src/{mod.replace('.', '/')}.py" if mod else None

    def view_name(node):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == 'as_view':
            node = node.func.value
        return node.id if isinstance(node, ast.Name) else None

    out = []

    def walk(items, prefix):
        for item in items:
            if isinstance(item, ast.Starred):  # *api_router.urls
                for p, v in router:
                    out.append((prefix + [f'{p}/'], v, file_of(v)))
                continue
            if not (isinstance(item, ast.Call) and getattr(item.func, 'id', None) in ('path', 're_path')):
                continue
            if not isinstance(item.args[0], ast.Constant):
                continue
            raw = item.args[0].value
            pat = raw if item.func.id == 're_path' else _converter(raw)
            target = item.args[1]
            if isinstance(target, ast.Call) and getattr(target.func, 'id', None) == 'include':
                inner = target.args[0]
                if isinstance(inner, ast.Tuple):
                    inner = inner.elts[0]
                if isinstance(inner, ast.List):
                    walk(inner.elts, prefix + [pat])
                continue
            name = view_name(target)
            if name:
                out.append((prefix + [pat], name, file_of(name)))

    urlpatterns = next(n.value for n in tree.body if isinstance(n, ast.Assign)
                       and getattr(n.targets[0], 'id', None) == 'urlpatterns')
    walk(urlpatterns.elts, [])
    return out


def resolve(url, table):
    """First route matching `url` (full path, e.g. 'api/tags/1/'), as Django does.

    Each level is a regex matched against what the previous level left; a
    view-level pattern ending in `$` must consume the rest, an include does not.
    """
    for levels, view, file in table:
        rest = url
        for pat in levels:
            m = re.match(pat.lstrip('^'), rest)
            if not m:
                break
            rest = rest[m.end():]
        else:
            return view, file
    return None


# --------------------------------------------------------------- frontend
_CONST = re.compile(r"""(?:resourceName|endpoint)\s*(?::\s*string)?\s*=\s*['"]([\w/-]+)['"]""")
_BASE = re.compile(r"""baseUrl\s*(?::\s*string)?\s*=\s*environment\.apiBaseUrl\s*\+\s*['"]([\w/-]+)['"]""")
_TEMPLATE = re.compile(r'`([^`]*)`')
_ACTION = re.compile(r"""getResourceUrl\(\s*([^,()]+?)\s*,\s*['"`]([^'"`]+)['"`]\s*\)""")


def endpoints(text):
    """Paths under /api/ one Angular file sends requests to (ids become 1)."""
    resource = next(iter(_CONST.findall(text)), None)
    base = next(iter(_BASE.findall(text)), None)
    urls = set()
    if re.search(r'extends Abstract\w*Service', text) and resource:
        urls |= {f'{resource}/', f'{resource}/1/'}
        for ident, action in _ACTION.findall(text):
            action = re.sub(r'\$\{[^}]*\}', '1', action)
            urls.add(f'{resource}/{action}/' if ident == 'null' else f'{resource}/1/{action}/')
    if base:
        urls.add(base if base.endswith('/') else base + '/')
    for body in _TEMPLATE.findall(text):
        body = (body.replace('${this.resourceName}', resource or '\0')
                    .replace('${this.endpoint}', resource or '\0')
                    .replace('${endpoint}', resource or '\0'))
        for marker in ('${environment.apiBaseUrl}', '${this.baseUrl}'):
            if marker in body:
                tail = body.split(marker, 1)[1]
                if marker == '${this.baseUrl}' and base:
                    tail = base + tail
                tail = re.sub(r'\$\{[^}]*\}', '1', tail)
                if tail and '\0' not in tail:
                    urls.add(tail)
    return {'api/' + u for u in urls}


def frontend(repo=REPO):
    out = {}
    for p in sorted((repo / UI).rglob('*.ts')):
        if p.name.endswith('.spec.ts'):
            continue
        urls = endpoints(p.read_text(encoding='utf-8'))
        if urls:
            out[p.relative_to(repo).as_posix()] = urls
    return out


# ------------------------------------------------------------------ tasks
def ui_graph():
    raw = json.loads((ROOT / 'oracle' / 'paperless-ui.json').read_text(encoding='utf-8'))
    return {UI + k: [UI + v for v in vs] for k, vs in raw.items()}


def callers_by_view(table, calls):
    views = defaultdict(set)
    unresolved = []
    for f, urls in calls.items():
        for u in sorted(urls):
            hit = resolve(u, table)
            if hit:
                views[hit].add(f)
            else:
                unresolved.append((f, u))
    return views, unresolved


def build(table, calls, graph):
    back = B.reverse(graph)
    paths = sorted(B.source_files('paperless-ngx'))
    counts = Counter(d for deps in graph.values() for d in deps)
    views, _ = callers_by_view(table, calls)
    tasks, rejected, seen = [], [], {}
    for (view, file), direct in sorted(views.items()):
        if not file:
            continue
        depth = {f: 0 for f in direct}
        for f in direct:
            for imp in back.get(f, ()):
                depth.setdefault(imp, 1)
        answers = sorted(depth)
        dirs = len({B.directory(a) for a in answers})
        why = None
        if not (B.MIN_ANSWER <= len(answers) <= B.MAX_ANSWER):
            why = f'{len(answers)} answers'
        elif max(depth.values()) < 1 or dirs < 2:
            why = f'depth {max(depth.values())}, {dirs} dirs'
        elif tuple(answers) in seen:
            why = f'same answers as {seen[tuple(answers)]}'
        # The adversary reads the view name as well as the file path.
        probe = {'target': f"{file.rsplit('/', 1)[0]}/{re.sub(r'(?<!^)(?=[A-Z])', '_', view).lower()}.py"}
        adversary = {'path_guess': round(G.f1(G.path_guess(probe, paths, len(answers)), answers), 3),
                     'hub_guess': round(G.f1(G.hub_guess(probe, paths, counts, len(answers)), answers), 3)}
        if not why and max(adversary.values()) > 0.35:
            why = f'adversary {adversary}'
        if why:
            rejected.append((view, why))
            continue
        seen[tuple(answers)] = view
        tasks.append({
            'id': f'paperless-xlang-{view.lower()}-{len(tasks)}',
            'repo': 'repos/paperless-ngx', 'tier': 4, 'kind': 'impact',
            'query': QUERY.format(view=view, file=file),
            'target': file, 'view': view,
            'expected_files': answers,
            'oracle': {'backend': 'ast route table, Django first-match order',
                       'frontend': 'harvested request URLs', 'imports': 'madge 8 --ts-config',
                       'relation': 'url_callers_plus_direct_importers',
                       'direct_callers': sorted(direct)},
            'gates': {'max_depth': max(depth.values()), 'directories': dirs, 'adversary_f1': adversary},
        })
    return tasks, rejected


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--out', type=Path, default=ROOT / 'tasks-xlang.json')
    ap.add_argument('--table', action='store_true', help='print view -> caller table and unresolved URLs')
    args = ap.parse_args()
    table = routes()
    calls = frontend()
    if args.table:
        views, unresolved = callers_by_view(table, calls)
        for (view, file), files in sorted(views.items()):
            print(f'{view} ({file})')
            for f in sorted(files):
                print(f'    {f}')
        print('\nunresolved:')
        for f, u in unresolved:
            print(f'    {u}  <- {f}')
        return
    rev = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
    tasks, rejected = build(table, calls, ui_graph())
    for view, why in rejected:
        print(f'reject {view}: {why}')
    for t in tasks:
        t['provenance'] = {'url': 'https://github.com/paperless-ngx/paperless-ngx.git', 'revision': rev,
                           'annotation': 'Django view -> Angular URL callers -> madge direct importers.'}
    args.out.write_text(json.dumps(tasks, indent=2) + '\n', encoding='utf-8')
    print(f'wrote {len(tasks)} tasks to {args.out}')


if __name__ == '__main__':
    main()
