# Navigation runs rescored on the current task set

Model `gemini-3.5-flash-lite`, 34 tasks, runs: results-nav-3arm, results-nav-importers.

| Arm | Mean F1 | Exact | Provider tokens/ep | Errors |
|---|---:|---:|---:|---:|
| without_map | 0.608 | 12/68 | 11,400 | 2 |
| importers_tool | 0.979 | 64/68 | 9,387 | 0 |
| index_only | 0.550 | 10/68 | 12,258 | 6 |
| with_map | 0.583 | 4/68 | 20,855 | 0 |

Paired per-task F1 delta vs without_map (5000-sample bootstrap):

| Arm | Slice | Tasks | Delta F1 | 95% CI | better/worse |
|---|---|---:|---:|---|---:|
| importers_tool | all | 34 | +0.372 | [+0.276, +0.470] | 28/0 |
| importers_tool | T1 | 28 | +0.333 | [+0.235, +0.436] | 22/0 |
| importers_tool | T2 | 2 | +0.139 | [+0.111, +0.167] | 2/0 |
| importers_tool | T3 | 4 | +0.760 | [+0.686, +0.805] | 4/0 |
| index_only | all | 34 | -0.058 | [-0.146, +0.020] | 11/13 |
| index_only | T1 | 28 | -0.058 | [-0.165, +0.041] | 10/11 |
| index_only | T2 | 2 | -0.064 | [-0.175, +0.047] | 1/1 |
| index_only | T3 | 4 | -0.053 | [-0.158, +0.000] | 0/1 |
| with_map | all | 34 | -0.024 | [-0.119, +0.076] | 12/14 |
| with_map | T1 | 28 | -0.047 | [-0.151, +0.069] | 9/13 |
| with_map | T2 | 2 | -0.105 | [-0.262, +0.052] | 1/1 |
| with_map | T3 | 4 | +0.173 | [+0.000, +0.346] | 2/0 |
