"""Stateless Groq adapter. Only public benchmark observations are sent.

Groq's free tier serves OpenAI-compatible chat completions with JSON mode,
which is exactly the shape the M1/M1v2 harness needs. JSON mode is required
on this endpoint (a plain call 403s), and the harness only ever wants a
single JSON action back, so the constraint costs nothing.

Model note: `qwen/qwen3.6-27b` emits reasoning tokens before the JSON. The
content field still carries exactly one action; reasoning inflates
completion-token accounting (recorded per call in metadata.usage) and is
counted by the harness's provider totals, same convention as Gemini's
thinking tokens.
"""
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request

from openrouter_agent import SYSTEM, SYSTEM_IMPACT, system_for  # noqa: F401

DEFAULT_MODEL = 'qwen/qwen3.6-27b'
BASE = 'https://api.groq.com/openai/v1/chat/completions'

# Free tier: ~7k input tokens per request on 27B models (413, not retryable),
# TPM-capped on 120B (429, retryable). An M1v2 impact episode's transcript
# grows past 60KB by late actions, so observations are compacted before
# sending: the task and the newest KEEP_FULL observations go whole, older
# ones keep only their head. The agent still sees every observation's
# beginning -- which for search results is the top-ranked lines, and for
# opens is the file's start -- and the full transcript is preserved in the
# checkpoint for auditing. This is a Groq-arm protocol detail, recorded in
# each call's metadata; the OpenRouter/Gemini arms send full transcripts.
# Both knobs are env-overridable for tighter free-tier fits (defaults keep
# the recorded behavior); overrides are recorded in metadata.
KEEP_FULL = int(os.environ.get('CODEARCH_GROQ_KEEP_FULL', '2'))
OLD_OBS_HEAD = int(os.environ.get('CODEARCH_GROQ_OBS_HEAD', '1200'))


def compact(request):
    """Head-truncate older observations so one request fits the free tier.

    The task, actions and failures pass whole (small, and the agent's own
    moves must be exact); observations older than the newest KEEP_FULL keep
    only their head -- for search results that is the top-ranked lines, for
    opens the file's start. The full transcript stays in the checkpoint.
    """
    messages = request.get('messages') if isinstance(request, dict) else None
    if not messages:
        return request
    observ_idx = [i for i, m in enumerate(messages)
                  if m.get('kind') not in ('task', 'action', 'failure')]
    keep_whole = set(observ_idx[-KEEP_FULL:])
    out = list(messages)
    for i in observ_idx:
        if i not in keep_whole and len(messages[i].get('content', '')) > OLD_OBS_HEAD:
            out[i] = {'kind': messages[i]['kind'],
                      'content': messages[i]['content'][:OLD_OBS_HEAD] + ' ...[truncated for length]'}
    return {'protocol': 1, 'messages': out}

# qwen3.6-27b reasons before answering: ~1200 completion tokens per turn,
# which busts the 1000 OTPM budget and (under a 900 cap) truncates before
# any JSON appears. `reasoning_effort: none` disables the trace; the content
# field then carries exactly one action in tens of tokens. Recorded per call.
REASONING_EFFORT = os.environ.get('CODEARCH_GROQ_REASONING_EFFORT', 'none')
# Free tier is ~30 requests/min. A 1.5 s floor between calls keeps the
# harness far under it. 413 on this tier is an input-Tokens-Per-Minute
# refusal (the request alone exceeds the per-minute budget), not a hard
# size error -- waiting out the minute clears it -- so it retries with a
# long backoff. 429/5xx retry faster. MIN_INTERVAL is env-overridable for
# minute-budget pacing under tight caps.
MIN_INTERVAL = float(os.environ.get('CODEARCH_GROQ_MIN_INTERVAL', '1.5'))
RETRY_STATUS = {429, 500, 502, 503, 504}
RETRIES = 5
_last = [0.0]


