"""Paired navigation benchmark. Run from any directory with Python 3.11+."""
import argparse
import hashlib
from functools import lru_cache
import json
import re
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CRATE = ROOT.parent
STOP = set('a an the to of in at by and or for with from is this that'.split())
BOILERPLATE = set('file files under at the repository root key symbols uses'.split())
# Every module extension codearch analyzes. The agent, the task builder and the
# gate must agree, or the index can list a dependent the agent may not answer.
SOURCE_SUFFIXES = frozenset({'.ts','.tsx','.js','.jsx','.mts','.cts','.mjs','.cjs','.py'})


@lru_cache(maxsize=100000)
def words(text):
    text = re.sub(r'([a-z0-9])([A-Z])', r'\1 \2', text)
    return set(re.findall(r'[a-z]+', text.lower())) - STOP


def score(query, text):
    return len(words(query) & words(text))


def bridge(binary, request):
    p = subprocess.run([str(binary)], input=json.dumps(request), text=True, capture_output=True, check=True)
    return json.loads(p.stdout)


def safe_file(root, rel):
    path = (root / rel).resolve()
    if not path.is_relative_to(root.resolve()) or not path.is_file():
        raise ValueError(f'Invalid source path: {rel}')
    return path


class Session:
    """Tool outputs are the only observations supplied to either policy."""
    def __init__(self, source, query, map_text, max_actions=12, map_files=None):
        self.source = source
        self.query = query
        self.paths = sorted(p.relative_to(source).as_posix() for p in source.rglob('*') if p.is_file() and p.suffix in SOURCE_SUFFIXES and '.git' not in p.parts)
        # On-demand files the map points at (e.g. .codearch/imports.md). Served
        # by exact path, and only to the arm that was given the map.
        self.map_files = dict(map_files or {})
        self.trace = []
        self.opened = set()
        self.searches = 0
        self.answer = []
        self.max_actions = max_actions
        task = {'query': query, 'files': self.paths, 'map': map_text}
        if self.map_files:
            task['map_files'] = sorted(self.map_files)
        self.observe('task', task)

    def observe(self, kind, value):
        self.trace.append({'kind':kind, 'content':json.dumps(value, ensure_ascii=False)})

    def action(self, action):
        self.observe('action', action)
        kind = action['tool']
        if kind == 'open':
            rel = action['path']
            if rel in self.map_files:
                self.opened.add(rel)
                self.observe('open', {'path':rel, 'source':self.map_files[rel]})
            elif rel in self.paths:
                self.opened.add(rel)
                self.observe('open', {'path':rel, 'source':safe_file(self.source,rel).read_text(encoding='utf-8')})
            else:
                raise ValueError('Only inventoried source files may be opened')
        elif kind == 'search':
            self.searches += 1
            query = action['query']
            hits = []
            for rel in self.paths:
                for line, text in enumerate(safe_file(self.source,rel).read_text(encoding='utf-8').splitlines(), 1):
                    weight = score(query, text)
                    if weight:
                        hits.append({'path':rel, 'line':line, 'text':text, 'score':weight})
            hits.sort(key=lambda x:(-x['score'], x['path'], x['line']))
            self.observe('search', {'hits':hits[:50], 'truncated':len(hits)>50})
        elif kind == 'answer':
            answer = action['files']
            if not isinstance(answer, list) or not all(isinstance(x,str) and x in self.paths for x in answer):
                raise ValueError('Answer must contain inventoried paths')
            self.answer = sorted(set(answer))
            return True
        else:
            raise ValueError(f'Unknown tool: {kind}')
        return False


def reference_policy(session):
    """One-hop lexical navigator, no access to expected answers or index.json."""
    initial = json.loads(session.trace[0]['content'])
    candidates = []
    for line in initial['map'].splitlines():
        match = re.match(r'- `([^`]+)`', line)
        if match and match[1] in session.paths:
            candidates.append((score(session.query,line),match[1]))
    candidates.sort(key=lambda x:(-x[0],x[1]))
    if not candidates or candidates[0][0] == 0:
        session.action({'tool':'search','query':session.query})
        hits = json.loads(session.trace[-1]['content'])['hits']
        candidates = [(h['score'],h['path']) for h in hits]
    if candidates:
        chosen = candidates[0][1]
        session.action({'tool':'open','path':chosen})
        session.action({'tool':'answer','files':[chosen]})
    else:
        session.action({'tool':'answer','files':[]})


def external_policy(session, command):
    # Adapter must return one JSON action; each episode starts with a fresh transcript.
    for _ in range(session.max_actions):
        p = subprocess.run(command, input=json.dumps({'protocol':1,'messages':session.trace}),
                           text=True,capture_output=True,timeout=120,check=True,cwd=ROOT)
        if session.action(json.loads(p.stdout)):
            return
    session.observe('failure','action budget exhausted')


