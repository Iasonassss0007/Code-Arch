"""Run actual LLM navigation episodes; checkpoints preserve usage and failures."""
import argparse
import hashlib
import json
import subprocess
import tempfile
import time
from pathlib import Path
from run import ROOT, CRATE, Session, bridge, safe_file, set_f1, summarize
from openrouter_agent import (complete, SYSTEM, SYSTEM_IMPACT, system_for,
                              MAX_TOKENS_M1, MAX_TOKENS_IMPACT, IMPORTERS_TOOL)
import gemini_agent
from build_tasks_nav import importers as walk_importers
from check_import_index import parse_index
import groq_agent

# The path CODEBASE.md names for the reverse import index.
IMPORTS_REL='.codearch/imports.md'
ARMS=('without_map','with_map','index_only','importers_tool')


def episode(session, model, request_fn=complete):
    calls=[]
    error=None
    ended=False
    for _ in range(session.max_actions):
        try:
            response=request_fn({'protocol':1,'messages':session.trace},model=model)
            calls.append(response.get('metadata',{}))
            if response.get('error'):
                raise ValueError(response['error'])
            ended=session.action(response['action'])
            if ended:
                break
        except Exception as exc:
            error=str(exc)
            session.observe('failure',error)
            break
    if not ended and not error:
        error='action budget exhausted'
        session.observe('failure',error)
    return calls,error


def usage(calls):
    result={'prompt_tokens':0,'completion_tokens':0,'cost_usd':0.0,'calls':len(calls),'complete':True}
    for call in calls:
        u=call.get('usage')
        if not u or any(u.get(k) is None for k in ['prompt_tokens','completion_tokens','cost']):
            result['complete']=False
            continue
        result['prompt_tokens']+=u['prompt_tokens']
        result['completion_tokens']+=u['completion_tokens']
        result['cost_usd']+=u['cost']
    return result


def provider_failure(row):
    """True when an episode died in transport rather than producing an answer.

    'action budget exhausted' is a real agent result and must be kept. An HTTP
    error or missing usage accounting is the provider failing, and re-running it
    is the only honest option -- scoring it as a wrong answer would attribute a
    billing problem to whichever arm drew it.
    """
    error=row.get('error')
    if not error:
        return False
    return 'HTTP' in error or not row.get('usage',{}).get('complete',True)


def write_json(path,value):
    temporary=path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')
    temporary.replace(path)


