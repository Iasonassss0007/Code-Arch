# M1v2 real-agent evaluation (impact)

Model: `gemini-3.5-flash-lite`. 8 tasks, 2 fresh trials per arm.

| Arm | Mean F1 | Exact | Context tokens | Provider tokens | Files opened | Searches | USD |
|---|---:|---:|---:|---:|---:|---:|---:|
| without_map | 0.475 | 1/16 | 208181 | 600936 | 0 | 17 | 0.000000 |
| with_map | 0.491 | 2/16 | 272945 | 749512 | 0 | 16 | 0.000000 |
| importers_tool | 0.570 | 2/16 | 207314 | 582376 | 0 | 16 | 0.000000 |

Context-token savings: -31.11%; provider-token savings: -24.72%; mean-F1 delta: +0.016; accuracy delta: 6.25 pp.

Context counts each observation/action once; provider usage includes repeated conversation input and system prompt. Both include map cost. Provider-reported USD includes any caching effects.

Impact tasks: transitive importers from madge over the whole checkout, scored by set F1. The with_map and index_only arms may open `.codearch/imports.md`; importers_tool queries the same index. Repeated trials on the same task are correlated. Not a code-change evaluation.