def evaluate_labels(clusters, predictions):
    by_id = {p['id']:p for p in predictions}
    if len(by_id) != len(predictions) or set(by_id) != {c['id'] for c in clusters}:
        raise ValueError('Label predictions must contain each cluster ID exactly once')
    rows = []
    for c in clusters:
        p = by_id[c['id']]
        name = ' '.join(sorted(words(p['name'])))
        siblings = [by_id[s['id']]['name'] for s in clusters if s['group']==c['group'] and s['id']!=c['id']]
        collision = any(words(s)==words(p['name']) for s in siblings)
        allowed = words(' '.join(c['dirs']+c['top_symbols']+c['external_deps'])) | BOILERPLATE
        aliases = c.get('grounding_aliases',{})
        content = words(p['name']+' '+p['summary'])
        unsupported = sorted(w for w in content if w not in allowed and aliases.get(w) not in allowed)
        specific = name in [' '.join(sorted(words(n))) for n in c['acceptable_names']] and not collision
        rows.append({'id':c['id'], **p, 'specific':specific,'collision':collision,
                     'grounded':not unsupported,'unsupported_tokens':unsupported})
    return {'count':len(rows), 'name_specificity':sum(r['specific'] for r in rows)/len(rows),
            'groundedness':sum(r['grounded'] for r in rows)/len(rows),
            'sibling_collision_rate':sum(r['collision'] for r in rows)/len(rows),'rows':rows}


def set_f1(predicted, expected):
    """Overlap F1 between an answer set and the oracle set.

    Impact tasks have 2-20 correct files, and exact set match would score every
    arm zero -- a floor effect, the mirror of the ceiling effect that made the
    file-location benchmark unable to separate the arms. Partial credit is what
    lets a real difference show up.
    """
    predicted, expected = set(predicted), set(expected)
    if not predicted or not expected:
        return 0.0
    hit = len(predicted & expected)
    if not hit:
        return 0.0
    precision = hit/len(predicted)
    recall = hit/len(expected)
    return 2*precision*recall/(precision+recall)


