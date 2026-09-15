# M1v2 3-arm run — analysis (2026-09-15)

`gemini-3.5-flash-lite`, 37 gated impact tasks × 2 trials × 3 arms, 222/222
episodes, 0 transport failures. Numbers recomputed from `checkpoint.json`.

## Verdict: null result, and the map is the wrong delivery for impact tasks

| Arm | Mean F1 | Exact | Precision | Recall | Answer size (expected 7.1) |
|---|---:|---:|---:|---:|---:|
| without_map | 0.577 | 12/74 | 0.749 | 0.632 | 4.8 |
| with_map | 0.539 | 4/74 | 0.658 | 0.613 | 7.5 |
| index_only | 0.515 | 12/74 | 0.667 | 0.544 | 4.9 |

Paired per-task delta vs without_map (task mean over trials, 5000-sample bootstrap):

| Comparison | n | ΔF1 | 95% CI | tasks better/worse |
|---|---:|---:|---|---:|
| with_map, all | 37 | −0.038 | [−0.129, +0.059] | 13/16 |
| with_map, T1 | 28 | −0.047 | [−0.154, +0.067] | 9/13 |
| with_map, T2 | 5 | −0.156 | [−0.356, +0.022] | 2/3 |
| with_map, T3 (TypeDI) | 4 | +0.173 | [0.000, +0.346] | 2/0 |
| index_only, all | 37 | −0.062 | [−0.150, +0.018] | 12/15 |

1. **No significant effect.** Every CI except T3 crosses zero, and T3 has only 4 tasks.
2. **The map makes answers longer without finding more files.** Answers grow 55% while recall stays flat
   and precision drops. The agent adds files from the target's cluster, and sharing
   a cluster doesn't mean a file depends on the target. That's why exact matches fall from 12 to 4.
3. **The information is there, but the agent doesn't use it.** The offline proxy
   (`results-nav-proxy/`) shows map + one `imports.md` open + a graph walk
   gets F1 0.890. The agent opened `imports.md` in only 12/74 (with_map) and
   10/74 (index_only) episodes, and even then scored 0.566 / 0.400. It
   doesn't walk the transitive closure. The gap between 0.89 and 0.52 comes from
   the agent's behavior, not from missing content.
4. **The index_only arm's errors are the agent's own mistakes.** All 8
   `Answer must contain inventoried paths` rejections are correct: the agent listed
   `.codearch/imports.md` itself as an answer file (6), or answered with a
   path that doesn't exist (`runtime-tests/workerd/index.Exp.test.ts`, 2). The
   index text confused the model about what counts as an answer.

## Implications

- Stop evaluating the whole map as upfront context for "who imports X" tasks. Grep already covers
  direct imports, and a small model can't turn a text index into a transitive walk.
- For impact tasks, serve the answer as a tool (`importers(path, transitive)`)
  that returns only the affected set. The proxy's 0.890 is roughly that arm's upper bound.
- Evaluate the map where search can't see the link: DI or indirect coupling
  (the T3 signal), URL contracts, and "where does X belong" questions.
- Follow-up arm `importers_tool` added to `run_agent.py`. A scripted agent
  (one `importers(target)` call, then answer) scores F1 0.890 through the real
  Session on all 37 tasks, matching the proxy.
