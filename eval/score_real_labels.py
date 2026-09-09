"""Score production DerivedLabeler against the frozen real-cluster references."""
import hashlib,json
from pathlib import Path
from run import ROOT,CRATE,bridge,evaluate_labels
clusters=json.loads((ROOT/'clusters-real.json').read_text())
review=json.loads((ROOT/'labels-reference-review.json').read_text())
if review['reference_sha256']!=hashlib.sha256((ROOT/'clusters-real.json').read_bytes()).hexdigest():
 raise ValueError('Reference set changed after blind review')
helper=CRATE/'target/debug'/('eval-support.exe' if __import__('os').name=='nt' else 'eval-support')
response=bridge(helper,{'op':'labels','clusters':clusters})
# eval-support now reports fallback counts alongside the labels; older
# builds returned a bare array. Accept both so this reproduction path
# keeps working unchanged.
predictions=response['labels'] if isinstance(response,dict) else response
result=evaluate_labels(clusters,predictions)
result.update({'reference_sha256':review['reference_sha256'],'human_reviewed':False,'blind_model_review':'labels-reference-review.json','labeler':'production DerivedLabeler'})
(ROOT/'labels-real-report.json').write_text(json.dumps(result,indent=2)+'\n')
lines=['# Real-cluster label evaluation','',f"20 extracted clusters. Name specificity {result['name_specificity']:.0%}; lexical groundedness {result['groundedness']:.0%}; sibling collisions {result['sibling_collision_rate']:.0%}.",
 '', 'References were authored from production summaries and member paths, then reviewed by GPT-4.1 Mini without candidate outputs. This is model-reviewed reference data, not human ground truth. Name acceptance remains a strict finite-list metric; groundedness is lexical.',
 '', '| Cluster | Derived name | Specific | Grounded | Collision |','|---|---|---|---|---|']
for row in result['rows']:
 lines.append(f"| {row['id']} | {row['name']} | {row['specific']} | {row['grounded']} | {row['collision']} |")
(ROOT/'labels-real-report.md').write_text('\n'.join(lines)+'\n')
print('\n'.join(lines))