def summarize(rows):
    result = {}
    for arm in ['without_map','with_map']:
        subset = [r for r in rows if r['arm']==arm]
        result[arm] = {'episodes':len(subset), 'correct':sum(r['correct'] for r in subset)}
        result[arm]['accuracy'] = result[arm]['correct']/len(subset)
        # Older result files predate per-episode F1; fall back to exact match.
        result[arm]['mean_f1'] = sum(r.get('f1', float(r['correct'])) for r in subset)/len(subset)
        for key in ['tokens','files_opened','searches']:
            result[arm][key] = sum(r[key] for r in subset)
    base, mapped = result['without_map'], result['with_map']
    result['accuracy_delta_pp'] = 100*(mapped['accuracy']-base['accuracy'])
    result['f1_delta'] = mapped['mean_f1']-base['mean_f1']
    result['helps_at_equal_or_better_f1'] = mapped['mean_f1'] >= base['mean_f1'] and mapped['tokens'] < base['tokens']
    result['token_savings_percent'] = 100*(base['tokens']-mapped['tokens'])/base['tokens'] if base['tokens'] else None
    result['helps_at_equal_or_better_accuracy'] = mapped['accuracy'] >= base['accuracy'] and mapped['tokens'] < base['tokens']
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=ROOT/'results')
    parser.add_argument('--tasks', type=Path, default=ROOT/'tasks.json')
    parser.add_argument('--agent-command', help='JSON argv array for a trusted adapter; omitted runs lexical proxy')
    parser.add_argument('--label-predictions',type=Path,help='JSON array of {id,name,summary} from another labeler')
    args = parser.parse_args()
    args.tasks = args.tasks.resolve()
    command = json.loads(args.agent_command) if args.agent_command else None
    if command is not None and (not isinstance(command,list) or not command or not all(isinstance(x,str) for x in command)):
        raise ValueError('--agent-command must be a nonempty JSON argv array')
    subprocess.run(['cargo','build','--locked','--bins'],cwd=CRATE,check=True)
    suffix = '.exe' if __import__('os').name=='nt' else ''
    binary = CRATE/'target'/'debug'/('codearch'+suffix)
    helper = CRATE/'target'/'debug'/('eval-support'+suffix)
    tasks = json.loads(args.tasks.read_text())
    assert tasks and len({t['id'] for t in tasks})==len(tasks), 'Duplicate or empty task IDs'
    clusters = json.loads((ROOT/'clusters.json').read_text())
    rows, maps = [], {}
    # Build artifacts outside the corpus; both arms see exactly the same source tree.
    with tempfile.TemporaryDirectory(prefix='codearch-m1-') as tmp:
        for repo in sorted({t['repo'] for t in tasks}):
            source = (ROOT/repo).resolve()
            pinned = {t['provenance']['revision'] for t in tasks if t['repo']==repo and 'provenance' in t}
            if pinned:
                actual = subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,text=True).strip()
                if pinned != {actual} or subprocess.check_output(['git','status','--porcelain'],cwd=source,text=True).strip():
                    raise ValueError('Benchmark repository must be clean and at its pinned revision')
            target = Path(tmp)/(source.name+'.md')
            state = Path(tmp)/(source.name+'-codearch')
            # --codearch-dir keeps imports.md out of the corpus checkout, same
            # pattern as run_agent.py. The map text is unchanged: it always
            # names the canonical .codearch/imports.md.
            subprocess.run([str(binary),str(source),'--out',str(target),'--no-index',
                            '--codearch-dir',str(state)],capture_output=True,check=True)
            maps[repo] = target.read_text(encoding='utf-8')
        for i, task in enumerate(tasks):
            source = (ROOT/task['repo']).resolve()
            for rel in task['expected_files']:
                safe_file(source,rel)
            # Counterbalance order; fresh sessions prohibit within-run history transfer.
            arms = ['without_map','with_map'] if i%2==0 else ['with_map','without_map']
            for arm in arms:
                session = Session(source,task['query'],maps[task['repo']] if arm=='with_map' else '')
                error = None
                try:
                    external_policy(session,command) if command else reference_policy(session)
                except (ValueError,KeyError,TypeError,subprocess.SubprocessError) as exc:
                    error = str(exc)
                    session.observe('failure',error)
                rows.append({'task':task['id'],'tier':task['tier'],'arm':arm,
                             'correct':error is None and session.answer==sorted(set(task['expected_files'])),
                             'answer':session.answer,'tokens':0,'files_opened':len(session.opened),
                             'searches':session.searches,'error':error,'trace':session.trace})
    counts = iter(bridge(helper, {'op':'tokens','texts':[e['content'] for r in rows for e in r['trace']]}))
    for row in rows:
        row['tokens'] = sum(next(counts) for _ in row['trace'])
    predictions = json.loads(args.label_predictions.read_text()) if args.label_predictions else bridge(helper,{'op':'labels','clusters':clusters})
    # eval-support wraps its answer with metadata (fell_back since M0.5);
    # --label-predictions files stay bare arrays. Both feed the same scorer.
    if isinstance(predictions, dict):
        predictions = predictions['labels']
    corpus = sorted({p for t in tasks for p in (ROOT/t['repo']).rglob('*') if '.git' not in p.parts})+[args.tasks,ROOT/'clusters.json']
    hashes = {p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in corpus if p.is_file()}
    report = {'schema':1,'policy':'external adapter' if command else 'lexical navigation proxy v1',
              'scope':'Fixed-policy navigation benchmark; not evidence of LLM coding-agent improvement. See task provenance for corpus scope.',
              'token_metric':'cl100k_base tokens over delivered observations and actions, each once; not provider-billed cumulative input or hidden reasoning',
              'harness_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'implementation_sha256':{p.relative_to(CRATE).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((CRATE/'src').rglob('*.rs'))+[CRATE/'Cargo.lock',CRATE/'Cargo.toml']},
              'map_options':{'budget':4000,'max_domains':12,'seed':24301},
              'corpus_sha256':hashes,'summary':summarize(rows),
              'by_tier':{str(t):summarize([r for r in rows if r['tier']==t]) for t in sorted({r['tier'] for r in rows})},
              'labels':evaluate_labels(clusters,predictions),'episodes':rows}
    args.out.mkdir(parents=True,exist_ok=True)
    (args.out/'report.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    for repo, content in maps.items():
        (args.out/(Path(repo).name+'-CODEBASE.md')).write_text(content,encoding='utf-8')
    summary = report['summary']
    lines = ['# M1 evaluation result','',report['scope'],'',f"Policy: {report['policy']}", '',
             '| Arm | Correct | Tokens | Files opened | Searches |','|---|---:|---:|---:|---:|']
    for arm in ['without_map','with_map']:
        s = summary[arm]
        lines.append(f"| {arm} | {s['correct']}/{s['episodes']} | {s['tokens']} | {s['files_opened']} | {s['searches']} |")
    lines += ['',f"Token savings: {summary['token_savings_percent']:.2f}%. Accuracy delta: {summary['accuracy_delta_pp']:.2f} percentage points.",
              f"Map helps under this policy: {summary['helps_at_equal_or_better_accuracy']}.",'',
              'Tokens include the complete map on every mapped episode, file inventory, task, tool outputs and actions. They are context volume, not billed model usage.', '',
              f"Labels: {report['labels']['count']} authored cases. Specificity {report['labels']['name_specificity']:.0%}; lexical groundedness {report['labels']['groundedness']:.0%}; sibling collisions {report['labels']['sibling_collision_rate']:.0%}.",'',
              'These are authored synthetic clusters, not independently human-reviewed annotations. Groundedness is a conservative lexical traceability check, not a semantic or grammatical noun classifier.', '',
              'No real-agent efficacy claim is supported. Run an LLM adapter before using this result to justify M2 or later breadth.']
    (args.out/'report.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
    print('\n'.join(lines))


if __name__=='__main__':
    main()
