# routes_tool on the fixed import index, gemini-3.6-flash, paid (2026-09-16)

8/8 episodes, 8 tasks x 1 trial, routes_tool arm only. $0.32 list price. Baseline:
without_map from `results-xlang-routes/` (same model, prompt, tasks; that arm does not
use codearch output, so the index fix does not change it).

| Arm | Mean F1 | Exact | Files opened | Searches | Context tokens | List USD |
|---|---:|---:|---:|---:|---:|---:|
| without_map | 0.874 | 2/8 | 14 | 29 | 212,680 | 1.14 |
| routes_tool | **1.000** | **8/8** | 0 | 0 | 97,294 | 0.32 |

Paired delta **+0.126 [+0.05, +0.21]** (95% bootstrap CI over 8 tasks); 6 wins, 0 losses,
2 ties. -54% context tokens, -72% list cost. Every episode: route_callers -> importers
-> answer.

## Caveats

- **Not independent of the oracle.** Gold is URL callers (Python oracle) plus direct
  importers (madge). route_join is a separate implementation of the same idea
  (precision 0.78 / recall 1.00 vs the oracle over 43 views, exact on these 8), and the
  fixed import index equals madge's edge set. The result shows that serving the
  cross-language edge as a lookup lets an agent use it perfectly; it does not show the
  edge definition is right beyond the oracle.
- The 8 tasks are exactly the ones route_join gets exact; on the 35 other views some
  answers would carry common-word false positives (`share`, `tasks`, `logo`).
- One trial per arm; the baseline comes from a separate (same-day, same-config) run.
- First run on the unfixed index: 0.703 (`results-xlang-routes/`).
