"""Tests for the navigation task generator and its adversary gate."""
import json
from pathlib import Path

import pytest

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


def test_session_serves_supplied_map_files_by_exact_path(tmp_path):
    from run import Session
    (tmp_path / "a.ts").write_text("x", encoding="utf-8")
    s = Session(tmp_path, "q", "map", map_files={".codearch/imports.md": "a.ts ← b.ts"})
    assert json.loads(s.trace[0]["content"])["map_files"] == [".codearch/imports.md"]
    s.action({"tool": "open", "path": ".codearch/imports.md"})
    assert json.loads(s.trace[-1]["content"])["source"] == "a.ts ← b.ts"
    assert ".codearch/imports.md" in s.opened


def test_session_without_map_files_is_unchanged_and_refuses_them(tmp_path):
    """The without_map arm must see exactly what M1 saw."""
    from run import Session
    (tmp_path / "a.ts").write_text("x", encoding="utf-8")
    s = Session(tmp_path, "q", "")
    assert set(json.loads(s.trace[0]["content"])) == {"query", "files", "map"}
    with pytest.raises(ValueError):
        s.action({"tool": "open", "path": ".codearch/imports.md"})


def test_impact_tasks_get_their_own_prompt_and_nav_prompt_is_untouched():
    from openrouter_agent import SYSTEM, SYSTEM_IMPACT, system_for
    assert system_for({"kind": "impact"}) is SYSTEM_IMPACT
    assert system_for({"kind": "locate"}) is SYSTEM
    assert system_for({}) is SYSTEM
    # The nav prompt excludes tests; impact ground truth includes them.
    assert "do not qualify" not in SYSTEM_IMPACT
    m1 = ROOT / "results-agent" / "report.json"
    if m1.exists():
        recorded = json.loads(m1.read_text(encoding="utf-8"))["config"]["system_prompt"]
        assert SYSTEM == recorded, "M1 must stay reproducible"


MODULE_FILES = ["a.ts", "b.tsx", "c.js", "d.jsx", "e.mts", "f.cts", "g.mjs", "h.cjs"]


def _mixed_tree(root):
    for name in MODULE_FILES + ["notes.md", "data.json"]:
        (root / name).write_text("x", encoding="utf-8")


def test_session_inventories_every_module_extension_codearch_analyzes(tmp_path):
    """The index lists .mts/.cts/.mjs/.cjs files, so the agent must be able to answer them."""
    from run import Session
    _mixed_tree(tmp_path)
    assert Session(tmp_path, "q", "").paths == MODULE_FILES


def test_task_builder_and_gate_inventory_the_same_files_as_the_session(tmp_path):
    from gate_tasks_nav import inventory
    _mixed_tree(tmp_path)
    assert inventory(str(tmp_path)) == MODULE_FILES
    assert sorted(B.source_files(str(tmp_path))) == MODULE_FILES


def test_index_parse_reads_importers_per_line():
    from check_import_index import parse_index
    text = (
        "# Reverse Import Index — t\n\n"
        "3 imports into 2 files · import resolution 98% · 0 unresolved specifiers carry no edge.\n\n"
        "src/a.ts ← src/b.ts, src/c.ts\n"
        "src/b.ts ← src/c.ts\n"
    )
    assert parse_index(text) == {"src/a.ts": ["src/b.ts", "src/c.ts"], "src/b.ts": ["src/c.ts"]}


def test_ceiling_walks_the_index_and_scores_inside_the_oracle_scope():
    from check_import_index import score_task
    back = {"src/a.ts": ["src/b.ts"], "src/b.ts": ["src/c.ts", "bench/x.ts"]}
    task = {"target": "src/a.ts", "expected_files": ["src/b.ts", "src/c.ts"]}
    row = score_task(back, task, "src")
    assert row["predicted"] == ["bench/x.ts", "src/b.ts", "src/c.ts"]
    assert row["recall"] == 1.0
    assert 0 < row["f1"] < 1          # bench/x.ts is outside what madge scanned
    assert row["f1_scoped"] == 1.0


def test_session_importers_tool_returns_transitive_set_by_depth(tmp_path):
    from run import Session
    from check_import_index import parse_index
    for f in ("a.ts", "b.ts", "c.ts"):
        (tmp_path / f).write_text("x", encoding="utf-8")
    back = parse_index("a.ts ← b.ts\nb.ts ← c.ts")
    s = Session(tmp_path, "q", "", importers=lambda rel: B.importers(back, rel))
    assert json.loads(s.trace[0]["content"])["tools"] == ["importers"]
    s.action({"tool": "importers", "path": "a.ts"})
    assert json.loads(s.trace[-1]["content"])["importers"] == [
        {"path": "b.ts", "depth": 1}, {"path": "c.ts", "depth": 2}]
    assert s.lookups == 1
    with pytest.raises(ValueError):
        s.action({"tool": "importers", "path": ".codearch/imports.md"})


def test_importers_tool_is_refused_without_the_arm_and_prompt_only_extends(tmp_path):
    from run import Session
    from openrouter_agent import SYSTEM_IMPACT, system_for
    (tmp_path / "a.ts").write_text("x", encoding="utf-8")
    with pytest.raises(ValueError):
        Session(tmp_path, "q", "").action({"tool": "importers", "path": "a.ts"})
    impact = {"kind": "impact"}
    assert system_for(impact) == system_for(impact, "with_map") == SYSTEM_IMPACT
    assert system_for(impact, "importers_tool").startswith(SYSTEM_IMPACT)
    assert '"importers"' in system_for(impact, "importers_tool")