# Free tier also caps output at ~1000 tokens/min on this model, so a 2048
# request is refused outright (429, never clears on retry). CODEARCH_GROQ_MAX_TOKENS
# lowers the per-request cap; it is recorded in each call's metadata.
def complete(request, model=None, system=SYSTEM, max_tokens=None,
             opener=urllib.request.urlopen, sleep=time.sleep, clock=time.monotonic):
    model = model or os.environ.get('CODEARCH_EVAL_MODEL', DEFAULT_MODEL)
    key = os.environ.get('GROQ_API_KEY')
    if not key:
        raise RuntimeError('GROQ_API_KEY is not configured')
    cap = min(max_tokens or 2048, int(os.environ.get('CODEARCH_GROQ_MAX_TOKENS', '2048')))
    payload = {'model': model,
               'messages': [{'role': 'system', 'content': system},
                            {'role': 'user',
                             'content': json.dumps(compact(request), ensure_ascii=False)}],
               'temperature': 0, 'max_tokens': cap,
               'response_format': {'type': 'json_object'},
               'reasoning_effort': REASONING_EFFORT}
    body = json.dumps(payload).encode()
    for attempt in range(RETRIES + 1):
        wait = _last[0] + MIN_INTERVAL - clock()
        if wait > 0:
            sleep(wait)
        _last[0] = clock()
        req = urllib.request.Request(BASE, data=body,
                                     headers={'Authorization': 'Bearer ' + key,
                                              'Content-Type': 'application/json',
                                              # Groq fronts this endpoint with
                                              # Cloudflare; urllib's default
                                              # Python UA is blocked (error
                                              # 1010). Any ordinary UA passes.
                                              'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) codearch-eval/1.0'})
        try:
            with opener(req, timeout=180) as response:
                result = json.load(response)
                return parse(result, model, system, cap,
                             compacted=any(len(m.get('content', '')) > OLD_OBS_HEAD
                                           for m in request.get('messages', [])))
        except urllib.error.HTTPError as exc:
            # 413 here is the ITPM budget refusing the single request;
            # waiting out the minute clears it, so it is retryable like 429.
            if (exc.code in RETRY_STATUS or exc.code == 413) and attempt < RETRIES:
                sleep(65 if exc.code == 413 else 10 * (attempt + 1))
                continue
            # "HTTP" in the message is what run_agent.provider_failure keys on.
            detail = exc.read().decode(errors='replace')[:200]
            raise RuntimeError(f'Groq HTTP {exc.code}: {detail}') from None
        except OSError as exc:
            if attempt < RETRIES:
                sleep(10 * (attempt + 1))
                continue
            raise RuntimeError(f'Groq HTTP transport failure: {type(exc).__name__}') from None


def parse(result, model, system=SYSTEM, max_tokens=None, compacted=False):
    usage = None
    u = result.get('usage') or {}
    if 'prompt_tokens' in u:
        usage = {'prompt_tokens': u['prompt_tokens'],
                 'completion_tokens': u.get('completion_tokens', 0),
                 'cost': 0.0}
    meta = {'model': result.get('model', model), 'provider': 'groq-free',
            'id': result.get('id'),
            'usage': usage,
            'system_sha256': hashlib.sha256(system.encode()).hexdigest(),
            'temperature': 0, 'max_tokens': max_tokens or 2048,
            'reasoning_effort': REASONING_EFFORT,
            'observations_compacted': compacted,
            'keep_full': KEEP_FULL, 'obs_head': OLD_OBS_HEAD}
    try:
        content = result['choices'][0]['message']['content']
        action = json.loads(content)
        if not isinstance(action, dict):
            raise ValueError('Expected JSON object')
    except (KeyError, IndexError, TypeError, ValueError):
        meta['finish_reason'] = (result.get('choices') or [{}])[0].get('finish_reason')
        return {'action': None, 'metadata': meta, 'error': 'Model did not return a JSON action'}
    return {'action': action, 'metadata': meta}


if __name__ == '__main__':
    try:
        print(json.dumps(complete(json.load(sys.stdin))))
    except Exception as exc:
        print(json.dumps({'action': None, 'error': str(exc), 'metadata': {}}))
