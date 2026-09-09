"""Tests for the navigation task generator and its adversary gate."""
import json
from pathlib import Path

import build_tasks_nav as B
from gate_tasks_nav import f1, path_guess, hub_guess, tokens
from run import set_f1

ROOT = Path(__file__).resolve().parent


def test_transitive_importers_follow_multiple_hops():
    # c <- b <- a : changing c must implicate both b and a.
    graph = {"a.ts": ["b.ts"], "b.ts": ["c.ts"], "c.ts": []}
    reached = B.importers(B.reverse(graph), "c.ts")
    assert reached == {"b.ts": 1, "a.ts": 2}


def test_importers_terminate_on_cycles():
    graph = {"a.ts": ["b.ts"], "b.ts": ["a.ts", "c.ts"], "c.ts": []}
    reached = B.importers(B.reverse(graph), "c.ts")
    assert set(reached) == {"a.ts", "b.ts"}


def test_f1_rewards_partial_overlap():
    assert set_f1(["a", "b"], ["a", "b"]) == 1.0
    assert set_f1([], ["a"]) == 0.0
    assert set_f1(["x"], ["a"]) == 0.0
    assert 0 < set_f1(["a", "x"], ["a", "b"]) < 1


def test_adversary_is_ranked_and_cardinality_matched():
    # Over-predicting must not be mistaken for a weak heuristic: the adversary
    # returns exactly `budget` paths, so precision cannot be diluted away.
    paths = [f"src/other/f{i}.ts" for i in range(50)] + ["src/billing/stripe.ts"]
    predicted = path_guess({"target": "src/billing/charge.ts"}, paths, 1)
    assert predicted == ["src/billing/stripe.ts"]


def test_hub_adversary_returns_most_imported():
    paths = ["a.ts", "b.ts", "c.ts"]
    counts = {"a.ts": 1, "b.ts": 9, "c.ts": 5}
    assert hub_guess({"target": "z.ts"}, paths, counts, 2) == ["b.ts", "c.ts"]


def test_stopwords_keep_generic_segments_from_matching():
    assert "src" not in tokens("src/billing/stripe.ts")
    assert "billing" in tokens("src/billing/stripe.ts")


def test_shipped_task_set_survives_its_own_gate():
    """The regression that caused the first benchmark to measure grep."""
    path = ROOT / "tasks-nav-gated.json"
    if not path.exists():
        return
    tasks = json.loads(path.read_text(encoding="utf-8"))
    assert tasks, "gated task set must not be empty"
    for t in tasks:
        worst = max(t["gates"]["adversary_f1"].values())
        assert worst <= 0.35, f"{t['id']} is solvable for free (F1 {worst})"
        assert len(t["expected_files"]) >= 2, f"{t['id']} is a lookup, not an impact task"
        assert t["gates"]["max_depth"] >= 2, f"{t['id']} needs no transitive step"


def test_transport_failures_are_retried_not_scored():
    """A provider outage must not be frozen into the report as a wrong answer."""
    from run_agent import provider_failure
    good = {"error": None, "usage": {"complete": True}}
    budget = {"error": "action budget exhausted", "usage": {"complete": True}}
    http = {"error": "OpenRouter HTTP 402", "usage": {"complete": True}}
    partial = {"error": "boom", "usage": {"complete": False}}
    assert not provider_failure(good)
    assert not provider_failure(budget)   # a real agent failure: keep it
    assert provider_failure(http)         # transport: retry it
    assert provider_failure(partial)
