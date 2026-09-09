import copy
import json
import tempfile
import unittest
from pathlib import Path
from run import Session, evaluate_labels, reference_policy, safe_file, summarize

class HarnessTests(unittest.TestCase):
    def test_path_escape_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'repo'; root.mkdir()
            (Path(tmp)/'secret.ts').write_text('secret')
            with self.assertRaises(ValueError): safe_file(root,'../secret.ts')

    def test_map_is_not_available_to_control_and_gold_is_not_in_observations(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); (root/'a.ts').write_text('export const needle = 1;')
            session=Session(root,'needle','')
            reference_policy(session)
            self.assertEqual(session.answer,['a.ts'])
            self.assertEqual(session.searches,1)
            self.assertEqual(len(session.opened),1)
            self.assertNotIn('expected_files',json.dumps(session.trace))
            self.assertEqual(json.loads(session.trace[0]['content'])['map'],'')

    def test_bad_map_can_reduce_correctness(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            (root/'a.ts').write_text('export const needle = 1;')
            (root/'b.ts').write_text('export const unrelated = 1;')
            session=Session(root,'needle','- `b.ts` — needle')
            reference_policy(session)
            self.assertEqual(session.answer,['b.ts'])
            self.assertEqual(session.searches,0)

    def test_labels_reject_hallucination_and_sibling_collision(self):
        c={'id':'a','group':'g','dirs':['src/auth'],'top_symbols':[], 'external_deps':[], 'acceptable_names':['Authentication'],'grounding_aliases':{'authentication':'auth'}}
        sibling=copy.deepcopy(c); sibling['id']='b'
        result=evaluate_labels([c,sibling],[{'id':i,'name':'Authentication','summary':'Unicorn database'} for i in ['a','b']])
        self.assertEqual(result['name_specificity'],0)
        self.assertEqual(result['groundedness'],0)
        self.assertEqual(result['sibling_collision_rate'],1)
        with self.assertRaises(ValueError): evaluate_labels([c],[])

    def test_lower_cost_with_lower_accuracy_is_not_a_win(self):
        rows=[{'arm':'without_map','correct':True,'tokens':100,'files_opened':1,'searches':1},
              {'arm':'with_map','correct':False,'tokens':10,'files_opened':1,'searches':0}]
        self.assertFalse(summarize(rows)['helps_at_equal_or_better_accuracy'])
        self.assertEqual(summarize(rows)['token_savings_percent'],90)

    def test_unknown_or_non_source_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); (root/'gold.json').write_text('{}')
            session=Session(root,'q','')
            with self.assertRaises(ValueError): session.action({'tool':'open','path':'gold.json'})
            with self.assertRaises(ValueError): session.action({'tool':'answer','files':['gold.json']})

if __name__=='__main__': unittest.main()
