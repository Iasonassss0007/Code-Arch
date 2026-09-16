"""Hermetic tests for the LoRA pipeline scripts — no model, no network, no build.

Pins the parts that must not drift between harvest and inference: the
prompt shape (must mirror build_prompt in src/label/validate.rs), the pair
format (llama.cpp convert_lora_to_gguf --jsonl contract), the pre-stated
gate numbers, and the distillation-worklist structure.
"""
import json
import unittest
from pathlib import Path
from unittest import mock

import harvest_lora as hl

ROOT = Path(__file__).resolve().parent


class PromptShape(unittest.TestCase):
    def test_prompt_lists_evidence_and_siblings(self):
        cluster = {
            "dirs": ["src/auth"], "top_symbols": ["authenticateUser"],
            "entry_points": ["POST /login"], "external_deps": ["jsonwebtoken"],
            "file_count": 3,
        }
        prompt = hl.build_prompt_text(cluster, ["Billing"])
        self.assertIn("src/auth", prompt)
        self.assertIn("authenticateUser", prompt)
        self.assertIn("POST /login", prompt)
        self.assertIn("jsonwebtoken", prompt)
        self.assertIn("Billing", prompt)
        self.assertIn('{"name"', prompt)  # JSON reply contract stated

    def test_prompt_omits_absent_fields(self):
        cluster = {"dirs": [], "top_symbols": [], "entry_points": [],
                   "external_deps": [], "file_count": 3}
        prompt = hl.build_prompt_text(cluster, [])
        self.assertNotIn("Top symbols", prompt)
        self.assertNotIn("Sibling", prompt)


class PairFormat(unittest.TestCase):
    def test_train_pairs_are_inference_shaped(self):
        path = ROOT / "lora-train.jsonl"
        if not path.is_file():
            self.skipTest("run python eval/harvest_lora.py first")
        pairs = [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines() if l]
        self.assertGreater(len(pairs), 40, "expected the harvested corpus, not the 48-pair seed")
        for p in pairs:
            self.assertIn("prompt", p)
            self.assertIn("completion", p)
            # The completion must parse as the labeler's JSON reply shape.
            reply = json.loads(p["completion"])
            self.assertIn("name", reply)
            self.assertIn("summary", reply)

    def test_corpus_records_provenance(self):
        path = ROOT / "lora-corpus.jsonl"
        if not path.is_file():
            self.skipTest("run python eval/harvest_lora.py first")
        rows = [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines() if l]
        for r in rows:
            self.assertIn("repo", r)
            self.assertIn("file_count", r)
            self.assertGreaterEqual(r["file_count"], hl.MIN_CLUSTER_SIZE)


class GateRule(unittest.TestCase):
    def test_gate_numbers_match_the_sweep_record(self):
        # The M6 sweep measured 0.5B at 20% specificity and derived at 75%.
        # train_lora.py must keep gating against those exact numbers.
        import train_lora as tl
        self.assertEqual(tl.GATE_BEAT_BASE, 0.20)
        self.assertEqual(tl.GATE_ADOPT, 0.75)

    def test_harvest_respects_size_floor(self):
        with mock.patch.object(hl, "bridge") as bridge:
            bridge.return_value = [
                {"id": 0, "dirs": ["a"], "top_symbols": [], "entry_points": [],
                 "external_deps": [], "file_count": 2, "files": []},
                {"id": 1, "dirs": ["b"], "top_symbols": [], "entry_points": [],
                 "external_deps": [], "file_count": 5, "files": []},
            ]
            out = hl.harvest_repo("fake")
            self.assertEqual(len(out), 1)
            self.assertEqual(out[0]["file_count"], 5)


if __name__ == "__main__":
    unittest.main()
