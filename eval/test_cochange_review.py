"""Hermetic tests for the co-change blind A/B harness. No network, no render."""
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cochange_review import blind, blind_items, fixture_backend, item_text, score, write_review


def fake_index(clusters):
    return {"clusters": [
        {"id": i, "name": name, "summary": f"{len(files)} files", "files": files}
        for i, (name, files) in enumerate(clusters)]}


class BlindTests(unittest.TestCase):
    def test_items_carry_no_provenance(self):
        index = fake_index([("A", ["a.ts", "b.ts"]), ("B", ["c.ts"])])
        units = blind_items(index, index, "typedi")
        items, manifest = blind(units)
        self.assertEqual(len(items), 4)
        for item in items:
            self.assertNotIn("arm", item)
            self.assertNotIn("cluster", item)
            text = item_text(item)
            # Arm labels must not leak into what the reviewer reads.
            self.assertNotIn("without", text)
            self.assertNotIn("arm", text)
        self.assertEqual(set(manifest), {i["id"] for i in items})
        arms = sorted(m["arm"] for m in manifest.values())
        self.assertEqual(arms, ["with", "with", "without", "without"])

    def test_blind_is_deterministic(self):
        index = fake_index([("A", ["a.ts"])])
        units = blind_items(index, index, "r")
        self.assertEqual(blind(units), blind(units))


class ScoreTests(unittest.TestCase):
    def test_accept_rates_and_delta(self):
        manifest = {"item-000": {"repo": "t", "arm": "without", "cluster": 0},
                    "item-001": {"repo": "t", "arm": "with", "cluster": 0}}
        reviews = [{"id": "item-000", "verdict": "review", "reason": "x"},
                   {"id": "item-001", "verdict": "accept", "reason": "y"}]
        scored = score([], manifest, reviews)
        self.assertEqual(scored["overall"]["without"]["accept_rate"], 0.0)
        self.assertEqual(scored["overall"]["with"]["accept_rate"], 1.0)
        self.assertAlmostEqual(scored["delta_pp"], 100.0)

    def test_mismatched_reviews_rejected(self):
        manifest = {"item-000": {"repo": "t", "arm": "without", "cluster": 0}}
        with self.assertRaises(ValueError):
            score([], manifest, [{"id": "item-999", "verdict": "accept", "reason": "x"}])


class FixtureTests(unittest.TestCase):
    def test_fixture_replays_end_to_end(self):
        import tempfile
        from cochange_review import FIXTURE
        data = json.loads(FIXTURE.read_text(encoding="utf-8"))
        items, manifest = data["items"], data["manifest"]
        reviews, meta = fixture_backend(items)
        with tempfile.TemporaryDirectory() as tmp:
            scored, _ = write_review(Path(tmp), items, manifest, reviews, meta)
            review = json.loads((Path(tmp) / "review.json").read_text(encoding="utf-8"))
        self.assertEqual(scored["overall"]["without"]["accept_rate"], 0.5)
        self.assertEqual(scored["overall"]["with"]["accept_rate"], 0.5)
        self.assertAlmostEqual(scored["delta_pp"], 0.0)
        # Same shape as labels-reference-review.json.
        self.assertEqual(set(review),
                         {"reference_sha256", "review_model", "usage", "reviews",
                          "human_reviewed", "candidate_outputs_visible_to_reviewer"})
        self.assertFalse(review["human_reviewed"])

    def test_live_without_key_is_a_credit_gate_not_a_crash(self):
        import os
        from cochange_review import live_backend
        old = os.environ.pop("OPENROUTER_API_KEY", None)
        try:
            with self.assertRaises(RuntimeError) as ctx:
                live_backend([])
            self.assertIn("credit", str(ctx.exception))
        finally:
            if old is not None:
                os.environ["OPENROUTER_API_KEY"] = old


if __name__ == "__main__":
    unittest.main()
