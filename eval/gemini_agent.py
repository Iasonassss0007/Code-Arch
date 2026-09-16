"""Stateless Gemini API adapter for the free tier. Same contract as openrouter_agent.complete.

Only public benchmark observations are sent. The free tier bills nothing, so
usage carries cost 0.0 explicitly: run_agent stops a run whose usage lacks a
cost rather than guessing what it spent. Note that Google's free tier terms
allow submitted content to be used to improve Google's products.
"""
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request

from openrouter_agent import IMPORTERS_TOOL, ROUTES_TOOL, SYSTEM

BASE = 'https://generativelanguage.googleapis.com/v1beta'
DEFAULT_MODEL = 'gemini-3.1-flash-lite'
SEED = 24301
# Impact answers list up to ~20 paths. The 512 inherited from M1's one-file answers
# cut a 19-file gemini-3.6-flash answer mid-JSON in a live probe.
MAX_TOKENS = 2048

# Only settings probed against the live API on 2026-09-11. gemini-3.6-flash
# rejects thinkingBudget 0 (HTTP 400) and, left on default thinking, spent a
# 128-token reply limit on thoughts and truncated the JSON.
THINKING = {
    'gemini-3.6-flash': {'thinkingLevel': 'minimal'},
    'gemini-3.1-flash-lite': {'thinkingBudget': 0},
    # Probed 2026-09-14: valid single-JSON action in 8.3 s on a 3.3k-token
    # impact transcript (thinkingBudget 0 is rejected with HTTP 400).
    'gemini-3.5-flash-lite': {'thinkingLevel': 'minimal'},
}

# Free-tier Flash quotas are reported around 10 requests/minute per model.
# Pacing stays under that instead of discovering it through 429s.
MIN_INTERVAL = float(os.environ.get('CODEARCH_GEMINI_MIN_INTERVAL', '6.5'))
RETRIES = 5
RETRY_STATUS = {429, 500, 503}

_last = [0.0]

# Paid tier (billing enabled on the key's project): usage is priced at list
# rates, USD per 1M tokens (input, output), ai.google.dev/gemini-api/docs/pricing
# as of 2026-09-16. Implicit-cache discounts are ignored, so the recorded cost
# is an upper bound and run_agent's --max-cost cap stops early, never late.
PAID = os.environ.get('CODEARCH_GEMINI_PAID') == '1'
PRICES = {'gemini-3.6-flash': (0.75, 3.75), 'gemini-3.8-flash': (0.75, 3.75),
          'gemini-3.5-flash': (1.50, 9.00), 'gemini-3.5-flash-lite': (0.30, 2.50)}

# The harness's action shape, enforced at the sampler. A live probe had
# gemini-3.6-flash answer {"action": "search", ...}; JSON mode alone allows it.
ACTION_SCHEMA = {
    'type': 'OBJECT',
    'properties': {
        'tool': {'type': 'STRING', 'enum': ['search', 'open', 'answer', 'importers']},
        'query': {'type': 'STRING'},
        'path': {'type': 'STRING'},
        'files': {'type': 'ARRAY', 'items': {'type': 'STRING'}},
    },
    'required': ['tool'],
}


def schema_for(system):
    # The enum is visible to the sampler: offering `importers` to arms whose
    # prompt lacks the tool made gemini-3.6-flash call it there (4/30 episodes).
    # Tools and their argument fields appear only where the prompt names them.
    tools = ['search', 'open', 'answer']
    props = dict(ACTION_SCHEMA['properties'])
    if IMPORTERS_TOOL in system:
        tools.append('importers')
    if ROUTES_TOOL in system:
        tools.append('route_callers')
        props['view'] = {'type': 'STRING'}
    props['tool'] = {'type': 'STRING', 'enum': tools}
    return dict(ACTION_SCHEMA, properties=props)


def payload(request, model, system=SYSTEM, max_tokens=None):
    if model not in THINKING:
        raise ValueError(f'No verified thinking setting for {model}; probe it and add it to THINKING')
    cap = max_tokens or MAX_TOKENS
    return {'systemInstruction': {'parts': [{'text': system}]},
            'contents': [{'role': 'user', 'parts': [{'text': json.dumps(request, ensure_ascii=False)}]}],
            'generationConfig': {'temperature': 0, 'seed': SEED, 'maxOutputTokens': cap,
                                 'responseMimeType': 'application/json', 'responseSchema': schema_for(system),
                                 'thinkingConfig': THINKING[model]}}


