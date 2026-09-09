import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from run import Session
from run_agent import episode,usage

class RealAgentTests(unittest.TestCase):
 def test_usage_counts_repeated_provider_input(self):
  calls=[{'usage':{'prompt_tokens':100,'completion_tokens':10,'cost':0.001}}]*2
  self.assertEqual(usage(calls)['prompt_tokens'],200)
  self.assertAlmostEqual(usage(calls)['cost_usd'],0.002)
  self.assertFalse(usage([{}])['complete'])
 def test_usage_retained_on_malformed_model_output(self):
  with TemporaryDirectory() as tmp:
   session=Session(Path(tmp),'q','')
   def request(*a,**kw): return {'error':'invalid JSON','metadata':{'usage':{'prompt_tokens':4,'completion_tokens':3,'cost':0.1}}}
   calls,error=episode(session,'fake',request)
   self.assertEqual(error,'invalid JSON')
   self.assertEqual(usage(calls)['completion_tokens'],3)
 def test_action_exhaustion_is_failure(self):
  with TemporaryDirectory() as tmp:
   session=Session(Path(tmp),'q','',max_actions=2)
   def request(*a,**kw): return {'action':{'tool':'search','query':'q'},'metadata':{}}
   calls,error=episode(session,'fake',request)
   self.assertEqual(error,'action budget exhausted')
   self.assertEqual(len(calls),2)
 def test_gold_and_usage_are_never_sent_to_model(self):
  with TemporaryDirectory() as tmp:
   p=Path(tmp); (p/'a.ts').write_text('x')
   session=Session(p,'q','')
   def request(payload,**kw):
    self.assertNotIn('expected_files',str(payload))
    self.assertNotIn('cost_usd',str(payload))
    return {'action':{'tool':'answer','files':['a.ts']},'metadata':{}}
   calls,error=episode(session,'fake',request)
   self.assertIsNone(error)
   self.assertEqual(session.answer,['a.ts'])
if __name__=='__main__': unittest.main()