def backend_for(provider):
    """(complete function, default model) for a provider name."""
    if provider=='gemini':
        return gemini_agent.complete,gemini_agent.DEFAULT_MODEL
    if provider=='groq':
        return groq_agent.complete,groq_agent.DEFAULT_MODEL
    if provider=='openrouter':
        return complete,'qwen/qwen3.5-flash-02-23'
    raise ValueError(f'Unknown provider: {provider}')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--tasks',type=Path,default=ROOT/'tasks-real.json')
    p.add_argument('--out',type=Path,default=ROOT/'results-agent')
    p.add_argument('--provider',choices=['openrouter','gemini','groq'],default='openrouter')
    p.add_argument('--model',default=None,help="default: the provider's default model")
    p.add_argument('--repeats',type=int,default=2)
    p.add_argument('--arms',default='without_map,with_map',
                   help="comma-separated subset of without_map,with_map,index_only,importers_tool. index_only serves no map text, only .codearch/imports.md on demand; importers_tool serves the same index as an importers(path) query")
    p.add_argument('--max-cost',type=float,default=1.0,help='Stop between episodes when reported USD reaches this cap')
    p.add_argument('--resume',action='store_true')
    args=p.parse_args()
    backend,default_model=backend_for(args.provider)
    args.model=args.model or default_model
    if args.repeats<1 or args.max_cost<=0: raise ValueError('Positive repeats and cost required')
    ALL_ARMS=[a.strip() for a in args.arms.split(',') if a.strip()]
    if not ALL_ARMS or any(a not in ARMS for a in ALL_ARMS) or len(set(ALL_ARMS))!=len(ALL_ARMS):
        raise ValueError("--arms must be a unique subset of "+",".join(ARMS))
    tasks=json.loads(args.tasks.read_text(encoding='utf-8'))
    if not tasks or len({t['id'] for t in tasks})!=len(tasks): raise ValueError('Invalid task IDs')
    args.out.mkdir(parents=True,exist_ok=True)
    suffix='.exe' if __import__('os').name=='nt' else ''
    subprocess.run(['cargo','build','--locked','--bins'],cwd=CRATE,check=True)
    helper=CRATE/'target/debug'/('eval-support'+suffix)
    binary=CRATE/'target/debug'/('codearch'+suffix)
    maps={}; imports={}; corpus={}; revisions={}
    with tempfile.TemporaryDirectory(prefix='codearch-agent-') as tmp:
        for repo in sorted({t['repo'] for t in tasks}):
            source=(ROOT/repo).resolve()
            revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,text=True).strip()
            pins={t['provenance']['revision'] for t in tasks if t['repo']==repo}
            if pins!={revision} or subprocess.check_output(['git','status','--porcelain'],cwd=source,text=True).strip():
                raise ValueError(f'Repository {repo} must be clean at its pinned revision')
            revisions[repo]=revision
            target=Path(tmp)/(source.name+'.md')
            state=Path(tmp)/(source.name+'-codearch')
            # --codearch-dir keeps imports.md out of the pinned checkout.
            subprocess.run([str(binary),str(source),'--out',str(target),'--no-index','--codearch-dir',str(state)],capture_output=True,check=True)
            maps[repo]=target.read_text(encoding='utf-8')
            imports[repo]=(state/'imports.md').read_text(encoding='utf-8')
            for file in sorted(source.rglob('*')):
                if file.is_file() and '.git' not in file.parts:
                    corpus[file.relative_to(ROOT).as_posix()]=hashlib.sha256(file.read_bytes()).hexdigest()
        config={'provider':args.provider,'model':args.model,'repeats':args.repeats,'arms':ALL_ARMS,'tasks':tasks,'revisions':revisions,
                'corpus_sha256':corpus,'map_sha256':{k:hashlib.sha256(v.encode()).hexdigest() for k,v in maps.items()},
                'imports_sha256':{k:hashlib.sha256(v.encode()).hexdigest() for k,v in imports.items()},
                'system_prompt':SYSTEM,'system_prompt_impact':SYSTEM_IMPACT,'system_prompt_importers_tool':SYSTEM_IMPACT+IMPORTERS_TOOL,'max_actions':12,'temperature':0,'seed':24301,
                'source_sha256':{str(f.relative_to(CRATE)):hashlib.sha256(f.read_bytes()).hexdigest() for f in sorted((CRATE/'src').rglob('*.rs'))+[CRATE/'Cargo.lock',ROOT/'run.py',Path(__file__),ROOT/'openrouter_agent.py']}}
        checkpoint=args.out/'checkpoint.json'
        rows=[]
        if args.resume and checkpoint.exists():
            old=json.loads(checkpoint.read_text(encoding='utf-8'))
            if old['config']!=config: raise ValueError('Resume config differs from checkpoint')
            rows=old['episodes']
        elif checkpoint.exists():
            raise ValueError('Output already has a checkpoint; use --resume or a new output directory')
        write_json(checkpoint,{'config':config,'episodes':rows})
        # A transport failure is not a measurement. Without this, a single
        # HTTP 402 or rate-limit blip would be frozen into the checkpoint as a
        # zero-score episode and silently skipped on --resume, biasing whichever
        # arm happened to hit it.
        retryable=[r for r in rows if provider_failure(r)]
        if retryable:
            print(f'discarding {len(retryable)} episode(s) that failed in transport; they will be re-run')
            rows=[r for r in rows if not provider_failure(r)]
        done={(r['task'],r['trial'],r['arm']) for r in rows}
        for trial in range(args.repeats):
            for i,task in enumerate(tasks):
                for rel in task['expected_files']: safe_file(ROOT/task['repo'],rel)
                # Arm order rotates deterministically so no arm always moves first.
                rot=(i+trial)%len(ALL_ARMS)
                arms=ALL_ARMS[rot:]+ALL_ARMS[:rot]
                for arm in arms:
                    if (task['id'],trial,arm) in done: continue
                    prior=usage([c for row in rows for c in row['calls']])
                    if prior['cost_usd']>=args.max_cost: raise RuntimeError('Cost cap reached; checkpoint saved')
                    if rows and not prior['complete']: raise RuntimeError('Missing usage; stopping rather than estimating spend')
                    # index_only: no map text in context, imports.md openable on
                    # demand. Tests whether the index alone (the artifact the
                    # offline proxy vindicates) beats map-text-in-context.
                    has_index=arm in ('with_map','index_only')
                    session=Session((ROOT/task['repo']).resolve(),task['query'],maps[task['repo']] if arm=='with_map' else '',
                                    map_files={IMPORTS_REL:imports[task['repo']]} if has_index else None,
                                    importers=(lambda rel,back=parse_index(imports[task['repo']]):walk_importers(back,rel)) if arm=='importers_tool' else None)
                    system=system_for(task,arm)
                    # Impact answers hold 2-20 paths; 512 truncates them
                    # (recorded live on the Gemini probe). M1 tasks keep 512.
                    cap=MAX_TOKENS_IMPACT if task.get('kind')=='impact' else MAX_TOKENS_M1
                    start=time.monotonic()
                    calls,error=episode(session,args.model,
                                        lambda request,model=None:backend(
                                            request,model=model,system=system,max_tokens=cap))
                    row={'task':task['id'],'trial':trial,'tier':task['tier'],'arm':arm,
                         'correct':error is None and session.answer==sorted(set(task['expected_files'])),
                         'f1':0.0 if error else set_f1(session.answer,task['expected_files']),
                         'expected':sorted(set(task['expected_files'])),
                         'answer':session.answer,'files_opened':len(session.opened),'searches':session.searches,'lookups':session.lookups,
                         'tokens':0,'error':error,'calls':calls,'usage':usage(calls),'trace':session.trace,
                         'elapsed_seconds':round(time.monotonic()-start,3)}
                    rows.append(row)
                    write_json(checkpoint,{'config':config,'episodes':rows})
                    print(f"{len(rows)}/{len(tasks)*args.repeats*len(ALL_ARMS)} {task['id']} {arm}: correct={row['correct']} opens={row['files_opened']} searches={row['searches']} error={error}",flush=True)
                    if error and ('HTTP' in error or not row['usage']['complete']):
                        raise RuntimeError('Provider failure; checkpoint saved, no further calls')
        counts=iter(bridge(helper,{'op':'tokens','texts':[e['content'] for row in rows for e in row['trace']]}))
        for row in rows: row['tokens']=sum(next(counts) for _ in row['trace'])
        def aggregate(subset):
            n=len(subset)
            return {'episodes':n,'correct':sum(r['correct'] for r in subset),
                    'mean_f1':sum(r.get('f1',float(r['correct'])) for r in subset)/n if n else 0.0,
                    'tokens':sum(r['tokens'] for r in subset),
                    'files_opened':sum(r['files_opened'] for r in subset),
                    'searches':sum(r['searches'] for r in subset),
                    'provider_usage':usage([c for r in subset for c in r['calls']])}
        # Classic pair keeps the recorded gate comparable; extra arms aggregate
        # the same way beside it.
        summary=summarize([r for r in rows if r['arm'] in ('without_map','with_map')])
        for arm in ['without_map','with_map']:
            summary[arm]['provider_usage']=usage([c for r in rows if r['arm']==arm for c in r['calls']])
        extra={arm:aggregate([r for r in rows if r['arm']==arm]) for arm in ALL_ARMS if arm not in ('without_map','with_map')}
        a=summary['without_map']['provider_usage']; b=summary['with_map']['provider_usage']
        a_total=a['prompt_tokens']+a['completion_tokens']
        summary['provider_token_savings_percent']=(100*(1-(b['prompt_tokens']+b['completion_tokens'])/a_total)
                                                    if a_total else None)
        report={'schema':3,'config':config,'summary':summary,'extra_arms':extra,
                'by_tier':{str(t):summarize([r for r in rows if r['tier']==t and r['arm'] in ('without_map','with_map')]) for t in sorted({r['tier'] for r in rows})},
                'episodes':rows}
        write_json(args.out/'report.json',report)
        for repo,text in maps.items(): (args.out/(Path(repo).name+'-CODEBASE.md')).write_text(text,encoding='utf-8')
        for repo,text in imports.items(): (args.out/(Path(repo).name+'-imports.md')).write_text(text,encoding='utf-8')
        impact=all(t.get('kind')=='impact' for t in tasks)
        lines=['# M1v2 real-agent evaluation (impact)' if impact else '# M1 real-agent evaluation','',f'Model: `{args.model}`. {len(tasks)} tasks, {args.repeats} fresh trials per arm.',
               '', '| Arm | Mean F1 | Exact | Context tokens | Provider tokens | Files opened | Searches | USD |',
               '|---|---:|---:|---:|---:|---:|---:|---:|']
        for arm in ['without_map','with_map']:
            s=summary[arm]; u=s['provider_usage']
            lines.append(f"| {arm} | {s['mean_f1']:.3f} | {s['correct']}/{s['episodes']} | {s['tokens']} | {u['prompt_tokens']+u['completion_tokens']} | {s['files_opened']} | {s['searches']} | {u['cost_usd']:.6f} |")
        for arm,s in extra.items():
            u=s['provider_usage']
            lines.append(f"| {arm} | {s['mean_f1']:.3f} | {s['correct']}/{s['episodes']} | {s['tokens']} | {u['prompt_tokens']+u['completion_tokens']} | {s['files_opened']} | {s['searches']} | {u['cost_usd']:.6f} |")
        lines+=['',f"Context-token savings: {summary['token_savings_percent']:.2f}%; "
                + (f"provider-token savings: {summary['provider_token_savings_percent']:.2f}%"
                   if summary['provider_token_savings_percent'] is not None else
                   'provider-token savings: n/a (no provider usage)')
                + f"; mean-F1 delta: {summary['f1_delta']:+.3f}; accuracy delta: {summary['accuracy_delta_pp']:.2f} pp.",
                '', 'Context counts each observation/action once; provider usage includes repeated conversation input and system prompt. Both include map cost. Provider-reported USD includes any caching effects.',
                '', ('Impact tasks: transitive importers from madge over the whole checkout, scored by set F1. The with_map and index_only arms may open `.codearch/imports.md`; importers_tool queries the same index. Repeated trials on the same task are correlated. Not a code-change evaluation.' if impact else
                     'This is a small file-location benchmark, not a code-change evaluation. Repeated trials on the same task are correlated. Tier 3 uses TypeDI runtime/decorator indirection, not a large enterprise application. Labels require separate review.')]
        (args.out/'report.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
        print('\n'.join(lines))


if __name__=='__main__': main()
