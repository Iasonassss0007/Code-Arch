"""Build the checked-in synthetic M1 corpus; no network or private repository data."""
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
DOMAINS = [
 ('auth','validateSession','Reject an expired login session','Authentication'),
 ('billing','calculateInvoice','Calculate invoice total including tax','Billing'),
 ('search','rankResults','Rank search results by relevance','Search'),
 ('email','sendMessage','Send an email message','Email'),
 ('cache','evictExpired','Evict expired cache entries','Cache'),
 ('storage','saveObject','Save an object to storage','Storage'),
 ('payments','refundPayment','Refund a payment','Payments'),
 ('catalog','listProducts','List available catalog products','Catalog'),
 ('cart','addItem','Add an item to the shopping cart','Cart'),
 ('checkout','submitOrder','Submit an order at checkout','Checkout'),
 ('inventory','reserveStock','Reserve stock in inventory','Inventory'),
 ('shipping','quoteDelivery','Quote shipping delivery cost','Shipping'),
 ('users','updateProfile','Update a user profile','Users'),
 ('reports','generateReport','Generate a report','Reports'),
 ('queue','retryJob','Retry a failed queue job','Queue'),
 ('events','publishEvent','Publish an event','Events'),
 ('metrics','recordCounter','Record a metrics counter','Metrics'),
 ('audit','appendRecord','Append an audit record','Audit'),
 ('config','loadSettings','Load configuration settings','Configuration'),
 ('notifications','notifyUser','Notify a user','Notifications'),
]
tasks, clusters = [], []
for i, (domain, symbol, prompt, name) in enumerate(DOMAINS):
    tier = 1 if i < 7 else 2 if i < 14 else 3
    repo = ROOT / 'fixtures' / f'tier{tier}'
    directory = f'src/{domain}' if tier != 2 else f'app/{domain}'
    files = {
      f'{directory}/service.ts': f'// {prompt}.\nexport function {symbol}(value: number) {{ return value + {i}; }}\n',
      f'{directory}/index.ts': (f"import {{ {symbol} }} from './service';\nexport const execute = {symbol};\n" if tier == 1 else f'// {domain} entry selected by '+('filesystem convention' if tier == 2 else 'runtime registry')+'.\nexport const key = "'+domain+'";\n'),
      f'{directory}/types.ts': f'export interface {domain.title()}Options {{ enabled: boolean; }}\n',
    }
    if tier == 3:
        files[f'{directory}/registry.ts'] = f'export const registration = {{ token: "{domain}", module: "./service" }};\n'
    for rel, content in files.items():
        p = repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding='utf-8')
    (repo / 'package.json').write_text(json.dumps({'name':f'm1-tier{tier}', 'private':True, 'dependencies':{'next':'*'} if tier == 2 else {}})+'\n')
    for kind, query in [('symbol',symbol), ('intent',prompt)]:
        tasks.append({'id':f'{domain}-{kind}', 'repo':f'fixtures/tier{tier}', 'tier':tier, 'query':query, 'expected_files':[f'{directory}/service.ts']})
    clusters.append({'id':domain,'group':f'tier{tier}','dirs':[directory], 'top_symbols':[symbol], 'external_deps':[], 'file_count':len(files), 'acceptable_names':[name], 'grounding_aliases':{'authentication':'auth','configuration':'config'}, 'annotation':'Authored synthetic subsystem; expected name assigned from its intended responsibility, not labeler output.'})
(ROOT/'tasks.json').write_text(json.dumps(tasks,indent=2)+'\n')
(ROOT/'clusters.json').write_text(json.dumps(clusters,indent=2)+'\n')
