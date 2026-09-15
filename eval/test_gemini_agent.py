"""Hermetic tests for the Gemini free-tier adapter. No network, no key."""
import io
import json
import urllib.error

import pytest

import gemini_agent as G


def response(text, **usage):
    body = {"candidates": [{"content": {"parts": [{"text": text}]}}],
            "usageMetadata": {"promptTokenCount": 100, "candidatesTokenCount": 10, **usage},
            "modelVersion": "gemini-3.1-flash-lite", "responseId": "r1"}
    return io.BytesIO(json.dumps(body).encode())


def http_error(code):
    return urllib.error.HTTPError("https://example.invalid", code, "error", {}, io.BytesIO(b"{}"))


class FakeClock:
    def __init__(self):
        self.now = 1000.0
        self.slept = []

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.now += seconds


@pytest.fixture
def env(monkeypatch):
    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    monkeypatch.setattr(G, "_last", [0.0])


def test_payload_sends_system_prompt_json_mode_seed_and_verified_thinking_setting():
    p = G.payload({"q": 1}, "gemini-3.6-flash", "SYS")
    assert p["systemInstruction"] == {"parts": [{"text": "SYS"}]}
    assert json.loads(p["contents"][0]["parts"][0]["text"]) == {"q": 1}
    cfg = p["generationConfig"]
    assert (cfg["temperature"], cfg["seed"], cfg["responseMimeType"]) == (0, 24301, "application/json")
    assert cfg["thinkingConfig"] == {"thinkingLevel": "minimal"}
    assert G.payload({}, "gemini-3.1-flash-lite", "SYS")["generationConfig"]["thinkingConfig"] == {"thinkingBudget": 0}


def test_payload_constrains_output_to_the_action_shape():
    # Live probe: gemini-3.6-flash answered {"action": "search", ...} instead of {"tool": ...}.
    schema = G.payload({}, "gemini-3.1-flash-lite", "SYS")["generationConfig"]["responseSchema"]
    assert schema["required"] == ["tool"]
    assert schema["properties"]["tool"]["enum"] == ["search", "open", "answer", "importers"]
    assert set(schema["properties"]) == {"tool", "query", "path", "files"}


def test_unparseable_reply_records_why_it_failed():
    body = {"candidates": [{"finishReason": "MAX_TOKENS", "content": {"parts": [{"text": '{"tool":"ans'}]}}],
            "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 512}}
    out = G.parse(body, "gemini-3.1-flash-lite", "SYS")
    assert out["error"].startswith("Model did not return a JSON action")
    assert "finishReason=MAX_TOKENS" in out["error"]
    assert '{"tool":"ans' in out["error"]


def test_reply_limit_fits_the_largest_impact_answer_with_room_to_over_answer():
    # Live probe: a 19-file answer from gemini-3.6-flash hit 512 tokens and was cut mid-JSON.
    # ASCII JSON never needs more tokens than characters; 2x allows naming extra files.
    from pathlib import Path
    tasks = json.loads((Path(__file__).parent / "tasks-nav-gated.json").read_text(encoding="utf-8"))
    longest = max(len(json.dumps({"tool": "answer", "files": t["expected_files"]})) for t in tasks)
    limit = G.payload({}, "gemini-3.6-flash", "SYS")["generationConfig"]["maxOutputTokens"]
    assert limit >= 2 * longest, f"{limit} tokens cannot hold 2x a {longest}-character answer"


def test_model_without_a_verified_thinking_setting_is_refused():
    # 3.x Flash rejects thinkingBudget 0 with HTTP 400; guessing would burn a run.
    with pytest.raises(ValueError):
        G.payload({}, "gemini-2.5-flash", "SYS")


def test_parse_reads_the_action_and_counts_thinking_at_zero_cost():
    result = json.load(response('{"tool":"answer","files":["a.ts"]}', thoughtsTokenCount=5))
    out = G.parse(result, "gemini-3.1-flash-lite", "SYS")
    assert out["action"] == {"tool": "answer", "files": ["a.ts"]}
    assert out["metadata"]["usage"] == {"prompt_tokens": 100, "completion_tokens": 15, "cost": 0.0}


def test_non_json_output_is_an_agent_error_not_a_transport_error():
    out = G.parse(json.load(response("not json")), "gemini-3.1-flash-lite", "SYS")
    assert out["action"] is None
    assert out["error"] == "Model did not return a JSON action"


def test_rate_limit_is_waited_out_then_retried(env):
    fake = FakeClock()
    calls = iter([http_error(429), response('{"tool":"search","query":"x"}')])

    def opener(req, timeout):
        item = next(calls)
        if isinstance(item, Exception):
            raise item
        return item

    out = G.complete({}, model="gemini-3.1-flash-lite", opener=opener, sleep=fake.sleep, clock=fake.clock)
    assert out["action"] == {"tool": "search", "query": "x"}
    assert max(fake.slept) >= 30


def test_persistent_failure_surfaces_as_a_provider_failure(env):
    from run_agent import provider_failure
    fake = FakeClock()

    def opener(req, timeout):
        raise http_error(429)

    with pytest.raises(RuntimeError) as exc:
        G.complete({}, model="gemini-3.1-flash-lite", opener=opener, sleep=fake.sleep, clock=fake.clock)
    assert provider_failure({"error": str(exc.value), "usage": {"complete": True}})


def test_timeouts_are_transport_failures_too(env):
    from run_agent import provider_failure
    fake = FakeClock()

    def opener(req, timeout):
        raise TimeoutError("timed out")

    with pytest.raises(RuntimeError) as exc:
        G.complete({}, model="gemini-3.1-flash-lite", opener=opener, sleep=fake.sleep, clock=fake.clock)
    assert provider_failure({"error": str(exc.value), "usage": {"complete": True}})


def test_calls_are_spaced_to_stay_under_the_free_tier_rate(env):
    fake = FakeClock()

    def opener(req, timeout):
        return response('{"tool":"search","query":"x"}')

    G.complete({}, model="gemini-3.1-flash-lite", opener=opener, sleep=fake.sleep, clock=fake.clock)
    G.complete({}, model="gemini-3.1-flash-lite", opener=opener, sleep=fake.sleep, clock=fake.clock)
    assert fake.slept and fake.slept[-1] == pytest.approx(G.MIN_INTERVAL)


def test_key_is_sent_in_a_header_never_in_the_url(env):
    seen = {}

    def opener(req, timeout):
        seen["url"], seen["key"] = req.full_url, req.get_header("X-goog-api-key")
        return response('{"tool":"search","query":"x"}')

    fake = FakeClock()
    G.complete({}, model="gemini-3.1-flash-lite", opener=opener, sleep=fake.sleep, clock=fake.clock)
    assert seen["key"] == "test-key"
    assert "test-key" not in seen["url"]


def test_run_agent_selects_backend_and_default_model_by_provider():
    from run_agent import backend_for
    import openrouter_agent
    fn, model = backend_for("gemini")
    assert fn is G.complete and model == G.DEFAULT_MODEL
    fn, model = backend_for("openrouter")
    assert fn is openrouter_agent.complete and model == "qwen/qwen3.5-flash-02-23"
