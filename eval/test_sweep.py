import json
import unittest
from pathlib import Path

from build_lora_pairs import build_pairs, derive_summary
from score_sweep import REGISTRY, choose_winner, download_command, render_markdown


def fake_row(spec, fb=0):
    return {'id': 'm', 'name_specificity': spec, 'summary_fell_back': fb,
            'size_mb': 100}


class SweepTests(unittest.TestCase):
    def test_registry_has_four_models_with_required_keys(self):
        self.assertEqual(len(REGISTRY['models']), 4)
        ids = [m['id'] for m in REGISTRY['models']]
        self.assertEqual(len(set(ids)), 4)
        for m in REGISTRY['models']:
            for key in ('id', 'repo', 'file', 'size_mb', 'license', 'role'):
                self.assertIn(key, m)
            self.assertIn('q4_k_m', m['file'].lower())

    def test_registry_exclusion_is_stated_not_silent(self):
        self.assertTrue(any('codegemma' in e['id'] for e in REGISTRY['excluded']))
        for e in REGISTRY['excluded']:
            self.assertIn('reason', e)

    def test_download_command_names_repo_and_file(self):
        cmd = download_command(REGISTRY['models'][0])
        self.assertIn(REGISTRY['models'][0]['repo'], cmd)
        self.assertIn(REGISTRY['models'][0]['file'], cmd)

    def test_winner_prefers_specificity_then_fallbacks_then_size(self):
        rows = [dict(fake_row(0.60, 5), id='a', size_mb=100),
                dict(fake_row(0.55, 0), id='b', size_mb=100),
                dict(fake_row(0.60, 15), id='c', size_mb=100)]
        self.assertEqual(choose_winner(rows)['id'], 'a')
        tied = [dict(fake_row(0.60, 5), id='a', size_mb=800),
                dict(fake_row(0.60, 5), id='b', size_mb=400)]
        self.assertEqual(choose_winner(tied)['id'], 'b')

    def test_render_marks_skipped_with_reproduce_command(self):
        report = {
            'clusters': 20,
            'derived': {'name_specificity': 0.75, 'groundedness': 0.9,
                        'sibling_collision_rate': 0.0, 'specific_count': 15,
                        'seconds': 0.5},
            'models': [dict(fake_row(0.60), id='tiny', groundedness=1.0,
                              sibling_collision_rate=0.0, specific_count=12,
                              fell_back=0, summary_fell_back=15, seconds=60.0,
                              size_mb=100)],
            'skipped': [{'id': 'ghost', 'reason': 'missing',
                         'download': 'huggingface-cli download R F --local-dir models'}],
            'winner': 'tiny', 'gap_to_derived_pp': -15.0,
        }
        text = render_markdown(report)
        self.assertIn('tiny', text)
        self.assertIn('ghost', text)
        self.assertIn('huggingface-cli download R F --local-dir models', text)
        self.assertIn('Rule winner', text)


class LoraPairsTests(unittest.TestCase):
    def test_pairs_cover_every_acceptable_name(self):
        clusters = [
            {'id': 'a', 'group': 'g', 'dirs': ['src/auth'],
             'top_symbols': ['loginUser'], 'entry_points': [],
             'external_deps': ['jsonwebtoken'], 'file_count': 3,
             'acceptable_names': ['Authentication', 'Auth Core']},
            {'id': 'b', 'group': 'g', 'dirs': ['src/billing'],
             'top_symbols': [], 'entry_points': [], 'external_deps': [],
             'file_count': 1, 'acceptable_names': ['Billing']},
        ]
        pairs = build_pairs(clusters)
        self.assertEqual(len(pairs), 3)
        by_id = {p['id']: p for p in pairs}
        self.assertIn('a::Authentication', by_id)
        # Siblings carry the other cluster's first acceptable name.
        self.assertIn('Billing', by_id['a::Authentication']['input']['siblings'])
        self.assertIn('loginUser', by_id['a::Authentication']['input']['top_symbols'])

    def test_derive_summary_matches_rust_template_shape(self):
        cluster = {'dirs': ['src/auth'], 'top_symbols': ['a', 'b', 'c', 'd'],
                   'external_deps': ['x', 'y', 'z'], 'file_count': 3}
        self.assertEqual(
            derive_summary(cluster),
            '3 files under `src/auth`; key symbols a, b, c; uses x, y.')
        single = dict(cluster, file_count=1)
        self.assertIn('1 file under', derive_summary(single))

    def test_real_reference_set_exports_cleanly(self):
        clusters = json.loads((Path(__file__).parent / 'clusters-real.json').read_text())
        pairs = build_pairs(clusters)
        self.assertEqual(len(pairs), sum(len(c['acceptable_names']) for c in clusters))
        for p in pairs:
            json.dumps(p)
            self.assertTrue(p['completion']['name'])
            self.assertTrue(p['completion']['summary'].endswith('.'))


if __name__ == '__main__':
    unittest.main()
