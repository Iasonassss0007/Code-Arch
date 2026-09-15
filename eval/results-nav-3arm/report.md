# M1v2 real-agent evaluation (impact)

Model: `gemini-3.5-flash-lite`. 37 tasks, 2 fresh trials per arm.

| Arm | Mean F1 | Exact | Context tokens | Provider tokens | Files opened | Searches | USD |
|---|---:|---:|---:|---:|---:|---:|---:|
| without_map | 0.577 | 12/74 | 369006 | 800752 | 2 | 72 | 0.000000 |
| with_map | 0.539 | 4/74 | 671362 | 1485944 | 12 | 64 | 0.000000 |
| index_only | 0.515 | 12/74 | 412246 | 871974 | 6 | 72 | 0.000000 |

Context-token savings: -81.94%; provider-token savings: -85.57%; mean-F1 delta: -0.038; accuracy delta: -10.81 pp.

Context counts each observation/action once; provider usage includes repeated conversation input and system prompt. Both include map cost. Provider-reported USD includes any caching effects.

Impact tasks: transitive importers from madge over the whole checkout, scored by set F1. The with_map and index_only arms may open `.codearch/imports.md`. Repeated trials on the same task are correlated. Not a code-change evaluation.
