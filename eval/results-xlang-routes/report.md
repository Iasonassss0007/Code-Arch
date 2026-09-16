# M1v2 real-agent evaluation (impact)

Model: `gemini-3.6-flash`. 8 tasks, 1 fresh trials per arm.

| Arm | Mean F1 | Exact | Context tokens | Provider tokens | Files opened | Searches | USD |
|---|---:|---:|---:|---:|---:|---:|---:|
| without_map | 0.874 | 2/8 | 212680 | 1504794 | 14 | 29 | 1.140041 |
| routes_tool | 0.703 | 2/8 | 103956 | 619311 | 3 | 7 | 0.470357 |

No without_map/with_map pair in this run; compare arms across runs by task.

Context counts each observation/action once; provider usage includes repeated conversation input and system prompt. Both include map cost. Provider-reported USD includes any caching effects.

Impact tasks: transitive importers from madge over the whole checkout, scored by set F1. The with_map and index_only arms may open `.codearch/imports.md`; importers_tool queries the same index. Repeated trials on the same task are correlated. Not a code-change evaluation.
