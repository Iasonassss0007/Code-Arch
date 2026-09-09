"""Blind model review of reference labels, followed by production fallback scoring."""
import json,os,hashlib,urllib.request
from pathlib import Path
from run import ROOT,CRATE,bridge,evaluate_labels

clusters=json.loads((ROOT/'clusters-real.json').read_text())
system='''Review reference labels for extracted repository clusters before candidate labels are scored. You see source-derived directories, symbols, member paths, and proposed acceptable names. Assess whether each proposed name faithfully describes this cluster and distinguishes it from sibling clusters. Flag vague or misleading names and explain why. Do not invent source facts. Return JSON {"reviews":[{"id":"...","verdict":"accept" or "review","reason":"..."}]}, one entry per cluster. This is an automated annotation review, not human ground truth.'''
payload={'model':'openai/gpt-4.1-mini','temperature':0,'max_tokens':3500,'response_format':{'type':'json_object'},
         'messages':[{'role':'system','content':system},{'role':'user','content':json.dumps(clusters)}]}
request=urllib.request.Request('https://openrouter.ai/api/v1/chat/completions',data=json.dumps(payload).encode(),
        headers={'Authorization':'Bearer '+os.environ['OPENROUTER_API_KEY'],'Content-Type':'application/json'})
with urllib.request.urlopen(request,timeout=100) as response: result=json.load(response)
reviews=json.loads(result['choices'][0]['message']['content'])['reviews']
if len(reviews)!=len(clusters) or {r['id'] for r in reviews}!={r['id'] for r in clusters}: raise ValueError('Invalid review IDs')
report={'reference_sha256':hashlib.sha256((ROOT/'clusters-real.json').read_bytes()).hexdigest(),
        'review_model':result['model'],'usage':result['usage'],'reviews':reviews,
        'human_reviewed':False,'candidate_outputs_visible_to_reviewer':False}
(ROOT/'labels-reference-review.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'usage':result['usage'],'reviews':reviews},indent=2))
