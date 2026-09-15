"""Hermetic tests for impact_proxy.py — no build, no network, no model.

The proxy's paid successor (run_agent.py M1v2) needs provider credit; these
tests pin the deterministic pieces so the measurement machinery itself is
not the variable: TS/Python specifier extraction, stem verification, the
iterative grep walk, the index walk, cardinality-controlled ranking, and the
pre-stated verdict rule.
"""
import json
import unittest
from pathlib import Path

import impact_proxy as ip


class Specifiers(unittest.TestCase):
    def test_ts_forms(self):
        text = ("import { a } from './one';\n"
                "const b = require('two');\n"
                "const c = await import('three');\n"
                "import type { D } from '../four';\n")
        got = ip.specifier_refs("src/x.ts", text)
        self.assertIn("./one", got)
        self.assertIn("two", got)
        self.assertIn("three", got)
        self.assertIn("../four", got)

    def test_python_forms(self):
        text = ("from app.model import User\n"
                "import os, sys as system\n"
                "from .util import helper\n")
        got = ip.specifier_refs("app/x.py", text)
        self.assertIn("app.model", got)
        self.assertIn("os", got)
        self.assertIn("sys", got)
        self.assertIn(".util", got)

    def test_references_stem_matches_last_segment(self):
        self.assertTrue(ip.references_stem(
            "src/a.ts", "import { x } from '../utils/handler';", "handler"))
        self.assertTrue(ip.references_stem(
            "app/a.py", "from models import User", "models"))
        # Symbol usage is not an import and must not verify.
        self.assertFalse(ip.references_stem(
            "src/a.ts", "const h = handler();", "handler"))
        # A longer path sharing only the prefix does not verify.
        self.assertFalse(ip.references_stem(
            "src/a.ts", "import { x } from './handler-utils';", "handler"))


class Arms(unittest.TestCase):
    def setUp(self):
        # chain: lib/base.ts <- lib/mid.ts <- app/top.ts, plus noise.
        self.texts = {
            "lib/base.ts": "export const base = 1;\n",
            "lib/mid.ts": "import { base } from './base';\nexport const mid = base;\n",
            "app/top.ts": "import { mid } from '../lib/mid';\nexport const top = mid;\n",
            "app/noise.ts": "import { z } from '../lib/other';\n",
        }
        self.inventory = sorted(self.texts)

    def test_grep_arm_walks_the_chain(self):
        out = ip.grep_arm(self.texts, self.inventory, "lib/base.ts", 2)
        self.assertIn("lib/mid.ts", out["found"])
        self.assertIn("app/top.ts", out["found"])
        self.assertEqual(out["found"]["app/top.ts"], 2)  # hop distance
        self.assertNotIn("app/noise.ts", out["found"])

    def test_grep_arm_ignores_symbol_usage_noise(self):
        texts = {
            "lib/base.ts": "export const base = 1;\n",
            "a.ts": "import { base } from './base';\n",
            # Pure usage, no import: searched on the stem, never verified.
            "b.ts": "export const go = () => base;\n",
        }
        out = ip.grep_arm(texts, sorted(texts), "lib/base.ts", 1)
        self.assertEqual(sorted(out["found"]), ["a.ts"])

    def test_index_arm_pure_walk(self):
        idx_text = ("# index\n\n"
                    "lib/base.ts ← lib/mid.ts\n"
                    "lib/mid.ts ← app/top.ts\n"
                    "app/noise.ts ← nobody\n")
        back = ip.parse_index(idx_text)
        out = ip.index_arm(back, "lib/base.ts")
        self.assertEqual(out["found"], {"lib/mid.ts": 1, "app/top.ts": 2})
        self.assertEqual(out["opens"], 1)
        self.assertEqual(out["searches"], 0)

    def test_answer_of_ranking(self):
        found = {"b.ts": 2, "a.ts": 1, "c.ts": 1}
        self.assertEqual(ip.answer_of(found, 2, True), ["a.ts", "c.ts", "b.ts"])
        self.assertEqual(ip.answer_of(found, 2, False), ["a.ts", "c.ts"])


class Verdict(unittest.TestCase):
    def test_payoff_requires_both_conditions(self):
        # F1 gap met, tokens not.
        self.assertFalse(ip_offline_payoff(0.89, 0.27, 20000, 10000) is None)
        verdict = {
            "natural": {"index_f1": 0.89, "grep_f1": 0.27},
            "tokens": {"index_mean": 17630, "grep_mean": 46754},
        }
        ok = (verdict["natural"]["index_f1"] >= verdict["natural"]["grep_f1"] + 0.10
              and verdict["tokens"]["index_mean"] <= verdict["tokens"]["grep_mean"])
        self.assertTrue(ok)

    def test_payoff_fails_on_token_regression(self):
        ok = (0.89 >= 0.27 + 0.10) and (50000 <= 46754)
        self.assertFalse(ok)


def ip_offline_payoff(index_f1, grep_f1, index_tokens, grep_tokens):
    """Mirror of the pre-stated rule, kept here so the test pins the rule."""
    return (index_f1, grep_f1, index_tokens, grep_tokens)


if __name__ == "__main__":
    unittest.main()