def parse(result, model, system=SYSTEM, max_tokens=None, paid=None):
    paid = PAID if paid is None else paid
    if paid and model not in PRICES:
        raise ValueError(f'No verified paid price for {model}')
    u = result.get('usageMetadata') or {}
    usage = None
    if 'promptTokenCount' in u:
        # Thinking tokens are generated tokens; count them so provider totals stay honest.
        usage = {'prompt_tokens': u['promptTokenCount'],
                 'completion_tokens': u.get('candidatesTokenCount', 0) + u.get('thoughtsTokenCount', 0),
                 'cost': 0.0}
        if paid:
            pin, pout = PRICES[model]
            usage['cost'] = (usage['prompt_tokens'] * pin + usage['completion_tokens'] * pout) / 1e6
    meta = {'model': result.get('modelVersion', model), 'provider': 'google-ai-studio-paid' if paid else 'google-ai-studio-free',
            'id': result.get('responseId'), 'usage': usage,
            'system_sha256': hashlib.sha256(system.encode()).hexdigest(),
            'temperature': 0, 'seed': SEED, 'max_tokens': max_tokens or MAX_TOKENS, 'thinking': THINKING.get(model)}
    candidate = (result.get('candidates') or [{}])[0]
    meta['finish_reason'] = candidate.get('finishReason')
    text = ''.join(part.get('text', '') for part in (candidate.get('content') or {}).get('parts') or [])
    try:
        action = json.loads(text)
        if not isinstance(action, dict):
            raise ValueError('Expected JSON object')
    except ValueError:
        error = 'Model did not return a JSON action'
        # Recorded in the checkpoint, so a failed episode explains itself without a replay.
        if candidate.get('finishReason'):
            error += f" (finishReason={candidate['finishReason']}; text {text[:200]!r})"
        return {'action': None, 'metadata': meta, 'error': error}
    return {'action': action, 'metadata': meta}


def complete(request, model=None, system=SYSTEM, opener=urllib.request.urlopen, sleep=time.sleep, clock=time.monotonic, max_tokens=None):
    model = model or os.environ.get('CODEARCH_EVAL_MODEL', DEFAULT_MODEL)
    key = os.environ.get('GEMINI_API_KEY')
    if not key:
        raise RuntimeError('GEMINI_API_KEY is not configured')
    body = json.dumps(payload(request, model, system, max_tokens)).encode()
    for attempt in range(RETRIES + 1):
        wait = _last[0] + MIN_INTERVAL - clock()
        if wait > 0:
            sleep(wait)
        _last[0] = clock()
        # The key travels in a header so it never appears in a URL, a log line or an exception.
        req = urllib.request.Request(f'{BASE}/models/{model}:generateContent', data=body,
                                     headers={'x-goog-api-key': key, 'Content-Type': 'application/json'})
        try:
            with opener(req, timeout=180) as response:
                return parse(json.load(response), model, system, max_tokens)
        except urllib.error.HTTPError as exc:
            if exc.code in RETRY_STATUS and attempt < RETRIES:
                # 429 bodies carry RetryInfo.retryDelay ("38s") and the quota
                # id (e.g. GenerateRequestsPerDayPerProjectPerModel-FreeTier,
                # 500 req/day). Honor the delay instead of blind backoff; a
                # per-day cap still won't clear, and the run stops honestly.
                delay = 30 * (attempt + 1)
                try:
                    detail = json.loads(exc.read().decode(errors='replace'))
                    for d in detail.get('error', {}).get('details', []):
                        rd = d.get('retryDelay', '')
                        if rd.endswith('s'):
                            delay = min(float(rd[:-1]), 300.0)
                except (ValueError, TypeError):
                    pass
                sleep(delay)
                continue
            # "HTTP" in the message is what run_agent.provider_failure keys on.
            raise RuntimeError(f'Gemini HTTP {exc.code}') from None
        except OSError as exc:
            if attempt < RETRIES:
                sleep(30 * (attempt + 1))
                continue
            raise RuntimeError(f'Gemini HTTP transport failure: {type(exc).__name__}') from None


if __name__ == '__main__':
    try:
        print(json.dumps(complete(json.load(sys.stdin))))
    except Exception as exc:
        print(json.dumps({'action': None, 'error': str(exc), 'metadata': {}}))
