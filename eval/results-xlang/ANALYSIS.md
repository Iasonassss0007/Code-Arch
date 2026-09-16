# Cross-language impact, gemini-3.5-flash-lite (2026-09-16)

48/48 episodes, 8 tasks (`tasks-xlang.json`), 3 arms x 2 trials, free tier.

| Arm | Mean F1 | Exact | Files opened | importers lookups |
|---|---:|---:|---:|---:|
| without_map | 0.475 | 1/16 | 0 | 0 |
| with_map | 0.491 | 2/16 | 0 | 0 |
| importers_tool | 0.570 | 2/16 | 0 | 0 |

Paired per-task delta vs without_map (95% bootstrap CI over 8 tasks): with_map +0.016
[-0.07, +0.09]; importers_tool +0.095 [-0.10, +0.31].

**Uninformative.** No arm opened a file and the tool was never called; the model answers
from the inventory after one search, so the arms cannot differ except by noise. The
importers_tool gap is not a tool effect (0 lookups). with_map cost +31% context tokens.
Superseded by `results-xlang-36flash-paid/`.
